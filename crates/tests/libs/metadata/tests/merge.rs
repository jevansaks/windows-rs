use windows_metadata::*;

/// Builds a winmd from inline RDL into `dir/name.winmd` and returns its path.
fn winmd(dir: &std::path::Path, name: &str, rdl: &str) -> String {
    let rdl_path = dir.join(format!("{name}.rdl"));
    std::fs::write(&rdl_path, rdl).unwrap();
    let out = dir.join(format!("{name}.winmd"));
    windows_rdl::reader()
        .input(&rdl_path)
        .output(&out)
        .write()
        .unwrap();
    out.to_string_lossy().into_owned()
}

fn winmd_with_default_refs(dir: &std::path::Path, name: &str, rdl: &str) -> String {
    let rdl_path = dir.join(format!("{name}.rdl"));
    std::fs::write(&rdl_path, rdl).unwrap();
    let out = dir.join(format!("{name}.winmd"));
    windows_rdl::reader()
        .input(&rdl_path)
        .reference_default()
        .output(&out)
        .write()
        .unwrap();
    out.to_string_lossy().into_owned()
}

fn explicit_layout_winmd(dir: &std::path::Path, name: &str) -> String {
    let mut file = writer::File::new(name);
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef(
        "Test",
        "EXPLICIT",
        value_type,
        TypeAttributes::ExplicitLayout | TypeAttributes::Sealed | TypeAttributes::Public,
    );
    let first = file.Field("first", &Type::I32, FieldAttributes::Public);
    file.FieldLayout(first, 4);
    let second = file.Field("second", &Type::U64, FieldAttributes::Public);
    file.FieldLayout(second, 12);
    let out = dir.join(format!("{name}.winmd"));
    std::fs::write(&out, file.into_stream()).unwrap();
    out.to_string_lossy().into_owned()
}

fn arch_bits(field: reader::Field) -> Option<i32> {
    field.attributes().find_map(|a| {
        (a.ctor().parent().name() == "SupportedArchitectureAttribute").then(|| {
            match a.value().first() {
                Some((_, Value::I32(v))) => *v,
                _ => 0,
            }
        })
    })
}

fn type_arch_bits(ty: reader::TypeDef) -> Option<i32> {
    ty.has_attribute("SupportedArchitectureAttribute")
        .then(|| ty.arches())
}

fn nested_field_offsets(index: &reader::Index, namespace: &str, name: &str) -> Vec<Option<u32>> {
    let outer = index.expect(namespace, name);
    let inner = index.nested(outer).next().unwrap();
    inner.fields().map(|field| field.offset()).collect()
}

fn type_ref<'a>(index: &'a reader::Index, namespace: &str, name: &str) -> reader::TypeRef<'a> {
    index
        .type_refs()
        .find(|ty| {
            let qualified = ty.qualified_name();
            qualified.namespace == namespace && qualified.name == name
        })
        .unwrap_or_else(|| panic!("missing TypeRef {namespace}.{name}"))
}

#[derive(Debug, PartialEq, Eq)]
struct AssemblyIdentity {
    version: (u16, u16, u16, u16),
    flags: u32,
    public_key_or_token: Vec<u8>,
    name: String,
    culture: String,
    hash_value: Vec<u8>,
}

fn assembly_identity(ty: reader::TypeRef) -> Option<AssemblyIdentity> {
    let assembly = ty.assembly()?;
    Some(AssemblyIdentity {
        version: assembly.version(),
        flags: assembly.flags().0,
        public_key_or_token: assembly.public_key_or_token().to_vec(),
        name: assembly.name().to_string(),
        culture: assembly.culture().to_string(),
        hash_value: assembly.hash_value().to_vec(),
    })
}

fn external_type_winmd(dir: &std::path::Path, name: &str, assembly_name: &str) -> String {
    let mut reference = writer::File::new(assembly_name);
    let value_type = writer::TypeDefOrRef::TypeRef(reference.TypeRef("System", "ValueType"));
    reference.TypeDef(
        "External",
        "VALUE",
        value_type,
        TypeAttributes::SequentialLayout | TypeAttributes::Sealed | TypeAttributes::Public,
    );
    let reference = reader::Index::new(vec![reader::File::new(reference.into_stream()).unwrap()]);

    let mut file = writer::File::new(name);
    file.set_reference(reference);
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef(
        "Test",
        "CONSUMER",
        value_type,
        TypeAttributes::SequentialLayout | TypeAttributes::Sealed | TypeAttributes::Public,
    );
    file.Field(
        "value",
        &Type::value_named("External", "VALUE"),
        FieldAttributes::Public,
    );
    let out = dir.join(format!("{name}.winmd"));
    std::fs::write(&out, file.into_stream()).unwrap();
    out.to_string_lossy().into_owned()
}

