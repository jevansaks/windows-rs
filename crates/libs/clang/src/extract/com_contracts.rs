use super::*;

#[derive(Clone)]
struct Captured {
    contract: Arc<NativeComContract>,
    methods: Vec<CXCursor>,
}

struct Capture<'a> {
    cached: HashMap<u32, Vec<(CXCursor, Option<Captured>)>>,
    active: Vec<CXCursor>,
    pointer_width: i32,
    native_types: &'a mut native_types::Capture,
}

pub(super) fn capture(
    traversal: &mut Traversal<'_>,
    output: &mut BTreeMap<Origin, Arc<NativeComContract>>,
    unproved: &mut BTreeSet<Origin>,
) {
    let Some((_, cursor)) = traversal.pending_classes.first() else {
        return;
    };
    let target =
        unsafe { clang_getTranslationUnitTargetInfo(clang_Cursor_getTranslationUnit(*cursor)) };
    let pointer_width = if target.is_null() {
        0
    } else {
        let width = unsafe { clang_TargetInfo_getPointerWidth(target) };
        unsafe { clang_TargetInfo_dispose(target) };
        width
    };
    let mut capture = Capture {
        pointer_width,
        cached: HashMap::new(),
        active: Vec::new(),
        native_types: &mut traversal.native_types,
    };
    for (origin, cursor) in &traversal.pending_classes {
        if let Some(captured) = capture.get(*cursor, traversal.macros) {
            output.insert(origin.clone(), captured.contract);
        } else {
            let definition = cursor_definition(*cursor);
            let children = cursor_children(definition);
            if unsafe { clang_getCursorKind(definition) } == CXCursor_StructDecl
                && children
                    .iter()
                    .any(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_FieldDecl)
                && (cursor_uuid(definition).is_some()
                    || cursor_uuid(unsafe { clang_getCanonicalCursor(definition) }).is_some())
                && children.iter().any(|child| unsafe {
                    match clang_getCursorKind(*child) {
                        CXCursor_CXXMethod => clang_CXXMethod_isVirtual(*child) != 0,
                        CXCursor_CXXBaseSpecifier => {
                            is_interface(clang_getTypeDeclaration(clang_getCursorType(*child)))
                        }
                        _ => false,
                    }
                })
            {
                unproved.insert(origin.clone());
            }
        }
    }
}

