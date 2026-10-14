use super::*;

pub(super) fn register_policy(args: &[&str]) -> Result<(), &'static str> {
    let mut disabled = false;
    let mut index = 0;
    while index < args.len() {
        let argument = args[index];
        if argument == "--no-default-config" {
            disabled = true;
        } else if matches!(
            argument,
            "-x" | "-target"
                | "--target"
                | "-I"
                | "-D"
                | "-U"
                | "-include"
                | "-imacros"
                | "-isystem"
                | "-iquote"
                | "-idirafter"
                | "-isysroot"
                | "--sysroot"
                | "-resource-dir"
                | "-working-directory"
        ) {
            index += 1;
            let Some(operand) = args.get(index) else {
                return Err("missing-driver-option-operand");
            };
            if operand.starts_with('@') {
                return Err("unbound-driver-option-operand");
            }
        } else if argument.starts_with('@')
            || argument == "--"
            || argument == "--config"
            || argument.starts_with("--config=")
            || argument == "-Xclang"
            || argument.starts_with("-Xclang=")
            || argument.starts_with("-Xarch")
            || argument == "-Xpreprocessor"
            || argument.starts_with("-Wp,")
            || argument == "-Xassembler"
            || argument.starts_with("-Wa,")
            || argument == "-mllvm"
            || argument.starts_with("-mllvm=")
            || argument.starts_with("/clang:")
            || argument == "-cc1"
            || argument.starts_with("--driver-mode=")
        {
            return Err("unbound-driver-argument-indirection");
        } else if let Some(value) = argument.strip_prefix("-mregparm=") {
            if value != "0" {
                return Err("nonzero-or-malformed-driver-register-policy");
            }
        } else if argument == "-mregparm" {
            index += 1;
            if args.get(index) != Some(&"0") {
                return Err("nonzero-or-malformed-driver-register-policy");
            }
        } else if !matches!(
            argument,
            "-fms-compatibility"
                | "-fms-extensions"
                | "-fno-exceptions"
                | "-fno-rtti"
                | "-nostdinc"
                | "-nostdinc++"
        ) && ![
            "-std=",
            "--target=",
            "-target=",
            "-D",
            "-U",
            "-I",
            "-Wno-",
            "-ferror-limit=",
            "-fms-compatibility-version=",
        ]
        .iter()
        .any(|prefix| argument.starts_with(prefix))
        {
            return Err("unbound-driver-option-grammar");
        }
        index += 1;
    }
    if disabled {
        Ok(())
    } else {
        Err("driver-default-configuration-not-disabled")
    }
}

pub(super) fn target_policy(triple: &str, width: i32) -> Result<(), &'static str> {
    let mut parts = triple.split('-');
    let architecture = parts.next();
    let _vendor = parts.next();
    let operating_system = parts.next();
    let environment = parts.next();
    let expected_width = match architecture {
        Some("i386" | "i486" | "i586" | "i686") => 32,
        Some("x86_64" | "aarch64") => 64,
        _ => return Err("unsupported-native-target-architecture"),
    };
    if operating_system != Some("windows")
        || !environment.is_some_and(|environment| {
            environment.strip_prefix("msvc").is_some_and(|version| {
                version
                    .chars()
                    .all(|character| character.is_ascii_digit() || character == '.')
            })
        })
        || parts.next().is_some()
        || width != expected_width
    {
        return Err("unsupported-native-target-abi");
    }
    Ok(())
}

pub(super) fn array_parameter_element_match(
    source_use: CXType,
    array: CXType,
    pointee: CXType,
) -> Result<(), &'static str> {
    if !clang_getUnqualifiedType::is_loaded() {
        return Err("unavailable-native-array-parameter-qualification-query");
    }
    let element = unsafe { clang_getCanonicalType(clang_getArrayElementType(array)) };
    let pointee = unsafe { clang_getCanonicalType(pointee) };
    let qualifiers = |ty| unsafe {
        (
            clang_isConstQualifiedType(ty) != 0,
            clang_isVolatileQualifiedType(ty) != 0,
            clang_isRestrictQualifiedType(ty) != 0,
        )
    };
    let source = qualifiers(source_use);
    let element_qualifiers = qualifiers(element);
    // C++17 [dcl.array] applies the array use's const qualifier to its element.
    let effective = (
        source.0 || element_qualifiers.0,
        element_qualifiers.1,
        element_qualifiers.2,
    );
    if element.kind == CXType_Invalid
        || pointee.kind == CXType_Invalid
        || (source.1 && !element_qualifiers.1)
        || (source.2 && !element_qualifiers.2)
        || effective != qualifiers(pointee)
        || unsafe { clang_getAddressSpace(element) } != unsafe { clang_getAddressSpace(pointee) }
    {
        return Err("unproved-native-array-parameter-qualification");
    }
    if unsafe {
        clang_equalTypes(
            clang_getUnqualifiedType(element),
            clang_getUnqualifiedType(pointee),
        )
    } == 0
    {
        return Err("unproved-native-array-parameter-element");
    }
    Ok(())
}

pub(super) fn function_transport(
    ty: CXType,
    policy: Result<(), &'static str>,
) -> Result<(), &'static str> {
    policy?;
    let canonical = unsafe { clang_getCanonicalType(ty) };
    if canonical.kind != CXType_FunctionProto
        || unsafe { clang_isFunctionTypeVariadic(canonical) } != 0
    {
        return Err("unsupported-native-function-prototype");
    }
    let convention = match unsafe { clang_getFunctionTypeCallingConv(canonical) } {
        CXCallingConv_C => "",
        CXCallingConv_X86StdCall => "__attribute__((stdcall))",
        CXCallingConv_Win64 => "__attribute__((ms_abi))",
        _ => return Err("unsupported-native-function-convention"),
    };
    let count = unsafe { clang_getNumArgTypes(canonical) };
    if count < 0 {
        return Err("unavailable-native-function-parameters");
    }
    let result = cx_string(unsafe {
        clang_getTypeSpelling(clang_getCanonicalType(clang_getResultType(canonical)))
    });
    let parameters = (0..count)
        .map(|index| {
            cx_string(unsafe {
                clang_getTypeSpelling(clang_getCanonicalType(clang_getArgType(
                    canonical,
                    index as u32,
                )))
            })
        })
        .collect::<Vec<_>>();
    let exception = match unsafe { clang_getExceptionSpecificationType(canonical) } {
        CXCursor_ExceptionSpecificationKind_None => "",
        CXCursor_ExceptionSpecificationKind_BasicNoexcept => "noexcept",
        _ => return Err("unsupported-native-function-exception-specification"),
    };
    let expected = format!("{result}({}){convention}{exception}", parameters.join(","));
    let actual = cx_string(unsafe { clang_getTypeSpelling(canonical) });
    let compact = |value: &str| {
        value
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect::<String>()
    };
    if compact(&actual) != compact(&expected) {
        return Err("unrepresented-native-function-transport");
    }
    Ok(())
}