fn named_type_definitions_winmd(dir: &std::path::Path, name: &str, assembly_name: &str) -> String {
    let mut file = writer::File::new(assembly_name);
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef(
        "N",
        "T",
        value_type,
        TypeAttributes::SequentialLayout | TypeAttributes::Sealed | TypeAttributes::Public,
    );
    file.Field("value", &Type::I32, FieldAttributes::Public);

    let outer = file.TypeDef(
        "N",
        "Outer",
        value_type,
        TypeAttributes::SequentialLayout | TypeAttributes::Sealed | TypeAttributes::Public,
    );
    let inner = file.TypeDef(
        "",
        "Inner",
        value_type,
        TypeAttributes::SequentialLayout | TypeAttributes::Sealed | TypeAttributes::NestedPublic,
    );
    file.NestedClass(inner, outer);
    file.Field("value", &Type::I32, FieldAttributes::Public);

    let out = dir.join(format!("{name}.winmd"));
    std::fs::write(&out, file.into_stream()).unwrap();
    out.to_string_lossy().into_owned()
}

fn named_type_consumer_winmd(
    dir: &std::path::Path,
    name: &str,
    reference: &str,
    fields: &[(&str, &str, &str)],
) -> String {
    let reference = reader::Index::read(reference).unwrap();
    let mut file = writer::File::new(name);
    file.set_reference(reference);
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef(
        "Test",
        "CONSUMER",
        value_type,
        TypeAttributes::SequentialLayout | TypeAttributes::Sealed | TypeAttributes::Public,
    );
    for (field, namespace, ty) in fields {
        file.Field(
            field,
            &Type::value_named(namespace, ty),
            FieldAttributes::Public,
        );
    }

    let out = dir.join(format!("{name}.winmd"));
    std::fs::write(&out, file.into_stream()).unwrap();
    out.to_string_lossy().into_owned()
}

#[test]
fn explicit_assembly_name() {
    let dir = std::env::temp_dir().join("win_merge_assembly_name");
    std::fs::create_dir_all(&dir).unwrap();

    let input = winmd(
        &dir,
        "input",
        "#[win32] mod Test { struct VALUE { value: i32 } }",
    );
    let output = dir.join("temporary-name.winmd");
    merge()
        .input(input)
        .assembly_name("Test.Assembly")
        .output(&output)
        .merge()
        .unwrap();

    let file = reader::File::read(output).unwrap();
    assert_eq!(file.assembly_name(), Some("Test.Assembly"));
}

#[test]
fn merge_preserves_named_property_kinds() {
    let dir = std::env::temp_dir().join("win_merge_named_property_kind");
    std::fs::create_dir_all(&dir).unwrap();

    let input = winmd(
        &dir,
        "input",
        r#"
            #[win32]
            mod Windows {
                mod Win32 {
                    mod Foundation {
                        mod Metadata {
                            attribute SupportedOSPlatformAttribute {
                                fn(platform: String);
                            }
                        }
                    }
                }
            }
        "#,
    );
    let output = dir.join("merged.winmd");
    merge().input(input).output(&output).merge().unwrap();

    let index = reader::Index::read(output).unwrap();
    let attribute = index.expect(
        "Windows.Win32.Foundation.Metadata",
        "SupportedOSPlatformAttribute",
    );
    let usage = attribute.attributes().next().unwrap();
    assert_eq!(usage.name(), "AttributeUsageAttribute");
    assert_eq!(usage.named_arg_kinds(), [0x54]);
}

