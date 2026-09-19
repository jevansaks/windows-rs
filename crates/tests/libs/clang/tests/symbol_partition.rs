#![cfg(target_pointer_width = "64")]

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
fn exact_symbol_retains_foreign_owner_and_dependencies() {
    let _guard = test_clang::libclang_guard();
    let (mut clang, dir) = fixture("header_exact_symbol");
    clang.symbols(["Pick", "Pick"]).write_by_header().unwrap();
    let rdl = std::fs::read_to_string(dir.join("rdl").join("foreign.rdl")).unwrap();
    assert_eq!(rdl.matches("fn Pick(").count(), 1, "{rdl}");
    assert!(!dir.join("rdl").join("api.rdl").exists());
    assert!(!rdl.contains("ForeignUnrelated"), "{rdl}");
    assert!(!rdl.contains("UNRELATED"), "{rdl}");
    assert!(rdl.contains("struct PAYLOAD"), "{rdl}");
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