pub(super) fn unsupported_declaration_attribute(cursor: CXCursor) -> Option<CXCursor> {
    unsupported_attribute(cursor, false)
}

pub(super) fn unsupported_method_attribute(
    cursor: CXCursor,
    validated_override: bool,
) -> Option<CXCursor> {
    unsupported_attribute(cursor, validated_override)
}

fn unsupported_attribute(cursor: CXCursor, validated_override: bool) -> Option<CXCursor> {
    let target = unsafe { clang_getCursorKind(cursor) };
    cursor_children(cursor).into_iter().find(|attribute| {
        let kind = unsafe { clang_getCursorKind(*attribute) };
        if unsafe { clang_isAttribute(kind) } == 0 {
            return false;
        }
        if kind == CXCursor_CXXOverrideAttr && target == CXCursor_CXXMethod && validated_override {
            return false;
        }
        if kind != CXCursor_AnnotateAttr {
            return true;
        }
        let spelling = cx_string(unsafe { clang_getCursorSpelling(*attribute) });
        if let Some(raw) = parse_raw_annotation(&spelling) {
            return !annotation_target_allowed(&raw.key, target) || annotation_value(raw).is_none();
        }
        if target != CXCursor_ParmDecl {
            return true;
        }
        let mut represented = ParamAnnotation::default();
        apply_parameter_annotation(&mut represented, &spelling, cursor);
        represented == ParamAnnotation::default()
    })
}

#[derive(Default)]
pub(super) struct Capture {
    cached: HashMap<usize, Vec<(CXType, NativeType, usize)>>,
    trace: Option<RejectionTrace>,
    referent_memory: bool,
}

#[derive(Default)]
struct RejectionTrace {
    path: Vec<Step>,
    first: Option<String>,
}

enum Step {
    Indirect,
    ArrayElement,
    FunctionParameter(u32),
    FunctionResult,
    RecordField(usize),
    EnumUnderlying,
}

impl Display for Step {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Indirect => formatter.write_str("indirect"),
            Self::ArrayElement => formatter.write_str("array-element"),
            Self::FunctionParameter(index) => write!(formatter, "function-parameter[{index}]"),
            Self::FunctionResult => formatter.write_str("function-result"),
            Self::RecordField(index) => write!(formatter, "field[{index}]"),
            Self::EnumUnderlying => formatter.write_str("enum-underlying"),
        }
    }
}

impl Capture {
    pub(super) fn for_arguments(args: &[&str]) -> Self {
        Self {
            referent_memory: register_policy(args).is_ok(),
            ..Self::default()
        }
    }

    pub(super) fn capture_typedef(&mut self, cursor: CXCursor) -> Option<NativeType> {
        let ty = unsafe { clang_getTypedefDeclUnderlyingType(cursor) };
        if let Some(attribute) = unsupported_declaration_attribute(cursor) {
            return self.reject(ty, 0, "unsupported-typedef-attribute", Some(attribute));
        }
        self.capture(ty)
    }

    pub(super) fn rejected_diagnostic(
        &mut self,
        ty: CXType,
        reason: &'static str,
        witness: CXCursor,
    ) -> (Option<NativeType>, Option<String>) {
        self.trace = Some(RejectionTrace::default());
        let native = self.reject(ty, 0, reason, Some(witness));
        let rejection = self.trace.take().and_then(|trace| trace.first);
        (native, rejection)
    }

    pub(super) fn capture(&mut self, ty: CXType) -> Option<NativeType> {
        self.read(ty, 0).map(|(native, _)| native)
    }

    pub(super) fn capture_diagnostic(
        &mut self,
        ty: CXType,
    ) -> (Option<NativeType>, Option<String>) {
        self.trace = Some(RejectionTrace::default());
        let native = self.capture(ty);
        let rejection = self.trace.take().and_then(|trace| trace.first);
        (native, rejection)
    }

    pub(super) fn capture_parameter(
        &mut self,
        method: CXCursor,
        parameters: &[CXCursor],
        index: usize,
        pointer_width: i32,
        diagnostic: bool,
    ) -> (Option<NativeType>, Option<String>) {
        if diagnostic {
            self.trace = Some(RejectionTrace::default());
        }
        let native = self.parameter(method, parameters, index, pointer_width);
        let rejection = self.trace.take().and_then(|trace| trace.first);
        (native, rejection)
    }