#[test]
fn arch_merge_constants() {
    let dir = std::env::temp_dir().join("win_merge_test");
    std::fs::create_dir_all(&dir).unwrap();

    let x64 = winmd(
        &dir,
        "x64",
        "mod Test { const SHARED: i32 = 7; const CTX_ALL: i32 = 100; const X64_ONLY: i32 = 1; }",
    );
    let arm = winmd(
        &dir,
        "arm",
        "mod Test { const SHARED: i32 = 7; const CTX_ALL: i32 = 200; const ARM_ONLY: i32 = 2; }",
    );

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    let apis = index.types().find(|t| t.name() == "Apis").unwrap();
    let consts: Vec<_> = apis.fields().filter(|f| f.constant().is_some()).collect();

    let shared: Vec<_> = consts.iter().filter(|f| f.name() == "SHARED").collect();
    assert_eq!(shared.len(), 1);
    assert_eq!(arch_bits(*shared[0]), None);

    let mut ctx: Vec<_> = consts
        .iter()
        .filter(|f| f.name() == "CTX_ALL")
        .filter_map(|f| arch_bits(*f))
        .collect();
    ctx.sort();
    assert_eq!(ctx, vec![2, 4]);

    // Arch-only constants are present and tagged.
    let x64_only = consts.iter().find(|f| f.name() == "X64_ONLY").unwrap();
    assert_eq!(arch_bits(*x64_only), Some(2));
    let arm_only = consts.iter().find(|f| f.name() == "ARM_ONLY").unwrap();
    assert_eq!(arch_bits(*arm_only), Some(4));
}

#[test]
fn arch_merge_preserves_explicit_field_layouts() {
    let dir = std::env::temp_dir().join("win_merge_field_layout");
    std::fs::create_dir_all(&dir).unwrap();

    let source =
        "#[win32] mod Test { struct VALUE { data: union { signed: i32, unsigned: u32 } } }";
    let x64 = winmd(&dir, "x64", source);
    let arm = winmd(&dir, "arm", source);
    let x86 = winmd(&dir, "x86", source);

    for input in [&x64, &arm, &x86] {
        let index = reader::Index::read(input).unwrap();
        assert_eq!(
            nested_field_offsets(&index, "Test", "VALUE"),
            [Some(0), Some(0)],
            "RDL compilation must emit explicit offsets before architecture merge"
        );
    }

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged).unwrap();
    assert_eq!(
        nested_field_offsets(&index, "Test", "VALUE"),
        [Some(0), Some(0)],
        "architecture merge must preserve explicit field offsets"
    );
}

#[test]
fn arch_merge_preserves_nonzero_field_layouts() {
    let dir = std::env::temp_dir().join("win_merge_nonzero_field_layout");
    std::fs::create_dir_all(&dir).unwrap();

    let x64 = explicit_layout_winmd(&dir, "x64");
    let arm = explicit_layout_winmd(&dir, "arm");
    let x86 = explicit_layout_winmd(&dir, "x86");

    for input in [&x64, &arm, &x86] {
        let index = reader::Index::read(input).unwrap();
        assert_eq!(
            index
                .expect("Test", "EXPLICIT")
                .fields()
                .map(|field| field.offset())
                .collect::<Vec<_>>(),
            [Some(4), Some(12)]
        );
    }

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged).unwrap();
    assert_eq!(
        index
            .expect("Test", "EXPLICIT")
            .fields()
            .map(|field| field.offset())
            .collect::<Vec<_>>(),
        [Some(4), Some(12)],
        "architecture merge must preserve the input offsets rather than inventing zero"
    );
}

#[test]
fn arch_merge_preserves_external_type_ref_scope() {
    let dir = std::env::temp_dir().join("win_merge_external_type_ref");
    std::fs::create_dir_all(&dir).unwrap();

    let source =
        "#[win32] mod Test { struct EXTERNAL_RESULT { value: Windows::Foundation::HResult, } }";
    let x64 = winmd_with_default_refs(&dir, "x64", source);
    let arm = winmd_with_default_refs(&dir, "arm", source);
    let x86 = winmd_with_default_refs(&dir, "x86", source);

    let mut expected = None;
    for input in [&x64, &arm, &x86] {
        let index = reader::Index::read(input).unwrap();
        let identity =
            assembly_identity(type_ref(&index, "Windows.Foundation", "HResult")).unwrap();
        assert_eq!(identity.name, "Windows");
        if let Some(expected) = &expected {
            assert_eq!(&identity, expected);
        } else {
            expected = Some(identity);
        }
    }

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged).unwrap();
    assert_eq!(
        assembly_identity(type_ref(&index, "Windows.Foundation", "HResult")),
        expected,
        "architecture merge must preserve the external assembly identity"
    );
}

