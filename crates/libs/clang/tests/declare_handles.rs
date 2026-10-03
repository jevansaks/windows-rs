use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use windows_clang::{
    EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition, extract,
};
use windows_metadata::{Type, reader::Item};

const NAMESPACE: &str = "Windows.Win32.Storage.Compression";

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-declare-handles-{name}-{}",
        std::process::id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn write_fixture(scratch: &Path, macro_definition: &str, declarations: &str) -> PathBuf {
    let macros = scratch.join("handles.h");
    let api = scratch.join("compressapi.h");
    std::fs::write(&macros, macro_definition).unwrap();
    std::fs::write(
        &api,
        format!("#include \"{}\"\n{declarations}", macros.to_string_lossy()),
    )
    .unwrap();
    api
}

fn emit(
    scratch: &Path,
    api: &Path,
    target: &str,
    exclusions: impl IntoIterator<Item = &'static str>,
) -> Result<BTreeMap<windows_clang::RdlPartition, String>, windows_clang::Error> {
    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!("#include \"{}\"\n", api.to_string_lossy()),
        )
        .with_root_dirs([scratch.to_string_lossy().to_string()])],
        &["-x", "c++", &format!("--target={target}")],
    )
    .unwrap();
    let mut partition = RootPartition::new("CmpApi", NAMESPACE);
    for exclusion in exclusions {
        partition = partition.with_exclusion(exclusion);
    }
    let policy =
        HeaderPartitionPolicy::new().with_traversed_header(api.to_string_lossy(), partition);
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Windows.Win32", &references);
    options.library = Some("cabinet.dll");
    snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
}

