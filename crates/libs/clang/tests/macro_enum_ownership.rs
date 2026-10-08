use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use windows_clang::{
    EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition, extract,
};
use windows_metadata::{
    Value,
    reader::{Index, Item},
};

const METADATA_RDL: &str = include_str!("../../../../metadata/metadata.rdl");

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-macro-enum-ownership-{name}-{}",
        std::process::id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn enum_value(index: &Index, namespace: &str, ty: &str, member: &str) -> Value {
    index
        .expect(namespace, ty)
        .fields()
        .find(|field| field.name() == member)
        .unwrap()
        .constant()
        .unwrap()
        .value()
}

fn assert_constant(index: &Index, namespace: &str, name: &str) {
    let Item::Const(_) = index.expect_item(namespace, name) else {
        panic!("{namespace}.{name} was not emitted as a constant");
    };
}

fn constant_value(index: &Index, namespace: &str, name: &str) -> Value {
    let Item::Const(field) = index.expect_item(namespace, name) else {
        panic!("{namespace}.{name} was not emitted as a constant");
    };
    field.constant().unwrap().value()
}

fn write_source(path: &Path) {
    std::fs::write(
        path,
        r#"
            #define DIALOPTION_BILLING 0x00000040
            #pragma push_macro("DIALOPTION_BILLING")
            #undef DIALOPTION_BILLING
            enum MODEMDEVCAPS_DIAL_OPTIONS : unsigned long {
                DIALOPTION_BILLING = 0x00000040
            };
            #pragma pop_macro("DIALOPTION_BILLING")

            #define POSITIVE_UNSIGNED 64U
            #pragma push_macro("POSITIVE_UNSIGNED")
            #undef POSITIVE_UNSIGNED
            enum POSITIVE_SIGNED_FLAGS : int {
                POSITIVE_UNSIGNED = 64
            };
            #pragma pop_macro("POSITIVE_UNSIGNED")

            typedef unsigned int TEST_DWORD;
            typedef TEST_DWORD TEST_DWORD_ALIAS;
            #define TYPED_SCALAR_VALUE ((TEST_DWORD_ALIAS)64)
            #pragma push_macro("TYPED_SCALAR_VALUE")
            #undef TYPED_SCALAR_VALUE
            enum TYPED_SCALAR_FLAGS : unsigned int {
                TYPED_SCALAR_VALUE = 64U
            };
            #pragma pop_macro("TYPED_SCALAR_VALUE")

            #define SCOPED_VALUE 64U
            #pragma push_macro("SCOPED_VALUE")
            #undef SCOPED_VALUE
            enum class SCOPED_FLAGS : unsigned int {
                SCOPED_VALUE = 64U
            };
            #pragma pop_macro("SCOPED_VALUE")

            #define NAMESPACE_VALUE 64U
            #pragma push_macro("NAMESPACE_VALUE")
            #undef NAMESPACE_VALUE
            namespace Domain {
                enum NAMESPACE_FLAGS : unsigned int {
                    NAMESPACE_VALUE = 64U
                };
            }
            #pragma pop_macro("NAMESPACE_VALUE")

            #define AMBIGUOUS_VALUE 64U
            #pragma push_macro("AMBIGUOUS_VALUE")
            #undef AMBIGUOUS_VALUE
            namespace First {
                enum AMBIGUOUS_FLAGS_A : unsigned int {
                    AMBIGUOUS_VALUE = 64U
                };
            }
            namespace Second {
                enum AMBIGUOUS_FLAGS_B : unsigned int {
                    AMBIGUOUS_VALUE = 64U
                };
            }
            #pragma pop_macro("AMBIGUOUS_VALUE")

            #define DIFFERENT_VALUE 64U
            #pragma push_macro("DIFFERENT_VALUE")
            #undef DIFFERENT_VALUE
            enum DIFFERENT_FLAGS : unsigned int {
                DIFFERENT_VALUE = 128U
            };
            #pragma pop_macro("DIFFERENT_VALUE")

            #define REQUESTED_VALUE 64U
            #pragma push_macro("REQUESTED_VALUE")
            #undef REQUESTED_VALUE
            enum __attribute__((annotate(
                "win32metadata:associated_constant=REQUESTED_VALUE")))
                REQUESTED_FLAGS : unsigned int {
                REQUESTED_VALUE = 64U
            };
            #pragma pop_macro("REQUESTED_VALUE")

            enum __attribute__((annotate(
                "win32metadata:associated_constant=REQUESTED_LATER")))
                REQUESTED_LATER_FLAGS : unsigned int {
                REQUESTED_LATER = 128U
            };
            #define REQUESTED_LATER 64U

            typedef void* TEST_HANDLE;
            #define HANDLE_NEGATIVE ((TEST_HANDLE)-2)
            #pragma push_macro("HANDLE_NEGATIVE")
            #undef HANDLE_NEGATIVE
            enum HANDLE_FLAGS : unsigned int {
                HANDLE_NEGATIVE = 0xfffffffeU
            };
            #pragma pop_macro("HANDLE_NEGATIVE")

            #define NEGATIVE_VALUE (-1)
            #pragma push_macro("NEGATIVE_VALUE")
            #undef NEGATIVE_VALUE
            enum NEGATIVE_FLAGS : unsigned int {
                NEGATIVE_VALUE = 0xffffffffU
            };
            #pragma pop_macro("NEGATIVE_VALUE")

            #define SIGNED_HIGH_BIT ((int)0x80000000)
            #pragma push_macro("SIGNED_HIGH_BIT")
            #undef SIGNED_HIGH_BIT
            enum HIGH_BIT_FLAGS : unsigned int {
                SIGNED_HIGH_BIT = 0x80000000U
            };
            #pragma pop_macro("SIGNED_HIGH_BIT")

            #define UNSIGNED_MAX_VALUE 0xffffffffU
            #pragma push_macro("UNSIGNED_MAX_VALUE")
            #undef UNSIGNED_MAX_VALUE
            enum SIGNED_MAX_FLAGS : int {
                UNSIGNED_MAX_VALUE = (int)0xffffffff
            };
            #pragma pop_macro("UNSIGNED_MAX_VALUE")

            #define WIDE_VALUE 64LL
            #pragma push_macro("WIDE_VALUE")
            #undef WIDE_VALUE
            enum WIDE_FLAGS : unsigned int {
                WIDE_VALUE = 64U
            };
            #pragma pop_macro("WIDE_VALUE")

            #define NARROW_VALUE ((unsigned short)64)
            #pragma push_macro("NARROW_VALUE")
            #undef NARROW_VALUE
            enum NARROW_FLAGS : unsigned int {
                NARROW_VALUE = 64U
            };
            #pragma pop_macro("NARROW_VALUE")

            #define OUT_OF_RANGE_VALUE 0x100000000LL
            #pragma push_macro("OUT_OF_RANGE_VALUE")
            #undef OUT_OF_RANGE_VALUE
            enum OUT_OF_RANGE_FLAGS : unsigned int {
                OUT_OF_RANGE_VALUE = 0U
            };
            #pragma pop_macro("OUT_OF_RANGE_VALUE")

            enum LATER_OVERRIDE_FLAGS : unsigned int {
                LATER_OVERRIDE = 128U
            };
            #define LATER_OVERRIDE 64U

            enum class SCOPED_LATER_FLAGS : unsigned int {
                SCOPED_LATER = 128U
            };
            #define SCOPED_LATER 64U

            namespace LaterDomain {
                enum NAMESPACE_LATER_FLAGS : unsigned int {
                    NAMESPACE_LATER = 128U
                };
            }
            #define NAMESPACE_LATER 64U
        "#,
    )
    .unwrap();
}