#[test]
fn arch_merge_prefers_local_type_ref_scope() {
    let dir = std::env::temp_dir().join("win_merge_local_type_ref");
    std::fs::create_dir_all(&dir).unwrap();

    let mut local_file = writer::File::new("Windows");
    let value_type = writer::TypeDefOrRef::TypeRef(local_file.TypeRef("System", "ValueType"));
    local_file.TypeDef(
        "Windows.Foundation",
        "HResult",
        value_type,
        TypeAttributes::SequentialLayout | TypeAttributes::Sealed | TypeAttributes::Public,
    );
    local_file.Field("value", &Type::I32, FieldAttributes::Public);
    let local = dir.join("local.winmd");
    std::fs::write(&local, local_file.into_stream()).unwrap();
    let source =
        "#[win32] mod Test { struct EXTERNAL_RESULT { value: Windows::Foundation::HResult, } }";
    let x64 = winmd_with_default_refs(&dir, "x64", source);
    let arm = winmd_with_default_refs(&dir, "arm", source);
    let x86 = winmd_with_default_refs(&dir, "x86", source);

    let merged = dir.join("merged.winmd");
    merge()
        .input(&local)
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged).unwrap();
    assert_eq!(
        index.get("Windows.Foundation", "HResult").count(),
        1,
        "the local definition must remain unique"
    );
    let reference = type_ref(&index, "Windows.Foundation", "HResult");
    assert!(
        matches!(reference.scope(), reader::ResolutionScope::Module(_)),
        "a local definition must win over external assembly candidates: {:?}",
        reference.scope()
    );
}

#[test]
fn arch_merge_rejects_conflicting_external_type_ref_scopes() {
    let dir = std::env::temp_dir().join("win_merge_conflicting_type_ref");
    std::fs::create_dir_all(&dir).unwrap();

    let x64 = external_type_winmd(&dir, "x64", "External.One");
    let arm = external_type_winmd(&dir, "arm", "External.One");
    let x86 = external_type_winmd(&dir, "x86", "External.Two");
    let merged = dir.join("merged.winmd");
    let error = merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("conflicting assembly references for `External.VALUE`"),
        "{error}"
    );
    assert!(error.contains("External.One"), "{error}");
    assert!(error.contains("External.Two"), "{error}");
}

#[test]
fn merge_localizes_included_assembly_type_refs() {
    let dir = std::env::temp_dir().join("win_merge_included_type_ref");
    std::fs::create_dir_all(&dir).unwrap();

    let included = named_type_definitions_winmd(&dir, "included", "Included");
    let consumer = named_type_consumer_winmd(
        &dir,
        "consumer",
        &included,
        &[("top", "N", "T"), ("nested", "N", "Outer/Inner")],
    );
    let merged = dir.join("merged.winmd");
    merge()
        .input(&included)
        .input(&consumer)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged).unwrap();
    for name in ["T", "Outer", "Outer/Inner"] {
        assert_eq!(
            type_ref(&index, "N", name).assembly(),
            None,
            "included type {name} must resolve to the merged module"
        );
    }
}

#[test]
fn merge_rejects_unrelated_local_and_external_type_refs() {
    let dir = std::env::temp_dir().join("win_merge_unrelated_type_ref");
    std::fs::create_dir_all(&dir).unwrap();

    let local = named_type_definitions_winmd(&dir, "local", "Local");
    let foreign = named_type_definitions_winmd(&dir, "foreign", "Foreign");
    let consumer = named_type_consumer_winmd(&dir, "consumer", &foreign, &[("value", "N", "T")]);
    let error = merge()
        .input(&local)
        .input(&consumer)
        .output(dir.join("merged.winmd"))
        .merge()
        .unwrap_err()
        .to_string();

    assert!(error.contains("N.T"), "{error}");
    assert!(error.contains("Foreign"), "{error}");
    assert!(error.contains("Local"), "{error}");
}

