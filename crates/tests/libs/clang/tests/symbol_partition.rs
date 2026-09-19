#![cfg(target_pointer_width = "64")]

use windows_metadata::HasAttributes;

fn fixture(name: &str) -> (windows_clang::Clang, std::path::PathBuf) {
    let dir = std::path::Path::new(env!("OUT_DIR")).join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("foreign.h"),
        r#"
        struct PAYLOAD { int value; };
        typedef PAYLOAD* PPAYLOAD;
        struct UNRELATED { int unused; };
        extern "C" unsigned long long Pick(PPAYLOAD payload, unsigned long flags);
        extern "C" int ForeignUnrelated();
        "#,
    )
    .unwrap();
    std::fs::write(
        dir.join("api.h"),
        r#"
        #include "foreign.h"
        extern "C" unsigned long long Pick(PPAYLOAD payload, unsigned long flags);
        extern "C" int SameHeaderUnrelated();
        struct ROOT_UNUSED { int unused; };
        "#,
    )
    .unwrap();
    let mut clang = windows_clang::clang();
    clang
        .args([
            "-x",
            "c++",
            "--target=x86_64-pc-windows-msvc",
            "-fms-extensions",
        ])
        .library("test.dll")
        .input(dir.join("api.h"))
        .namespace("Test")
        .scope("__no_directory_scope__")
        .scope_header("api")
        .output(dir.join("rdl"));
    (clang, dir)
}

#[test]
fn exact_symbol_prefers_scoped_owner_and_retains_foreign_dependencies() {
    let _guard = test_clang::libclang_guard();
    let (mut clang, dir) = fixture("header_exact_symbol");
    clang.symbols(["Pick", "Pick"]).write_by_header().unwrap();
    let rdl = std::fs::read_to_string(dir.join("rdl").join("api.rdl")).unwrap();
    let foreign = std::fs::read_to_string(dir.join("rdl").join("foreign.rdl")).unwrap();
    assert_eq!(rdl.matches("fn Pick(").count(), 1, "{rdl}");
    assert!(!rdl.contains("SameHeaderUnrelated"), "{rdl}");
    assert!(!foreign.contains("ForeignUnrelated"), "{foreign}");
    assert!(!foreign.contains("UNRELATED"), "{foreign}");
    assert!(foreign.contains("struct PAYLOAD"), "{foreign}");
    let output = dir.join("selected.winmd");
    windows_rdl::reader()
        .input(dir.join("rdl"))
        .output(&output)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&output).unwrap();
    let methods: Vec<_> = index
        .expect("Test", "Apis")
        .methods()
        .map(|m| m.name())
        .collect();
    assert_eq!(methods, ["Pick"]);
}

#[test]
fn exact_symbol_reports_absent_or_excluded_declarations() {
    let _guard = test_clang::libclang_guard();
    let (mut clang, _) = fixture("header_absent_symbol");
    let error = clang.symbol("Missing").write_by_header().unwrap_err();
    assert!(
        error
            .message
            .contains("selected function `Missing` was not found"),
        "{error:?}"
    );

    let (mut clang, _) = fixture("header_excluded_symbol");
    let error = clang
        .symbol("Pick")
        .exclude_header("foreign")
        .exclude_header("api")
        .write_by_header()
        .unwrap_err();
    assert!(
        error
            .message
            .contains("selected function `Pick` was not found"),
        "{error:?}"
    );
}

#[test]
fn exact_symbol_without_redeclaration_retains_foreign_owner() {
    let _guard = test_clang::libclang_guard();
    let (mut clang, dir) = fixture("header_foreign_symbol");
    clang.symbol("ForeignUnrelated").write_by_header().unwrap();
    let rdl = std::fs::read_to_string(dir.join("rdl").join("foreign.rdl")).unwrap();
    assert!(rdl.contains("fn ForeignUnrelated("), "{rdl}");
    assert!(!rdl.contains("fn Pick("), "{rdl}");
    assert!(!dir.join("rdl").join("api.rdl").exists());
}

