use super::*;

#[derive(Default)]
pub(super) struct Capture {
    cached: HashMap<usize, Vec<(CXType, NativeType, usize)>>,
    trace: Option<RejectionTrace>,
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
                NativeTypeLayer::Record(Box::new(record(self, ty, depth, &mut height)?))
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

fn declaration(cursor: CXCursor) -> Option<NativeDeclaration> {
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
    for child in cursor_children(cursor) {
        let kind = unsafe { clang_getCursorKind(child) };
        if unsafe { clang_isAttribute(kind) } != 0 {
            return capture.reject(ty, depth, "record-attribute", Some(child));
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