#[test]
fn suppressed_declare_handle_tag_projects_public_handle_aliases() {
    helpers::ensure_libclang();

    for (architecture, target) in [
        ("x86", "i686-pc-windows-msvc"),
        ("x64", "x86_64-pc-windows-msvc"),
        ("arm64", "aarch64-pc-windows-msvc"),
    ] {
        let scratch = scratch(architecture);
        let api = write_fixture(
            &scratch,
            "#define DECLARE_HANDLE(name) \
             struct name##__ { int unused; }; typedef struct name##__ *name\n",
            "DECLARE_HANDLE(COMPRESSOR_HANDLE);\n\
             typedef COMPRESSOR_HANDLE *PCOMPRESSOR_HANDLE;\n\
             typedef COMPRESSOR_HANDLE DECOMPRESSOR_HANDLE;\n\
             typedef COMPRESSOR_HANDLE *PDECOMPRESSOR_HANDLE;\n\
             extern \"C\" int CreateCompressor(PCOMPRESSOR_HANDLE compressor);\n\
             extern \"C\" int Compress(COMPRESSOR_HANDLE compressor);\n\
             extern \"C\" void CloseCompressor(COMPRESSOR_HANDLE compressor);\n\
             extern \"C\" int CreateDecompressor(PDECOMPRESSOR_HANDLE decompressor);\n\
             extern \"C\" int Decompress(DECOMPRESSOR_HANDLE decompressor);\n\
             extern \"C\" void CloseDecompressor(DECOMPRESSOR_HANDLE decompressor);\n",
        );
        let partitions = emit(&scratch, &api, target, ["COMPRESSOR_HANDLE__"]).unwrap();
        let rdl = partitions.values().cloned().collect::<String>();
        assert!(rdl.contains("type COMPRESSOR_HANDLE = *mut void;"), "{rdl}");
        assert!(
            rdl.contains("type DECOMPRESSOR_HANDLE = COMPRESSOR_HANDLE;"),
            "{rdl}"
        );
        assert!(
            rdl.contains("type PCOMPRESSOR_HANDLE = *mut COMPRESSOR_HANDLE;"),
            "{rdl}"
        );
        assert!(
            rdl.contains("type PDECOMPRESSOR_HANDLE = *mut COMPRESSOR_HANDLE;"),
            "{rdl}"
        );
        assert!(!rdl.contains("COMPRESSOR_HANDLE__"), "{rdl}");

        let winmd = scratch.join(format!("{architecture}.winmd"));
        windows_rdl::reader()
            .input_texts(partitions.values())
            .reference_default()
            .output(&winmd)
            .write()
            .unwrap();
        let index = windows_metadata::reader::Index::read(&winmd).unwrap();
        let compressor = Type::value_named(NAMESPACE, "COMPRESSOR_HANDLE");
        assert_eq!(
            index
                .expect(NAMESPACE, "COMPRESSOR_HANDLE")
                .underlying_type(),
            Some(Type::PtrMut(Box::new(Type::Void), 1))
        );
        assert_eq!(
            index
                .expect(NAMESPACE, "DECOMPRESSOR_HANDLE")
                .underlying_type(),
            Some(compressor.clone())
        );
        assert_eq!(
            index
                .expect(NAMESPACE, "PCOMPRESSOR_HANDLE")
                .underlying_type(),
            Some(Type::PtrMut(Box::new(compressor.clone()), 1))
        );
        assert_eq!(
            index
                .expect(NAMESPACE, "PDECOMPRESSOR_HANDLE")
                .underlying_type(),
            Some(Type::PtrMut(Box::new(compressor.clone()), 1))
        );
        assert_function_parameter(
            &index,
            "CreateCompressor",
            Type::value_named(NAMESPACE, "PCOMPRESSOR_HANDLE"),
        );
        assert_function_parameter(&index, "Compress", compressor.clone());
        assert_function_parameter(&index, "CloseCompressor", compressor.clone());
        assert_function_parameter(
            &index,
            "CreateDecompressor",
            Type::value_named(NAMESPACE, "PDECOMPRESSOR_HANDLE"),
        );
        assert_function_parameter(
            &index,
            "Decompress",
            Type::value_named(NAMESPACE, "DECOMPRESSOR_HANDLE"),
        );
        assert_function_parameter(
            &index,
            "CloseDecompressor",
            Type::value_named(NAMESPACE, "DECOMPRESSOR_HANDLE"),
        );
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

#[test]
fn nonexcluded_declare_handles_keep_existing_output() {
    helpers::ensure_libclang();

    let scratch = scratch("existing-output");
    let api = write_fixture(
        &scratch,
        "#define DECLARE_HANDLE(name) \
         struct name##__ { int unused; }; typedef struct name##__ *name\n",
        "DECLARE_HANDLE(OTHER_HANDLE);\n\
         extern \"C\" void UseOtherHandle(OTHER_HANDLE value);\n",
    );
    let partitions = emit(&scratch, &api, "x86_64-pc-windows-msvc", []).unwrap();
    let rdl = partitions.values().cloned().collect::<String>();

    assert!(rdl.contains("struct OTHER_HANDLE__"), "{rdl}");
    assert!(
        rdl.contains("type OTHER_HANDLE = *mut OTHER_HANDLE__;"),
        "{rdl}"
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn lookalikes_and_invalid_declare_handle_expansions_remain_errors() {
    helpers::ensure_libclang();

    for (name, macro_definition, declarations, excluded) in [
        (
            "ordinary-lookalike",
            "",
            "struct MANUAL_HANDLE__ { int unused; };\n\
             typedef struct MANUAL_HANDLE__ *MANUAL_HANDLE;\n\
             extern \"C\" void UseManualHandle(MANUAL_HANDLE value);\n",
            "MANUAL_HANDLE__",
        ),
        (
            "spoofed-layout",
            "#define DECLARE_HANDLE(name) \
             struct name##__ { long long unused; }; typedef struct name##__ *name\n",
            "DECLARE_HANDLE(SPOOFED_HANDLE);\n\
             extern \"C\" void UseSpoofedHandle(SPOOFED_HANDLE value);\n",
            "SPOOFED_HANDLE__",
        ),
        (
            "missing-definition",
            "#define DECLARE_HANDLE(name) \
             struct name##__; typedef struct name##__ *name\n",
            "DECLARE_HANDLE(FORWARD_HANDLE);\n\
             extern \"C\" void UseForwardHandle(FORWARD_HANDLE value);\n",
            "FORWARD_HANDLE__",
        ),
        (
            "wrong-typedef",
            "#define DECLARE_HANDLE(name) \
             struct name##__ { int unused; }; typedef const struct name##__ *name\n",
            "DECLARE_HANDLE(CONST_HANDLE);\n\
             extern \"C\" void UseConstHandle(CONST_HANDLE value);\n",
            "CONST_HANDLE__",
        ),
        (
            "by-value-private-use",
            "#define DECLARE_HANDLE(name) \
             struct name##__ { int unused; }; typedef struct name##__ *name\n",
            "DECLARE_HANDLE(VALUE_HANDLE);\n\
             extern \"C\" void UseValueHandle(VALUE_HANDLE value);\n\
             extern \"C\" void UsePrivateValue(struct VALUE_HANDLE__ value);\n",
            "VALUE_HANDLE__",
        ),
    ] {
        let scratch = scratch(name);
        let api = write_fixture(&scratch, macro_definition, declarations);
        let error = emit(&scratch, &api, "x86_64-pc-windows-msvc", [excluded]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("owner-excluded local type `{excluded}`")),
            "{name}: {error}"
        );
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

fn assert_function_parameter(index: &windows_metadata::reader::Index, name: &str, expected: Type) {
    let Item::Fn(function) = index.expect_item(NAMESPACE, name) else {
        panic!("{name} was not emitted as a function");
    };
    assert_eq!(function.signature(&[]).types, [expected]);
}