#[test]
fn merge_rejects_unrelated_nested_local_and_external_type_refs() {
    let dir = std::env::temp_dir().join("win_merge_unrelated_nested_type_ref");
    std::fs::create_dir_all(&dir).unwrap();

    let local = named_type_definitions_winmd(&dir, "local", "Local");
    let foreign = named_type_definitions_winmd(&dir, "foreign", "Foreign");
    let consumer =
        named_type_consumer_winmd(&dir, "consumer", &foreign, &[("value", "N", "Outer/Inner")]);
    let error = merge()
        .input(&local)
        .input(&consumer)
        .output(dir.join("merged.winmd"))
        .merge()
        .unwrap_err()
        .to_string();

    assert!(error.contains("N.Outer"), "{error}");
    assert!(error.contains("N.Outer/Inner"), "{error}");
    assert!(error.contains("Foreign"), "{error}");
    assert!(error.contains("Local"), "{error}");
}

#[test]
fn writer_reference_preserves_system_sentinel_identity() {
    let mut reference = writer::File::new("System");
    let value_type = writer::TypeDefOrRef::TypeRef(reference.TypeRef("System", "ValueType"));
    reference.TypeDef(
        "Sentinel",
        "VALUE",
        value_type,
        TypeAttributes::SequentialLayout | TypeAttributes::Sealed | TypeAttributes::Public,
    );
    let reference = reader::Index::new(vec![reader::File::new(reference.into_stream()).unwrap()]);

    let mut file = writer::File::new("consumer");
    file.set_reference(reference);
    file.TypeRef("System", "Object");
    file.TypeRef("Sentinel", "VALUE");
    let index = reader::Index::new(vec![reader::File::new(file.into_stream()).unwrap()]);

    let system = assembly_identity(type_ref(&index, "System", "Object")).unwrap();
    let sentinel = assembly_identity(type_ref(&index, "Sentinel", "VALUE")).unwrap();
    assert_eq!(sentinel, system);
    assert_eq!(sentinel.name, "mscorlib");
}

#[test]
fn arch_merge_preserves_cross_namespace_native_typedef_alias() {
    let dir = std::env::temp_dir().join("win_merge_native_typedef_alias");
    std::fs::create_dir_all(&dir).unwrap();

    let source = "#[win32] mod Windows { mod Win32 { \
        mod Foundation { type LPVOID = *mut void; } \
        mod Networking { mod WinHttp { \
            type HINTERNET = Windows::Win32::Foundation::LPVOID; \
        } } \
    } }";
    let x64 = winmd(&dir, "x64", source);
    let arm = winmd(&dir, "arm", source);
    let x86 = winmd(&dir, "x86", source);
    let expected = Some(Type::value_named("Windows.Win32.Foundation", "LPVOID"));

    for input in [&x64, &arm, &x86] {
        let index = reader::Index::read(input).unwrap();
        assert_eq!(
            index
                .expect("Windows.Win32.Networking.WinHttp", "HINTERNET")
                .underlying_type(),
            expected
        );
    }

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged).unwrap();
    assert_eq!(
        index
            .expect("Windows.Win32.Networking.WinHttp", "HINTERNET")
            .underlying_type(),
        expected,
        "architecture merge must not flatten a native typedef alias"
    );
}

#[test]
fn union_enums_merges_members() {
    let dir = std::env::temp_dir().join("win_merge_enum_union");
    std::fs::create_dir_all(&dir).unwrap();

    // A `um` header truncates the enum; the `km` header defines it fully.
    let um = winmd(
        &dir,
        "um",
        "#[win32] mod Test { #[repr(i32)] enum E { A = 0 } }",
    );
    let km = winmd(
        &dir,
        "km",
        "#[win32] mod Test { #[repr(i32)] enum E { A = 0, B = 1, C = 2 } }",
    );

    let merged = dir.join("merged.winmd");
    merge()
        .input(&um)
        .input(&km)
        .union_enums()
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    let enums: Vec<_> = index.types().filter(|t| t.name() == "E").collect();
    assert_eq!(enums.len(), 1, "same-named enums should union into one");

    let mut members: Vec<_> = enums[0]
        .fields()
        .filter(|f| f.constant().is_some())
        .map(|f| f.name().to_string())
        .collect();
    members.sort();
    assert_eq!(members, vec!["A", "B", "C"]);
}

