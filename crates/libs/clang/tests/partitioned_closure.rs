use std::path::{Path, PathBuf};
use windows_clang::{
    ExtractionOptions, Fact, FactData, FactKind, Input, Snapshot, extract_partitioned_with_options,
    extract_with_options,
};

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-partitioned-closure-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn input(
    name: &str,
    header: &Path,
    partition: &str,
    namespace: &str,
) -> windows_clang::PartitionedInput {
    let suffix = header.file_name().unwrap().to_string_lossy().to_string();
    Input::new(name, format!("#include \"{}\"\n", header.display()))
        .with_root_suffixes([suffix])
        .partitioned(name)
        .with_root(
            header.to_string_lossy(),
            partition.to_string(),
            namespace.to_string(),
        )
}

fn fact<'a>(snapshot: &'a Snapshot, tu: &str, name: &str) -> &'a Fact {
    snapshot
        .facts()
        .iter()
        .find(|fact| fact.origin.tu == tu && fact.name == name)
        .unwrap()
}

fn parent_name<'a>(snapshot: &'a Snapshot, fact: &Fact) -> Option<&'a str> {
    let parent = fact.parent.as_ref()?;
    snapshot
        .facts()
        .iter()
        .find(|candidate| candidate.origin == *parent)
        .map(|parent| parent.name.as_str())
}

