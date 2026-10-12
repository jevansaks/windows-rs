use super::*;

#[derive(Default)]
pub(super) struct Capture {
    cached: HashMap<usize, Vec<(CXType, NativeType, usize)>>,
}

impl Capture {
    pub(super) fn capture(&mut self, ty: CXType) -> Option<NativeType> {
        self.read(ty, 0).map(|(native, _)| native)
    }

    fn read(&mut self, ty: CXType, depth: usize) -> Option<(NativeType, usize)> {
        if depth == 64 {
            return None;
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
            return (depth + height <= 64).then(|| (native.clone(), *height));
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
                )?))
            }
            CXType_ConstantArray => NativeTypeLayer::Array {
                target: Box::new(self.child(
                    unsafe { clang_getArrayElementType(ty) },
                    depth,
                    &mut height,
                )?),
                len: u64::try_from(unsafe { clang_getArraySize(ty) }).ok()?,
            },
            CXType_FunctionProto => {
                let count = unsafe { clang_getNumArgTypes(ty) };
                let convention = unsafe { clang_getFunctionTypeCallingConv(ty) };
                if count < 0
                    || matches!(convention, CXCallingConv_Invalid | CXCallingConv_Unexposed)
                {
                    return None;
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
                            )
                        })
                        .collect::<Option<_>>()?,
                    result: Box::new(self.child(
                        unsafe { clang_getResultType(ty) },
                        depth,
                        &mut height,
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
            _ => return None,
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

    fn child(&mut self, ty: CXType, depth: usize, height: &mut usize) -> Option<NativeType> {
        let (native, child_height) = self.read(ty, depth + 1)?;
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
        return None;
    }
    let mut fields = Vec::new();
    for child in cursor_children(cursor) {
        let kind = unsafe { clang_getCursorKind(child) };
        if unsafe { clang_isAttribute(kind) } != 0 {
            return None;
        }
        let anonymous = matches!(kind, CXCursor_StructDecl | CXCursor_UnionDecl)
            && unsafe { clang_Cursor_isAnonymousRecordDecl(child) } != 0;
        if kind == CXCursor_FieldDecl || anonymous {
            if cursor_children(child)
                .iter()
                .any(|child| unsafe { clang_isAttribute(clang_getCursorKind(*child)) } != 0)
            {
                return None;
            }
            if kind == CXCursor_FieldDecl
                && !matches!(
                    unsafe { clang_getCXXAccessSpecifier(child) },
                    CX_CXXInvalidAccessSpecifier | CX_CXXPublic
                )
            {
                return None;
            }
            let field_ty = unsafe { clang_getCursorType(child) };
            let native = capture.child(field_ty, depth, height)?;
            let offset = if anonymous {
                anonymous_offset(ty, &native, 0)?
            } else {
                unsafe { clang_Cursor_getOffsetOfField(child) }
            };
            let align = unsafe { clang_Type_getAlignOf(field_ty) };
            if offset < 0 || align <= 0 || native.size < 0 {
                return None;
            }
            let bit_width = if unsafe { clang_Cursor_isBitField(child) } != 0 {
                Some(u32::try_from(unsafe { clang_getFieldDeclBitWidth(child) }).ok()?)
            } else {
                None
            };
            fields.push(NativeField {
                declaration: declaration(child)?,
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
            return None;
        }
    }
    Some(NativeRecord {
        declaration: declaration(cursor)?,
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
        return None;
    }
    let underlying = capture.child(
        unsafe { clang_getEnumDeclIntegerType(cursor) },
        depth,
        height,
    )?;
    if underlying.layer != NativeTypeLayer::Primitive {
        return None;
    }
    let mut variants = Vec::new();
    for child in cursor_children(cursor) {
        match unsafe { clang_getCursorKind(child) } {
            CXCursor_EnumConstantDecl => {
                if cursor_children(child)
                    .iter()
                    .any(|child| unsafe { clang_isAttribute(clang_getCursorKind(*child)) } != 0)
                {
                    return None;
                }
                variants.push(NativeVariant {
                    declaration: declaration(child)?,
                    signed_value: unsafe { clang_getEnumConstantDeclValue(child) },
                    unsigned_value: unsafe { clang_getEnumConstantDeclUnsignedValue(child) },
                });
            }
            CXCursor_TypeRef => {}
            _ => return None,
        }
    }
    Some(NativeEnum {
        declaration: declaration(cursor)?,
        underlying,
        scoped: unsafe { clang_EnumDecl_isScoped(cursor) } != 0,
        variants,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