    fn parameter(
        &mut self,
        method: CXCursor,
        parameters: &[CXCursor],
        index: usize,
        pointer_width: i32,
    ) -> Option<NativeType> {
        let Some(&parameter) = parameters.get(index) else {
            return self.reject(
                unsafe { clang_getCursorType(method) },
                0,
                "unproved-native-array-parameter-index",
                Some(method),
            );
        };
        let raw = unsafe { clang_getCursorType(parameter) };
        let Some((source_use, Some(array))) = function_param_view(raw) else {
            if matches!(
                unsafe { clang_getCanonicalType(raw) }.kind,
                CXType_IncompleteArray | CXType_ConstantArray
            ) {
                self.raw_use(raw, 0)?;
                return self.reject(
                    raw,
                    0,
                    "unproved-native-array-parameter-source",
                    Some(parameter),
                );
            }
            return self.capture(raw);
        };
        self.raw_use(raw, 0)?;
        let function = unsafe { clang_getCanonicalType(clang_getCursorType(method)) };
        let tu = unsafe { clang_Cursor_getTranslationUnit(method) };
        let count = unsafe { clang_getNumArgTypes(function) };
        if tu.is_null()
            || function.kind != CXType_FunctionProto
            || usize::try_from(count).ok() != Some(parameters.len())
            || unsafe { clang_Cursor_getNumArguments(method) } != count
            || unsafe {
                clang_equalCursors(
                    clang_Cursor_getArgument(method, u32::try_from(index).ok()?),
                    parameter,
                )
            } == 0
            || unsafe { clang_Cursor_getTranslationUnit(parameter) } != tu
            || unsafe { clang_getCursorKind(parameter) } != CXCursor_ParmDecl
            || unsafe { clang_equalCursors(clang_getCursorSemanticParent(parameter), method) } == 0
        {
            return self.reject(
                raw,
                0,
                "unproved-native-array-parameter-owner",
                Some(parameter),
            );
        }
        let pointer = unsafe { clang_getArgType(function, u32::try_from(index).ok()?) };
        let pointer = unsafe { clang_getCanonicalType(pointer) };
        let pointee = unsafe { clang_getCanonicalType(clang_getPointeeType(pointer)) };
        let size = unsafe { clang_Type_getSizeOf(pointer) };
        let align = unsafe { clang_Type_getAlignOf(pointer) };
        if pointer.kind != CXType_Pointer
            || !matches!(pointer_width, 32 | 64)
            || size * 8 != i64::from(pointer_width)
            || align <= 0
            || unsafe { clang_getAddressSpace(pointer) } != 0
        {
            return self.reject(
                raw,
                0,
                "unproved-native-array-parameter-pointer",
                Some(parameter),
            );
        }
        if let Err(reason) = array_parameter_element_match(source_use, array, pointee) {
            return self.reject(raw, 0, reason, Some(parameter));
        }
        self.capture(pointer)
    }

    fn reject<T>(
        &mut self,
        ty: CXType,
        depth: usize,
        reason: &'static str,
        witness: Option<CXCursor>,
    ) -> Option<T> {
        if let Some(trace) = &mut self.trace
            && trace.first.is_none()
        {
            let canonical = unsafe { clang_getCanonicalType(ty) };
            let witness = witness.map(|cursor| {
                (
                    unsafe { clang_getCursorKind(cursor) },
                    cx_string(unsafe { clang_getCursorSpelling(cursor) }),
                    unsafe { clang_isCursorDefinition(cursor) },
                    unsafe { clang_getCXXAccessSpecifier(cursor) },
                    cursor_locations(cursor),
                    cursor_locations(unsafe { clang_getCursorSemanticParent(cursor) }),
                )
            });
            trace.first = Some(format!(
                "reason={reason} path={:?} depth={depth} canonical_kind={} \
                 spelling={:?} const={} volatile={} restrict={} size={} align={} \
                 address_space={} pod={} witness={witness:?}",
                trace
                    .path
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                canonical.kind,
                cx_string(unsafe { clang_getTypeSpelling(canonical) }),
                unsafe { clang_isConstQualifiedType(canonical) },
                unsafe { clang_isVolatileQualifiedType(canonical) },
                unsafe { clang_isRestrictQualifiedType(canonical) },
                unsafe { clang_Type_getSizeOf(canonical) },
                unsafe { clang_Type_getAlignOf(canonical) },
                unsafe { clang_getAddressSpace(canonical) },
                unsafe { clang_isPODType(canonical) },
            ));
        }
        None
    }

    fn read(&mut self, ty: CXType, depth: usize) -> Option<(NativeType, usize)> {
        if depth == 64 {
            return self.reject(ty, depth, "capture-depth-limit", None);
        }
        self.raw_use(ty, depth)?;
        let ty = unsafe { clang_getCanonicalType(ty) };
        // Canonical QualType identity selects a bucket; native equality confirms the key.
        let key = ty.data[0] as usize;
        if let Some((_, native, height)) = self
            .cached
            .get(&key)
            .into_iter()
            .flatten()
            .find(|(candidate, _, _)| unsafe { clang_equalTypes(*candidate, ty) } != 0)
        {
            if depth + height <= 64 {
                return Some((native.clone(), *height));
            }
            return self.reject(ty, depth, "cached-subtree-depth-limit", None);
        }
        let mut height = 1;
        let layer = match ty.kind {
            CXType_Void | CXType_Bool | CXType_Char_U | CXType_UChar | CXType_Char16
            | CXType_Char32 | CXType_UShort | CXType_UInt | CXType_ULong | CXType_ULongLong
            | CXType_Char_S | CXType_SChar | CXType_WChar | CXType_Short | CXType_Int
            | CXType_Long | CXType_LongLong | CXType_Float | CXType_Double | CXType_LongDouble => {
                NativeTypeLayer::Primitive
            }
            CXType_Pointer | CXType_LValueReference | CXType_RValueReference => {
                NativeTypeLayer::Indirect(Box::new(self.child(
                    unsafe { clang_getPointeeType(ty) },
                    depth,
                    &mut height,
                    Step::Indirect,
                )?))
            }
            CXType_ConstantArray => NativeTypeLayer::Array {
                target: Box::new(self.child(
                    unsafe { clang_getArrayElementType(ty) },
                    depth,
                    &mut height,
                    Step::ArrayElement,
                )?),
                len: u64::try_from(unsafe { clang_getArraySize(ty) })
                    .ok()
                    .or_else(|| self.reject(ty, depth, "array-size", None))?,
            },
            CXType_FunctionProto => {
                let count = unsafe { clang_getNumArgTypes(ty) };
                let convention = unsafe { clang_getFunctionTypeCallingConv(ty) };
                if count < 0
                    || matches!(convention, CXCallingConv_Invalid | CXCallingConv_Unexposed)
                {
                    return self.reject(ty, depth, "function-arguments-or-convention", None);
                }
                NativeTypeLayer::Function {
                    convention,
                    variadic: unsafe { clang_isFunctionTypeVariadic(ty) } != 0,
                    ref_qualifier: unsafe { clang_Type_getCXXRefQualifier(ty) },
                    params: (0..count)
                        .map(|index| {
                            self.child(
                                unsafe { clang_getArgType(ty, index as u32) },
                                depth,
                                &mut height,
                                Step::FunctionParameter(index as u32),
                            )
                        })
                        .collect::<Option<_>>()?,
                    result: Box::new(self.child(
                        unsafe { clang_getResultType(ty) },
                        depth,
                        &mut height,
                        Step::FunctionResult,
                    )?),
                }
            }
            CXType_Record => {
                let cursor = cursor_definition(unsafe { clang_getTypeDeclaration(ty) });
                if is_interface(cursor)
                    && unsafe { clang_Cursor_getModule(cursor) }.is_null()
                    && matches!(
                        unsafe { clang_getCursorKind(cursor) },
                        CXCursor_StructDecl | CXCursor_ClassDecl
                    )
                {
                    NativeTypeLayer::Interface(Box::new(declaration(cursor)?))
                } else {
                    NativeTypeLayer::Record(Box::new(record(self, ty, depth, &mut height)?))
                }
            }
            CXType_Enum => {
                NativeTypeLayer::Enum(Box::new(enumeration(self, ty, depth, &mut height)?))
            }
            // Dependent, incomplete and unsupported forms have no native evidence.
            _ => return self.reject(ty, depth, "unsupported-canonical-kind", None),
        };
        let native = NativeType(Arc::new(NativeTypeData {
            kind: ty.kind,
            is_const: unsafe { clang_isConstQualifiedType(ty) } != 0,
            is_volatile: unsafe { clang_isVolatileQualifiedType(ty) } != 0,
            is_restrict: unsafe { clang_isRestrictQualifiedType(ty) } != 0,
            size: unsafe { clang_Type_getSizeOf(ty) },
            address_space: unsafe { clang_getAddressSpace(ty) },
            spelling: cx_string(unsafe { clang_getTypeSpelling(ty) }),
            layer,
        }));
        self.cached
            .entry(key)
            .or_default()
            .push((ty, native.clone(), height));
        Some((native, height))
    }