#[test]
fn exact_symbol_merges_redeclaration_contracts() {
    let _guard = test_clang::libclang_guard();
    let (mut clang, dir) = fixture("header_redeclaration_contracts");
    std::fs::write(
        dir.join("foreign.h"),
        r#"
        #define _Out_writes_bytes_opt_(n)
        extern "C" __attribute__((annotate("win32metadata:set_last_error")))
        unsigned long Pick(_Out_writes_bytes_opt_(count) void* data, unsigned long count);
    "#,
    )
    .unwrap();
    std::fs::write(
        dir.join("api.h"),
        r#"
        #include "foreign.h"
        extern "C" __attribute__((annotate("win32metadata:supported_os=windows5.0")))
        __attribute__((annotate("win32metadata:import_library=preferred.dll")))
        unsigned long Pick(void* buffer, unsigned long length);
        extern "C" int SameHeaderUnrelated();
    "#,
    )
    .unwrap();
    clang.symbol("Pick").write_by_header().unwrap();
    let rdl_dir = dir.join("rdl");
    let rdl = std::fs::read_to_string(rdl_dir.join("api.rdl")).unwrap();
    assert!(!rdl_dir.join("foreign.rdl").exists());
    assert_eq!(rdl.matches("supported_os").count(), 1, "{rdl}");
    let output = dir.join("selected.winmd");
    windows_rdl::reader()
        .input(&rdl_dir)
        .input_text(windows_rdl::WIN32_METADATA_RDL)
        .reference_default()
        .output(&output)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&output).unwrap();
    let methods: Vec<_> = index.expect("Test", "Apis").methods().collect();
    assert_eq!(methods.len(), 1);
    let method = methods[0];
    assert_eq!(method.name(), "Pick");
    assert!(
        method
            .attributes()
            .any(|a| a.name() == "SupportedOSPlatformAttribute")
    );
    let import = method.impl_map().unwrap();
    assert_eq!(import.import_scope().name(), "preferred.dll");
    assert!(
        import
            .flags()
            .contains(windows_metadata::PInvokeAttributes::SupportsLastError)
    );
    let param = method.params().find(|p| p.sequence() == 1).unwrap();
    assert_eq!(param.name(), "buffer");
    assert_eq!(
        param.flags(),
        windows_metadata::ParamAttributes::Out | windows_metadata::ParamAttributes::Optional
    );
    let count = param
        .attributes()
        .find(|a| a.name() == "MemorySizeAttribute")
        .unwrap();
    assert_eq!(
        count.value(),
        vec![(
            "BytesParamIndex".to_string(),
            windows_metadata::Value::I16(1)
        )]
    );
}

#[test]
fn exact_symbol_rejects_conflicting_redeclaration_contracts() {
    let _guard = test_clang::libclang_guard();
    for key in ["supported_os", "import_library"] {
        let (mut clang, dir) = fixture(&format!("header_conflicting_{key}"));
        std::fs::write(
            dir.join("foreign.h"),
            format!(
                r#"
            extern "C" __attribute__((annotate("win32metadata:{key}=one"))) int Pick(int x);
        "#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("api.h"),
            format!(
                r#"
            #include "foreign.h"
            extern "C" __attribute__((annotate("win32metadata:{key}=two"))) int Pick(int x);
        "#
            ),
        )
        .unwrap();
        let error = clang.symbol("Pick").write_by_header().unwrap_err();
        assert!(
            error
                .message
                .contains(&format!("conflicting redeclaration annotation `{key}`")),
            "{error:?}"
        );
    }
    let (mut clang, dir) = fixture("header_conflicting_direction");
    std::fs::write(
        dir.join("foreign.h"),
        r#"
        #define _In_
        extern "C" int Pick(_In_ int* value);
    "#,
    )
    .unwrap();
    std::fs::write(
        dir.join("api.h"),
        r#"
        #include "foreign.h"
        #define _Out_
        extern "C" int Pick(_Out_ int* value);
    "#,
    )
    .unwrap();
    let error = clang.symbol("Pick").write_by_header().unwrap_err();
    assert!(
        error
            .message
            .contains("conflicting redeclaration parameter contract"),
        "{error:?}"
    );

    let (mut clang, dir) = fixture("header_conflicting_size");
    std::fs::write(
        dir.join("foreign.h"),
        r#"
        #define _Out_writes_bytes_(n)
        extern "C" int Pick(_Out_writes_bytes_(4) char* value);
    "#,
    )
    .unwrap();
    std::fs::write(
        dir.join("api.h"),
        r#"
        #include "foreign.h"
        extern "C" int Pick(_Out_writes_bytes_(8) char* value);
    "#,
    )
    .unwrap();
    let error = clang.symbol("Pick").write_by_header().unwrap_err();
    assert!(
        error
            .message
            .contains("conflicting redeclaration parameter contract"),
        "{error:?}"
    );
}

#[test]
fn exact_symbol_rejects_ambiguous_overloads() {
    let _guard = test_clang::libclang_guard();
    let (mut clang, dir) = fixture("header_ambiguous_symbol");
    std::fs::write(
        dir.join("api.h"),
        "int Pick(int value);\nint Pick(float value);\n",
    )
    .unwrap();
    let error = clang.symbol("Pick").write_by_header().unwrap_err();
    assert!(
        error
            .message
            .contains("selected function `Pick` has multiple"),
        "{error:?}"
    );
}

#[test]
fn exact_symbol_rejects_constant_selection() {
    let _guard = test_clang::libclang_guard();
    let (mut clang, _) = fixture("header_mixed_selection");
    let error = clang
        .symbol("Pick")
        .constant("AnyConstant")
        .write_by_header()
        .unwrap_err();
    assert!(
        error
            .message
            .contains("function and constant selections cannot be combined"),
        "{error:?}"
    );
}
