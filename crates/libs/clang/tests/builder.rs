#[test]
fn builder_uses_the_snapshot_pipeline() {
    helpers::ensure_libclang();

    let scratch =
        std::env::temp_dir().join(format!("windows-clang-builder-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        scratch.join("dependency.h"),
        "typedef struct DEPENDENCY { int value; } DEPENDENCY;\n",
    )
    .unwrap();
    std::fs::write(
        scratch.join("api.h"),
        "#include \"dependency.h\"\n\
         typedef struct API { DEPENDENCY dependency; } API;\n\
         extern \"C\" API GetApi();\n\
         extern \"C\" API GetOtherApi();\n",
    )
    .unwrap();

    let include = format!("-I{}", scratch.display());
    let output = scratch.join("output").join("api.rdl");
    windows_clang::clang()
        .input_text("#include \"api.h\"\n")
        .args(["-x", "c++", include.as_str()])
        .parallelism(2)
        .filter("api.h")
        .symbols(["GetApi"])
        .reference_default()
        .namespace("Builder")
        .library("builder.dll")
        .output(&output)
        .write()
        .unwrap();

    let rdl = std::fs::read_to_string(output).unwrap();
    assert!(rdl.contains("struct API"));
    assert!(rdl.contains("struct DEPENDENCY"));
    assert!(rdl.contains("extern \"C\" fn GetApi() -> API"));
    assert!(!rdl.contains("GetOtherApi"));
    assert!(rdl.contains("#[library(\"builder.dll\")]"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn builder_excludes_source_directories() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-builder-excluded-{}",
        std::process::id()
    ));
    let resource = scratch.join("resource");
    let sdk = scratch.join("sdk");
    std::fs::create_dir_all(&resource).unwrap();
    std::fs::create_dir_all(&sdk).unwrap();
    std::fs::write(
        resource.join("resource.h"),
        "typedef struct RESOURCE_ONLY { int value; } RESOURCE_ONLY;\n",
    )
    .unwrap();
    std::fs::write(
        sdk.join("api.h"),
        "typedef struct SDK_ONLY { unsigned value; } SDK_ONLY;\n",
    )
    .unwrap();

    let resource_include = format!("-I{}", resource.display());
    let sdk_include = format!("-I{}", sdk.display());
    let output = scratch.join("output.rdl");
    windows_clang::clang()
        .input_text("#include \"resource.h\"\n#include \"api.h\"\n")
        .args(["-x", "c++", resource_include.as_str(), sdk_include.as_str()])
        .filter("resource.h")
        .filter("api.h")
        .exclude_path(&resource)
        .namespace("Builder")
        .output(&output)
        .write()
        .unwrap();

    let rdl = std::fs::read_to_string(output).unwrap();
    assert!(rdl.contains("struct SDK_ONLY"), "{rdl}");
    assert!(!rdl.contains("RESOURCE_ONLY"), "{rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn builder_parses_multiple_inputs_in_parallel() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-builder-parallel-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        scratch.join("first.h"),
        "typedef struct FIRST { unsigned value; } FIRST;\n",
    )
    .unwrap();
    std::fs::write(
        scratch.join("second.h"),
        "typedef struct SECOND { unsigned value; } SECOND;\n",
    )
    .unwrap();

    let include = format!("-I{}", scratch.display());
    let output = scratch.join("output.rdl");
    windows_clang::clang()
        .input_text("#include \"first.h\"\n")
        .input_text("#include \"second.h\"\n")
        .args(["-x", "c++", include.as_str()])
        .parallelism(2)
        .filter("first.h")
        .filter("second.h")
        .namespace("Builder")
        .output(&output)
        .write()
        .unwrap();

    let rdl = std::fs::read_to_string(output).unwrap();
    assert!(rdl.contains("struct FIRST"), "{rdl}");
    assert!(rdl.contains("struct SECOND"), "{rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn builder_excludes_items_supplied_by_references() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-builder-reference-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let reference = scratch.join("reference.winmd");
    windows_rdl::reader()
        .input_text(
            "#[win32]
            mod Reference {
                const EXISTING_VALUE: i32 = 1;
                #[library(\"reference.dll\")]
                extern \"C\" fn ExistingFunction() -> i32;
            }",
        )
        .output(&reference)
        .write()
        .unwrap();

    let output = scratch.join("api.rdl");
    windows_clang::clang()
        .input_text(
            "#define EXISTING_VALUE 1
             extern \"C\" int ExistingFunction();
             extern \"C\" int NewFunction();",
        )
        .args(["-x", "c++"])
        .reference(&reference)
        .namespace("Builder")
        .library("builder.dll")
        .output(&output)
        .write()
        .unwrap();

    let rdl = std::fs::read_to_string(output).unwrap();
    assert!(!rdl.contains("EXISTING_VALUE"));
    assert!(!rdl.contains("ExistingFunction"));
    assert!(rdl.contains("NewFunction"));

    std::fs::remove_dir_all(scratch).unwrap();
}
