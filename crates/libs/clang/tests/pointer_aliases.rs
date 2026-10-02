use windows_clang::{
    EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition, extract,
};
use windows_metadata::{Type, reader::Item};

const BASE_SOURCE: &str = r#"
typedef unsigned short WCHAR;
typedef WCHAR *PWSTR, *LPWSTR;
typedef const WCHAR *LPCWSTR, *PCWSTR;
typedef PCWSTR *PPCWSTR;
"#;

const API_SOURCE: &str = r#"
typedef struct POINTER_ALIAS_FIELDS {
    PWSTR *mutableMutable;
    PWSTR const *constMutable;
    PCWSTR *mutableConst;
    PCWSTR const *constConst;
    LPCWSTR *legacyMutableConst;
    PPCWSTR aliasChain;
} POINTER_ALIAS_FIELDS;

extern "C" void PointerAliasParameters(
    PWSTR *mutableMutable,
    PWSTR const *constMutable,
    PCWSTR *mutableConst,
    PCWSTR const *constConst,
    LPCWSTR *legacyMutableConst,
    PPCWSTR aliasChain);
"#;

#[test]
fn dependency_pointer_aliases_preserve_nested_constness() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-pointer-aliases-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let base = scratch.join("base.h");
    let api = scratch.join("api.h");
    std::fs::write(&base, BASE_SOURCE).unwrap();
    std::fs::write(
        &api,
        format!("#include \"{}\"\n{API_SOURCE}", base.to_string_lossy()),
    )
    .unwrap();

    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!("#include \"{}\"\n", api.to_string_lossy()),
        )
        .with_roots([api.to_string_lossy().to_string()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let legacy = snapshot.emit_with_library("Legacy", "test.dll").unwrap();
    assert!(!legacy.contains("type PCWSTR"));
    assert!(!legacy.contains("type PWSTR"));
    assert!(legacy.contains("mutableConst: *mut PCWSTR,"));

    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        api.to_string_lossy(),
        RootPartition::new("api", "Windows.Win32.Test"),
    );
    let references = windows_clang::MetadataReferences::new([windows_metadata::reader::File::new(
        windows_default::WINRT.to_vec(),
    )
    .unwrap()]);
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.library = Some("test.dll");
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let output = partitions.values().cloned().collect::<String>();

    assert_eq!(output.matches("type PCWSTR = *const u16;").count(), 1);
    assert_eq!(output.matches("type PWSTR = *mut u16;").count(), 1);
    assert_eq!(output.matches("type PPCWSTR = *mut PCWSTR;").count(), 1);
    assert!(!output.contains("type LPCWSTR"));
    assert!(output.contains("mutableMutable: *mut Windows::Win32::PWSTR,"));
    assert!(output.contains("constMutable: *const Windows::Win32::PWSTR,"));
    assert!(output.contains("mutableConst: *mut Windows::Win32::PCWSTR,"));
    assert!(output.contains("constConst: *const Windows::Win32::PCWSTR,"));
    assert!(output.contains("legacyMutableConst: *mut Windows::Win32::PCWSTR,"));
    assert!(
        output.contains("aliasChain: Windows::Win32::PPCWSTR,"),
        "{output}"
    );
    assert!(output.contains(
        "extern \"C\" fn PointerAliasParameters(mutableMutable: *mut Windows::Win32::PWSTR, \
         constMutable: *const Windows::Win32::PWSTR, mutableConst: *mut \
         Windows::Win32::PCWSTR, constConst: *const Windows::Win32::PCWSTR, \
         legacyMutableConst: *mut Windows::Win32::PCWSTR, aliasChain: \
         Windows::Win32::PPCWSTR);"
    ));

    let winmd = scratch.join("pointer-aliases.winmd");
    let mut compiler = windows_rdl::reader();
    for output in partitions.values() {
        compiler.input_text(output);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();

    assert_eq!(
        index.expect("Windows.Win32", "PWSTR").underlying_type(),
        Some(Type::PtrMut(Box::new(Type::U16), 1))
    );
    assert_eq!(
        index.expect("Windows.Win32", "PCWSTR").underlying_type(),
        Some(Type::PtrConst(Box::new(Type::U16), 1))
    );
    assert_eq!(
        index.expect("Windows.Win32", "PPCWSTR").underlying_type(),
        Some(pointer_to_alias(false, "PCWSTR"))
    );

    let record = index.expect("Windows.Win32.Test", "POINTER_ALIAS_FIELDS");
    let fields = record
        .fields()
        .map(|field| (field.name().to_string(), field.ty()))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(fields["mutableMutable"], pointer_to_alias(false, "PWSTR"));
    assert_eq!(fields["constMutable"], pointer_to_alias(true, "PWSTR"));
    assert_eq!(fields["mutableConst"], pointer_to_alias(false, "PCWSTR"));
    assert_eq!(fields["constConst"], pointer_to_alias(true, "PCWSTR"));
    assert_eq!(
        fields["legacyMutableConst"],
        pointer_to_alias(false, "PCWSTR")
    );
    assert_eq!(
        fields["aliasChain"],
        Type::value_named("Windows.Win32", "PPCWSTR")
    );

    let function = index
        .iter_items()
        .find_map(|(namespace, name, item)| {
            (namespace == "Windows.Win32.Test" && name == "PointerAliasParameters").then_some(item)
        })
        .unwrap();
    let Item::Fn(function) = function else {
        panic!("PointerAliasParameters was not emitted as a function");
    };
    assert_eq!(
        function.signature(&[]).types,
        [
            pointer_to_alias(false, "PWSTR"),
            pointer_to_alias(true, "PWSTR"),
            pointer_to_alias(false, "PCWSTR"),
            pointer_to_alias(true, "PCWSTR"),
            pointer_to_alias(false, "PCWSTR"),
            Type::value_named("Windows.Win32", "PPCWSTR"),
        ]
    );

    let unsupported = scratch.join("unsupported.winmd");
    let error = windows_rdl::reader()
        .input_text("#[win32]\nmod Test {\n    struct BAD { value: *mut *const u16 }\n}\n")
        .output(&unsupported)
        .write()
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("mixed `*mut` and `*const` pointer chains are not representable"),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

fn pointer_to_alias(outer_const: bool, name: &str) -> Type {
    let target = Box::new(Type::value_named("Windows.Win32", name));
    if outer_const {
        Type::PtrConst(target, 1)
    } else {
        Type::PtrMut(target, 1)
    }
}