    fn raw_use(&mut self, mut ty: CXType, depth: usize) -> Option<()> {
        if depth == 64 {
            // read and cached subtree heights report the unchanged native capture limit.
            return Some(());
        }
        loop {
            match ty.kind {
                CXType_Typedef => {
                    let cursor = unsafe { clang_getTypeDeclaration(ty) };
                    if let Some(attribute) = unsupported_declaration_attribute(cursor) {
                        return self.reject(
                            ty,
                            depth,
                            "unsupported-typedef-attribute",
                            Some(attribute),
                        );
                    }
                    ty = unsafe { clang_getTypedefDeclUnderlyingType(cursor) };
                }
                CXType_Elaborated => ty = unsafe { clang_Type_getNamedType(ty) },
                CXType_Attributed => {
                    if !self.referent_memory || function_transport(ty, Ok(())).is_err() {
                        return self.reject(ty, depth, "unsupported-type-use-attribute", None);
                    }
                    ty = unsafe { clang_Type_getModifiedType(ty) };
                }
                CXType_Pointer | CXType_LValueReference | CXType_RValueReference => {
                    return self.raw_use(unsafe { clang_getPointeeType(ty) }, depth + 1);
                }
                CXType_ConstantArray | CXType_IncompleteArray => {
                    return self.raw_use(unsafe { clang_getArrayElementType(ty) }, depth + 1);
                }
                CXType_FunctionProto => {
                    let count = unsafe { clang_getNumArgTypes(ty) };
                    if count < 0 {
                        return self.reject(ty, depth, "unavailable-function-type-use", None);
                    }
                    for index in 0..count {
                        self.raw_use(unsafe { clang_getArgType(ty, index as u32) }, depth + 1)?;
                    }
                    return self.raw_use(unsafe { clang_getResultType(ty) }, depth + 1);
                }
                _ => return Some(()),
            }
        }
    }

    fn child(
        &mut self,
        ty: CXType,
        depth: usize,
        height: &mut usize,
        step: Step,
    ) -> Option<NativeType> {
        if let Some(trace) = &mut self.trace {
            trace.path.push(step);
        }
        let result = self.read(ty, depth + 1);
        if let Some(trace) = &mut self.trace {
            trace.path.pop();
        }
        let (native, child_height) = result?;
        *height = (*height).max(child_height + 1);
        Some(native)
    }
}

pub(super) fn declaration(cursor: CXCursor) -> Option<NativeDeclaration> {
    let (spelling, expansion, _, _) = cursor_locations(cursor)?;
    let parent = unsafe { clang_getCursorSemanticParent(cursor) };
    if unsafe { clang_Cursor_isNull(parent) } != 0 {
        return None;
    }
    let kind = unsafe { clang_getCursorKind(parent) };
    let locations = cursor_locations(parent);
    if locations.is_none() && kind != CXCursor_TranslationUnit {
        return None;
    }
    let (parent_spelling, parent_expansion) = locations
        .map_or((None, None), |(spelling, expansion, _, _)| {
            (Some(spelling), Some(expansion))
        });
    Some(NativeDeclaration {
        tu: cx_string(unsafe {
            clang_getTranslationUnitSpelling(clang_Cursor_getTranslationUnit(cursor))
        }),
        kind: unsafe { clang_getCursorKind(cursor) },
        name: cx_string(unsafe { clang_getCursorSpelling(cursor) }),
        spelling,
        expansion,
        anonymous: unsafe { clang_Cursor_isAnonymous(cursor) } != 0,
        parent: NativeParent {
            kind,
            name: cx_string(unsafe { clang_getCursorSpelling(parent) }),
            spelling: parent_spelling,
            expansion: parent_expansion,
        },
    })
}