impl Capture<'_> {
    fn get(&mut self, cursor: CXCursor, macros: &MacroDefinitions) -> Option<Captured> {
        let cursor = cursor_definition(cursor);
        let hash = unsafe { clang_hashCursor(cursor) };
        if let Some((_, result)) = self
            .cached
            .get(&hash)
            .into_iter()
            .flatten()
            .find(|(candidate, _)| unsafe { clang_equalCursors(*candidate, cursor) != 0 })
        {
            return result.clone();
        }
        if self
            .active
            .iter()
            .any(|candidate| unsafe { clang_equalCursors(*candidate, cursor) != 0 })
        {
            return None;
        }
        self.active.push(cursor);
        let result = self.read(cursor, macros);
        self.active.pop();
        self.cached
            .entry(hash)
            .or_default()
            .push((cursor, result.clone()));
        result
    }

    fn read(&mut self, cursor: CXCursor, macros: &MacroDefinitions) -> Option<Captured> {
        if unsafe { clang_isCursorDefinition(cursor) } == 0
            || !unsafe { clang_Cursor_getModule(cursor) }.is_null()
        {
            return None;
        }
        let canonical = unsafe { clang_getCanonicalCursor(cursor) };
        let guid = match cursor_uuid(cursor).or_else(|| cursor_uuid(canonical)) {
            Some(guid) => guid,
            // An absent IID is evidence for an ordinary interface, not a COM provider key.
            None if is_interface(cursor) => String::new(),
            None => return None,
        };
        if cursor_uuid(canonical).is_some_and(|other| other != guid) {
            return None;
        }
        let children = cursor_children(cursor);
        let bases: Vec<_> = children
            .iter()
            .copied()
            .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_CXXBaseSpecifier)
            .collect();
        let mut inherited = match bases.as_slice() {
            [] => Captured {
                contract: Arc::new(NativeComContract {
                    guid: guid.clone(),
                    has_fields: false,
                    pointer_width: self.pointer_width,
                    bases: Vec::new(),
                    slots: Vec::new(),
                }),
                methods: Vec::new(),
            },
            [base]
                if unsafe {
                    clang_isVirtualBase(*base) == 0
                        && clang_getCXXAccessSpecifier(*base) == CX_CXXPublic
                } =>
            {
                let declaration = unsafe { clang_getTypeDeclaration(clang_getCursorType(*base)) };
                if !is_interface(declaration) {
                    return None;
                }
                self.get(declaration, macros)?
            }
            _ => return None,
        };
        let mut slots = inherited.contract.slots.clone();
        let mut base_guids = inherited.contract.bases.clone();
        if !bases.is_empty() {
            base_guids.push(inherited.contract.guid.clone());
        }
        for child in children {
            match unsafe { clang_getCursorKind(child) } {
                CXCursor_Constructor | CXCursor_Destructor | CXCursor_ConversionFunction => {
                    return None;
                }
                CXCursor_CXXMethod if unsafe { clang_CXXMethod_isVirtual(child) } != 0 => {
                    let function = unsafe { clang_getCursorType(child) };
                    let convention = unsafe { clang_getFunctionTypeCallingConv(function) };
                    let exception = unsafe { clang_getCursorExceptionSpecificationType(child) };
                    if unsafe { clang_Cursor_isVariadic(child) } != 0
                        || unsafe { clang_CXXMethod_isConst(child) } != 0
                        || unsafe { clang_Type_getCXXRefQualifier(function) } != CXRefQualifier_None
                        || !match self.pointer_width {
                            32 => convention == CXCallingConv_X86StdCall,
                            64 => matches!(convention, CXCallingConv_C | CXCallingConv_Win64),
                            _ => false,
                        }
                    {
                        return None;
                    }
                    let native_result = self
                        .native_types
                        .capture(unsafe { clang_getCursorResultType(child) });
                    let native_params = cursor_children(child)
                        .into_iter()
                        .filter(|param| unsafe { clang_getCursorKind(*param) == CXCursor_ParmDecl })
                        .map(|param| {
                            self.native_types
                                .capture(unsafe { clang_getCursorType(param) })
                        })
                        .collect();
                    let method = NativeComMethod {
                        name: cx_string(unsafe { clang_getCursorSpelling(child) }),
                        convention,
                        params: callable_params(child, macros, false).ok()?,
                        result: type_ref(unsafe { clang_getCursorResultType(child) })?,
                        native_params,
                        native_result,
                        native_exception: (exception >= 0).then_some(exception),
                    };
                    let mut overridden = std::ptr::null_mut();
                    let mut count = 0;
                    unsafe { clang_getOverriddenCursors(child, &mut overridden, &mut count) };
                    let targets = if count == 0 {
                        Vec::new()
                    } else {
                        unsafe { std::slice::from_raw_parts(overridden, count as usize) }.to_vec()
                    };
                    unsafe { clang_disposeOverriddenCursors(overridden) };
                    match targets.as_slice() {
                        [] => {
                            inherited.methods.push(child);
                            slots.push(method);
                        }
                        [target] => {
                            let canonical = unsafe { clang_getCanonicalCursor(*target) };
                            let positions: Vec<_> = inherited
                                .methods
                                .iter()
                                .enumerate()
                                .filter(|(_, candidate)| unsafe {
                                    clang_equalCursors(
                                        clang_getCanonicalCursor(**candidate),
                                        canonical,
                                    ) != 0
                                })
                                .map(|(index, _)| index)
                                .collect();
                            let [index] = positions.as_slice() else {
                                return None;
                            };
                            let previous = &slots[*index];
                            if previous.convention != method.convention
                                || previous.name != method.name
                                || !same_signature(inherited.methods[*index], child)
                            {
                                return None;
                            }

                            inherited.methods[*index] = child;
                            slots[*index] = method;
                        }
                        _ => return None,
                    }
                }
                _ => {}
            }
        }
        if slots.is_empty() {
            return None;
        }
        Some(Captured {
            contract: Arc::new(NativeComContract {
                guid,
                pointer_width: self.pointer_width,
                has_fields: cursor_children(cursor)
                    .iter()
                    .any(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_FieldDecl),
                bases: base_guids,
                slots,
            }),
            methods: inherited.methods,
        })
    }
}

fn same_signature(left: CXCursor, right: CXCursor) -> bool {
    unsafe {
        let left_ty = clang_getCursorType(left);
        let right_ty = clang_getCursorType(right);
        let count = clang_getNumArgTypes(left_ty);
        count >= 0
            && count == clang_getNumArgTypes(right_ty)
            && clang_equalTypes(
                clang_getCanonicalType(clang_getCursorResultType(left)),
                clang_getCanonicalType(clang_getCursorResultType(right)),
            ) != 0
            && (0..count).all(|index| {
                clang_equalTypes(
                    clang_getCanonicalType(clang_getArgType(left_ty, index as u32)),
                    clang_getCanonicalType(clang_getArgType(right_ty, index as u32)),
                ) != 0
            })
    }
}
