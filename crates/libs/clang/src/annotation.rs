use super::*;

const WIN32_METADATA_PREFIX: &str = "win32metadata:";

#[derive(Debug, Clone)]
pub struct Win32MetadataAnnotation {
    pub key: String,
    pub value: Option<String>,
}

impl Win32MetadataAnnotation {
    pub fn is(&self, key: &str) -> bool {
        self.key == key
    }

    pub fn is_set_last_error(&self) -> bool {
        self.key == "set_last_error"
    }

    pub fn import_library(&self) -> Option<&str> {
        (self.key == "import_library")
            .then_some(self.value.as_deref())
            .flatten()
    }

    pub fn targets_return(&self) -> bool {
        matches!(
            self.key.as_str(),
            "raii_free"
                | "invalid_handle"
                | "free_with"
                | "do_not_release"
                | "not_null_terminated"
                | "null_null_terminated"
        )
    }

    pub fn to_rdl_attr(&self) -> Option<TokenStream> {
        let value = self.value.as_deref();
        Some(match self.key.as_str() {
            "raii_free" => {
                let value = value?;
                quote! { #[raii_free(#value)] }
            }
            "invalid_handle" => {
                let value = parse_integer::<i64>(value?)?;
                let value = Literal::i64_unsuffixed(value);
                quote! { #[invalid_handle(#value)] }
            }
            "free_with" => {
                let value = value?;
                quote! { #[free_with(#value)] }
            }
            "do_not_release" => quote! { #[do_not_release] },
            "not_null_terminated" => quote! { #[not_null_terminated] },
            "null_null_terminated" => quote! { #[null_null_terminated] },
            "array_count_param" => {
                let value = parse_integer::<i16>(value?)?;
                let value = Literal::i16_unsuffixed(value);
                quote! { #[len_param(#value)] }
            }
            "array_count_const" => {
                let value = parse_integer::<i32>(value?)?;
                let value = Literal::i32_unsuffixed(value);
                quote! { #[len_const(#value)] }
            }
            "array_count_field" => {
                let value = value?;
                quote! { #[Windows::Win32::Metadata::NativeArrayInfo(CountFieldName = #value)] }
            }
            "memory_size_param" => {
                let value = parse_integer::<i16>(value?)?;
                let value = Literal::i16_unsuffixed(value);
                quote! { #[size_param(#value)] }
            }
            "can_return_errors_as_success" => quote! { #[errors_as_success] },
            "can_return_multiple_success_values" => quote! { #[multiple_success_values] },
            "retained" => quote! { #[retained] },
            "ignore_if_return" => {
                let value = value?;
                quote! { #[ignore_if_return(#value)] }
            }
            "also_usable_for" => {
                let value = value?;
                quote! { #[also_usable_for(#value)] }
            }
            "associated_enum" => {
                let value = value?;
                quote! { #[associated_enum(#value)] }
            }
            "associated_constant" => {
                let value = value?;
                quote! { #[associated_constant(#value)] }
            }
            "native_inheritance" => {
                let value = value?;
                quote! { #[native_inheritance(#value)] }
            }
            "struct_size_field" => {
                let value = value?;
                quote! { #[struct_size_field(#value)] }
            }
            "native_encoding" => {
                let value = value?;
                quote! { #[encoding(#value)] }
            }
            "ansi" => quote! { #[ansi] },
            "unicode" => quote! { #[unicode] },
            "agile" => quote! { #[agile] },
            "const" => quote! { #[native_const] },
            "supported_os" => {
                let value = value?;
                quote! { #[supported_os(#value)] }
            }
            "static_library" => {
                let value = value?;
                quote! { #[static_library(#value)] }
            }
            "in"
            | "out"
            | "optional"
            | "reserved"
            | "retval"
            | "com_out_ptr"
            | "canonical_name"
            | "reduce_pointer_level"
            | "import_library"
            | "set_last_error" => return None,
            _ => return None,
        })
    }
}

pub fn apply_metadata_type_annotations(
    mut ty: metadata::Type,
    annotations: &[Win32MetadataAnnotation],
) -> metadata::Type {
    for annotation in annotations {
        if annotation.is("reduce_pointer_level") {
            ty = match ty {
                metadata::Type::PtrMut(inner, 1) | metadata::Type::PtrConst(inner, 1) => *inner,
                metadata::Type::PtrMut(inner, depth) => metadata::Type::PtrMut(inner, depth - 1),
                metadata::Type::PtrConst(inner, depth) => {
                    metadata::Type::PtrConst(inner, depth - 1)
                }
                other => other,
            };
        }
    }
    ty
}

fn parse_integer<T>(value: &str) -> Option<T>
where
    T: TryFrom<i128>,
{
    let value = value.trim();
    let (negative, value) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    let value = value.trim_end_matches(['u', 'U', 'l', 'L']);
    let (radix, digits) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .map_or((10, value), |digits| (16, digits));
    let magnitude = i128::from_str_radix(digits, radix).ok()?;
    T::try_from(if negative { -magnitude } else { magnitude }).ok()
}

fn parse_win32_metadata_annotation(spelling: &str) -> Option<Win32MetadataAnnotation> {
    let payload = spelling.strip_prefix(WIN32_METADATA_PREFIX)?;
    let (key, value) = payload
        .split_once('=')
        .map_or((payload, None), |(key, value)| {
            (key, Some(value.to_string()))
        });
    Some(Win32MetadataAnnotation {
        key: key.to_string(),
        value,
    })
}

pub fn validate_win32_metadata_annotation_tree(cursor: &Cursor) -> Result<(), Error> {
    for child in cursor.children() {
        if child.kind() == CXCursor_AnnotateAttr {
            let spelling = child.name();
            if spelling.starts_with(WIN32_METADATA_PREFIX) {
                validate_win32_metadata_annotation(cursor.kind(), &spelling, &child)?;
            }
        } else {
            validate_win32_metadata_annotation_tree(&child)?;
        }
    }
    Ok(())
}

pub fn win32_metadata_canonical_names(cursor: &Cursor) -> HashMap<String, String> {
    fn collect(cursor: &Cursor, result: &mut HashMap<String, String>) {
        if cursor.kind() == CXCursor_TypedefDecl {
            for annotation in extract_win32_metadata_annotations(cursor) {
                if annotation.is("canonical_name")
                    && let Some(value) = annotation.value
                {
                    result.insert(cursor.name(), value);
                }
            }
        }
        for child in cursor.children() {
            collect(&child, result);
        }
    }

    let mut result = HashMap::new();
    collect(cursor, &mut result);
    result
}

fn validate_win32_metadata_annotation(
    target: CXCursorKind,
    spelling: &str,
    cursor: &Cursor,
) -> Result<(), Error> {
    let annotation = parse_win32_metadata_annotation(spelling).unwrap();
    let requires_value = matches!(
        annotation.key.as_str(),
        "import_library"
            | "supported_os"
            | "static_library"
            | "raii_free"
            | "invalid_handle"
            | "free_with"
            | "array_count_param"
            | "array_count_const"
            | "array_count_field"
            | "memory_size_param"
            | "ignore_if_return"
            | "also_usable_for"
            | "associated_enum"
            | "associated_constant"
            | "native_inheritance"
            | "struct_size_field"
            | "native_encoding"
            | "canonical_name"
    );
    let valueless = matches!(
        annotation.key.as_str(),
        "set_last_error"
            | "can_return_errors_as_success"
            | "can_return_multiple_success_values"
            | "agile"
            | "do_not_release"
            | "not_null_terminated"
            | "null_null_terminated"
            | "retained"
            | "in"
            | "out"
            | "optional"
            | "reserved"
            | "retval"
            | "com_out_ptr"
            | "const"
            | "ansi"
            | "unicode"
            | "reduce_pointer_level"
    );

    let error = if !requires_value && !valueless {
        Some(format!(
            "unknown win32metadata annotation `{}`",
            annotation.key
        ))
    } else if requires_value
        && annotation
            .value
            .as_deref()
            .is_none_or(|value| value.is_empty())
    {
        Some(format!(
            "win32metadata annotation `{}` requires a value",
            annotation.key
        ))
    } else if valueless && annotation.value.is_some() {
        Some(format!(
            "win32metadata annotation `{}` does not accept a value",
            annotation.key
        ))
    } else if !annotation_target_allowed(&annotation.key, target) {
        Some(format!(
            "win32metadata annotation `{}` is not valid on this declaration",
            annotation.key
        ))
    } else {
        None
    };

    if let Some(message) = error {
        let (file, line, column) = cursor.source_location();
        Err(Error::new(&message, &file, line, column))
    } else {
        Ok(())
    }
}

fn annotation_target_allowed(key: &str, target: CXCursorKind) -> bool {
    match key {
        "set_last_error" | "import_library" | "static_library" => target == CXCursor_FunctionDecl,
        "can_return_errors_as_success" | "can_return_multiple_success_values" => {
            matches!(target, CXCursor_FunctionDecl | CXCursor_CXXMethod)
        }
        "agile" => matches!(
            target,
            CXCursor_ClassDecl | CXCursor_StructDecl | CXCursor_ClassTemplate
        ),
        "raii_free" | "invalid_handle" => matches!(
            target,
            CXCursor_FunctionDecl | CXCursor_CXXMethod | CXCursor_ParmDecl | CXCursor_TypedefDecl
        ),
        "free_with" | "do_not_release" | "not_null_terminated" | "null_null_terminated" => {
            matches!(
                target,
                CXCursor_FunctionDecl
                    | CXCursor_CXXMethod
                    | CXCursor_ParmDecl
                    | CXCursor_FieldDecl
                    | CXCursor_TypedefDecl
            )
        }
        "retained" | "ignore_if_return" | "array_count_param" | "array_count_const"
        | "memory_size_param" | "in" | "out" | "optional" | "reserved" | "retval"
        | "com_out_ptr" => target == CXCursor_ParmDecl,
        "array_count_field" => target == CXCursor_FieldDecl,
        "also_usable_for" | "canonical_name" => target == CXCursor_TypedefDecl,
        "associated_enum" => matches!(
            target,
            CXCursor_ParmDecl | CXCursor_FieldDecl | CXCursor_VarDecl
        ),
        "associated_constant" => target == CXCursor_EnumDecl,
        "native_inheritance" | "struct_size_field" => {
            matches!(target, CXCursor_ClassDecl | CXCursor_StructDecl)
        }
        "native_encoding" => matches!(target, CXCursor_FieldDecl | CXCursor_VarDecl),
        "const" => matches!(target, CXCursor_ParmDecl | CXCursor_FieldDecl),
        "ansi" | "unicode" => matches!(
            target,
            CXCursor_FunctionDecl | CXCursor_CXXMethod | CXCursor_FieldDecl | CXCursor_VarDecl
        ),
        "supported_os" => matches!(
            target,
            CXCursor_FunctionDecl
                | CXCursor_CXXMethod
                | CXCursor_ClassDecl
                | CXCursor_StructDecl
                | CXCursor_EnumDecl
                | CXCursor_TypedefDecl
        ),
        "reduce_pointer_level" => matches!(target, CXCursor_ParmDecl | CXCursor_FieldDecl),
        _ => false,
    }
}

pub fn extract_win32_metadata_annotations(cursor: &Cursor) -> Vec<Win32MetadataAnnotation> {
    cursor
        .children()
        .into_iter()
        .filter(|child| child.kind() == CXCursor_AnnotateAttr)
        .filter_map(|child| parse_win32_metadata_annotation(&child.name()))
        .collect()
}

pub fn win32_metadata_attrs(
    annotations: &[Win32MetadataAnnotation],
    include_return: bool,
) -> Vec<TokenStream> {
    annotations
        .iter()
        .filter(|annotation| annotation.targets_return() == include_return)
        .filter_map(Win32MetadataAnnotation::to_rdl_attr)
        .collect()
}

pub fn all_win32_metadata_attrs(annotations: &[Win32MetadataAnnotation]) -> Vec<TokenStream> {
    annotations
        .iter()
        .filter_map(Win32MetadataAnnotation::to_rdl_attr)
        .collect()
}

/// SAL, legacy direction, and MIDL parameter flags; unset flags let the reader infer from type.
#[derive(Debug, Default, Clone)]
pub struct ParamAnnotation {
    pub in_param: bool,
    pub out_param: bool,
    pub optional: bool,
    pub retval: bool,
    pub reserved: bool,
    pub com_out_ptr: bool,
    /// Bare `_COM_Outptr_` token; promoted only for caller-chosen `void**` outputs.
    pub com_out_ptr_token: bool,
    /// Pure `_z_` string SAL without a count; counted buffers stay raw pointers.
    pub null_terminated: bool,
    /// SAL size before name -> index resolution.
    pub size: Option<SalSize>,
    /// Resolved array/size attribute.
    pub array: Option<ArrayInfo>,
    /// Explicit win32metadata annotations carried by Clang `AnnotateAttr` cursors.
    pub win32_metadata: Vec<Win32MetadataAnnotation>,
}

impl ParamAnnotation {
    pub fn has_sal_annotation(&self) -> bool {
        self.in_param
            || self.out_param
            || self.optional
            || self.retval
            || self.reserved
            || self.com_out_ptr
            || self.size.is_some()
            || self.array.is_some()
    }

    pub fn is_annotated(&self) -> bool {
        self.has_sal_annotation() || !self.win32_metadata.is_empty()
    }
}

#[derive(Debug, Clone)]
pub enum SalSizeArg {
    Const(i32),
    Name(String),
}

#[derive(Debug, Clone)]
pub struct SalSize {
    /// `*_bytes_*` maps to `MemorySize`; other size macros map to element counts.
    pub bytes: bool,
    pub arg: SalSizeArg,
}

#[derive(Debug, Clone)]
pub enum ArrayInfo {
    CountParamIndex(i16),
    CountConst(i32),
    BytesParamIndex(i16),
}

#[derive(Debug, Default)]
pub struct MethodAnnotation {
    pub is_propget: bool,
    pub is_propput: bool,
}

/// Scans only comments before the method name so parameter comments are ignored.
pub fn extract_method_annotation(
    tokens: &[(CXTokenKind, String)],
    method_name: &str,
) -> MethodAnnotation {
    let mut annotation = MethodAnnotation::default();
    for (kind, spelling) in tokens {
        if *kind == CXToken_Identifier && spelling == method_name {
            break;
        }
        if *kind == CXToken_Comment {
            if spelling.contains("[propget]") {
                annotation.is_propget = true;
            }
            if spelling.contains("[propput]") {
                annotation.is_propput = true;
            }
        }
    }
    annotation
}

/// Extracts SAL attributes from a parameter; MIDL block comments are scanned separately
/// because they sit outside the `ParmDecl` extent.
pub fn extract_param_annotation(cursor: &Cursor, tu: &TranslationUnit) -> ParamAnnotation {
    let mut annotation = ParamAnnotation::default();

    for child in cursor.children() {
        match child.kind() {
            CXCursor_AnnotateAttr => {
                // Portable SAL stubs keep the macro argument in the spelling.
                let spelling = child.name();
                if let Some(metadata) = parse_win32_metadata_annotation(&spelling) {
                    match metadata.key.as_str() {
                        "in" => annotation.in_param = true,
                        "out" => annotation.out_param = true,
                        "optional" => annotation.optional = true,
                        "reserved" => {
                            annotation.reserved = true;
                            annotation.optional = true;
                        }
                        "retval" => annotation.retval = true,
                        "com_out_ptr" => {
                            annotation.com_out_ptr = true;
                            annotation.out_param = true;
                        }
                        _ => annotation.win32_metadata.push(metadata),
                    }
                    continue;
                }
                let (name, arg) = split_sal_annotation(&spelling);
                apply_sal_string(name, &mut annotation);
                if annotation.size.is_none() {
                    annotation.size = capture_sal_size(name, arg);
                }
            }
            CXCursor_UnexposedAttr => {
                // Windows SDK SAL attributes surface as unexposed attribute tokens.
                let tokens = tu.tokenize(tu.to_expansion_range(child.extent()));
                for (kind, spelling) in &tokens {
                    if *kind == CXToken_Identifier {
                        apply_sal_string(spelling, &mut annotation);
                    }
                }
            }
            _ => {}
        }
    }

    annotation
}

fn split_sal_annotation(s: &str) -> (&str, Option<&str>) {
    match s.find('(') {
        Some(open) if s.ends_with(')') => (&s[..open], Some(&s[open + 1..s.len() - 1])),
        _ => (s, None),
    }
}

/// Captures only unambiguous literal or identifier SAL sizes.
fn capture_sal_size(name: &str, arg: Option<&str>) -> Option<SalSize> {
    let bytes = sal_size_kind(name)?;
    // `_*_to_`/`_*_part_` variants carry the buffer extent first.
    let first = arg?.split(',').next()?.trim();
    Some(SalSize {
        bytes,
        arg: parse_size_arg(first)?,
    })
}

fn sal_size_kind(name: &str) -> Option<bool> {
    let is_size =
        name.contains("_reads_") || name.contains("_writes_") || name.contains("_updates_");
    is_size.then(|| name.contains("_bytes"))
}

/// Parses simple SAL size arguments; strips `*param` because the pointer parameter is
/// the count carrier.
fn parse_size_arg(s: &str) -> Option<SalSizeArg> {
    let s = s.trim();
    if let Some(n) = parse_int_literal(s) {
        Some(SalSizeArg::Const(n))
    } else {
        let name = s.trim_start_matches('*').trim();
        is_c_identifier(name).then(|| SalSizeArg::Name(name.to_string()))
    }
}

fn parse_int_literal(s: &str) -> Option<i32> {
    let t = s.trim_end_matches(['u', 'U', 'l', 'L']);
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i32::from_str_radix(hex, 16).ok()
    } else {
        t.parse::<i32>().ok()
    }
}

fn is_c_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Caller-chosen COM out-pointers use `void**`; concrete interface outputs keep their
/// declared interface type.
fn is_void_double_ptr(ty: &metadata::Type) -> bool {
    let mut depth = 0usize;
    let mut cur = ty;
    loop {
        match cur {
            metadata::Type::PtrMut(inner, d) => {
                depth += *d;
                cur = inner;
            }
            metadata::Type::Void => return depth == 2,
            _ => return false,
        }
    }
}

/// Conventional `REFIID` selector names for caller-chosen COM out-pointers.
const IID_SELECTOR_PARAM_NAMES: [&str; 3] = ["riid", "iid", "riidltf"];

fn is_hresult(ty: &metadata::Type) -> bool {
    matches!(ty, metadata::Type::ValueName(tn) if tn.name == "HRESULT")
}

/// `REFIID`/`REFCLSID` shape after IID aliases have collapsed to `GUID`.
fn is_const_guid_ptr(ty: &metadata::Type) -> bool {
    matches!(ty, metadata::Type::PtrConst(inner, 1)
        if matches!(inner.as_ref(), metadata::Type::ValueName(tn) if tn.name == "GUID"))
}

/// Some `[iid_is]` creators spell the caller-chosen out as `IUnknown**` or
/// `IInspectable**`; concrete interface outputs must stay typed.
fn is_base_interface_out_ptr(ty: &metadata::Type) -> bool {
    let metadata::Type::PtrMut(inner, 1) = ty else {
        return false;
    };
    matches!(inner.as_ref(),
        metadata::Type::ClassName(tn) | metadata::Type::ValueName(tn)
            if tn.name == "IUnknown" || tn.name == "IInspectable")
}

/// Recovers missing `[iid_is]` for HRESULT + REFIID + single-object `void**` or base
/// interface outputs. Array outputs, `In` parameters, and concrete interfaces are excluded.
pub(crate) fn infer_iid_is(params: &mut [Param], return_type: &metadata::Type) {
    if !is_hresult(return_type) {
        return;
    }
    let has_iid_selector = params
        .iter()
        .any(|p| IID_SELECTOR_PARAM_NAMES.contains(&p.name.as_str()) && is_const_guid_ptr(&p.ty));
    if !has_iid_selector {
        return;
    }
    for param in params.iter_mut() {
        if !param.annotation.com_out_ptr
            && !param.annotation.in_param
            && param.annotation.array.is_none()
            && param.annotation.size.is_none()
            && (is_void_double_ptr(&param.ty) || is_base_interface_out_ptr(&param.ty))
        {
            param.annotation.com_out_ptr = true;
            // Keep inferred ComOutPtr direction aligned with annotated paths.
            param.annotation.out_param = true;
            // Normalize inferred ComOutPtr to the same `void**` RDL shape as annotated ones.
            param.ty = metadata::Type::PtrMut(
                Box::new(metadata::Type::PtrMut(Box::new(metadata::Type::Void), 1)),
                1,
            );
        }
    }
}

pub(crate) fn parse_params(
    cursor: &Cursor,
    midl_annotations: &[ParamAnnotation],
    parser: &mut Parser<'_>,
) -> Vec<Param> {
    let mut params = vec![];
    let mut param_idx = 0usize;
    for child in cursor.children() {
        if child.kind() != CXCursor_ParmDecl {
            continue;
        }
        let mut name = child.name();
        if name.is_empty() || is_midl_synthetic_param_name(&name) {
            name = format!("param{param_idx}");
        }
        let sal_annotation = extract_param_annotation(&child, parser.tu);
        let mut annotation = if sal_annotation.has_sal_annotation() {
            sal_annotation
        } else {
            let mut fallback = midl_annotations.get(param_idx).cloned().unwrap_or_default();
            fallback.win32_metadata = sal_annotation.win32_metadata;
            fallback
        };
        let mut ty = param_metadata_type(&child.ty(), &annotation, parser);
        ty = apply_metadata_type_annotations(ty, &annotation.win32_metadata);
        // Token-recovered `_COM_Outptr_` becomes ComOutPtr only for caller-chosen `void**`.
        if annotation.com_out_ptr_token && is_void_double_ptr(&ty) {
            annotation.com_out_ptr = true;
        }
        if annotation.com_out_ptr {
            ty = metadata::Type::PtrMut(
                Box::new(metadata::Type::PtrMut(Box::new(metadata::Type::Void), 1)),
                1,
            );
        }
        // Inline fixed arrays get `CountConst` only when direction makes the role clear;
        // SAL counts take precedence below.
        if annotation.size.is_none()
            && (annotation.in_param
                || annotation.out_param
                || matches!(ty, metadata::Type::PtrConst(..)))
            && let Some(n) = inline_array_param_count(&child.ty())
        {
            annotation.array = Some(ArrayInfo::CountConst(n));
        }
        param_idx += 1;
        params.push(Param {
            name,
            ty,
            annotation,
        });
    }
    resolve_param_array_info(&mut params);
    params
}

/// Resolves SAL sizes to parameter indices or constants; unresolved names and constant
/// byte counts are dropped.
pub fn resolve_param_array_info(params: &mut [Param]) {
    let index_of: HashMap<&str, i16> = params
        .iter()
        .enumerate()
        .map(|(i, p)| (p.name.as_str(), i as i16))
        .collect();

    let resolved: Vec<Option<ArrayInfo>> = params
        .iter()
        .map(|p| {
            p.annotation.size.as_ref().and_then(|size| match &size.arg {
                SalSizeArg::Const(n) if !size.bytes => Some(ArrayInfo::CountConst(*n)),
                SalSizeArg::Const(_) => None,
                SalSizeArg::Name(name) => index_of.get(name.as_str()).map(|&idx| {
                    if size.bytes {
                        ArrayInfo::BytesParamIndex(idx)
                    } else {
                        ArrayInfo::CountParamIndex(idx)
                    }
                }),
            })
        })
        .collect();

    // SAL counts override inline-array counts; unresolved SAL preserves the inline count.
    for (p, info) in params.iter_mut().zip(resolved) {
        if info.is_some() {
            p.annotation.array = info;
        }
    }
}

/// Maps MIDL parameter block comments by token position; also recovers SAL tokens that
/// abstract COM methods do not expose as `ParmDecl` attributes.
pub fn scan_method_param_annotations(
    tokens: &[(CXTokenKind, String)],
    method_name: &str,
    macro_defs: &HashMap<String, Vec<String>>,
) -> Vec<ParamAnnotation> {
    let mut result = Vec::new();
    let mut current = ParamAnnotation::default();
    let mut past_name = false;
    let mut paren_depth: i32 = 0;
    let mut in_params = false;

    for (kind, spelling) in tokens {
        if !past_name {
            if *kind == CXToken_Identifier && spelling == method_name {
                past_name = true;
            }
            continue;
        }

        match (*kind, spelling.as_str()) {
            (CXToken_Punctuation, "(") => {
                paren_depth += 1;
                if paren_depth == 1 {
                    in_params = true;
                    current = ParamAnnotation::default();
                }
            }
            (CXToken_Punctuation, ")") => {
                if paren_depth > 0 {
                    paren_depth -= 1;
                }
                if paren_depth == 0 && in_params {
                    // Avoid a fake default for zero-/single-parameter methods with no MIDL comment.
                    if !result.is_empty() || current.is_annotated() {
                        result.push(current.clone());
                    }
                    break;
                }
            }
            (CXToken_Punctuation, ",") if in_params && paren_depth == 1 => {
                result.push(current.clone());
                current = ParamAnnotation::default();
            }
            (CXToken_Comment, s) if in_params && paren_depth == 1 => {
                apply_midl_param_comment(s, &mut current);
            }
            // The SDK's empty legacy macros are the predecessors of `_In_`, `_Out_`, and `_opt_`.
            // Require the empty macro definition so unrelated identifiers are not annotations.
            (CXToken_Identifier, "IN")
                if in_params
                    && paren_depth == 1
                    && macro_defs.get("IN").is_some_and(Vec::is_empty) =>
            {
                current.in_param = true;
            }
            (CXToken_Identifier, "OUT")
                if in_params
                    && paren_depth == 1
                    && macro_defs.get("OUT").is_some_and(Vec::is_empty) =>
            {
                current.out_param = true;
            }
            (CXToken_Identifier, "OPTIONAL")
                if in_params
                    && paren_depth == 1
                    && macro_defs.get("OPTIONAL").is_some_and(Vec::is_empty) =>
            {
                current.optional = true;
            }
            // Recover bare pure `_z_` SAL tokens for COM methods; counted buffers stay raw.
            (CXToken_Identifier, s)
                if in_params && paren_depth == 1 && s.starts_with("_COM_Outptr_") =>
            {
                current.out_param = true;
                current.com_out_ptr_token = true;
                if s.starts_with("_COM_Outptr_opt") {
                    current.optional = true;
                }
            }
            // Abstract COM methods can expose `_COM_Outptr_` only as a token. Record it
            // speculatively, including `_opt_`; `parse_params` promotes only `void**`.
            (CXToken_Identifier, s)
                if in_params
                    && paren_depth == 1
                    && matches!(
                        s,
                        "_In_z_" | "_In_opt_z_" | "_Out_z_" | "_Inout_z_" | "_Inout_opt_z_"
                    ) =>
            {
                apply_sal_string(s, &mut current);
            }
            _ => {}
        }
    }

    result
}

pub fn apply_midl_param_comment(comment: &str, annotation: &mut ParamAnnotation) {
    if comment.contains("[in]") {
        annotation.in_param = true;
    }
    if comment.contains("[out]") {
        annotation.out_param = true;
    }
    if comment.contains("[retval]") {
        annotation.retval = true;
    }
    if comment.contains("[optional]") {
        annotation.optional = true;
    }
    // `[iid_is]` on an output has the same ComOutPtr meaning as `_COM_Outptr_`.
    if comment.contains("[iid_is]") && annotation.out_param {
        annotation.com_out_ptr = true;
    }
}

/// Handles known Win32 SAL names and ignores unrelated annotations.
fn apply_sal_string(sal: &str, annotation: &mut ParamAnnotation) {
    // Exact directional prefixes; near-namesakes (`_Outref_`, `_Inexpressible_`) carry no meaning.
    if sal.starts_with("_In_") || sal.starts_with("_Inout_") {
        annotation.in_param = true;
    }
    if sal.starts_with("_Out_")
        || sal.starts_with("_Outptr_")
        || sal.starts_with("_COM_Outptr_")
        || sal.starts_with("_Inout_")
    {
        annotation.out_param = true;
    }
    // `_opt_`, or a plain (non-`_COM_`) `_Outptr_..._result_maybenull_`, marks optional.
    if sal.contains("_opt_") || (sal.starts_with("_Outptr_") && sal.contains("_result_maybenull_"))
    {
        annotation.optional = true;
    }

    if sal == "_Reserved_" {
        annotation.reserved = true;
    }

    // Only the pure `_z_` names promote a raw character pointer to a string wrapper.
    if matches!(
        sal,
        "_In_z_" | "_In_opt_z_" | "_Out_z_" | "_Inout_z_" | "_Inout_opt_z_"
    ) {
        annotation.null_terminated = true;
    }
    if sal.starts_with("_COM_Outptr_") {
        annotation.com_out_ptr = true;
    }
}

/// Emits only attributes needed to match reader defaults and writer ordering.
pub fn param_attrs_for_annotation(
    annotation: &ParamAnnotation,
    ty: &metadata::Type,
) -> Vec<TokenStream> {
    if !annotation.is_annotated() {
        return vec![];
    }

    let in_param = annotation.in_param;
    let out_param = annotation.out_param;
    let optional = annotation.optional;
    let retval = annotation.retval;

    let is_mutable = matches!(ty, metadata::Type::RefMut(_) | metadata::Type::PtrMut(..));

    let mut attrs = vec![];

    // Keep array/size attributes before direction attributes to match the writer.
    if let Some(array) = &annotation.array {
        attrs.push(array_info_attr(array));
    }

    if annotation.reserved {
        attrs.push(quote! { #[reserved] });
    }

    // Emit the source spelling; the RDL reader maps it to `ComOutPtrAttribute`.
    if annotation.com_out_ptr {
        attrs.push(quote! { #[iid_is] });
    }

    // Emit `#[in]` only when it overrides the type-based default or marks In+Out.
    if in_param && (out_param || is_mutable) {
        attrs.push(quote! { #[r#in] });
    }

    // Emit `#[out]` only when it overrides the type-based default or marks In+Out.
    if out_param && (in_param || !is_mutable) {
        attrs.push(quote! { #[out] });
    }

    if optional {
        attrs.push(quote! { #[opt] });
    }

    if retval {
        attrs.push(quote! { #[retval] });
    }

    attrs.extend(
        annotation
            .win32_metadata
            .iter()
            .filter_map(Win32MetadataAnnotation::to_rdl_attr),
    );

    attrs
}

/// Detects no-return from type spelling or `_Analysis_noreturn_` SAL.
pub fn detect_does_not_return(cursor: &Cursor) -> bool {
    if cursor.ty().spelling().contains("noreturn") {
        return true;
    }
    cursor
        .children()
        .iter()
        .any(|c| c.kind() == CXCursor_AnnotateAttr && c.name() == "_Analysis_noreturn_")
}

pub fn does_not_return_attr() -> TokenStream {
    quote! { #[noreturn] }
}

/// Emits the RDL pseudo-attributes that map to native array and memory-size metadata.
fn array_info_attr(info: &ArrayInfo) -> TokenStream {
    let (name, lit) = match info {
        ArrayInfo::CountParamIndex(i) => ("len_param", Literal::i16_unsuffixed(*i)),
        ArrayInfo::CountConst(n) => ("len_const", Literal::i32_unsuffixed(*n)),
        ArrayInfo::BytesParamIndex(i) => ("size_param", Literal::i16_unsuffixed(*i)),
    };

    let name = write_ident(name);
    quote! { #[#name(#lit)] }
}