#[test]
fn union_enums_rejects_conflicting_values() {
    let dir = std::env::temp_dir().join("win_merge_enum_conflict");
    std::fs::create_dir_all(&dir).unwrap();

    let a = winmd(
        &dir,
        "a",
        "#[win32] mod Test { #[repr(i32)] enum E { A = 0 } }",
    );
    let b = winmd(
        &dir,
        "b",
        "#[win32] mod Test { #[repr(i32)] enum E { A = 9 } }",
    );

    let merged = dir.join("merged.winmd");
    let result = merge()
        .input(&a)
        .input(&b)
        .union_enums()
        .output(&merged)
        .merge();

    assert!(result.is_err(), "conflicting member values must error");
}

#[test]
fn union_enums_rejects_conflicting_non_sentinel_max() {
    let dir = std::env::temp_dir().join("win_merge_enum_max_conflict");
    std::fs::create_dir_all(&dir).unwrap();

    // `FOO_MAX` contains "Max" but is a real value, not an NT count sentinel (which is spelled
    // `Max*` or `*Maximum`). A differing value across copies is a genuine conflict and must be
    // rejected, not silently reconciled by the sentinel tolerance.
    let a = winmd(
        &dir,
        "a",
        "#[win32] mod Test { #[repr(i32)] enum E { A = 0, FOO_MAX = 1 } }",
    );
    let b = winmd(
        &dir,
        "b",
        "#[win32] mod Test { #[repr(i32)] enum E { A = 0, FOO_MAX = 2 } }",
    );

    let merged = dir.join("merged.winmd");
    let result = merge()
        .input(&a)
        .input(&b)
        .union_enums()
        .output(&merged)
        .merge();

    assert!(
        result.is_err(),
        "a conflicting non-sentinel `Max` member must error"
    );
}

#[test]
fn union_enums_merges_partial_copies() {
    let dir = std::env::temp_dir().join("win_merge_enum_partial");
    std::fs::create_dir_all(&dir).unwrap();

    // Neither copy is a superset: `um` contributes `Named` (which `km` omits) and a lower `Max*`
    // count sentinel, while `km` contributes members `um` omits and the larger sentinel. The
    // union carries every member, the larger sentinel wins, and `Named` is appended.
    let um = winmd(
        &dir,
        "um",
        "#[win32] mod Test { #[repr(i32)] enum E { Shared = 1, Named = 38, MaxE = 2 } }",
    );
    let km = winmd(
        &dir,
        "km",
        "#[win32] mod Test { #[repr(i32)] enum E { First = 0, Shared = 1, Second = 2, MaxE = 3 } }",
    );

    let merged = dir.join("merged.winmd");
    merge()
        .input(&um)
        .input(&km)
        .union_enums()
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    let enums: Vec<_> = index.types().filter(|t| t.name() == "E").collect();
    assert_eq!(enums.len(), 1, "same-named enums should union into one");

    let members: Vec<(String, i64)> = enums[0]
        .fields()
        .filter_map(|f| {
            f.constant().map(|c| {
                let value = match c.value() {
                    Value::I32(v) => v as i64,
                    other => panic!("unexpected value {other:?}"),
                };
                (f.name().to_string(), value)
            })
        })
        .collect();

    // The larger `MaxE` sentinel (3) wins over the truncated one (2), and `Named` is appended.
    assert!(members.contains(&("First".to_string(), 0)));
    assert!(members.contains(&("Shared".to_string(), 1)));
    assert!(members.contains(&("Second".to_string(), 2)));
    assert!(members.contains(&("Named".to_string(), 38)));
    assert!(members.contains(&("MaxE".to_string(), 3)));
    assert_eq!(
        members.iter().filter(|(n, _)| n == "MaxE").count(),
        1,
        "sentinel must not be duplicated"
    );
}