fn record(
    capture: &mut Capture,
    ty: CXType,
    depth: usize,
    height: &mut usize,
) -> Option<NativeRecord> {
    let cursor = cursor_definition(unsafe { clang_getTypeDeclaration(ty) });
    let kind = unsafe { clang_getCursorKind(cursor) };
    let align = unsafe { clang_Type_getAlignOf(ty) };
    if !matches!(kind, CXCursor_StructDecl | CXCursor_UnionDecl)
        || unsafe { clang_isCursorDefinition(cursor) } == 0
        || unsafe { clang_isPODType(ty) } == 0
        || unsafe { clang_Type_getSizeOf(ty) } < 0
        || align <= 0
    {
        return capture.reject(
            ty,
            depth,
            "record-definition-kind-pod-or-layout",
            Some(cursor),
        );
    }
    let mut fields = Vec::new();
    let mut aggregate_abi_proved = true;
    for child in cursor_children(cursor) {
        let kind = unsafe { clang_getCursorKind(child) };
        if unsafe { clang_isAttribute(kind) } != 0 {
            if !capture.referent_memory {
                return capture.reject(ty, depth, "record-attribute", Some(child));
            }
            aggregate_abi_proved = false;
            continue;
        }
        let anonymous = matches!(kind, CXCursor_StructDecl | CXCursor_UnionDecl)
            && unsafe { clang_Cursor_isAnonymousRecordDecl(child) } != 0;
        if kind == CXCursor_FieldDecl || anonymous {
            if let Some(attribute) = cursor_children(child)
                .iter()
                .find(|child| unsafe { clang_isAttribute(clang_getCursorKind(**child)) } != 0)
            {
                return capture.reject(ty, depth, "field-attribute", Some(*attribute));
            }
            if kind == CXCursor_FieldDecl
                && !matches!(
                    unsafe { clang_getCXXAccessSpecifier(child) },
                    CX_CXXInvalidAccessSpecifier | CX_CXXPublic
                )
            {
                return capture.reject(ty, depth, "field-access", Some(child));
            }
            let field_ty = unsafe { clang_getCursorType(child) };
            let native = capture.child(field_ty, depth, height, Step::RecordField(fields.len()))?;
            let offset = if anonymous {
                anonymous_offset(ty, &native, 0)
                    .or_else(|| capture.reject(ty, depth, "anonymous-offset", Some(child)))?
            } else {
                unsafe { clang_Cursor_getOffsetOfField(child) }
            };
            let align = unsafe { clang_Type_getAlignOf(field_ty) };
            if offset < 0 || align <= 0 || native.size < 0 {
                return capture.reject(field_ty, depth, "field-offset-align-or-size", Some(child));
            }
            let bit_width = if unsafe { clang_Cursor_isBitField(child) } != 0 {
                Some(
                    u32::try_from(unsafe { clang_getFieldDeclBitWidth(child) })
                        .ok()
                        .or_else(|| capture.reject(field_ty, depth, "bit-width", Some(child)))?,
                )
            } else {
                None
            };
            fields.push(NativeField {
                declaration: declaration(child).or_else(|| {
                    capture.reject(field_ty, depth, "field-source-or-parent", Some(child))
                })?,
                ordinal: fields.len(),
                offset,
                align,
                bit_width,
                ty: native,
            });
        } else if !matches!(
            kind,
            CXCursor_StructDecl
                | CXCursor_UnionDecl
                | CXCursor_EnumDecl
                | CXCursor_TypedefDecl
                | CXCursor_TypeAliasDecl
                | CXCursor_CXXAccessSpecifier
        ) {
            return capture.reject(ty, depth, "unsupported-record-child", Some(child));
        }
    }
    Some(NativeRecord {
        declaration: declaration(cursor)
            .or_else(|| capture.reject(ty, depth, "record-source-or-parent", Some(cursor)))?,
        align,
        aggregate_abi_proved,
        fields,
    })
}

// Anonymous aggregate offsets use the same promoted-member compiler query as inline records.
fn anonymous_offset(parent: CXType, native: &NativeType, relative: i64) -> Option<i64> {
    let NativeTypeLayer::Record(record) = &native.layer else {
        return None;
    };
    for field in &record.fields {
        let relative = relative.checked_add(field.offset)?;
        if field.declaration.name.is_empty() {
            if let Some(offset) = anonymous_offset(parent, &field.ty, relative) {
                return Some(offset);
            }
        } else {
            let name = CString::new(field.declaration.name.as_str()).ok()?;
            let promoted = unsafe { clang_Type_getOffsetOf(parent, name.as_ptr()) };
            if promoted >= 0 {
                return promoted.checked_sub(relative);
            }
        }
    }
    None
}