#[test]
fn macro_enum_ownership_survives_header_planning_and_physical_metadata() {
    helpers::ensure_libclang();

    let scratch = scratch("planner");
    let header = scratch.join("provider.h");
    write_source(&header);
    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!("#include \"{}\"\n", header.to_string_lossy()),
        )
        .with_roots([header.to_string_lossy().to_string()])],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("provider", "Example.Provider"),
    );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example", &references);
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let rdl = partitions.values().next().unwrap();

    assert!(!rdl.contains("const DIALOPTION_BILLING"), "{rdl}");
    assert!(!rdl.contains("const POSITIVE_UNSIGNED"), "{rdl}");
    assert!(!rdl.contains("const TYPED_SCALAR_VALUE"), "{rdl}");
    for name in [
        "SCOPED_VALUE",
        "NAMESPACE_VALUE",
        "AMBIGUOUS_VALUE",
        "DIFFERENT_VALUE",
        "REQUESTED_VALUE",
        "REQUESTED_LATER",
        "HANDLE_NEGATIVE",
        "NEGATIVE_VALUE",
        "SIGNED_HIGH_BIT",
        "UNSIGNED_MAX_VALUE",
        "WIDE_VALUE",
        "NARROW_VALUE",
        "OUT_OF_RANGE_VALUE",
        "SCOPED_LATER",
        "NAMESPACE_LATER",
    ] {
        assert!(rdl.contains(&format!("const {name}:")), "{name}: {rdl}");
    }
    assert!(rdl.contains("DIFFERENT_VALUE = 128"), "{rdl}");
    assert!(rdl.contains("LATER_OVERRIDE = 64"), "{rdl}");
    assert!(!rdl.contains("const LATER_OVERRIDE"), "{rdl}");
    assert!(rdl.contains("REQUESTED_LATER = 128"), "{rdl}");
    assert!(rdl.contains("SCOPED_LATER = 128"), "{rdl}");
    assert!(rdl.contains("NAMESPACE_LATER = 128"), "{rdl}");

    let winmd = scratch.join("macro-enum-ownership.winmd");
    windows_rdl::reader()
        .input_text(METADATA_RDL)
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = Index::read(&winmd).unwrap();
    let namespace = "Example.Provider";

    assert_eq!(
        enum_value(
            &index,
            namespace,
            "MODEMDEVCAPS_DIAL_OPTIONS",
            "DIALOPTION_BILLING"
        ),
        Value::U32(64)
    );
    assert_eq!(
        enum_value(
            &index,
            namespace,
            "POSITIVE_SIGNED_FLAGS",
            "POSITIVE_UNSIGNED"
        ),
        Value::I32(64)
    );
    assert_eq!(
        enum_value(
            &index,
            namespace,
            "TYPED_SCALAR_FLAGS",
            "TYPED_SCALAR_VALUE"
        ),
        Value::U32(64)
    );
    assert_eq!(
        enum_value(&index, namespace, "DIFFERENT_FLAGS", "DIFFERENT_VALUE"),
        Value::U32(128)
    );
    assert_eq!(
        enum_value(&index, namespace, "LATER_OVERRIDE_FLAGS", "LATER_OVERRIDE"),
        Value::U32(64)
    );
    assert_eq!(
        enum_value(&index, namespace, "HIGH_BIT_FLAGS", "SIGNED_HIGH_BIT"),
        Value::U32(2_147_483_648)
    );
    assert_eq!(
        constant_value(&index, namespace, "SIGNED_HIGH_BIT"),
        Value::I32(-2_147_483_648)
    );
    assert_eq!(
        enum_value(&index, namespace, "SIGNED_MAX_FLAGS", "UNSIGNED_MAX_VALUE"),
        Value::I32(-1)
    );
    assert_eq!(
        constant_value(&index, namespace, "UNSIGNED_MAX_VALUE"),
        Value::U32(u32::MAX)
    );
    for name in [
        "SCOPED_VALUE",
        "NAMESPACE_VALUE",
        "AMBIGUOUS_VALUE",
        "DIFFERENT_VALUE",
        "REQUESTED_VALUE",
        "REQUESTED_LATER",
        "HANDLE_NEGATIVE",
        "NEGATIVE_VALUE",
        "SIGNED_HIGH_BIT",
        "UNSIGNED_MAX_VALUE",
        "WIDE_VALUE",
        "NARROW_VALUE",
        "OUT_OF_RANGE_VALUE",
        "SCOPED_LATER",
        "NAMESPACE_LATER",
    ] {
        assert_constant(&index, namespace, name);
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn macro_enum_ownership_does_not_cross_translation_units() {
    helpers::ensure_libclang();

    let scratch = scratch("translation-units");
    let header = scratch.join("provider.h");
    std::fs::write(
        &header,
        r#"
            #ifdef ENUM_TRANSLATION_UNIT
            enum CROSS_TU_FLAGS : unsigned int {
                CROSS_TU_VALUE = 64U
            };
            #else
            #define CROSS_TU_VALUE 64U
            #endif
        "#,
    )
    .unwrap();
    let header_name = header.to_string_lossy().to_string();
    let snapshot = extract(
        [
            Input::new(
                "enum.cpp",
                format!(
                    "#define ENUM_TRANSLATION_UNIT\n#include \"{}\"\n",
                    header.to_string_lossy()
                ),
            )
            .with_roots([header_name.clone()]),
            Input::new(
                "macro.cpp",
                format!("#include \"{}\"\n", header.to_string_lossy()),
            )
            .with_roots([header_name.clone()]),
        ],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header_name,
        RootPartition::new("provider", "Example.Provider"),
    );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example", &references))
        .unwrap();
    let rdl = partitions.values().next().unwrap();

    assert!(rdl.contains("CROSS_TU_VALUE = 64"), "{rdl}");
    assert!(rdl.contains("const CROSS_TU_VALUE: u32 = 64"), "{rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn macro_enum_ownership_does_not_cross_headers() {
    helpers::ensure_libclang();

    let scratch = scratch("headers");
    let macro_header = scratch.join("macro.h");
    let enum_header = scratch.join("enum.h");
    std::fs::write(&macro_header, "#define CROSS_HEADER_VALUE 64U\n").unwrap();
    std::fs::write(
        &enum_header,
        "enum CROSS_HEADER_FLAGS : unsigned int {\n\
             CROSS_HEADER_VALUE = 64U\n\
         };\n",
    )
    .unwrap();
    let macro_name = macro_header.to_string_lossy().to_string();
    let enum_name = enum_header.to_string_lossy().to_string();
    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!(
                "#include \"{}\"\n\
                 #pragma push_macro(\"CROSS_HEADER_VALUE\")\n\
                 #undef CROSS_HEADER_VALUE\n\
                 #include \"{}\"\n\
                 #pragma pop_macro(\"CROSS_HEADER_VALUE\")\n",
                macro_header.to_string_lossy(),
                enum_header.to_string_lossy(),
            ),
        )
        .with_roots([macro_name.clone(), enum_name.clone()])],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    assert!(
        snapshot
            .constants()
            .iter()
            .any(|constant| constant.name == "CROSS_HEADER_VALUE"),
        "{}",
        snapshot.dump()
    );
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            macro_name,
            RootPartition::new("provider", "Example.Provider"),
        )
        .with_traversed_header(
            enum_name,
            RootPartition::new("provider", "Example.Provider"),
        );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example", &references))
        .unwrap();
    let rdl = partitions.values().cloned().collect::<String>();

    assert!(rdl.contains("CROSS_HEADER_VALUE = 64"), "{rdl}");
    assert!(rdl.contains("const CROSS_HEADER_VALUE: u32 = 64"), "{rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}