#[test]
fn partitioned_closure_materializes_exact_transitive_dependencies() {
    helpers::ensure_libclang();

    let scratch = scratch("transitive");
    let dependencies = scratch.join("dependencies.h");
    let root = scratch.join("root.h");
    std::fs::write(
        &dependencies,
        "#define DEFINE_ENUM_FLAG_OPERATORS(type)\n\
         #define ASSOCIATED_VALUE 7\n\
         typedef unsigned BASE_SCALAR;\n\
         typedef BASE_SCALAR VALUE_ALIAS;\n\
         typedef struct FIELD_DEP { VALUE_ALIAS value; } FIELD_DEP;\n\
         typedef struct BASE_RECORD { int base; } BASE_RECORD;\n\
         struct DERIVED_RECORD : BASE_RECORD { FIELD_DEP field; };\n\
         typedef VALUE_ALIAS (__stdcall *CALLBACK_DEP)(FIELD_DEP* value);\n\
         enum ASSOCIATED_ENUM : unsigned { ASSOCIATED_NONE = 0 };\n\
         DEFINE_ENUM_FLAG_OPERATORS(ASSOCIATED_ENUM)\n\
         struct __declspec(uuid(\"00000000-0000-0000-c000-000000000046\")) IBASE {\n\
             virtual int Base() = 0;\n\
         };\n\
         struct __declspec(uuid(\"11111111-1111-1111-1111-111111111111\")) IDERIVED : IBASE {\n\
             virtual int Derived() = 0;\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &root,
        "#include \"dependencies.h\"\n\
         #define W32M(text) __attribute__((annotate(text)))\n\
         typedef struct ROOT_RECORD {\n\
             DERIVED_RECORD derived;\n\
             CALLBACK_DEP callback;\n\
         } ROOT_RECORD;\n\
         W32M(\"win32metadata:associated_enum=ASSOCIATED_ENUM\")\n\
         extern \"C\" VALUE_ALIAS RootFunction(\n\
             ROOT_RECORD* value, CALLBACK_DEP callback, IDERIVED* derived);\n\
         enum          W32M(\"win32metadata:associated_constant=ASSOCIATED_VALUE\")\n\
             ROOT_ENUM : unsigned { ROOT_NONE = 0 };\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned_with_options(
        [input("root.cpp", &root, "root", "Closure.Root")],
        &[
            "-x",
            "c++",
            include.as_str(),
            "--target=x86_64-pc-windows-msvc",
        ],
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();

    for name in [
        "BASE_SCALAR",
        "VALUE_ALIAS",
        "FIELD_DEP",
        "BASE_RECORD",
        "DERIVED_RECORD",
        "CALLBACK_DEP",
        "ASSOCIATED_ENUM",
        "IBASE",
        "IDERIVED",
    ] {
        assert!(
            !matches!(fact(&snapshot, "root.cpp", name).data, FactData::None),
            "{name} was not materialized"
        );
    }
    assert!(matches!(
        fact(&snapshot, "root.cpp", "ASSOCIATED_VALUE").data,
        FactData::Macro { .. }
    ));
    assert!(snapshot.constants().iter().any(|constant| {
        constant.root.tu == "root.cpp"
            && constant.name == "ASSOCIATED_VALUE"
            && constant.value == windows_clang::Value::Signed(7)
    }));
    let enum_flag = snapshot
        .facts()
        .iter()
        .find(|fact| {
            fact.origin.tu == "root.cpp"
                && fact.kind == FactKind::EnumFlag
                && matches!(
                    &fact.data,
                    FactData::EnumFlag { target } if target == "ASSOCIATED_ENUM"
                )
        })
        .unwrap();
    assert_eq!(enum_flag.name, "DEFINE_ENUM_FLAG_OPERATORS");
    assert!(enum_flag.spelling.file.ends_with("dependencies.h"));
    assert!(enum_flag.expansion.file.ends_with("dependencies.h"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn partitioned_closure_uses_canonical_later_tu_definition() {
    helpers::ensure_libclang();

    let scratch = scratch("later-definition");
    let forward = scratch.join("forward.h");
    let definition = scratch.join("definition.h");
    let provider = scratch.join("provider.h");
    std::fs::write(
        &forward,
        "#define W32M(text) __attribute__((annotate(text)))\n\
         struct COMPLETE_LATER;\n\
         enum W32M(\"win32metadata:associated_constant=LATE_VALUE\")\n\
             FORWARD_ENUM : unsigned { FORWARD_NONE = 0 };\n",
    )
    .unwrap();
    std::fs::write(
        &definition,
        "#include \"forward.h\"\n\
         #define LATE_VALUE 9\n\
         #define DEFINE_COMPLETE struct COMPLETE_LATER { int value; };\n\
         DEFINE_COMPLETE\n",
    )
    .unwrap();
    std::fs::write(
        &provider,
        "#include \"definition.h\"\ntypedef unsigned PROVIDER_ROOT;\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let inputs = vec![
        input("forward.cpp", &forward, "forward", "Closure.Forward"),
        input("provider.cpp", &provider, "provider", "Closure.Provider"),
    ];
    let arguments = [
        "-x",
        "c++",
        include.as_str(),
        "--target=x86_64-pc-windows-msvc",
    ];
    let serial =
        extract_partitioned_with_options(inputs.clone(), &arguments, &ExtractionOptions::new())
            .unwrap();
    let snapshot = extract_partitioned_with_options(
        inputs,
        &arguments,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), snapshot.dump());

    let definition = snapshot
        .facts()
        .iter()
        .find(|fact| {
            fact.origin.tu == "provider.cpp" && fact.name == "COMPLETE_LATER" && fact.definition
        })
        .unwrap();
    assert!(matches!(
        &definition.data,
        FactData::Record { fields, .. } if fields.len() == 1
    ));
    assert!(matches!(
        fact(&snapshot, "provider.cpp", "LATE_VALUE").data,
        FactData::Macro { .. }
    ));
    assert!(snapshot.constants().iter().any(|constant| {
        constant.root.tu == "provider.cpp"
            && constant.name == "LATE_VALUE"
            && constant.value == windows_clang::Value::Signed(9)
    }));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn partitioned_closure_does_not_follow_same_leaf_or_unreachable_types() {
    helpers::ensure_libclang();

    let scratch = scratch("same-leaf");
    let dependencies = scratch.join("dependencies.h");
    let root = scratch.join("root.h");
    std::fs::write(
        &dependencies,
        "namespace A { struct SHARED { int selected; }; }\n\
         namespace B { struct SHARED { int other; }; }\n\
         struct UNUSED { int value; };\n",
    )
    .unwrap();
    std::fs::write(
        &root,
        "#include \"dependencies.h\"\ntypedef A::SHARED ROOT_ALIAS;\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned_with_options(
        [input("root.cpp", &root, "root", "Closure.Root")],
        &[
            "-x",
            "c++",
            include.as_str(),
            "--target=x86_64-pc-windows-msvc",
        ],
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();

    let shared: Vec<_> = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.origin.tu == "root.cpp" && fact.name == "SHARED")
        .collect();
    let selected = shared
        .iter()
        .find(|fact| parent_name(&snapshot, fact) == Some("A"))
        .unwrap();
    let other = shared
        .iter()
        .find(|fact| parent_name(&snapshot, fact) == Some("B"))
        .unwrap();
    assert!(matches!(selected.data, FactData::Record { .. }));
    assert!(matches!(other.data, FactData::None));
    assert!(matches!(
        fact(&snapshot, "root.cpp", "UNUSED").data,
        FactData::None
    ));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn ordinary_extraction_preserves_legacy_reachable_struct_decoding() {
    helpers::ensure_libclang();

    let scratch = scratch("ordinary");
    let dependencies = scratch.join("dependencies.h");
    let root = scratch.join("root.h");
    std::fs::write(
        &dependencies,
        "struct REACHABLE { int value; };\nstruct UNREACHABLE { int value; };\n",
    )
    .unwrap();
    std::fs::write(
        &root,
        "#include \"dependencies.h\"\ntypedef REACHABLE ROOT_ALIAS;\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let root_suffix = root.file_name().unwrap().to_string_lossy().to_string();
    let snapshot = extract_with_options(
        [
            Input::new("root.cpp", format!("#include \"{}\"\n", root.display()))
                .with_root_suffixes([root_suffix]),
        ],
        &[
            "-x",
            "c++",
            include.as_str(),
            "--target=x86_64-pc-windows-msvc",
        ],
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();

    assert!(matches!(
        fact(&snapshot, "root.cpp", "REACHABLE").data,
        FactData::Record { .. }
    ));
    assert!(matches!(
        fact(&snapshot, "root.cpp", "UNREACHABLE").data,
        FactData::None
    ));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn partitioned_closure_preserves_annotated_redeclarations() {
    helpers::ensure_libclang();

    let scratch = scratch("annotated-redeclaration");
    let root = scratch.join("root.h");
    let annotations = scratch.join("annotations.h");
    let provider = scratch.join("provider.h");
    std::fs::write(&root, "typedef unsigned SHARED_ALIAS;\n").unwrap();
    std::fs::write(
        &annotations,
        "#define W32M(text) __attribute__((annotate(text)))\n\
         W32M(\"win32metadata:also_usable_for=OTHER_ALIAS\")\n\
         typedef unsigned SHARED_ALIAS;\n",
    )
    .unwrap();
    std::fs::write(
        &provider,
        "#include \"annotations.h\"\ntypedef unsigned PROVIDER_ROOT;\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned_with_options(
        [
            input("root.cpp", &root, "root", "Closure.Root"),
            input("provider.cpp", &provider, "provider", "Closure.Provider"),
        ],
        &[
            "-x",
            "c++",
            include.as_str(),
            "-DWIN32METADATA=1",
            "--target=x86_64-pc-windows-msvc",
        ],
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    let rdl = snapshot.emit("Closure").unwrap();

    assert!(rdl.contains("#[also_usable_for(\"OTHER_ALIAS\")]"), "{rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn partitioned_closure_validates_annotations_in_pruned_declarations() {
    helpers::ensure_libclang();

    let scratch = scratch("annotation-validation");
    let dependencies = scratch.join("dependencies.h");
    let root = scratch.join("root.h");
    std::fs::write(
        &dependencies,
        "#define W32M(text) __attribute__((annotate(text)))\n\
         extern \"C\" void IgnoredFunction(\n\
             W32M(\"win32metadata:set_last_error\") void* value);\n",
    )
    .unwrap();
    std::fs::write(
        &root,
        "#include \"dependencies.h\"\ntypedef unsigned ROOT_ALIAS;\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let error = extract_partitioned_with_options(
        [input("root.cpp", &root, "root", "Closure.Root")],
        &[
            "-x",
            "c++",
            include.as_str(),
            "-DWIN32METADATA=1",
            "--target=x86_64-pc-windows-msvc",
        ],
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("win32metadata annotation `set_last_error` is not valid"),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn partitioned_closure_retains_parenthesized_string_aliases() {
    helpers::ensure_libclang();

    let scratch = scratch("string-alias");
    let dependencies = scratch.join("dependencies.h");
    let root = scratch.join("root.h");
    std::fs::write(&dependencies, "#define PRIVATE_TEXT \"value\"\n").unwrap();
    std::fs::write(
        &root,
        "#include \"dependencies.h\"\n#define PUBLIC_TEXT (PRIVATE_TEXT)\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned_with_options(
        [input("root.cpp", &root, "root", "Closure.Root")],
        &[
            "-x",
            "c++",
            include.as_str(),
            "--target=x86_64-pc-windows-msvc",
        ],
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();

    assert!(snapshot.constants().iter().any(|constant| {
        constant.name == "PUBLIC_TEXT"
            && constant.value == windows_clang::Value::Utf8("value".to_string())
    }));

    std::fs::remove_dir_all(scratch).unwrap();
}