fn enumeration(
    capture: &mut Capture,
    ty: CXType,
    depth: usize,
    height: &mut usize,
) -> Option<NativeEnum> {
    let cursor = cursor_definition(unsafe { clang_getTypeDeclaration(ty) });
    if unsafe { clang_isCursorDefinition(cursor) } == 0 || unsafe { clang_Type_getSizeOf(ty) } < 0 {
        return capture.reject(ty, depth, "enum-definition-or-size", Some(cursor));
    }
    let underlying = capture.child(
        unsafe { clang_getEnumDeclIntegerType(cursor) },
        depth,
        height,
        Step::EnumUnderlying,
    )?;
    if underlying.layer != NativeTypeLayer::Primitive {
        return capture.reject(ty, depth, "nonprimitive-enum-underlying", Some(cursor));
    }
    let mut variants = Vec::new();
    for child in cursor_children(cursor) {
        match unsafe { clang_getCursorKind(child) } {
            CXCursor_EnumConstantDecl => {
                if let Some(attribute) = cursor_children(child)
                    .iter()
                    .find(|child| unsafe { clang_isAttribute(clang_getCursorKind(**child)) } != 0)
                {
                    return capture.reject(ty, depth, "enum-variant-attribute", Some(*attribute));
                }
                variants.push(NativeVariant {
                    declaration: declaration(child).or_else(|| {
                        capture.reject(ty, depth, "enum-variant-source-or-parent", Some(child))
                    })?,
                    signed_value: unsafe { clang_getEnumConstantDeclValue(child) },
                    unsigned_value: unsafe { clang_getEnumConstantDeclUnsignedValue(child) },
                });
            }
            CXCursor_TypeRef => {}
            _ => return capture.reject(ty, depth, "unsupported-enum-child", Some(child)),
        }
    }
    Some(NativeEnum {
        declaration: declaration(cursor)
            .or_else(|| capture.reject(ty, depth, "enum-source-or-parent", Some(cursor)))?,
        underlying,
        scoped: unsafe { clang_EnumDecl_isScoped(cursor) } != 0,
        variants,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_attribute_public_api_evidence() {
        let _library = Library::new().unwrap();
        let index = Index::new().unwrap();
        assert!(register_policy(&["--no-default-config"]).is_ok());
        assert!(register_policy(&["--no-default-config", "-mregparm=0"]).is_ok());
        assert!(register_policy(&["--no-default-config", "-mregparm", "0"]).is_ok());
        assert!(register_policy(&["--no-default-config", "-nostdinc", "-nostdinc++"]).is_ok());
        for (target, width) in [
            ("i686-pc-windows-msvc", 32),
            ("x86_64-pc-windows-msvc", 64),
            ("aarch64-pc-windows-msvc", 64),
        ] {
            assert!(target_policy(target, width).is_ok());
        }
        for (target, width) in [
            ("i686-pc-windows-msvc", 64),
            ("x86_64-pc-linux-gnu", 64),
            ("x86_64-pc-windows-gnu", 64),
            ("i586-intel-elfiamcu", 32),
        ] {
            assert!(target_policy(target, width).is_err());
        }
        for arguments in [
            vec![],
            vec!["--no-default-config", "-mregparm=2"],
            vec!["--no-default-config", "-mregparm="],
            vec!["--no-default-config", "-mregparm"],
            vec!["--no-default-config", "-Xclang", "-mregparm"],
            vec!["--no-default-config", "--config=unknown.cfg"],
            vec!["--no-default-config", "@unknown.rsp"],
            vec!["-I", "--no-default-config"],
            vec!["-isystem", "--no-default-config"],
            vec!["--", "--no-default-config"],
            vec!["-unknown-option", "--no-default-config"],
            vec!["--no-default-config", "-unknown-option"],
            vec!["--no-default-config", "-Wp,@unknown.rsp"],
            vec!["--no-default-config", "-Xpreprocessor", "@unknown.rsp"],
            vec!["--no-default-config", "-I", "@unknown.rsp"],
        ] {
            assert!(register_policy(&arguments).is_err(), "{arguments:?}");
        }
        let input = Input::new(
            "attribute-evidence.hpp",
            r#"
            struct Plain { char first; int second; };
            #pragma pack(push, 8)
            struct Pack8 { char first; int second; };
            #pragma pack(pop)
            #pragma pack(push, 1)
            struct Pack1 { char first; int second; };
            #pragma pack(pop)
            struct __declspec(uuid("e0e043c4-87b6-433a-84ad-75c33e3a0584")) Explicit { int value; };
            struct _GUID {
                unsigned long data1;
                unsigned short data2, data3;
                unsigned char data4[8];
            };
            "#,
        );
        let tu = TranslationUnit::parse(
            &index,
            &input,
            &[
                "--no-default-config",
                "-x",
                "c++",
                "--target=x86_64-pc-windows-msvc",
            ],
        )
        .unwrap();
        let mut capture = Capture::default();
        let mut memory = Capture::for_arguments(&[
            "--no-default-config",
            "-x",
            "c++",
            "--target=x86_64-pc-windows-msvc",
        ]);
        for cursor in cursor_children(unsafe { clang_getTranslationUnitCursor(tu.0) })
            .into_iter()
            .filter(|cursor| unsafe { clang_getCursorKind(*cursor) } == CXCursor_StructDecl)
        {
            let name = cx_string(unsafe { clang_getCursorSpelling(cursor) });
            let ty = unsafe { clang_getCursorType(cursor) };
            let (native, rejection) = capture.capture_diagnostic(ty);
            if name == "Plain" {
                assert!(native.is_some());
            } else if name == "Explicit" {
                assert!(native.is_none());
            } else if name == "_GUID" && unsafe { clang_isCursorDefinition(cursor) } != 0 {
                let complete = memory.capture(ty).unwrap();
                let NativeTypeLayer::Record(record) = &complete.layer else {
                    panic!()
                };
                assert!(!record.aggregate_abi_proved);
                assert_eq!(complete.size, 16);
                assert_eq!(record.align, 4);
                assert_eq!(record.fields.len(), 4);
                assert_eq!(
                    record
                        .fields
                        .iter()
                        .map(|field| field.offset)
                        .collect::<Vec<_>>(),
                    [0, 32, 48, 64]
                );
                assert!(native.is_none());
                let rejection = rejection.unwrap();
                assert!(rejection.contains("reason=record-attribute"), "{rejection}");
                assert!(
                    rejection.contains("witness=Some((400, \"\", 0, 0, None, None))"),
                    "{rejection}"
                );
            }
        }
    }

    #[test]
    fn native_use_site_attribute_observability() {
        fn attributes(cursor: CXCursor) -> Vec<(i32, String)> {
            cursor_children(cursor)
                .into_iter()
                .filter(|child| unsafe { clang_isAttribute(clang_getCursorKind(*child)) } != 0)
                .map(|child| {
                    (
                        unsafe { clang_getCursorKind(child) },
                        cx_string(unsafe { clang_getCursorSpelling(child) }),
                    )
                })
                .collect()
        }

        let _library = Library::new().unwrap();
        let index = Index::new().unwrap();
        let input = Input::new(
            "use-site-attributes.hpp",
            r#"
            struct Payload { int value; };
            typedef Payload *Plain;
            typedef Payload *__attribute__((noderef)) NoDeref;
            typedef NoDeref *Nested;
            typedef NoDeref Array[2];
            typedef Payload *Metadata __attribute__((annotate("win32metadata:also_usable_for=Other")));
            struct Interface {
                __attribute__((annotate("win32metadata:preserve_result")))
                virtual long __stdcall Normal(Payload *value
                    __attribute__((annotate("_In_")))
                    __attribute__((annotate("win32metadata:optional")))) = 0;
                virtual void __stdcall Aliased(NoDeref value) = 0;
                virtual void __stdcall Inline(Payload *__attribute__((noderef)) value) = 0;
                virtual void __stdcall Hidden(Payload *const value
                    __attribute__((pass_object_size(0)))) = 0;
                #if defined(_M_IX86)
                virtual long __stdcall Registers(Payload *value)
                    __attribute__((regparm(2))) = 0;
                #endif
            };
            "#,
        );
        let name = CString::new(input.name.as_str()).unwrap();
        let source = CString::new(input.source.as_str()).unwrap();
        let mut unsaved = CXUnsavedFile {
            Filename: name.as_ptr(),
            Contents: source.as_ptr(),
            Length: input.source.len().try_into().unwrap(),
        };
        for target in ["x86_64-pc-windows-msvc", "i686-pc-windows-msvc"] {
            let argument_values = [
                "-x".into(),
                "c++".into(),
                "--no-default-config".into(),
                format!("--target={target}"),
            ];
            let policy = register_policy(
                &argument_values
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            );
            assert!(policy.is_ok());
            let arguments: Vec<_> = argument_values
                .map(|argument| CString::new(argument).unwrap())
                .into();
            let arguments: Vec<_> = arguments.iter().map(|argument| argument.as_ptr()).collect();
            for additional in [0, CXTranslationUnit_IncludeAttributedTypes] {
                let options = CXTranslationUnit_DetailedPreprocessingRecord
                    | CXTranslationUnit_SkipFunctionBodies
                    | additional;
                let value = unsafe {
                    clang_parseTranslationUnit(
                        index.0,
                        name.as_ptr(),
                        arguments.as_ptr(),
                        arguments.len().try_into().unwrap(),
                        &mut unsaved,
                        1,
                        options,
                    )
                };
                assert!(!value.is_null());
                let tu = TranslationUnit(value, native_lifetime::original());
                let diagnostic_count = unsafe { clang_getNumDiagnostics(tu.0) };
                for diagnostic_index in 0..diagnostic_count {
                    let diagnostic = unsafe { clang_getDiagnostic(tu.0, diagnostic_index) };
                    println!(
                        "diagnostic target={target} severity={} text={:?}",
                        unsafe { clang_getDiagnosticSeverity(diagnostic) },
                        cx_string(unsafe {
                            clang_formatDiagnostic(
                                diagnostic,
                                clang_defaultDiagnosticDisplayOptions(),
                            )
                        }),
                    );
                    unsafe { clang_disposeDiagnostic(diagnostic) };
                }
                assert!(tu.errors().is_empty(), "{:?}", tu.errors());
                println!(
                    "target={target} options={options} include_attributed={} diagnostics={diagnostic_count}",
                    additional != 0
                );
                let mut capture = Capture::default();
                let mut aliases = BTreeMap::new();
                let mut functions = BTreeMap::new();
                for cursor in cursor_children(unsafe { clang_getTranslationUnitCursor(tu.0) }) {
                    if unsafe { clang_getCursorKind(cursor) } == CXCursor_TypedefDecl {
                        let name = cx_string(unsafe { clang_getCursorSpelling(cursor) });
                        let ty = unsafe { clang_getTypedefDeclUnderlyingType(cursor) };
                        if name == "NoDeref" {
                            assert_eq!(ty.kind == CXType_Attributed, additional != 0);
                        }
                        if name == "Metadata" {
                            assert_eq!(
                                attributes(cursor),
                                [(
                                    CXCursor_AnnotateAttr,
                                    "win32metadata:also_usable_for=Other".into()
                                )]
                            );
                        }
                        aliases.insert(name, capture.capture(ty));
                    }
                    for method in cursor_children(cursor).into_iter().filter(
                        |child| unsafe { clang_getCursorKind(*child) } == CXCursor_CXXMethod,
                    ) {
                        let name = cx_string(unsafe { clang_getCursorSpelling(method) });
                        let function = unsafe { clang_getCursorType(method) };
                        if name == "Normal" {
                            assert!(function_transport(function, policy).is_ok());
                            assert!(unsupported_declaration_attribute(method).is_none());
                        } else if name == "Registers" {
                            assert_eq!(
                                function_transport(function, policy),
                                Err("unrepresented-native-function-transport")
                            );
                        }
                        functions.insert(name.clone(), function);
                        for parameter in cursor_children(method).into_iter().filter(
                            |child| unsafe { clang_getCursorKind(*child) } == CXCursor_ParmDecl,
                        ) {
                            let ty = unsafe { clang_getCursorType(parameter) };
                            if matches!(name.as_str(), "Aliased" | "Inline") {
                                assert_eq!(
                                    capture.capture(ty).is_none(),
                                    additional != 0,
                                    "{name}"
                                );
                            }
                            if name == "Normal" {
                                let annotation = parameter_annotation(parameter);
                                assert!(annotation.input && annotation.optional);
                                assert!(unsupported_declaration_attribute(parameter).is_none());
                                assert_eq!(
                                    attributes(method),
                                    [(
                                        CXCursor_AnnotateAttr,
                                        "win32metadata:preserve_result".into()
                                    )]
                                );
                                assert_eq!(
                                    unsafe { clang_getFunctionTypeCallingConv(function) },
                                    if target.starts_with("i686") {
                                        CXCallingConv_X86StdCall
                                    } else {
                                        CXCallingConv_C
                                    }
                                );
                            } else if name == "Hidden" {
                                assert!(unsupported_declaration_attribute(parameter).is_some());
                                assert_eq!(
                                    attributes(parameter),
                                    [(CXCursor_UnexposedAttr, String::new())]
                                );
                            }
                        }
                    }
                }
                assert_eq!(aliases.len(), 5);
                assert!(aliases["Plain"].is_some());
                if additional == 0 {
                    assert_eq!(aliases["Plain"], aliases["NoDeref"]);
                } else {
                    for name in ["NoDeref", "Nested", "Array"] {
                        assert!(aliases[name].is_none(), "{name}");
                    }
                }
                if additional != 0 {
                    let normal = functions["Normal"];
                    assert_eq!(normal.kind, CXType_Attributed);
                    let modified = unsafe { clang_Type_getModifiedType(normal) };
                    if target.starts_with("i686") {
                        let registers = functions["Registers"];
                        assert!(matches!(
                            registers.kind,
                            CXType_Attributed | CXType_FunctionProto
                        ));
                        let registers_modified = if registers.kind == CXType_Attributed {
                            unsafe { clang_Type_getModifiedType(registers) }
                        } else {
                            registers
                        };
                        let canonical_equal = unsafe {
                            clang_equalTypes(
                                clang_getCanonicalType(normal),
                                clang_getCanonicalType(registers),
                            )
                        };
                        let modified_canonical_equal = unsafe {
                            clang_equalTypes(
                                clang_getCanonicalType(modified),
                                clang_getCanonicalType(registers_modified),
                            )
                        };
                        assert_eq!(canonical_equal, 0);
                        assert_eq!(modified_canonical_equal, 0);
                    }
                }
            }
            let target_argument = format!("--target={target}");
            let driver = ["--no-default-config", "-x", "c++", target_argument.as_str()];
            for policy_bound in [true, false] {
                let args = if policy_bound {
                    &driver[..]
                } else {
                    &driver[1..]
                };
                let snapshot = extract([input.clone()], args).unwrap();
                let interface = snapshot
                    .facts
                    .iter()
                    .find(|fact| fact.name == "Interface")
                    .unwrap();
                let contract = snapshot
                    .native_com_contracts
                    .get(&interface.origin)
                    .unwrap_or_else(|| {
                        panic!(
                            "missing native contract: target={target} policy_bound={policy_bound} \
                        origin={:?} typed_slots={:?}",
                            interface.origin, interface.data
                        )
                    });
                for slot in &contract.slots {
                    assert_eq!(slot.params.len(), 1, "{}", slot.name);
                    assert_eq!(slot.native_params.len(), 1, "{}", slot.name);
                    let proved = policy_bound && slot.name == "Normal";
                    assert_eq!(slot.native_params[0].is_some(), proved, "{}", slot.name);
                    assert_eq!(
                        slot.native_result.is_some(),
                        policy_bound && slot.name != "Registers",
                        "{}",
                        slot.name
                    );
                    if proved {
                        assert!(slot.params[0].annotation.input);
                        assert!(slot.params[0].annotation.optional);
                    }
                }
            }
        }
        let unsupported = extract(
            [input],
            &[
                "--no-default-config",
                "-x",
                "c++",
                "--target=x86_64-pc-windows-gnu",
            ],
        )
        .unwrap();
        let interface = unsupported
            .facts
            .iter()
            .find(|fact| fact.name == "Interface")
            .unwrap();
        assert!(
            unsupported.native_com_contracts[&interface.origin]
                .slots
                .iter()
                .all(|slot| {
                    slot.native_result.is_none() && slot.native_params.iter().all(Option::is_none)
                })
        );
    }

    #[test]
    fn native_evidence_diagnostics_preserve_capture_and_rejection() {
        let _library = Library::new().unwrap();
        let index = Index::new().unwrap();
        let input = Input::new(
            "diagnostic.hpp",
            r#"
            struct Incomplete;
            struct __declspec(uuid("eb4ee56d-e9e2-4ead-9341-0603c96e843c")) Attributed { int value; };
            typedef const volatile int *const volatile QualifiedPointer;
            typedef const volatile Incomplete *const volatile IncompletePointer;
            typedef Attributed *AttributedPointer;
            "#,
        );
        let tu = TranslationUnit::parse(
            &index,
            &input,
            &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
        )
        .unwrap();
        let aliases: BTreeMap<_, _> =
            cursor_children(unsafe { clang_getTranslationUnitCursor(tu.0) })
                .into_iter()
                .filter(|cursor| unsafe { clang_getCursorKind(*cursor) } == CXCursor_TypedefDecl)
                .map(|cursor| {
                    (
                        cx_string(unsafe { clang_getCursorSpelling(cursor) }),
                        unsafe { clang_getTypedefDeclUnderlyingType(cursor) },
                    )
                })
                .collect();
        let mut normal = Capture::default();
        let mut diagnostic = Capture::default();
        for (name, ty) in &aliases {
            let expected = normal.capture(*ty);
            let (actual, reason) = diagnostic.capture_diagnostic(*ty);
            assert_eq!(actual, expected, "{name}");
            if name == "QualifiedPointer" {
                let native = actual.unwrap();
                assert!(native.is_const && native.is_volatile);
                assert!(reason.is_none());
            } else {
                assert!(actual.is_none(), "{name}");
                let reason = reason.unwrap();
                assert!(reason.contains("path=[\"indirect\"]"), "{reason}");
                assert!(reason.contains("diagnostic.hpp"), "{reason}");
                let expected = if name == "IncompletePointer" {
                    "reason=record-definition-kind-pod-or-layout"
                } else {
                    "reason=record-attribute"
                };
                assert!(reason.contains(expected), "{reason}");
            }
        }
        let ty = aliases["QualifiedPointer"];
        diagnostic.trace = Some(RejectionTrace::default());
        assert!(diagnostic.read(ty, 63).is_none());
        assert!(normal.read(ty, 63).is_none());
        let reason = diagnostic.trace.take().unwrap().first.unwrap();
        assert!(
            reason.contains("reason=cached-subtree-depth-limit"),
            "{reason}"
        );
        assert!(reason.contains("depth=63"), "{reason}");
        assert!(diagnostic.capture(ty).is_some());
    }

    #[test]
    fn native_evidence_cache_respects_remaining_depth() {
        let _library = Library::new().unwrap();
        let index = Index::new().unwrap();
        let input = Input::new("depth.hpp", "typedef int *Pointer;");
        let tu = TranslationUnit::parse(
            &index,
            &input,
            &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
        )
        .unwrap();
        let cursor = cursor_children(unsafe { clang_getTranslationUnitCursor(tu.0) })
            .into_iter()
            .find(|cursor| unsafe { clang_getCursorKind(*cursor) } == CXCursor_TypedefDecl)
            .unwrap();
        let ty = unsafe { clang_getTypedefDeclUnderlyingType(cursor) };
        let mut capture = Capture::default();
        assert!(capture.read(ty, 63).is_none());
        let native = capture.capture(ty).unwrap();
        let (at_limit, height) = capture.read(ty, 62).unwrap();
        assert_eq!(height, 2);
        assert!(Arc::ptr_eq(&native.0, &at_limit.0));
        assert!(capture.read(ty, 63).is_none());
        assert!(capture.read(ty, 64).is_none());
    }
}