#[test]
fn arch_merge_divergent_struct() {
    let dir = std::env::temp_dir().join("win_merge_div");
    std::fs::create_dir_all(&dir).unwrap();

    // CTX has a different shape per arch (the CONTEXT pattern): a 2-field struct on x64,
    // a 1-field struct on arm64. It must NOT collapse - both copies survive, arch-tagged.
    let x64 = winmd(
        &dir,
        "x64",
        "#[win32] mod Test { struct CTX { a: i32, b: i32 } }",
    );
    let arm = winmd(&dir, "arm", "#[win32] mod Test { struct CTX { x: i32 } }");

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    let ctx: Vec<_> = index.types().filter(|t| t.name() == "CTX").collect();
    assert_eq!(ctx.len(), 2);
    let mut tags: Vec<_> =
        ctx.iter()
            .map(|t| {
                t.attributes()
                    .find_map(|a| {
                        (a.ctor().parent().name() == "SupportedArchitectureAttribute").then(|| {
                            match a.value().first() {
                                Some((_, Value::I32(v))) => *v,
                                _ => 0,
                            }
                        })
                    })
                    .unwrap()
            })
            .collect();
    tags.sort();
    assert_eq!(tags, vec![2, 4]);
}

#[test]
fn arch_merge_normalizes_native_sized_callback_signature() {
    let dir = std::env::temp_dir().join("win_merge_native_callback");
    std::fs::create_dir_all(&dir).unwrap();

    let wide = "#[win32] mod Test { extern fn CB(value: usize) -> isize; }";
    let x64 = winmd(&dir, "x64", wide);
    let arm = winmd(&dir, "arm", wide);
    let x86 = winmd(
        &dir,
        "x86",
        "#[win32] mod Test { extern fn CB(value: u32) -> i32; }",
    );

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    let callbacks: Vec<_> = index.types().filter(|ty| ty.name() == "CB").collect();
    assert_eq!(callbacks.len(), 1);
    assert_eq!(type_arch_bits(callbacks[0]), None);

    let signature = callbacks[0]
        .methods()
        .find(|method| method.name() == "Invoke")
        .unwrap()
        .signature(&[]);
    assert_eq!(signature.return_type, Type::ISize);
    assert_eq!(signature.types, vec![Type::USize]);
}

#[test]
fn arch_merge_does_not_infer_native_size_without_native_evidence() {
    let dir = std::env::temp_dir().join("win_merge_fixed_callback");
    std::fs::create_dir_all(&dir).unwrap();

    let wide = "#[win32] mod Test { extern fn CB() -> i64; }";
    let x64 = winmd(&dir, "x64", wide);
    let arm = winmd(&dir, "arm", wide);
    let x86 = winmd(&dir, "x86", "#[win32] mod Test { extern fn CB() -> i32; }");

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    let mut arches: Vec<_> = index
        .types()
        .filter(|ty| ty.name() == "CB")
        .filter_map(type_arch_bits)
        .collect();
    arches.sort();
    assert_eq!(arches, vec![1, 6]);
}

#[test]
fn arch_merge_rejects_fixed_integer_with_wrong_pointer_width() {
    let dir = std::env::temp_dir().join("win_merge_wrong_width_callback");
    std::fs::create_dir_all(&dir).unwrap();

    let x64 = winmd(
        &dir,
        "x64",
        "#[win32] mod Test { extern fn CB() -> isize; }",
    );
    let arm = winmd(&dir, "arm", "#[win32] mod Test { extern fn CB() -> i32; }");
    let x86 = winmd(&dir, "x86", "#[win32] mod Test { extern fn CB() -> i32; }");

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&arm, 4)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    let mut arches: Vec<_> = index
        .types()
        .filter(|ty| ty.name() == "CB")
        .filter_map(type_arch_bits)
        .collect();
    arches.sort();
    assert_eq!(arches, vec![2, 5]);
}

#[test]
fn arch_merge_rejects_callback_attribute_mismatch() {
    let dir = std::env::temp_dir().join("win_merge_callback_attributes");
    std::fs::create_dir_all(&dir).unwrap();

    let x64 = winmd(
        &dir,
        "x64",
        "#[win32] mod Test { extern \"C\" fn CB() -> isize; }",
    );
    let x86 = winmd(&dir, "x86", "#[win32] mod Test { extern fn CB() -> i32; }");

    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x64, 2)
        .arch_input(&x86, 1)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    let mut arches: Vec<_> = index
        .types()
        .filter(|ty| ty.name() == "CB")
        .filter_map(type_arch_bits)
        .collect();
    arches.sort();
    assert_eq!(arches, vec![1, 2]);
}
