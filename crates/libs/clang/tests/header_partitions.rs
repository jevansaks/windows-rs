use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities,
    PartitionConflictReason, PartitionItemKind, RdlPartition, RootPartition, Snapshot, TypeRef,
    TypeReference, TypeReferenceKind, extract, extract_partitioned,
};
use windows_metadata::{
    Type, Value,
    reader::{HasAttributes, Item},
};

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-header-partitions-{name}-{}",
        std::process::id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn aggregate_snapshot(scratch: &Path, headers: &[&Path]) -> Snapshot {
    let source = headers
        .iter()
        .map(|header| format!("#include \"{}\"\n", header.to_string_lossy()))
        .collect::<String>();
    extract(
        [Input::new("aggregate.cpp", source)
            .with_root_dirs([scratch.to_string_lossy().to_string()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap()
}

fn duplicate_declaration_snapshot(
    scratch: &Path,
    declaration: &str,
) -> (PathBuf, PathBuf, Snapshot) {
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(&first, declaration).unwrap();
    std::fs::write(&second, declaration).unwrap();
    let roots = [scratch.to_string_lossy().to_string()];
    let snapshot = extract(
        [
            Input::new(
                "first.cpp",
                format!("#include \"{}\"\n", first.to_string_lossy()),
            )
            .with_root_dirs(roots.clone()),
            Input::new(
                "second.cpp",
                format!("#include \"{}\"\n", second.to_string_lossy()),
            )
            .with_root_dirs(roots),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    (first, second, snapshot)
}

fn duplicate_declaration_policy(
    first: &Path,
    first_partition: RootPartition,
    second: &Path,
    second_partition: RootPartition,
) -> HeaderPartitionPolicy {
    HeaderPartitionPolicy::new()
        .with_traversed_header_for_input("first.cpp", first.to_string_lossy(), first_partition)
        .with_traversed_header_for_input("second.cpp", second.to_string_lossy(), second_partition)
}

fn output<'a>(partitions: &'a BTreeMap<RdlPartition, String>, namespace: &str) -> &'a str {
    partitions
        .iter()
        .find(|(partition, _)| partition.namespace == namespace)
        .unwrap()
        .1
}

fn nonempty_references() -> BTreeMap<String, TypeReference> {
    BTreeMap::from([(
        "EXTERNAL_TYPE".to_string(),
        TypeReference::new("Example.External", "EXTERNAL_TYPE", TypeReferenceKind::Type),
    )])
}

fn colliding_ntstatus_snapshot(name: &str) -> (PathBuf, Snapshot, HeaderPartitionPolicy) {
    let scratch = scratch(name);
    let kernel = scratch.join("kernel.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(
        &kernel,
        "typedef long NTSTATUS;\n\
         extern \"C\" NTSTATUS KernelCall(void);\n",
    )
    .unwrap();
    std::fs::write(&first, "typedef unsigned short NTSTATUS;\n").unwrap();
    std::fs::write(&second, "typedef unsigned int NTSTATUS;\n").unwrap();
    let include = |header: &Path| format!("#include \"{}\"\n", header.to_string_lossy());
    let roots = [scratch.to_string_lossy().to_string()];
    let snapshot = extract(
        [
            Input::new("kernel.cpp", include(&kernel)).with_root_dirs(roots.clone()),
            Input::new("first.cpp", include(&first)).with_root_dirs(roots.clone()),
            Input::new("second.cpp", include(&second)).with_root_dirs(roots),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "kernel.cpp",
            kernel.to_string_lossy(),
            RootPartition::new("kernel", "Example.Kernel").with_exclusion("NTSTATUS"),
        )
        .with_traversed_header_for_input(
            "first.cpp",
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header_for_input(
            "second.cpp",
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        );
    (scratch, snapshot, policy)
}

#[test]
fn namespace_containers_do_not_enter_partition_symbol_collisions() {
    helpers::ensure_libclang();

    let scratch = scratch("namespace-container-collisions");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(
        &first,
        "#pragma once\n\
         #define W32M(text) __attribute__((annotate(text)))\n\
         #define CSTR_LESS_THAN 1\n\
         #define FIRST_FLAG 4\n\
         #pragma push_macro(\"CSTR_LESS_THAN\")\n\
         #pragma push_macro(\"FIRST_FLAG\")\n\
         #undef CSTR_LESS_THAN\n\
         #undef FIRST_FLAG\n\
         namespace Windows { namespace MetadataEnumValues {\n\
             enum COMPARESTRING_RESULT : int { CSTR_LESS_THAN = 1 };\n\
             typedef unsigned short FIRST_ALIAS;\n\
             enum FIRST_FLAGS : unsigned long { FIRST_FLAG = 4 };\n\
         } }\n\
         namespace ABI { namespace FirstNative {\n\
             struct Windows { int first; };\n\
             struct FIRST_WINDOWS_HOLDER { Windows value; };\n\
         } }\n\
         struct GLOBAL_CONTROL { int value; };\n\
         namespace NativeOnly { struct HIDDEN_NATIVE { int value; }; }\n\
         W32M(\"win32metadata:associated_enum=COMPARESTRING_RESULT\")\n\
         extern \"C\" int FirstCompare(void);\n\
         #pragma pop_macro(\"FIRST_FLAG\")\n\
         #pragma pop_macro(\"CSTR_LESS_THAN\")\n",
    )
    .unwrap();
    std::fs::write(
        &second,
        "#pragma once\n\
         #define W32M(text) __attribute__((annotate(text)))\n\
         #define SECOND_FLAG 8\n\
         #pragma push_macro(\"SECOND_FLAG\")\n\
         #undef SECOND_FLAG\n\
         namespace Windows { namespace MetadataEnumValues {\n\
             enum SECOND_FLAGS : unsigned long { SECOND_FLAG = 8 };\n\
             typedef unsigned short SECOND_ALIAS;\n\
         } }\n\
         namespace ABI { namespace SecondNative {\n\
             struct Windows { unsigned long long second; };\n\
             struct SECOND_WINDOWS_HOLDER { Windows value; };\n\
         } }\n\
         W32M(\"win32metadata:associated_enum=SECOND_FLAGS\")\n\
         extern \"C\" unsigned long SecondFlags(void);\n\
         #pragma pop_macro(\"SECOND_FLAG\")\n",
    )
    .unwrap();

    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!(
                "#include \"{}\"\n#include \"{}\"\n",
                first.to_string_lossy(),
                second.to_string_lossy()
            ),
        )
        .with_root_dirs([scratch.to_string_lossy().to_string()])],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();

    assert!(
        !snapshot
            .constants()
            .iter()
            .any(|constant| constant.name == "CSTR_LESS_THAN"),
        "{}",
        snapshot.dump()
    );
    for name in ["FIRST_FLAG", "SECOND_FLAG"] {
        assert!(
            snapshot
                .constants()
                .iter()
                .any(|constant| constant.name == name),
            "{name}\n{}",
            snapshot.dump()
        );
    }

    let references = windows_clang::MetadataReferences::new([windows_metadata::reader::File::new(
        windows_default::WINRT.to_vec(),
    )
    .unwrap()]);
    let focused_exclusions = BTreeSet::from([
        "FIRST_WINDOWS_HOLDER".to_string(),
        "SECOND_WINDOWS_HOLDER".to_string(),
        "Windows".to_string(),
    ]);
    let mut focused_options = EmitOptions::new("Example.Focused", references.types());
    focused_options.excluded_types = Some(&focused_exclusions);
    focused_options.library = Some("test.dll");
    let focused = snapshot
        .emit_by_header_with_options(&focused_options)
        .unwrap();
    let focused_rdl = focused.values().cloned().collect::<String>();
    for name in [
        "COMPARESTRING_RESULT",
        "FIRST_ALIAS",
        "FIRST_FLAGS",
        "SECOND_FLAGS",
        "SECOND_ALIAS",
    ] {
        assert!(focused_rdl.contains(name), "{focused:#?}");
    }

    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        );
    let mut options = EmitOptions::new("Example.Common", references.types());
    options.library = Some("test.dll");
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let first_rdl = output(&partitions, "Example.First");
    let second_rdl = output(&partitions, "Example.Second");

    for name in [
        "COMPARESTRING_RESULT",
        "FIRST_ALIAS",
        "FIRST_FLAGS",
        "FIRST_WINDOWS_HOLDER",
        "GLOBAL_CONTROL",
    ] {
        assert!(first_rdl.contains(name), "{partitions:#?}");
    }
    for name in ["SECOND_ALIAS", "SECOND_FLAGS", "SECOND_WINDOWS_HOLDER"] {
        assert!(second_rdl.contains(name), "{partitions:#?}");
    }
    assert!(
        first_rdl.contains("#[associated_enum(\"COMPARESTRING_RESULT\")]"),
        "{first_rdl}"
    );
    assert!(
        second_rdl.contains("#[associated_enum(\"SECOND_FLAGS\")]"),
        "{second_rdl}"
    );
    assert!(first_rdl.contains("CSTR_LESS_THAN = 1"), "{first_rdl}");
    assert!(!first_rdl.contains("const CSTR_LESS_THAN"), "{first_rdl}");
    assert!(
        first_rdl.contains("const FIRST_FLAG: i32 = 4"),
        "{first_rdl}"
    );
    assert!(
        second_rdl.contains("const SECOND_FLAG: i32 = 8"),
        "{second_rdl}"
    );
    assert!(
        !partitions
            .values()
            .any(|rdl| rdl.contains("HIDDEN_NATIVE") || rdl.contains("__partition_")),
        "{partitions:#?}"
    );

    let winmd = scratch.join("namespace-container-collisions.winmd");
    windows_rdl::reader()
        .input_text(include_str!("../../../../metadata/metadata.rdl"))
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();

    let compare = index.expect("Example.First", "COMPARESTRING_RESULT");
    assert_eq!(compare.underlying_type(), Some(Type::I32));
    assert_eq!(
        compare
            .fields()
            .find(|field| field.name() == "CSTR_LESS_THAN")
            .unwrap()
            .constant()
            .unwrap()
            .value(),
        Value::I32(1)
    );
    assert_eq!(
        index
            .expect("Example.First", "FIRST_ALIAS")
            .underlying_type(),
        Some(Type::U16)
    );
    for (namespace, enum_name, member, value) in [
        ("Example.First", "FIRST_FLAGS", "FIRST_FLAG", 4),
        ("Example.Second", "SECOND_FLAGS", "SECOND_FLAG", 8),
    ] {
        let ty = index.expect(namespace, enum_name);
        assert_eq!(ty.underlying_type(), Some(Type::U32));
        assert_eq!(
            ty.fields()
                .find(|field| field.name() == member)
                .unwrap()
                .constant()
                .unwrap()
                .value(),
            Value::U32(value)
        );
    }
    assert_eq!(
        index
            .expect("Example.Second", "SECOND_ALIAS")
            .underlying_type(),
        Some(Type::U16)
    );

    for (namespace, holder, field, field_type) in [
        ("Example.First", "FIRST_WINDOWS_HOLDER", "first", Type::I32),
        (
            "Example.Second",
            "SECOND_WINDOWS_HOLDER",
            "second",
            Type::U64,
        ),
    ] {
        let windows = index.expect(namespace, "Windows");
        assert_eq!(
            windows.fields().next().unwrap().ty(),
            field_type,
            "{namespace}.Windows.{field}"
        );
        let holder = index.expect(namespace, holder);
        assert_eq!(
            holder.fields().next().unwrap().ty(),
            Type::value_named(namespace, "Windows")
        );
    }
    assert!(index.contains("Example.First", "GLOBAL_CONTROL"));
    assert!(!index.contains("Example.First", "HIDDEN_NATIVE"));
    assert!(!index.contains("Example.Second", "HIDDEN_NATIVE"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn clang_flag_enum_preserves_partition_ownership_and_representation() {
    helpers::ensure_libclang();

    let scratch = scratch("clang-flag-enum");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(
        &first,
        "namespace Windows { namespace FirstNative {\n\
             enum [[clang::flag_enum]] SHARED_FLAGS : int {\n\
                 FIRST_ZERO = 0,\n\
                 FIRST_ONE = 1,\n\
                 FIRST_NEGATIVE = -1,\n\
                 FIRST_HIGH_BIT = (-2147483647 - 1),\n\
             };\n\
             enum SHARED_PLAIN : int {\n\
                 FIRST_PLAIN_ZERO = 0,\n\
                 FIRST_PLAIN_NEGATIVE = -1,\n\
             };\n\
         } }\n",
    )
    .unwrap();
    std::fs::write(
        &second,
        "namespace Windows { namespace SecondNative {\n\
             enum [[clang::flag_enum]] SHARED_FLAGS : unsigned int {\n\
                 SECOND_ZERO = 0u,\n\
                 SECOND_ONE = 1u,\n\
                 SECOND_HIGH_BIT = 0x80000000u,\n\
                 SECOND_ALL = 0xffffffffu,\n\
             };\n\
             enum SHARED_PLAIN : unsigned int {\n\
                 SECOND_PLAIN_ZERO = 0u,\n\
                 SECOND_PLAIN_HIGH_BIT = 0x80000000u,\n\
             };\n\
         } }\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first, &second]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        );
    let references = windows_clang::MetadataReferences::new([windows_metadata::reader::File::new(
        windows_default::WINRT.to_vec(),
    )
    .unwrap()]);
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example.Common", references.types()))
        .unwrap();
    let first_rdl = output(&partitions, "Example.First");
    let second_rdl = output(&partitions, "Example.Second");

    assert!(
        first_rdl.contains("#[repr(i32)]\n        #[flags]\n        enum SHARED_FLAGS"),
        "{partitions:#?}"
    );
    assert!(
        first_rdl.contains("FIRST_NEGATIVE = -1")
            && first_rdl.contains("FIRST_HIGH_BIT = -2147483648"),
        "{first_rdl}"
    );
    assert!(
        first_rdl.contains("#[repr(i32)]\n        enum SHARED_PLAIN")
            && !first_rdl.contains("#[flags]\n        enum SHARED_PLAIN"),
        "{first_rdl}"
    );
    assert!(
        second_rdl.contains("#[repr(u32)]\n        #[flags]\n        enum SHARED_FLAGS"),
        "{partitions:#?}"
    );
    assert!(
        second_rdl.contains("SECOND_HIGH_BIT = 2147483648")
            && second_rdl.contains("SECOND_ALL = 4294967295"),
        "{second_rdl}"
    );
    assert!(
        second_rdl.contains("#[repr(u32)]\n        enum SHARED_PLAIN")
            && !second_rdl.contains("#[flags]\n        enum SHARED_PLAIN"),
        "{second_rdl}"
    );

    let winmd = scratch.join("clang-flag-enum.winmd");
    windows_rdl::reader()
        .input_text(include_str!("../../../../metadata/metadata.rdl"))
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();

    for (namespace, expected_type, values) in [
        (
            "Example.First",
            Type::I32,
            [
                ("FIRST_ZERO", Value::I32(0)),
                ("FIRST_ONE", Value::I32(1)),
                ("FIRST_NEGATIVE", Value::I32(-1)),
                ("FIRST_HIGH_BIT", Value::I32(i32::MIN)),
            ],
        ),
        (
            "Example.Second",
            Type::U32,
            [
                ("SECOND_ZERO", Value::U32(0)),
                ("SECOND_ONE", Value::U32(1)),
                ("SECOND_HIGH_BIT", Value::U32(0x8000_0000)),
                ("SECOND_ALL", Value::U32(u32::MAX)),
            ],
        ),
    ] {
        let flags = index.expect(namespace, "SHARED_FLAGS");
        assert_eq!(flags.underlying_type(), Some(expected_type.clone()));
        assert!(flags.attributes().any(|attribute| {
            attribute.name() == "FlagsAttribute"
                && attribute.ctor().parent().namespace() == "System"
        }));
        for (name, value) in values {
            assert_eq!(
                flags
                    .fields()
                    .find(|field| field.name() == name)
                    .unwrap()
                    .constant()
                    .unwrap()
                    .value(),
                value
            );
        }

        let plain = index.expect(namespace, "SHARED_PLAIN");
        assert_eq!(plain.underlying_type(), Some(expected_type));
        assert!(!plain.attributes().any(|attribute| {
            attribute.name() == "FlagsAttribute"
                && attribute.ctor().parent().namespace() == "System"
        }));
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn traversed_headers_select_roots_and_route_dependencies_to_default() {
    helpers::ensure_libclang();

    let scratch = scratch("closure");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "typedef struct DEPENDENCY { int value; } DEPENDENCY;\n\
         typedef unsigned INCLUDED_ONLY;\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             typedef struct PUBLIC_TYPE {{ DEPENDENCY dependency; }} PUBLIC_TYPE;\n\
             typedef unsigned PUBLIC_ALIAS;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public"),
    );
    let references = nonempty_references();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let public_rdl = output(&partitions, "Example.Public");
    let common_rdl = output(&partitions, "Example.Common");

    assert!(public_rdl.contains("struct PUBLIC_TYPE"), "{public_rdl}");
    assert!(
        public_rdl.contains("type PUBLIC_ALIAS = u32"),
        "{public_rdl}"
    );
    assert!(!public_rdl.contains("struct DEPENDENCY"), "{public_rdl}");
    assert!(common_rdl.contains("struct DEPENDENCY"), "{common_rdl}");
    assert!(
        !partitions.values().any(|rdl| rdl.contains("INCLUDED_ONLY")),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn associated_enums_from_included_headers_follow_the_annotated_root() {
    helpers::ensure_libclang();

    let scratch = scratch("associated-enum-dependency");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         namespace Windows { namespace MetadataEnumValues {\n\
             enum [[clang::flag_enum]] DEPENDENCY_FLAGS : unsigned int {\n\
                 DEPENDENCY_NONE = 0u,\n\
                 DEPENDENCY_HIGH = 0x80000000u,\n\
             };\n\
             typedef enum _DEPENDENCY_STATUS : int {\n\
                 DEPENDENCY_FAILED = -1,\n\
                 DEPENDENCY_OK = 0,\n\
             } DEPENDENCY_STATUS;\n\
             enum UNSELECTED_ENUM : unsigned int { UNSELECTED_VALUE = 1u };\n\
             enum INCLUDED_ONLY_ENUM : unsigned int { INCLUDED_ONLY_VALUE = 1u };\n\
         } }\n\
         typedef unsigned INCLUDED_ONLY_ALIAS;\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        "#pragma once\n\
         #define W32M(text) __attribute__((annotate(text)))\n\
         #include \"dependency.h\"\n\
         struct PUBLIC_ASSOCIATIONS {\n\
             W32M(\"win32metadata:associated_enum=DEPENDENCY_STATUS\") int status;\n\
         };\n\
         extern \"C\" void UseDependencyFlags(\n\
             W32M(\"win32metadata:associated_enum=DEPENDENCY_FLAGS\") unsigned flags);\n\
         extern \"C\" void UseUnselectedEnum(\n\
             W32M(\"win32metadata:associated_enum=UNSELECTED_ENUM\") unsigned value);\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public"),
    );
    let references = nonempty_references();
    let selected_functions = BTreeSet::from(["UseDependencyFlags".to_string()]);
    let mut options = EmitOptions::new("Example.Common", &references);
    options.functions = Some(&selected_functions);
    options.library = Some("test.dll");
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let public_rdl = output(&partitions, "Example.Public");

    assert!(
        public_rdl.contains("struct PUBLIC_ASSOCIATIONS"),
        "{public_rdl}"
    );
    assert!(
        public_rdl.contains("fn UseDependencyFlags("),
        "{public_rdl}"
    );
    assert!(
        public_rdl.contains("#[associated_enum(\"DEPENDENCY_FLAGS\")]"),
        "{public_rdl}"
    );
    assert!(
        public_rdl.contains("#[associated_enum(\"DEPENDENCY_STATUS\")]"),
        "{public_rdl}"
    );
    assert!(
        public_rdl.contains("#[repr(u32)]\n        #[flags]\n        enum DEPENDENCY_FLAGS"),
        "{partitions:#?}"
    );
    assert!(
        public_rdl.contains("DEPENDENCY_HIGH = 2147483648"),
        "{public_rdl}"
    );
    assert!(
        public_rdl.contains("#[repr(i32)]\n        enum DEPENDENCY_STATUS"),
        "{partitions:#?}"
    );
    assert!(
        public_rdl.contains("DEPENDENCY_FAILED = -1"),
        "{public_rdl}"
    );
    assert!(
        !partitions.values().any(|rdl| {
            rdl.contains("UNSELECTED_ENUM")
                || rdl.contains("UseUnselectedEnum")
                || rdl.contains("INCLUDED_ONLY_ENUM")
                || rdl.contains("INCLUDED_ONLY_ALIAS")
        }),
        "{partitions:#?}"
    );
    assert!(
        !partitions
            .keys()
            .any(|partition| partition.namespace == "Example.Common"),
        "{partitions:#?}"
    );

    let legacy = extract(
        [Input::new(
            "legacy.cpp",
            format!("#include \"{}\"\n", public.to_string_lossy()),
        )
        .with_roots([public.to_string_lossy().to_string()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap()
    .emit_with_options(&options)
    .unwrap();
    assert!(
        legacy.contains("struct PUBLIC_ASSOCIATIONS") && legacy.contains("fn UseDependencyFlags("),
        "{legacy}"
    );
    assert!(
        !legacy.contains("enum DEPENDENCY_FLAGS")
            && !legacy.contains("enum DEPENDENCY_STATUS")
            && !legacy.contains("enum UNSELECTED_ENUM"),
        "{legacy}"
    );

    let winmd = scratch.join("associated-enum-dependency.winmd");
    windows_rdl::reader()
        .input_text(include_str!("../../../../metadata/metadata.rdl"))
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();

    let flags = index.expect("Example.Public", "DEPENDENCY_FLAGS");
    assert_eq!(flags.underlying_type(), Some(Type::U32));
    assert!(flags.attributes().any(|attribute| {
        attribute.name() == "FlagsAttribute" && attribute.ctor().parent().namespace() == "System"
    }));
    assert_eq!(
        flags
            .fields()
            .find(|field| field.name() == "DEPENDENCY_HIGH")
            .unwrap()
            .constant()
            .unwrap()
            .value(),
        Value::U32(0x8000_0000)
    );

    let status = index.expect("Example.Public", "DEPENDENCY_STATUS");
    assert_eq!(status.underlying_type(), Some(Type::I32));
    assert!(!status.attributes().any(|attribute| {
        attribute.name() == "FlagsAttribute" && attribute.ctor().parent().namespace() == "System"
    }));
    assert_eq!(
        status
            .fields()
            .find(|field| field.name() == "DEPENDENCY_FAILED")
            .unwrap()
            .constant()
            .unwrap()
            .value(),
        Value::I32(-1)
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn associated_enum_owners_use_exclusions_authority_and_conflict_audit() {
    helpers::ensure_libclang();

    let scratch = scratch("associated-enum-owners");
    let dependency = scratch.join("dependency.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         enum SHARED_ASSOCIATED : unsigned int { SHARED_ASSOCIATED_VALUE = 1u };\n",
    )
    .unwrap();
    std::fs::write(
        &first,
        "#pragma once\n\
         #define W32M(text) __attribute__((annotate(text)))\n\
         #include \"dependency.h\"\n\
         struct FIRST_ROOT {\n\
             W32M(\"win32metadata:associated_enum=SHARED_ASSOCIATED\") unsigned value;\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &second,
        "#pragma once\n\
         #define W32M(text) __attribute__((annotate(text)))\n\
         #include \"dependency.h\"\n\
         struct SECOND_ROOT {\n\
             W32M(\"win32metadata:associated_enum=SHARED_ASSOCIATED\") unsigned value;\n\
         };\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first, &second]);
    let first_owner = RootPartition::new("first", "Example.First");
    let second_owner = RootPartition::new("second", "Example.Second")
        .with_remap("SHARED_ASSOCIATED", "SHARED_RENAMED")
        .with_flags("SHARED_ASSOCIATED");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(first.to_string_lossy(), first_owner.clone())
        .with_traversed_header(second.to_string_lossy(), second_owner.clone());
    let reverse = HeaderPartitionPolicy::new()
        .with_traversed_header(second.to_string_lossy(), second_owner.clone())
        .with_traversed_header(first.to_string_lossy(), first_owner.clone());
    let references = nonempty_references();
    let options = EmitOptions::new("Example.Common", &references);

    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let audit = plan.audit(&options).unwrap();
    let reverse_audit = snapshot
        .plan_header_partitions(&reverse, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&options)
        .unwrap();
    assert_eq!(audit, reverse_audit);
    assert_eq!(audit.conflicts().len(), 1, "{audit}");
    assert_eq!(audit.conflicts()[0].name, "SHARED_ASSOCIATED");
    assert_eq!(
        audit.conflicts()[0].reason,
        PartitionConflictReason::AmbiguousRootCandidates
    );
    assert_eq!(audit.conflicts()[0].owners.len(), 2);
    assert!(plan.emit_with_options(&options).is_err());

    let excluded_policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            first_owner.with_exclusion("SHARED_ASSOCIATED"),
        )
        .with_traversed_header(second.to_string_lossy(), second_owner);
    let excluded = snapshot
        .plan_header_partitions(&excluded_policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(excluded.audit(&options).unwrap().is_clean());
    let excluded = excluded.emit_with_options(&options).unwrap();
    assert!(
        output(&excluded, "Example.Second").contains("#[flags]\n        enum SHARED_RENAMED"),
        "{excluded:#?}"
    );
    assert!(
        !output(&excluded, "Example.First").contains("enum SHARED_ASSOCIATED"),
        "{excluded:#?}"
    );
    assert!(
        excluded
            .values()
            .all(|rdl| !rdl.contains("associated_enum(\"SHARED_ASSOCIATED\")")),
        "{excluded:#?}"
    );
    assert!(
        excluded
            .values()
            .filter(|rdl| rdl.contains("struct FIRST_ROOT") || rdl.contains("struct SECOND_ROOT"))
            .all(|rdl| rdl.contains("associated_enum(\"SHARED_RENAMED\")")),
        "{excluded:#?}"
    );

    let authorities = NamespaceAuthorities::new().with_exact("SHARED_ASSOCIATED", "Example.Second");
    let authoritative = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap();
    assert!(authoritative.audit(&options).unwrap().is_clean());
    let authoritative = authoritative.emit_with_options(&options).unwrap();
    assert!(
        output(&authoritative, "Example.Second").contains("#[flags]\n        enum SHARED_RENAMED"),
        "{authoritative:#?}"
    );
    assert!(
        !output(&authoritative, "Example.First").contains("enum SHARED_ASSOCIATED"),
        "{authoritative:#?}"
    );

    let reverse_authoritative = snapshot
        .plan_header_partitions(&reverse, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    assert_eq!(authoritative, reverse_authoritative);

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn associated_enum_dependencies_do_not_collapse_owned_same_leaf_types() {
    helpers::ensure_libclang();

    let scratch = scratch("associated-enum-same-leaf");
    let first_enum = scratch.join("first-enum.h");
    let second_enum = scratch.join("second-enum.h");
    let first_api = scratch.join("first-api.h");
    let second_api = scratch.join("second-api.h");
    std::fs::write(
        &first_enum,
        "#pragma once\n\
         namespace Windows { namespace FirstNative {\n\
             enum SHARED_ASSOCIATED : int { FIRST_ASSOCIATED_VALUE = -1 };\n\
         } }\n",
    )
    .unwrap();
    std::fs::write(
        &second_enum,
        "#pragma once\n\
         namespace Windows { namespace SecondNative {\n\
             enum SHARED_ASSOCIATED : unsigned int { SECOND_ASSOCIATED_VALUE = 1u };\n\
         } }\n",
    )
    .unwrap();
    std::fs::write(
        &first_api,
        "#pragma once\n\
         #define W32M(text) __attribute__((annotate(text)))\n\
         #include \"first-enum.h\"\n\
         struct FIRST_ASSOCIATED_ROOT {\n\
             W32M(\"win32metadata:associated_enum=SHARED_ASSOCIATED\") int value;\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &second_api,
        "#pragma once\n\
         #define W32M(text) __attribute__((annotate(text)))\n\
         #include \"second-enum.h\"\n\
         struct SECOND_ASSOCIATED_ROOT {\n\
             W32M(\"win32metadata:associated_enum=SHARED_ASSOCIATED\") unsigned value;\n\
         };\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first_api, &second_api]);
    let first_owner = RootPartition::new("first", "Example.First");
    let second_owner = RootPartition::new("second", "Example.Second");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(first_enum.to_string_lossy(), first_owner.clone())
        .with_traversed_header(first_api.to_string_lossy(), first_owner)
        .with_traversed_header(second_enum.to_string_lossy(), second_owner.clone())
        .with_traversed_header(second_api.to_string_lossy(), second_owner);
    let references = nonempty_references();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let first = partitions
        .iter()
        .filter(|(partition, _)| partition.namespace == "Example.First")
        .map(|(_, rdl)| rdl.as_str())
        .collect::<String>();
    let second = partitions
        .iter()
        .filter(|(partition, _)| partition.namespace == "Example.Second")
        .map(|(_, rdl)| rdl.as_str())
        .collect::<String>();

    assert!(first.contains("enum SHARED_ASSOCIATED"), "{first}");
    assert!(first.contains("FIRST_ASSOCIATED_VALUE = -1"), "{first}");
    assert!(first.contains("struct FIRST_ASSOCIATED_ROOT"), "{first}");
    assert!(
        first.contains("#[associated_enum(\"SHARED_ASSOCIATED\")]"),
        "{first}"
    );
    assert!(second.contains("enum SHARED_ASSOCIATED"), "{second}");
    assert!(second.contains("SECOND_ASSOCIATED_VALUE = 1"), "{second}");
    assert!(second.contains("struct SECOND_ASSOCIATED_ROOT"), "{second}");
    assert!(
        second.contains("#[associated_enum(\"SHARED_ASSOCIATED\")]"),
        "{second}"
    );
    assert!(
        !partitions.values().any(|rdl| rdl.contains("__partition_")),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn namespaced_exact_dependency_uses_default_namespace() {
    helpers::ensure_libclang();

    let scratch = scratch("namespaced-exact-dependency");
    let dependency = scratch.join("DirectXMath.h");
    let public = scratch.join("x3daudio.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         namespace DirectX {\n\
         struct XMFLOAT3 { float x; float y; float z; };\n\
         struct UNUSED_VECTOR { float x; float y; float z; float w; };\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        "#include \"DirectXMath.h\"\n\
         typedef DirectX::XMFLOAT3 X3DAUDIO_VECTOR;\n\
         typedef struct X3DAUDIO_EMITTER { X3DAUDIO_VECTOR position; } X3DAUDIO_EMITTER;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("XAudio2", "Example.XAudio2"),
    );
    let references = nonempty_references();
    let options = EmitOptions::new("Windows.Win32", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let xaudio = output(&partitions, "Example.XAudio2");
    let default = output(&partitions, "Windows.Win32");

    assert!(
        xaudio.contains("type X3DAUDIO_VECTOR = Windows::Win32::XMFLOAT3"),
        "{xaudio}"
    );
    assert!(xaudio.contains("struct X3DAUDIO_EMITTER"), "{xaudio}");
    assert!(default.contains("struct XMFLOAT3"), "{default}");
    assert!(
        !partitions.values().any(|rdl| rdl.contains("UNUSED_VECTOR")),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn explicit_dependency_header_mapping_wins_over_default_namespace() {
    helpers::ensure_libclang();

    let scratch = scratch("explicit-namespaced-dependency");
    let dependency = scratch.join("DirectXMath.h");
    let public = scratch.join("x3daudio.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         namespace DirectX {\n\
         struct XMFLOAT3 { float x; float y; float z; };\n\
         struct UNUSED_VECTOR { float x; float y; float z; float w; };\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        "#include \"DirectXMath.h\"\n\
         typedef DirectX::XMFLOAT3 X3DAUDIO_VECTOR;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            public.to_string_lossy(),
            RootPartition::new("XAudio2", "Example.XAudio2"),
        )
        .with_traversed_header(
            dependency.to_string_lossy(),
            RootPartition::new("DirectX", "Example.DirectX").with_exclusion("UNUSED_VECTOR"),
        );
    let references = nonempty_references();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Windows.Win32", &references))
        .unwrap();
    let xaudio = output(&partitions, "Example.XAudio2");
    let directx = output(&partitions, "Example.DirectX");

    assert!(
        xaudio.contains("type X3DAUDIO_VECTOR = Example::DirectX::XMFLOAT3"),
        "{xaudio}"
    );
    assert!(directx.contains("struct XMFLOAT3"), "{directx}");
    assert!(
        !partitions
            .keys()
            .any(|key| key.namespace == "Windows.Win32")
    );
    assert!(
        !partitions.values().any(|rdl| rdl.contains("UNUSED_VECTOR")),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn shared_unlisted_dependency_emits_once_in_default_namespace() {
    helpers::ensure_libclang();

    let scratch = scratch("shared-default-dependency");
    let dependency = scratch.join("DirectXMath.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         namespace DirectX { struct XMFLOAT3 { float x; float y; float z; }; }\n",
    )
    .unwrap();
    std::fs::write(
        &first,
        "#include \"DirectXMath.h\"\n\
         typedef DirectX::XMFLOAT3 FIRST_VECTOR;\n",
    )
    .unwrap();
    std::fs::write(
        &second,
        "#include \"DirectXMath.h\"\n\
         typedef DirectX::XMFLOAT3 SECOND_VECTOR;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first, &second]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("First", "Example.First"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("Second", "Example.Second"),
        );
    let references = nonempty_references();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Windows.Win32", &references))
        .unwrap();
    let combined = partitions.values().cloned().collect::<Vec<_>>().join("\n");

    assert_eq!(combined.matches("struct XMFLOAT3").count(), 1, "{combined}");
    assert!(
        output(&partitions, "Example.First")
            .contains("type FIRST_VECTOR = Windows::Win32::XMFLOAT3"),
        "{partitions:#?}"
    );
    assert!(
        output(&partitions, "Example.Second")
            .contains("type SECOND_VECTOR = Windows::Win32::XMFLOAT3"),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn transitive_unlisted_dependencies_use_default_namespace() {
    helpers::ensure_libclang();

    let scratch = scratch("transitive-default-dependency");
    let dependency = scratch.join("internal.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         namespace Internal {\n\
         struct LEAF { int value; };\n\
         struct MIDDLE { LEAF leaf; };\n\
         struct UNUSED { int value; };\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        "#include \"internal.h\"\n\
         typedef Internal::MIDDLE PUBLIC_CHAIN;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("Public", "Example.Public"),
    );
    let references = nonempty_references();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Windows.Win32", &references))
        .unwrap();
    let public = output(&partitions, "Example.Public");
    let default = output(&partitions, "Windows.Win32");

    assert!(
        public.contains("type PUBLIC_CHAIN = Windows::Win32::MIDDLE"),
        "{public}"
    );
    assert!(default.contains("struct MIDDLE"), "{default}");
    assert!(default.contains("leaf: LEAF"), "{default}");
    assert!(default.contains("struct LEAF"), "{default}");
    assert!(
        !partitions.values().any(|rdl| rdl.contains("UNUSED")),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn canonical_dependency_typedef_does_not_require_a_tagged_owner() {
    helpers::ensure_libclang();

    let scratch = scratch("canonical-dependency-typedef");
    let foundation = scratch.join("minwindef.h");
    let alternate = scratch.join("alternate.h");
    let intsafe = scratch.join("intsafe.h");
    let satellite = scratch.join("satellite.h");
    std::fs::write(&foundation, "typedef unsigned long DWORD;\n").unwrap();
    std::fs::write(&alternate, "typedef unsigned long long DWORD;\n").unwrap();
    std::fs::write(&intsafe, "typedef unsigned long DWORD;\n").unwrap();
    std::fs::write(
        &satellite,
        "#include \"minwindef.h\"\n\
         #include \"intsafe.h\"\n\
         typedef struct SATELLITE_VALUE { DWORD value; } SATELLITE_VALUE;\n",
    )
    .unwrap();

    let include = format!("-I{}", scratch.display());
    let snapshot = extract(
        [
            Input::new("foundation.cpp", "#include \"minwindef.h\"\n")
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
            Input::new("alternate.cpp", "#include \"alternate.h\"\n")
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
            Input::new("satellites.cpp", "#include \"satellite.h\"\n")
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
        ],
        &[
            "-x",
            "c++",
            "--target=x86_64-pc-windows-msvc",
            include.as_str(),
        ],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "foundation.cpp",
            "minwindef.h",
            RootPartition::new("MinWinDef", "Example.Foundation"),
        )
        .with_traversed_header_for_input(
            "alternate.cpp",
            "alternate.h",
            RootPartition::new("Alternate", "Example.Alternate"),
        )
        .with_traversed_header_for_input(
            "satellites.cpp",
            "satellite.h",
            RootPartition::new("Satellite", "Example.Satellite"),
        );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let satellite = output(&partitions, "Example.Satellite");

    assert!(satellite.contains("struct SATELLITE_VALUE"), "{satellite}");
    assert!(satellite.contains("value: u32"), "{satellite}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn canonical_name_on_dependency_record_still_requires_closure() {
    helpers::ensure_libclang();

    let scratch = scratch("canonical-name-record");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(&dependency, "struct DWORD { unsigned value; };\n").unwrap();
    std::fs::write(
        &public,
        "#include \"dependency.h\"\n\
         typedef struct PUBLIC_VALUE { struct DWORD value; } PUBLIC_VALUE;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("Public", "Example.Public"),
    );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let public = output(&partitions, "Example.Public");
    let common = output(&partitions, "Example.Common");

    assert!(common.contains("struct DWORD"), "{common}");
    assert!(public.contains("value: Example::Common::DWORD"), "{public}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn aggregate_facts_route_to_two_logical_namespaces() {
    helpers::ensure_libclang();

    let scratch = scratch("namespaces");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(&first, "typedef unsigned FIRST_VALUE;\n").unwrap();
    std::fs::write(&second, "typedef unsigned SECOND_VALUE;\n").unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first, &second]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        );
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_header_partitions_with_options(
            &policy,
            &EmitOptions::new("Example.Common", &references),
        )
        .unwrap();

    assert!(output(&partitions, "Example.First").contains("type FIRST_VALUE = u32"));
    assert!(output(&partitions, "Example.Second").contains("type SECOND_VALUE = u32"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn shared_header_candidates_use_exclusions_authority_and_owner_settings() {
    helpers::ensure_libclang();

    let scratch = scratch("shared");
    let shared = scratch.join("shared.h");
    std::fs::write(
        &shared,
        "typedef int FIRST_ONLY;\n\
         typedef int SECOND_ONLY;\n\
         typedef unsigned short FORCED;\n\
         extern \"C\" int RoutedFunction(void);\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&shared]);
    let first = RootPartition::new("first", "Example.First").with_exclusion("SECOND_ONLY");
    let second = RootPartition::new("second", "Example.Second")
        .with_exclusion("FIRST_ONLY")
        .with_remap("SECOND_ONLY", "SECOND_RENAMED")
        .with_u32_type("FORCED")
        .with_library("RoutedFunction", "routed.dll");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(shared.to_string_lossy(), first.clone())
        .with_traversed_header(shared.to_string_lossy(), second.clone());
    let reverse = HeaderPartitionPolicy::new()
        .with_traversed_header(shared.to_string_lossy(), second)
        .with_traversed_header(shared.to_string_lossy(), first);
    let authorities = NamespaceAuthorities::new()
        .with_exact("FORCED", "Example.Second")
        .with_exact("RoutedFunction", "Example.Second");
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let reverse_partitions = snapshot
        .plan_header_partitions(&reverse, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    assert_eq!(partitions, reverse_partitions);
    let first = output(&partitions, "Example.First");
    let second = output(&partitions, "Example.Second");

    assert!(first.contains("type FIRST_ONLY = i32"), "{first}");
    assert!(!first.contains("SECOND_ONLY"), "{first}");
    assert!(second.contains("type SECOND_RENAMED = i32"), "{second}");
    assert!(second.contains("type FORCED = u32"), "{second}");
    assert!(second.contains("#[library(\"routed.dll\")]"), "{second}");
    assert!(second.contains("fn RoutedFunction() -> i32"), "{second}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn shared_header_default_owner_accepts_named_overrides() {
    helpers::ensure_libclang();

    let scratch = scratch("named-overrides");
    let shared = scratch.join("shared.h");
    std::fs::write(
        &shared,
        "typedef int DEFAULT_TYPE;\n\
         typedef int SPECIAL_TYPE;\n\
         #define DEFAULT_VALUE 1\n\
         #define SPECIAL_VALUE 2\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&shared]);
    let default = RootPartition::new("default", "Example.Default");
    let special = RootPartition::new("special", "Example.Special");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input("AGGREGATE.CPP", shared.to_string_lossy(), default)
        .with_traversed_header_override("SHARED.H", "SPECIAL_TYPE", special.clone())
        .with_traversed_header_override_for_input(
            "aggregate.cpp",
            "shared.h",
            "SPECIAL_VALUE",
            special,
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let default = output(&partitions, "Example.Default");
    let special = output(&partitions, "Example.Special");

    assert!(default.contains("type DEFAULT_TYPE = i32"), "{default}");
    assert!(
        default.contains("const DEFAULT_VALUE: i32 = 1"),
        "{default}"
    );
    assert!(!default.contains("SPECIAL_TYPE"), "{default}");
    assert!(!default.contains("SPECIAL_VALUE"), "{default}");
    assert!(special.contains("type SPECIAL_TYPE = i32"), "{special}");
    assert!(
        special.contains("const SPECIAL_VALUE: i32 = 2"),
        "{special}"
    );
    assert!(!special.contains("DEFAULT_TYPE"), "{special}");
    assert!(!special.contains("DEFAULT_VALUE"), "{special}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn unlisted_dependencies_use_default_namespace_and_authority_overrides() {
    helpers::ensure_libclang();

    let scratch = scratch("dependency-audit");
    let dependency = scratch.join("dependency.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         typedef struct DEP_FIRST { int value; } DEP_FIRST;\n\
         typedef struct DEP_SECOND { int value; } DEP_SECOND;\n",
    )
    .unwrap();
    std::fs::write(
        &first,
        format!(
            "#include \"{}\"\n\
             typedef struct FIRST_ROOT {{ DEP_FIRST first; DEP_SECOND second; }} FIRST_ROOT;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &second,
        format!(
            "#include \"{}\"\n\
             typedef struct SECOND_ROOT {{ DEP_FIRST first; DEP_SECOND second; }} SECOND_ROOT;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first, &second]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let dependencies = output(&partitions, "Example.Common");
    assert!(dependencies.contains("struct DEP_FIRST"), "{dependencies}");
    assert!(dependencies.contains("struct DEP_SECOND"), "{dependencies}");

    let authorities = NamespaceAuthorities::new().with_wildcard("DEP_*", "Example.Dependencies");
    let plan = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let dependencies = output(&partitions, "Example.Dependencies");
    assert!(dependencies.contains("struct DEP_FIRST"), "{dependencies}");
    assert!(dependencies.contains("struct DEP_SECOND"), "{dependencies}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn audit_reports_every_unresolved_shared_header_owner() {
    helpers::ensure_libclang();

    let scratch = scratch("audit");
    let shared = scratch.join("shared.h");
    std::fs::write(
        &shared,
        "typedef unsigned FIRST_CONFLICT;\n\
         typedef unsigned SECOND_CONFLICT;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&shared]);
    let first = RootPartition::new("first", "Example.First");
    let second = RootPartition::new("second", "Example.Second");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(shared.to_string_lossy(), second.clone())
        .with_traversed_header(shared.to_string_lossy(), first.clone());
    let reverse = HeaderPartitionPolicy::new()
        .with_traversed_header(shared.to_string_lossy(), first)
        .with_traversed_header(shared.to_string_lossy(), second);
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let audit = plan.audit(&options).unwrap();
    let reverse_audit = snapshot
        .plan_header_partitions(&reverse, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&options)
        .unwrap();

    assert_eq!(audit, reverse_audit);
    assert_eq!(audit.conflicts().len(), 2, "{audit}");
    assert_eq!(
        audit
            .conflicts()
            .iter()
            .map(|conflict| conflict.name.as_str())
            .collect::<Vec<_>>(),
        ["FIRST_CONFLICT", "SECOND_CONFLICT"]
    );
    assert!(audit.conflicts().iter().all(|conflict| {
        conflict.kind == PartitionItemKind::Type
            && conflict.reason == PartitionConflictReason::AmbiguousRootCandidates
            && conflict.owners.len() == 2
    }));
    let error = plan.emit_with_options(&options).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("header partition planning found 2 conflict(s)"),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn dependency_closure_reports_all_independent_blockers() {
    helpers::ensure_libclang();

    let scratch = scratch("dependency-blockers");
    let dependencies = scratch.join("dependencies.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependencies,
        "class BAD_UNSUPPORTED {\n\
         private:\n\
             int value;\n\
         };\n\
         struct SHARED_WRAPPER {\n\
             BAD_UNSUPPORTED unsupported;\n\
         };\n\
         class __declspec(uuid(\"12345678-1234-5678-90ab-cdef12345678\")) BAD_COCLASS;\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             struct ROOT_B {{\n\
                 SHARED_WRAPPER wrapper;\n\
                 BAD_COCLASS *coclass;\n\
             }};\n\
             struct ROOT_A {{\n\
                 SHARED_WRAPPER wrapper;\n\
             }};\n",
            dependencies.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public"),
    );
    let references = nonempty_references();
    let options = EmitOptions::new("Example.Common", &references);

    let audit_error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&options)
        .unwrap_err()
        .to_string();
    let emit_error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap_err()
        .to_string();
    assert_eq!(audit_error, emit_error);
    assert!(
        emit_error.contains("header partition dependency closure found 2 blocker(s)"),
        "{emit_error}"
    );
    assert!(
        emit_error.contains(
            "coverage: selected_roots=2 processed_unique_dependencies=3 \
             resolved_dependencies=1 unique_blockers=2"
        ),
        "{emit_error}"
    );
    let coclass = emit_error.find("`BAD_COCLASS`").unwrap();
    let unsupported = emit_error.find("`BAD_UNSUPPORTED`").unwrap();
    assert!(coclass < unsupported, "{emit_error}");
    let coclass_blocker = &emit_error[coclass..unsupported];
    assert!(
        coclass_blocker.contains("classified as a coclass GUID value, not a type fact"),
        "{coclass_blocker}"
    );
    assert!(
        coclass_blocker.contains("type `ROOT_B`"),
        "{coclass_blocker}"
    );
    assert!(
        !coclass_blocker.contains("type `ROOT_A`"),
        "{coclass_blocker}"
    );
    let unsupported_blocker = &emit_error[unsupported..];
    assert!(
        unsupported_blocker
            .contains("unsupported declaration: class is not a public data-only record"),
        "{unsupported_blocker}"
    );
    assert!(
        unsupported_blocker.contains("type `ROOT_A`"),
        "{unsupported_blocker}"
    );
    assert!(
        unsupported_blocker.contains("type `ROOT_B`"),
        "{unsupported_blocker}"
    );
    assert!(
        emit_error.contains(
            "limitation: dependencies beneath a missing, ambiguous, or unsupported type cannot be \
             inspected until that blocker is resolved"
        ),
        "{emit_error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn macro_generated_declarations_use_expansion_provenance() {
    helpers::ensure_libclang();

    let scratch = scratch("macro");
    let macros = scratch.join("macros.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &macros,
        "#define DECLARE_FIXED typedef unsigned EXPANDED_TYPE\n\
         #define INCLUDED_CONSTANT 7\n\
         #define CONSTANT_VALUE 9\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             DECLARE_FIXED;\n\
             #define PUBLIC_CONSTANT CONSTANT_VALUE\n",
            macros.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let expanded = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == "EXPANDED_TYPE")
        .unwrap();
    assert!(
        expanded.spelling.file.ends_with("macros.h"),
        "{expanded:#?}"
    );
    assert!(
        expanded.expansion.file.ends_with("public.h"),
        "{expanded:#?}"
    );

    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public"),
    );
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_header_partitions_with_options(
            &policy,
            &EmitOptions::new("Example.Common", &references),
        )
        .unwrap();
    let public_rdl = output(&partitions, "Example.Public");

    assert!(
        public_rdl.contains("type EXPANDED_TYPE = u32"),
        "{public_rdl}"
    );
    assert!(
        public_rdl.contains("const PUBLIC_CONSTANT: i32 = 9"),
        "{public_rdl}"
    );
    assert!(!public_rdl.contains("INCLUDED_CONSTANT"), "{public_rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn legacy_partitioned_api_behavior_is_unchanged() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new("legacy.h", "typedef unsigned LEGACY_VALUE;\n")
            .partitioned("legacy-input")
            .with_root("legacy.h", "legacy", "Example.Legacy")],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert!(output(&partitions, "Example.Legacy").contains("type LEGACY_VALUE = u32"));
}

#[test]
fn excluded_dependency_reports_owner_diagnostic_without_panicking() {
    helpers::ensure_libclang();

    let scratch = scratch("excluded-dependency");
    let public = scratch.join("public.h");
    std::fs::write(
        &public,
        "typedef struct HIDDEN { int value; } HIDDEN;\n\
         typedef struct PUBLIC_TYPE { HIDDEN hidden; } PUBLIC_TYPE;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public").with_exclusion("HIDDEN"),
    );
    let references = BTreeMap::new();
    let error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();

    assert!(
        error.to_string().contains(
            "owner-excluded local type `HIDDEN` in partition `public` namespace \
             `Example.Public` is required without a retained public alias"
        ),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn collision_scoped_excluded_type_resolves_by_public_reference_name() {
    helpers::ensure_libclang();

    let (scratch, snapshot, policy) = colliding_ntstatus_snapshot("colliding-ntstatus-reference");
    let references = BTreeMap::from([(
        "NTSTATUS".to_string(),
        TypeReference::new(
            "Windows.Win32.Foundation",
            "NTSTATUS",
            TypeReferenceKind::Type,
        ),
    )]);
    let mut options = EmitOptions::new("Windows.Win32", &references);
    options.library = Some("ntdll.dll");
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let kernel = output(&partitions, "Example.Kernel");
    let first = output(&partitions, "Example.First");
    let second = output(&partitions, "Example.Second");

    assert!(
        kernel.contains("fn KernelCall() -> Windows::Win32::Foundation::NTSTATUS"),
        "{kernel}"
    );
    assert!(!kernel.contains("type NTSTATUS ="), "{kernel}");
    assert!(first.contains("type NTSTATUS = u16"), "{first}");
    assert!(second.contains("type NTSTATUS = u32"), "{second}");
    assert!(
        !partitions.values().any(|rdl| rdl.contains("__partition_")),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn collision_scoped_excluded_type_without_reference_reports_public_name() {
    helpers::ensure_libclang();

    let (scratch, snapshot, policy) =
        colliding_ntstatus_snapshot("colliding-ntstatus-missing-reference");
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Windows.Win32", &references);
    options.library = Some("ntdll.dll");
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let audit = plan.audit(&options).unwrap_err().to_string();
    let emission = plan.emit_with_options(&options).unwrap_err().to_string();

    assert_eq!(audit, emission);
    assert!(
        audit.contains(
            "owner-excluded local type `NTSTATUS` in partition `kernel` namespace \
             `Example.Kernel` is required without a retained public alias"
        ),
        "{audit}"
    );
    assert!(audit.contains("function `KernelCall`"), "{audit}");
    assert!(!audit.contains("__partition_"), "{audit}");
    assert!(audit.ends_with("no RDL was emitted"), "{audit}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn equivalent_alias_and_primitive_typedefs_preserve_excluded_dependency_route() {
    helpers::ensure_libclang();

    let scratch = scratch("equivalent-alias-partitions");
    let programming = scratch.join("programming.h");
    let display = scratch.join("display.h");
    let kernel = scratch.join("kernel.h");
    let constants = scratch.join("constants.h");
    std::fs::write(
        &programming,
        "typedef long LONG;\n\
         typedef LONG NTSTATUS;\n",
    )
    .unwrap();
    std::fs::write(&display, "typedef long NTSTATUS;\n").unwrap();
    std::fs::write(&kernel, "typedef LONG NTSTATUS;\n").unwrap();
    std::fs::write(&constants, "#define STATUS_SUCCESS ((NTSTATUS)0)\n").unwrap();

    let policy = |include_display| {
        let mut policy = HeaderPartitionPolicy::new().with_traversed_header(
            programming.to_string_lossy(),
            RootPartition::new("programming", "Example.WindowsProgramming"),
        );
        if include_display {
            policy = policy.with_traversed_header(
                display.to_string_lossy(),
                RootPartition::new("display", "Example.Display"),
            );
        }
        policy
            .with_traversed_header(
                kernel.to_string_lossy(),
                RootPartition::new("kernel", "Example.Kernel").with_exclusion("NTSTATUS"),
            )
            .with_traversed_header(
                constants.to_string_lossy(),
                RootPartition::new("constants", "Example.Foundation"),
            )
    };
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let baseline = aggregate_snapshot(&scratch, &[&programming, &kernel, &constants])
        .plan_header_partitions(&policy(false), &NamespaceAuthorities::new())
        .unwrap();
    assert!(baseline.audit(&options).unwrap().is_clean());
    let baseline = baseline.emit_with_options(&options).unwrap();

    let expanded = aggregate_snapshot(&scratch, &[&programming, &display, &kernel, &constants])
        .plan_header_partitions(&policy(true), &NamespaceAuthorities::new())
        .unwrap();
    assert!(expanded.audit(&options).unwrap().is_clean());
    let expanded = expanded.emit_with_options(&options).unwrap();
    let baseline_programming = output(&baseline, "Example.WindowsProgramming");
    let baseline_constants = output(&baseline, "Example.Foundation");
    let expanded_programming = output(&expanded, "Example.WindowsProgramming");
    let expanded_display = output(&expanded, "Example.Display");
    let expanded_constants = output(&expanded, "Example.Foundation");

    assert!(
        baseline_programming.contains("type NTSTATUS = i32"),
        "{baseline_programming}"
    );
    assert_eq!(baseline_programming, expanded_programming);
    assert!(
        expanded_display.contains("type NTSTATUS = i32"),
        "{expanded_display}"
    );
    assert!(
        baseline_constants
            .contains("const STATUS_SUCCESS: Example::WindowsProgramming::NTSTATUS = 0"),
        "{baseline_constants}"
    );
    assert_eq!(baseline_constants, expanded_constants);
    assert!(
        !expanded.values().any(|rdl| rdl.contains("__partition_")),
        "{expanded:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn excluded_dependency_owner_diagnostics_batch_all_blockers_and_referrers() {
    helpers::ensure_libclang();

    let scratch = scratch("excluded-dependency-batch");
    let public = scratch.join("public.h");
    std::fs::write(
        &public,
        "typedef struct ALPHA { int value; } ALPHA;\n\
         typedef struct BETA { int value; } BETA;\n\
         typedef struct SHARED_ROOT { ALPHA alpha; BETA beta; } SHARED_ROOT;\n\
         typedef struct ALPHA_ROOT { ALPHA alpha; } ALPHA_ROOT;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public")
            .with_exclusion("ALPHA")
            .with_exclusion("BETA"),
    );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let first = plan.audit(&options).unwrap_err().to_string();
    let second = plan.audit(&options).unwrap_err().to_string();
    let emission = plan.emit_with_options(&options).unwrap_err().to_string();

    assert_eq!(first, second);
    assert_eq!(first, emission);
    assert!(
        first.starts_with("header partition owner validation found 2 blocker(s)"),
        "{first}"
    );
    assert_eq!(first.matches("owner-excluded local type").count(), 2);
    assert!(first.find("`ALPHA`").unwrap() < first.find("`BETA`").unwrap());
    let alpha = &first[first.find("`ALPHA`").unwrap()..first.find("`BETA`").unwrap()];
    assert!(alpha.contains("type `ALPHA_ROOT`"), "{alpha}");
    assert!(alpha.contains("type `SHARED_ROOT`"), "{alpha}");
    let beta = &first[first.find("`BETA`").unwrap()..];
    assert!(beta.contains("type `SHARED_ROOT`"), "{beta}");
    assert!(!beta.contains("type `ALPHA_ROOT`"), "{beta}");
    assert!(first.ends_with("no RDL was emitted"), "{first}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn excluded_empty_forward_dependency_reports_owner_diagnostic() {
    helpers::ensure_libclang();

    let scratch = scratch("empty-forward-dependency");
    let public = scratch.join("public.h");
    std::fs::write(
        &public,
        "struct EMPTY_RECORD;\n\
         typedef struct PUBLIC_TYPE { EMPTY_RECORD *empty; } PUBLIC_TYPE;\n\
         struct EMPTY_RECORD {};\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public").exclude_empty_records(),
    );
    let references = BTreeMap::new();
    let error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();

    assert!(
        error.to_string().contains(
            "owner-excluded local type `EMPTY_RECORD` in partition `public` namespace \
             `Example.Public` is required without a retained public alias"
        ),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn legacy_partitioned_dependencies_remain_unowned() {
    helpers::ensure_libclang();

    let scratch = scratch("legacy-unowned-dependency");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "typedef struct DEPENDENCY { int value; } DEPENDENCY;\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             typedef struct PUBLIC_TYPE {{ DEPENDENCY dependency; }} PUBLIC_TYPE;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = extract_partitioned(
        [Input::new(
            "legacy.cpp",
            format!("#include \"{}\"\n", public.to_string_lossy()),
        )
        .partitioned("legacy-input")
        .with_root(public.to_string_lossy(), "public", "Example.Public")],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let error = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("selected type `DEPENDENCY` has no tagged root owner"),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn shared_required_dependency_emits_once_in_default_namespace() {
    helpers::ensure_libclang();

    let scratch = scratch("effective-dependency-owner");
    let dependency = scratch.join("dependency.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         typedef struct DEPENDENCY { int value; } DEPENDENCY;\n",
    )
    .unwrap();
    std::fs::write(
        &first,
        format!(
            "#include \"{}\"\n\
             typedef struct FIRST_ROOT {{ DEPENDENCY value; }} FIRST_ROOT;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &second,
        format!(
            "#include \"{}\"\n\
             typedef struct SECOND_ROOT {{ DEPENDENCY value; }} SECOND_ROOT;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first, &second]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("shared", "Example.Shared").with_library("FirstApi", "first.dll"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("shared", "Example.Shared").with_library("SecondApi", "second.dll"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let combined = partitions.values().cloned().collect::<Vec<_>>().join("\n");
    assert_eq!(
        combined.matches("struct DEPENDENCY").count(),
        1,
        "{combined}"
    );
    assert!(combined.contains("struct FIRST_ROOT"), "{combined}");
    assert!(combined.contains("struct SECOND_ROOT"), "{combined}");
    assert!(
        output(&partitions, "Example.Common").contains("struct DEPENDENCY"),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn equivalent_same_namespace_roots_compare_effective_item_policy() {
    helpers::ensure_libclang();

    let scratch = scratch("effective-root-policy");
    let (first, second, snapshot) =
        duplicate_declaration_snapshot(&scratch, "typedef unsigned SHARED_VALUE;\n");
    let policy = duplicate_declaration_policy(
        &first,
        RootPartition::new("first", "Example.Shared")
            .with_library("FirstApi", "first.dll")
            .with_exclusion("FIRST_UNUSED")
            .with_remap("FIRST_OTHER", "FIRST_REMAPPED")
            .with_flags("FIRST_FLAGS")
            .exclude_empty_records(),
        &second,
        RootPartition::new("second", "Example.Shared")
            .with_library("SecondApi", "second.dll")
            .with_exclusion("SECOND_UNUSED")
            .with_u32_type("SECOND_U32"),
    );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let route = partitions
        .keys()
        .find(|partition| partition.namespace == "Example.Shared")
        .unwrap();
    assert_eq!(route.partition, "first");
    assert_eq!(route.header, first.to_string_lossy().replace('\\', "/"));
    assert_eq!(
        partitions
            .values()
            .map(|rdl| rdl.matches("type SHARED_VALUE = u32").count())
            .sum::<usize>(),
        1,
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn exact_authority_collapses_equivalent_logical_routes() {
    helpers::ensure_libclang();

    let scratch = scratch("exact-equivalent-routes");
    let (first, second, snapshot) =
        duplicate_declaration_snapshot(&scratch, "typedef unsigned EXACT_SHARED;\n");
    let policy = duplicate_declaration_policy(
        &first,
        RootPartition::new("first", "Example.First"),
        &second,
        RootPartition::new("second", "Example.Second"),
    );
    let authorities = NamespaceAuthorities::new().with_exact("EXACT_SHARED", "Example.Authority");
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let route = partitions
        .keys()
        .find(|partition| partition.namespace == "Example.Authority")
        .unwrap();
    assert_eq!(route.partition, "first");
    assert_eq!(route.header, first.to_string_lossy().replace('\\', "/"));
    assert!(output(&partitions, "Example.Authority").contains("type EXACT_SHARED = u32"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn wildcard_authority_collapses_equivalent_logical_routes() {
    helpers::ensure_libclang();

    let scratch = scratch("wildcard-equivalent-routes");
    let (first, second, snapshot) =
        duplicate_declaration_snapshot(&scratch, "typedef unsigned WILDCARD_SHARED;\n");
    let first_partition = RootPartition::new("first", "Example.First");
    let second_partition = RootPartition::new("second", "Example.Second");
    let policy = duplicate_declaration_policy(
        &first,
        first_partition.clone(),
        &second,
        second_partition.clone(),
    );
    let reverse = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input("second.cpp", second.to_string_lossy(), second_partition)
        .with_traversed_header_for_input("first.cpp", first.to_string_lossy(), first_partition);
    let authorities = NamespaceAuthorities::new().with_wildcard("WILDCARD_*", "Example.Authority");
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let reverse_partitions = snapshot
        .plan_header_partitions(&reverse, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    assert_eq!(partitions, reverse_partitions);
    let route = partitions
        .keys()
        .find(|partition| partition.namespace == "Example.Authority")
        .unwrap();
    assert_eq!(route.partition, "first");
    assert_eq!(route.header, first.to_string_lossy().replace('\\', "/"));
    assert!(output(&partitions, "Example.Authority").contains("type WILDCARD_SHARED = u32"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn relevant_function_library_difference_remains_a_route_conflict() {
    helpers::ensure_libclang();

    let scratch = scratch("effective-library-conflict");
    let (first, second, snapshot) =
        duplicate_declaration_snapshot(&scratch, "extern \"C\" int SharedFunction(void);\n");
    let policy = duplicate_declaration_policy(
        &first,
        RootPartition::new("first", "Example.Shared").with_library("SharedFunction", "first.dll"),
        &second,
        RootPartition::new("second", "Example.Shared").with_library("SharedFunction", "second.dll"),
    );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let audit = plan.audit(&options).unwrap();

    assert_eq!(audit.conflicts().len(), 1, "{audit}");
    assert_eq!(audit.conflicts()[0].name, "SharedFunction");
    assert_eq!(
        audit.conflicts()[0].reason,
        PartitionConflictReason::AmbiguousOwners
    );
    let error = plan.emit_with_options(&options).unwrap_err();
    assert!(error.to_string().contains("SharedFunction"), "{error}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn equivalent_function_libraries_use_the_effective_value() {
    helpers::ensure_libclang();

    let scratch = scratch("effective-library-equivalence");
    let (first, second, snapshot) =
        duplicate_declaration_snapshot(&scratch, "extern \"C\" int SharedFunction(void);\n");
    let policy = duplicate_declaration_policy(
        &first,
        RootPartition::new("first", "Example.Shared").with_library("SharedFunction", "shared.dll"),
        &second,
        RootPartition::new("second", "Example.Shared")
            .with_library("UnrelatedFunction", "other.dll"),
    );
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("shared.dll");
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let route = partitions
        .keys()
        .find(|partition| partition.namespace == "Example.Shared")
        .unwrap();
    assert_eq!(route.partition, "first");
    let rdl = output(&partitions, "Example.Shared");
    assert!(rdl.contains("#[library(\"shared.dll\")]"), "{rdl}");
    assert!(rdl.contains("fn SharedFunction() -> i32"), "{rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn equivalent_declarations_in_different_namespaces_emit_independently() {
    helpers::ensure_libclang();

    let scratch = scratch("different-namespace-routes");
    let (first, second, snapshot) =
        duplicate_declaration_snapshot(&scratch, "typedef void* DISTINCT_HANDLE;\n");
    let policy = duplicate_declaration_policy(
        &first,
        RootPartition::new("first", "Example.First"),
        &second,
        RootPartition::new("second", "Example.Second"),
    );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let audit = plan.audit(&options).unwrap();

    assert!(audit.is_clean(), "{audit}");
    let partitions = plan.emit_with_options(&options).unwrap();
    for namespace in ["Example.First", "Example.Second"] {
        let rdl = output(&partitions, namespace);
        assert!(rdl.contains("type DISTINCT_HANDLE = *mut void"), "{rdl}");
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn equivalent_declarations_keep_namespace_specific_routes() {
    helpers::ensure_libclang();

    let scratch = scratch("equivalent-declaration-routes");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    let declarations = |function: &str| {
        format!(
            "#ifndef SHARED_GUID_DEFINITIONS\n\
             #define SHARED_GUID_DEFINITIONS\n\
             #define DEFINE_GUID(name, l, w1, w2, b1, b2, b3, b4, b5, b6, b7, b8)\n\
             #endif\n\
             typedef void* SHARED_HANDLE;\n\
             typedef unsigned short SHARED_PORT;\n\
             extern \"C\" SHARED_HANDLE {function}(SHARED_HANDLE previous, SHARED_PORT port);\n\
             DEFINE_GUID(GUID_SHARED, 0x12345678, 0x1234, 0x5678, 0x90, 0xab, 0xcd, \
                 0xef, 0x12, 0x34, 0x56, 0x78)\n"
        )
    };
    std::fs::write(&first, declarations("FirstOpen")).unwrap();
    std::fs::write(&second, declarations("SecondOpen")).unwrap();

    let forward = aggregate_snapshot(&scratch, &[&first, &second]);
    let reverse = aggregate_snapshot(&scratch, &[&second, &first]);
    let first_partition =
        RootPartition::new("first", "Example.First").with_library("FirstOpen", "first.dll");
    let second_partition =
        RootPartition::new("second", "Example.Second").with_library("SecondOpen", "second.dll");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(first.to_string_lossy(), first_partition.clone())
        .with_traversed_header(second.to_string_lossy(), second_partition.clone());
    let reverse_policy = HeaderPartitionPolicy::new()
        .with_traversed_header(second.to_string_lossy(), second_partition)
        .with_traversed_header(first.to_string_lossy(), first_partition);
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);

    let plan = forward
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let reverse_plan = reverse
        .plan_header_partitions(&reverse_policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(reverse_plan.audit(&options).unwrap().is_clean());
    let reverse_partitions = reverse_plan.emit_with_options(&options).unwrap();

    assert_eq!(partitions, reverse_partitions);
    let first_rdl = output(&partitions, "Example.First");
    assert!(first_rdl.contains("fn FirstOpen"), "{first_rdl}");
    assert!(first_rdl.contains("type SHARED_HANDLE"), "{first_rdl}");
    assert!(first_rdl.contains("type SHARED_PORT"), "{first_rdl}");
    assert!(first_rdl.contains("const GUID_SHARED"), "{first_rdl}");
    assert!(!first_rdl.contains("SecondOpen"), "{first_rdl}");
    let second_rdl = output(&partitions, "Example.Second");
    assert!(second_rdl.contains("fn SecondOpen"), "{second_rdl}");
    assert!(second_rdl.contains("type SHARED_HANDLE"), "{second_rdl}");
    assert!(second_rdl.contains("type SHARED_PORT"), "{second_rdl}");
    assert!(second_rdl.contains("const GUID_SHARED"), "{second_rdl}");
    assert!(!second_rdl.contains("FirstOpen"), "{second_rdl}");
    assert!(
        partitions.values().all(|rdl| !rdl.contains("__partition_")),
        "{partitions:#?}"
    );

    let winmd = scratch.join("equivalent-declaration-routes.winmd");
    let mut compiler = windows_rdl::reader();
    for rdl in partitions.values() {
        compiler.input_text(rdl);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    for (namespace, function) in [
        ("Example.First", "FirstOpen"),
        ("Example.Second", "SecondOpen"),
    ] {
        assert_eq!(
            index.expect(namespace, "SHARED_HANDLE").underlying_type(),
            Some(Type::PtrMut(Box::new(Type::Void), 1))
        );
        assert_eq!(
            index.expect(namespace, "SHARED_PORT").underlying_type(),
            Some(Type::U16)
        );
        let Item::Fn(function) = index.expect_item(namespace, function) else {
            panic!("{namespace}.{function} was not emitted as a function");
        };
        let signature = function.signature(&[]);
        assert_eq!(
            signature.return_type,
            Type::value_named(namespace, "SHARED_HANDLE")
        );
        assert_eq!(
            signature.types,
            [
                Type::value_named(namespace, "SHARED_HANDLE"),
                Type::value_named(namespace, "SHARED_PORT"),
            ]
        );
        let Item::Const(guid) = index.expect_item(namespace, "GUID_SHARED") else {
            panic!("{namespace}.GUID_SHARED was not emitted as a constant");
        };
        assert_eq!(
            guid.find_attribute("GuidAttribute").unwrap().value(),
            [
                Value::U32(0x12345678),
                Value::U16(0x1234),
                Value::U16(0x5678),
                Value::U8(0x90),
                Value::U8(0xab),
                Value::U8(0xcd),
                Value::U8(0xef),
                Value::U8(0x12),
                Value::U8(0x34),
                Value::U8(0x56),
                Value::U8(0x78),
            ]
            .into_iter()
            .map(|value| (String::new(), value))
            .collect::<Vec<_>>()
        );
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn equivalent_declarations_coalesce_per_destination_deterministically() {
    helpers::ensure_libclang();

    let scratch = scratch("equivalent-declarations-per-destination");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    let other = scratch.join("other.h");
    for header in [&first, &second, &other] {
        std::fs::write(header, "typedef long SHARED_TIME;\n").unwrap();
    }
    let forward = aggregate_snapshot(&scratch, &[&first, &second, &other]);
    let reverse = aggregate_snapshot(&scratch, &[&other, &second, &first]);
    let first_partition = RootPartition::new("first", "Example.Time");
    let second_partition = RootPartition::new("second", "Example.Time");
    let other_partition = RootPartition::new("other", "Example.Other");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(first.to_string_lossy(), first_partition.clone())
        .with_traversed_header(second.to_string_lossy(), second_partition.clone())
        .with_traversed_header(other.to_string_lossy(), other_partition.clone());
    let reverse_policy = HeaderPartitionPolicy::new()
        .with_traversed_header(other.to_string_lossy(), other_partition)
        .with_traversed_header(second.to_string_lossy(), second_partition)
        .with_traversed_header(first.to_string_lossy(), first_partition);
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let emit = |snapshot: Snapshot, policy: &HeaderPartitionPolicy| {
        let plan = snapshot
            .plan_header_partitions(policy, &NamespaceAuthorities::new())
            .unwrap();
        assert!(plan.audit(&options).unwrap().is_clean());
        plan.emit_with_options(&options).unwrap()
    };

    let partitions = emit(forward, &policy);
    let reverse_partitions = emit(reverse, &reverse_policy);
    assert_eq!(partitions, reverse_partitions);
    assert_eq!(partitions.len(), 2);
    assert!(
        output(&partitions, "Example.Time").contains("type SHARED_TIME = i32"),
        "{partitions:#?}"
    );
    assert!(
        output(&partitions, "Example.Other").contains("type SHARED_TIME = i32"),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn one_declaration_with_multiple_namespaces_remains_ambiguous() {
    helpers::ensure_libclang();

    let scratch = scratch("one-declaration-multiple-namespaces");
    let shared = scratch.join("shared.h");
    std::fs::write(
        &shared,
        "typedef void* SHARED_HANDLE;\n\
         extern \"C\" SHARED_HANDLE OpenShared(void);\n",
    )
    .unwrap();
    let snapshot = aggregate_snapshot(&scratch, &[&shared]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            shared.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            shared.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let audit = plan.audit(&options).unwrap();

    assert_eq!(audit.conflicts().len(), 2, "{audit}");
    assert!(audit.conflicts().iter().all(|conflict| {
        conflict.reason == PartitionConflictReason::AmbiguousRootCandidates
            && conflict.owners.len() == 2
    }));
    assert!(plan.emit_with_options(&options).is_err());

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn route_audit_uses_source_names_after_owner_remaps() {
    helpers::ensure_libclang();

    let scratch = scratch("route-source-names");
    let shared = scratch.join("shared.h");
    std::fs::write(&shared, "typedef unsigned SOURCE_NAME;\n").unwrap();
    let include = format!("#include \"{}\"\n", shared.to_string_lossy());
    let snapshot = extract(
        [
            Input::new("first.cpp", &include)
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
            Input::new("second.cpp", &include)
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "first.cpp",
            "shared.h",
            RootPartition::new("first", "Example.First").with_remap("SOURCE_NAME", "REMAPPED"),
        )
        .with_traversed_header_for_input(
            "second.cpp",
            "shared.h",
            RootPartition::new("second", "Example.Second").with_remap("SOURCE_NAME", "REMAPPED"),
        );
    let references = BTreeMap::new();
    let audit = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert_eq!(audit.conflicts().len(), 1, "{audit}");
    assert_eq!(audit.conflicts()[0].name, "SOURCE_NAME");
    assert!(!audit.to_string().contains("REMAPPED"), "{audit}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn authority_routes_use_policy_partition_and_physical_header() {
    helpers::ensure_libclang();

    let scratch = scratch("authority-partition");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "typedef struct DEPENDENCY { int value; } DEPENDENCY;\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             typedef struct PUBLIC_TYPE {{ DEPENDENCY dependency; }} PUBLIC_TYPE;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let partition = RootPartition::new("public", "Example.Public");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(public.to_string_lossy(), partition.clone())
        .with_authority_partition("authority");
    let default_policy =
        HeaderPartitionPolicy::new().with_traversed_header(public.to_string_lossy(), partition);
    let authorities = NamespaceAuthorities::new().with_exact("DEPENDENCY", "Example.Dependencies");
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);

    let partitions = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let authority = partitions
        .keys()
        .find(|partition| partition.namespace == "Example.Dependencies")
        .unwrap();
    assert_eq!(authority.partition, "authority");
    assert_eq!(
        authority.header,
        dependency.to_string_lossy().replace('\\', "/")
    );
    assert_ne!(authority.partition, "aggregate.cpp");

    let default_partitions = snapshot
        .plan_header_partitions(&default_policy, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let default_authority = default_partitions
        .keys()
        .find(|partition| partition.namespace == "Example.Dependencies")
        .unwrap();
    assert_eq!(default_authority.partition, "Example.Dependencies");
    assert_eq!(default_authority.header, authority.header);

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn relative_case_variant_headers_use_resolved_physical_paths() {
    helpers::ensure_libclang();

    let scratch = scratch("physical-paths");
    let include = scratch.join("Include");
    std::fs::create_dir_all(&include).unwrap();
    let public = include.join("PublicHeader.h");
    std::fs::write(
        &public,
        "typedef struct ORIGINAL { int value; } ORIGINAL;\n\
         typedef struct HOLDER { ORIGINAL value; } HOLDER;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        "include/publicheader.h",
        RootPartition::new("public", "Example.Public").with_remap("ORIGINAL", "RENAMED"),
    );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let (partition, public_rdl) = partitions.first_key_value().unwrap();

    assert_eq!(
        partition.header,
        public.to_string_lossy().replace('\\', "/")
    );
    assert!(public_rdl.contains("struct RENAMED"), "{public_rdl}");
    assert!(public_rdl.contains("value: RENAMED"), "{public_rdl}");
    assert!(!public_rdl.contains("ORIGINAL"), "{public_rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn associated_constants_follow_annotated_declaration_owner() {
    helpers::ensure_libclang();

    let scratch = scratch("associated-constant");
    let provider = scratch.join("provider.h");
    let group = scratch.join("group.h");
    std::fs::write(
        &provider,
        "#define ERROR_TARGET 42UL\n\
         #define ERROR_ALIAS ERROR_TARGET\n\
         #define ERROR_NOISE 19\n",
    )
    .unwrap();
    std::fs::write(
        &group,
        format!(
            "#include \"{}\"\n\
             enum __attribute__((annotate(\
                 \"win32metadata:associated_constant=ERROR_ALIAS\"))) \
                 ERROR_KIND : unsigned long {{ ERROR_LOCAL = 1 }};\n",
            provider.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&group]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            group.to_string_lossy(),
            RootPartition::new("group", "Example.Group"),
        )
        .with_traversed_header(
            provider.to_string_lossy(),
            RootPartition::new("provider", "Example.Provider"),
        );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let group_rdl = output(&partitions, "Example.Group");

    assert!(group_rdl.contains("enum ERROR_KIND"), "{group_rdl}");
    assert!(
        group_rdl.contains("const ERROR_ALIAS: u32 = 42"),
        "{group_rdl}"
    );
    assert!(!group_rdl.contains("ERROR_TARGET"), "{group_rdl}");
    assert!(!group_rdl.contains("ERROR_NOISE"), "{group_rdl}");
    assert!(
        !output(&partitions, "Example.Provider").contains("ERROR_ALIAS"),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn input_qualified_headers_preserve_psapi_compile_variants() {
    helpers::ensure_libclang();

    let scratch = scratch("input-qualified-psapi");
    let common = scratch.join("common.h");
    let header = scratch.join("PsApi.h");
    std::fs::write(&common, "typedef void *HANDLE;\ntypedef int BOOL;\n").unwrap();
    std::fs::write(
        &header,
        format!(
            "#include \"{}\"\n\
             extern \"C\" BOOL EmptyWorkingSet(HANDLE process);\n\
             extern \"C\" BOOL EnumProcesses(unsigned *processes, unsigned bytes, \
             unsigned *needed);\n",
            common.to_string_lossy()
        ),
    )
    .unwrap();
    let include = format!("#include \"{}\"\n", header.to_string_lossy());
    let snapshot = extract(
        [
            Input::new("psapi1.cpp", &include)
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
            Input::new(
                "psapi2.cpp",
                format!(
                    "#define EmptyWorkingSet K32EmptyWorkingSet\n\
                     #define EnumProcesses K32EnumProcesses\n\
                     {include}"
                ),
            )
            .with_root_dirs([scratch.to_string_lossy().to_string()]),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let v1 = RootPartition::new("psapi1", "Example.System.ProcessStatus")
        .with_library("EmptyWorkingSet", "PSAPI.dll")
        .with_library("EnumProcesses", "PSAPI.dll");
    let v2 = RootPartition::new("psapi2", "Example.System.ProcessStatus")
        .with_library("K32EmptyWorkingSet", "KERNEL32.dll")
        .with_library("K32EnumProcesses", "KERNEL32.dll");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input("PSAPI1.CPP", "psapi.h", v1)
        .with_traversed_header_for_input("psapi2.cpp", "PSAPI.H", v2);
    let references = BTreeMap::from([
        (
            "BOOL".to_string(),
            TypeReference::new("Example.Foundation", "BOOL", TypeReferenceKind::Type),
        ),
        (
            "HANDLE".to_string(),
            TypeReference::new("Example.Foundation", "HANDLE", TypeReferenceKind::Type),
        ),
    ]);
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let process_status = partitions.values().cloned().collect::<Vec<_>>().join("\n");

    assert_eq!(partitions.len(), 2, "{partitions:#?}");
    for name in [
        "EmptyWorkingSet",
        "K32EmptyWorkingSet",
        "EnumProcesses",
        "K32EnumProcesses",
    ] {
        assert_eq!(
            process_status.matches(&format!("fn {name}(")).count(),
            1,
            "{process_status}"
        );
    }
    assert!(
        process_status.contains("#[library(\"PSAPI.dll\")]"),
        "{process_status}"
    );
    assert!(
        process_status.contains("#[library(\"KERNEL32.dll\")]"),
        "{process_status}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn input_qualified_exclusions_filter_equivalent_alias_owners() {
    helpers::ensure_libclang();

    let scratch = scratch("input-qualified-exclusions");
    let header = scratch.join("PsApi.h");
    std::fs::write(
        &header,
        "#ifndef PSAPI_VERSION\n\
         #define PSAPI_VERSION 2\n\
         #endif\n\
         typedef struct _MODULEINFO { unsigned value; } MODULEINFO, *LPMODULEINFO;\n\
         typedef int (__stdcall *PENUM_PAGE_FILE_CALLBACK)(LPMODULEINFO value);\n\
         #if PSAPI_VERSION > 1\n\
         #define GetModuleInformation K32GetModuleInformation\n\
         #endif\n\
         extern \"C\" int GetModuleInformation(\
             LPMODULEINFO value, PENUM_PAGE_FILE_CALLBACK callback);\n",
    )
    .unwrap();
    let include = format!("#include \"{}\"\n", header.to_string_lossy());
    let snapshot = extract(
        [
            Input::new(
                "win32metadata-psapi-v1.cpp",
                format!("#define PSAPI_VERSION 1\n{include}"),
            )
            .with_root_dirs([scratch.to_string_lossy().to_string()]),
            Input::new(
                "win32metadata-psapi-v2.cpp",
                format!("#define PSAPI_VERSION 2\n{include}"),
            )
            .with_root_dirs([scratch.to_string_lossy().to_string()]),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "win32metadata-psapi-v1.cpp",
            "psapi.h",
            RootPartition::new("PsApi1", "Example.System.ProcessStatus")
                .with_library("GetModuleInformation", "PSAPI.dll"),
        )
        .with_traversed_header_for_input(
            "win32metadata-psapi-v2.cpp",
            "psapi.h",
            RootPartition::new("PsApi2", "Example.System.ProcessStatus")
                .with_exclusion("_MODULEINFO")
                .with_exclusion("PENUM_PAGE_FILE_CALLBACK")
                .with_library("K32GetModuleInformation", "KERNEL32.dll"),
        );
    let references = BTreeMap::new();
    let selected = BTreeSet::from([
        "GetModuleInformation".to_string(),
        "K32GetModuleInformation".to_string(),
    ]);
    let mut options = EmitOptions::new("Example.Common", &references);
    options.functions = Some(&selected);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let v1 = partitions
        .iter()
        .find(|(partition, _)| partition.partition == "PsApi1")
        .unwrap()
        .1;
    let v2 = partitions
        .iter()
        .find(|(partition, _)| partition.partition == "PsApi2")
        .unwrap()
        .1;

    assert!(v1.contains("struct MODULEINFO"), "{v1}");
    assert!(v1.contains("type LPMODULEINFO = *mut MODULEINFO"), "{v1}");
    assert!(v1.contains("extern fn PENUM_PAGE_FILE_CALLBACK"), "{v1}");
    assert!(!v2.contains("struct MODULEINFO"), "{v2}");
    assert!(!v2.contains("type LPMODULEINFO"), "{v2}");
    assert!(!v2.contains("extern fn PENUM_PAGE_FILE_CALLBACK"), "{v2}");
    assert!(v1.contains("fn GetModuleInformation("), "{v1}");
    assert!(v2.contains("fn K32GetModuleInformation("), "{v2}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn selected_namespaced_native_functions_emit_from_traversed_headers() {
    helpers::ensure_libclang();

    let scratch = scratch("selected-namespaced-functions");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    let external_winmd = scratch.join("external.winmd");
    let output_winmd = scratch.join("output.winmd");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         namespace Support {\n\
             struct ArcData { unsigned helper; };\n\
             class HiddenSession {};\n\
         }\n\
         namespace ABI { namespace External { namespace Api {\n\
             struct EXTERNAL_RECORD { int duplicate; };\n\
         } } }\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#pragma once\n\
             #include \"{}\"\n\
             #define W32M(text) __attribute__((annotate(text)))\n\
             namespace Native {{\n\
                 enum Status {{ Ok = 0, Failed = 1 }};\n\
                 typedef Status NativeStatus;\n\
                 enum FillMode {{ Alternate = 0, Winding = 1 }};\n\
                 typedef FillMode NativeFillMode;\n\
                 struct ArcData {{ float x; float y; }};\n\
                 class Path {{}};\n\
                 namespace Exports {{\n\
                     extern \"C\" NativeStatus __stdcall AddPathArc(\n\
                         Path* path,\n\
                         ArcData* arc,\n\
                         ABI::External::Api::EXTERNAL_RECORD* external);\n\
                     W32M(\"win32metadata:import_library=annotated.dll\")\n\
                     extern \"C\" NativeStatus __stdcall CreatePath(\n\
                         NativeFillMode mode,\n\
                         Path** path);\n\
                     extern \"C\" NativeStatus __stdcall Unselected(ArcData* arc);\n\
                 }}\n\
             }}\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();
    windows_rdl::reader()
        .input_text(
            "#[win32]\n\
             mod External {\n\
                 mod Api {\n\
                     struct EXTERNAL_RECORD {\n\
                         value: i32,\n\
                     }\n\
                 }\n\
             }\n",
        )
        .output(&external_winmd)
        .write()
        .unwrap();
    let references = windows_clang::MetadataReferences::new([windows_metadata::reader::File::new(
        std::fs::read(&external_winmd).unwrap(),
    )
    .unwrap()]);
    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!("#include \"{}\"\n", public.to_string_lossy()),
        )
        .with_roots([public.to_string_lossy().to_string()])],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=i686-pc-windows-msvc",
        ],
    )
    .unwrap();
    assert_eq!(
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.name == "ArcData")
            .count(),
        2
    );

    let functions = BTreeSet::from(["AddPathArc".to_string(), "CreatePath".to_string()]);
    let mut options = EmitOptions::new("Example.Common", references.types());
    options.functions = Some(&functions);
    let legacy = snapshot
        .emit_with_options(&options)
        .unwrap_err()
        .to_string();
    assert!(
        legacy.contains("selected function `AddPathArc` was not found"),
        "{legacy}"
    );

    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("native", "Example.Native").with_library("AddPathArc", "mapped.dll"),
    );
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let native = output(&partitions, "Example.Native");

    assert!(native.contains("enum Status"), "{native}");
    assert!(native.contains("type NativeStatus = Status"), "{native}");
    assert!(native.contains("enum FillMode"), "{native}");
    assert!(
        native.contains("type NativeFillMode = FillMode"),
        "{native}"
    );
    assert!(native.contains("struct ArcData"), "{native}");
    assert!(native.contains("x: f32"), "{native}");
    assert!(native.contains("y: f32"), "{native}");
    assert!(!native.contains("helper"), "{native}");
    assert!(!native.contains("HiddenSession"), "{native}");
    assert!(!native.contains("EXTERNAL_RECORD {"), "{native}");
    assert!(!native.contains("Unselected"), "{native}");
    assert!(native.contains("#[library(\"mapped.dll\")]"), "{native}");
    assert!(native.contains("#[library(\"annotated.dll\")]"), "{native}");
    assert!(
        native.contains(
            "extern fn AddPathArc(path: *mut void, arc: *mut ArcData, external: *mut \
             External::Api::EXTERNAL_RECORD) -> NativeStatus"
        ),
        "{native}"
    );
    assert!(
        native.contains(
            "extern fn CreatePath(mode: NativeFillMode, path: *mut *mut void) -> NativeStatus"
        ),
        "{native}"
    );

    windows_rdl::reader()
        .input_texts(partitions.values())
        .reference(&external_winmd)
        .reference_default()
        .output(&output_winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&output_winmd).unwrap();
    assert_eq!(
        index
            .expect("Example.Native", "NativeStatus")
            .underlying_type(),
        Some(Type::value_named("Example.Native", "Status"))
    );
    assert_eq!(
        index
            .expect("Example.Native", "NativeFillMode")
            .underlying_type(),
        Some(Type::value_named("Example.Native", "FillMode"))
    );
    let arc = index.expect("Example.Native", "ArcData");
    assert_eq!(
        arc.fields()
            .map(|field| (field.name().to_string(), field.ty()))
            .collect::<Vec<_>>(),
        [("x".to_string(), Type::F32), ("y".to_string(), Type::F32)]
    );
    let Item::Fn(add_path_arc) = index.expect_item("Example.Native", "AddPathArc") else {
        panic!("Example.Native.AddPathArc was not emitted as a function");
    };
    assert_eq!(add_path_arc.calling_convention(), "system");
    assert_eq!(
        add_path_arc.impl_map().unwrap().import_scope().name(),
        "mapped.dll"
    );
    assert_eq!(
        add_path_arc.signature(&[]).types,
        [
            Type::PtrMut(Box::new(Type::Void), 1),
            Type::PtrMut(Box::new(Type::value_named("Example.Native", "ArcData")), 1),
            Type::PtrMut(
                Box::new(Type::value_named("External.Api", "EXTERNAL_RECORD")),
                1
            ),
        ]
    );
    assert_eq!(
        add_path_arc.signature(&[]).return_type,
        Type::value_named("Example.Native", "NativeStatus")
    );
    let Item::Fn(create_path) = index.expect_item("Example.Native", "CreatePath") else {
        panic!("Example.Native.CreatePath was not emitted as a function");
    };
    assert_eq!(create_path.calling_convention(), "system");
    assert_eq!(
        create_path.impl_map().unwrap().import_scope().name(),
        "annotated.dll"
    );
    assert_eq!(
        create_path.signature(&[]).types,
        [
            Type::value_named("Example.Native", "NativeFillMode"),
            Type::PtrMut(Box::new(Type::Void), 2),
        ]
    );
    assert_eq!(
        create_path.signature(&[]).return_type,
        Type::value_named("Example.Native", "NativeStatus")
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn aggregate_and_satellite_inputs_match_case_variant_selectors() {
    helpers::ensure_libclang();

    let scratch = scratch("aggregate-satellite");
    let aggregate = scratch.join("Aggregate.h");
    let satellite = scratch.join("Satellite.h");
    std::fs::write(&aggregate, "typedef unsigned AGGREGATE_VALUE;\n").unwrap();
    std::fs::write(
        &satellite,
        "#ifndef SATELLITE_NAME\n\
         #define SATELLITE_NAME SATELLITE_VALUE\n\
         #endif\n\
         typedef unsigned SATELLITE_NAME;\n",
    )
    .unwrap();
    let snapshot = extract(
        [
            Input::new(
                "aggregate.cpp",
                format!(
                    "#include \"{}\"\n#include \"{}\"\n",
                    aggregate.to_string_lossy(),
                    satellite.to_string_lossy()
                ),
            ),
            Input::new(
                "satellite.cpp",
                format!(
                    "#define SATELLITE_NAME SATELLITE_SPECIAL\n#include \"{}\"\n",
                    satellite.to_string_lossy()
                ),
            ),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "AGGREGATE.CPP",
            "aggregate.h",
            RootPartition::new("aggregate", "Example.Aggregate"),
        )
        .with_traversed_header_for_input(
            "SATELLITE.CPP",
            "SATELLITE.H",
            RootPartition::new("satellite", "Example.Satellite"),
        );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert!(output(&partitions, "Example.Aggregate").contains("type AGGREGATE_VALUE = u32"));
    assert!(output(&partitions, "Example.Satellite").contains("type SATELLITE_SPECIAL = u32"));
    assert!(
        !partitions
            .values()
            .any(|rdl| rdl.contains("SATELLITE_VALUE"))
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn satellite_included_only_alias_is_not_retargeted_to_aggregate_owner() {
    helpers::ensure_libclang();

    let scratch = scratch("satellite-included-only-alias");
    let base = scratch.join("base.h");
    let iis = scratch.join("iis.h");
    let msxml = scratch.join("msxml.h");
    let satellite = scratch.join("ioapiset.h");
    std::fs::write(&base, "typedef unsigned __int64 ULONG_PTR, *PULONG_PTR;\n").unwrap();
    std::fs::write(&iis, "typedef unsigned __int64 ULONG_PTR, *PULONG_PTR;\n").unwrap();
    std::fs::write(&msxml, "typedef unsigned __int64 ULONG_PTR, *PULONG_PTR;\n").unwrap();
    std::fs::write(
        &satellite,
        "extern \"C\" void UseSatellitePointer(PULONG_PTR value);\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let aggregate_source =
        "#include \"base.h\"\n#include \"ioapiset.h\"\n#include \"iis.h\"\n#include \"msxml.h\"\n";
    let satellite_source = "#include \"base.h\"\n#include \"ioapiset.h\"\n#include \"msxml.h\"\n";
    let snapshot = extract(
        [
            Input::new("aggregate.cpp", aggregate_source)
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
            Input::new("satellite.cpp", satellite_source)
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
        ],
        &[
            "-x",
            "c++",
            "--target=x86_64-pc-windows-msvc",
            include.as_str(),
        ],
    )
    .unwrap();
    let function = snapshot
        .facts()
        .iter()
        .find(|fact| fact.origin.tu == "satellite.cpp" && fact.name == "UseSatellitePointer")
        .unwrap();
    let FactData::Function { params, .. } = &function.data else {
        panic!("UseSatellitePointer was not extracted as a function");
    };
    let TypeRef::Named { declaration, .. } = &params[0].ty else {
        panic!("UseSatellitePointer did not retain its PULONG_PTR reference");
    };
    assert_eq!(declaration.file, base.to_string_lossy().replace('\\', "/"));
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "aggregate.cpp",
            "iis.h",
            RootPartition::new("iis", "Example.Iis"),
        )
        .with_traversed_header_for_input(
            "aggregate.cpp",
            "msxml.h",
            RootPartition::new("msxml", "Example.MsXml"),
        )
        .with_traversed_header_for_input(
            "satellite.cpp",
            "ioapiset.h",
            RootPartition::new("satellite", "Example.Satellite")
                .with_library("UseSatellitePointer", "satellite.dll"),
        );
    let references = BTreeMap::new();
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let options = EmitOptions::new("Example.Common", &references);
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();

    for namespace in ["Example.Iis", "Example.MsXml"] {
        assert!(
            output(&partitions, namespace).contains("type PULONG_PTR = *mut u64"),
            "{partitions:#?}"
        );
    }
    assert!(
        !partitions
            .keys()
            .any(|partition| partition.namespace == "Example.Common"),
        "{partitions:#?}"
    );
    let satellite = output(&partitions, "Example.Satellite");
    assert!(
        satellite.contains("fn UseSatellitePointer(value: *mut u64)"),
        "{satellite}"
    );

    let winmd = scratch.join("satellite-included-only-alias.winmd");
    let mut compiler = windows_rdl::reader();
    for rdl in partitions.values() {
        compiler.input_text(rdl);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    for namespace in ["Example.Iis", "Example.MsXml"] {
        assert_eq!(
            index.expect(namespace, "PULONG_PTR").underlying_type(),
            Some(Type::PtrMut(Box::new(Type::U64), 1))
        );
    }
    let Item::Fn(function) = index.expect_item("Example.Satellite", "UseSatellitePointer") else {
        panic!("UseSatellitePointer was not emitted as a function");
    };
    let signature = function.signature(&[]);
    assert_eq!(signature.return_type, Type::Void);
    assert_eq!(signature.types, [Type::PtrMut(Box::new(Type::U64), 1)]);

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn canonical_pointer_aliases_keep_declaration_specific_routes() {
    helpers::ensure_libclang();

    let scratch = scratch("canonical-pointer-routes");
    let common_types = scratch.join("common_types.h");
    let common_api = scratch.join("common_api.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(&common_types, "typedef void* PVOID;\n").unwrap();
    std::fs::write(
        &common_api,
        format!(
            "#include \"{}\"\n\
             extern \"C\" void CommonUse(PVOID value);\n",
            common_types.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &first,
        "typedef void* PVOID;\n\
         typedef PVOID PSID;\n\
         typedef PVOID (ENCLAVE_TARGET_FUNCTION)(PVOID);\n\
         typedef ENCLAVE_TARGET_FUNCTION (*PENCLAVE_TARGET_FUNCTION);\n\
         typedef PENCLAVE_TARGET_FUNCTION LPENCLAVE_TARGET_FUNCTION;\n\
         typedef void (*WORKERCALLBACKFUNC)(PVOID);\n\
         typedef void (*APC_CALLBACK_FUNCTION)(unsigned long, PVOID, PVOID);\n\
         extern \"C\" void FirstUse(PSID value);\n\
         extern \"C\" void FirstRaw(PVOID* value);\n\
         extern \"C\" PVOID ReadPointer(PVOID const* source);\n",
    )
    .unwrap();
    std::fs::write(
        &second,
        "#define VOID void\n\
         typedef VOID* PVOID;\n\
         typedef PVOID TBS_HCONTEXT, *PTBS_HCONTEXT;\n\
         extern \"C\" void SecondUse(PTBS_HCONTEXT value, TBS_HCONTEXT context);\n",
    )
    .unwrap();
    let snapshot = aggregate_snapshot(&scratch, &[&first, &second, &common_api]);
    let reverse = aggregate_snapshot(&scratch, &[&second, &first, &common_api]);
    let selected_snapshot = snapshot.clone();
    let selected_reverse = reverse.clone();
    let common_partition =
        RootPartition::new("common", "Example.Common").with_library("CommonUse", "common.dll");
    let first_partition = RootPartition::new("first", "Example.First")
        .with_library("FirstUse", "first.dll")
        .with_library("FirstRaw", "first.dll")
        .with_library("ReadPointer", "first.dll");
    let second_partition =
        RootPartition::new("second", "Example.Second").with_library("SecondUse", "second.dll");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(common_api.to_string_lossy(), common_partition.clone())
        .with_traversed_header(first.to_string_lossy(), first_partition.clone())
        .with_traversed_header(second.to_string_lossy(), second_partition.clone());
    let reverse_policy = HeaderPartitionPolicy::new()
        .with_traversed_header(second.to_string_lossy(), second_partition)
        .with_traversed_header(first.to_string_lossy(), first_partition)
        .with_traversed_header(common_api.to_string_lossy(), common_partition);
    let references = BTreeMap::new();
    let functions = BTreeSet::from([
        "CommonUse".to_string(),
        "FirstRaw".to_string(),
        "FirstUse".to_string(),
        "SecondUse".to_string(),
    ]);
    let mut options = EmitOptions::new("Example.Common", &references);
    options.functions = Some(&functions);
    let emit = |snapshot: Snapshot, policy: &HeaderPartitionPolicy, options: &EmitOptions<'_>| {
        let plan = snapshot
            .plan_header_partitions(policy, &NamespaceAuthorities::new())
            .unwrap();
        assert!(plan.audit(options).unwrap().is_clean());
        plan.emit_with_options(options).unwrap()
    };
    let partitions = emit(snapshot, &policy, &options);
    let reverse_partitions = emit(reverse, &reverse_policy, &options);
    assert_eq!(partitions, reverse_partitions);

    let first = output(&partitions, "Example.First");
    assert!(!first.contains("type PVOID"), "{first}");
    assert!(first.contains("type PSID = *mut void"), "{first}");
    assert!(first.contains("fn FirstUse(value: PSID)"), "{first}");
    assert!(
        first.contains("fn FirstRaw(value: *mut *mut void)"),
        "{first}"
    );
    let second = output(&partitions, "Example.Second");
    assert!(second.contains("type PVOID = *mut void"), "{second}");
    assert!(
        second.contains("type PTBS_HCONTEXT = *mut PVOID"),
        "{second}"
    );
    assert!(second.contains("type TBS_HCONTEXT = PVOID"), "{second}");
    assert!(
        second.contains("fn SecondUse(value: PTBS_HCONTEXT, context: TBS_HCONTEXT)"),
        "{second}"
    );

    let winmd = scratch.join("canonical-pointer-routes.winmd");
    let mut compiler = windows_rdl::reader();
    for rdl in partitions.values() {
        compiler.input_text(rdl);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    assert!(!index.contains("Example.First", "PVOID"));
    assert_eq!(
        index.expect("Example.Second", "PVOID").underlying_type(),
        Some(Type::PtrMut(Box::new(Type::Void), 1))
    );
    let Item::Fn(first_use) = index.expect_item("Example.First", "FirstUse") else {
        panic!("Example.First.FirstUse was not emitted as a function");
    };
    assert_eq!(
        first_use.signature(&[]).types,
        [Type::value_named("Example.First", "PSID")]
    );
    assert_eq!(
        index.expect("Example.First", "PSID").underlying_type(),
        Some(Type::PtrMut(Box::new(Type::Void), 1))
    );
    let Item::Fn(first_raw) = index.expect_item("Example.First", "FirstRaw") else {
        panic!("Example.First.FirstRaw was not emitted as a function");
    };
    assert_eq!(
        first_raw.signature(&[]).types,
        [Type::PtrMut(Box::new(Type::Void), 2)]
    );
    let Item::Fn(second_use) = index.expect_item("Example.Second", "SecondUse") else {
        panic!("Example.Second.SecondUse was not emitted as a function");
    };
    assert_eq!(
        second_use.signature(&[]).types,
        [
            Type::value_named("Example.Second", "PTBS_HCONTEXT"),
            Type::value_named("Example.Second", "TBS_HCONTEXT")
        ]
    );
    assert_eq!(
        index
            .expect("Example.Second", "PTBS_HCONTEXT")
            .underlying_type(),
        Some(Type::PtrMut(
            Box::new(Type::value_named("Example.Second", "PVOID")),
            1
        ))
    );
    assert_eq!(
        index
            .expect("Example.Second", "TBS_HCONTEXT")
            .underlying_type(),
        Some(Type::value_named("Example.Second", "PVOID"))
    );
    let common = output(&partitions, "Example.Common");
    assert!(!common.contains("type PVOID"), "{common}");
    assert!(
        common.contains("fn CommonUse(value: *mut void)"),
        "{common}"
    );
    let Item::Fn(common_use) = index.expect_item("Example.Common", "CommonUse") else {
        panic!("Example.Common.CommonUse was not emitted as a function");
    };
    assert_eq!(
        common_use.signature(&[]).types,
        [Type::PtrMut(Box::new(Type::Void), 1)]
    );

    let selected_functions = functions
        .iter()
        .cloned()
        .chain(["ReadPointer".to_string()])
        .collect::<BTreeSet<_>>();
    let mut selected_options = EmitOptions::new("Example.Common", &references);
    selected_options.functions = Some(&selected_functions);
    let selected_partitions = emit(selected_snapshot, &policy, &selected_options);
    let selected_reverse_partitions = emit(selected_reverse, &reverse_policy, &selected_options);
    assert_eq!(selected_partitions, selected_reverse_partitions);
    let selected_first = output(&selected_partitions, "Example.First");
    assert!(
        selected_first.contains("type PVOID = *mut void"),
        "{selected_first}"
    );
    assert!(
        selected_first.contains("fn ReadPointer(source: *const PVOID) -> PVOID"),
        "{selected_first}"
    );
    let selected_winmd = scratch.join("selected-canonical-pointer-routes.winmd");
    let mut compiler = windows_rdl::reader();
    for rdl in selected_partitions.values() {
        compiler.input_text(rdl);
    }
    compiler
        .reference_default()
        .output(&selected_winmd)
        .write()
        .unwrap();
    let selected_index = windows_metadata::reader::Index::read(&selected_winmd).unwrap();
    assert_eq!(
        selected_index
            .expect("Example.First", "PVOID")
            .underlying_type(),
        Some(Type::PtrMut(Box::new(Type::Void), 1))
    );
    let Item::Fn(read_pointer) = selected_index.expect_item("Example.First", "ReadPointer") else {
        panic!("Example.First.ReadPointer was not emitted as a function");
    };
    let signature = read_pointer.signature(&[]);
    assert_eq!(
        signature.types,
        [Type::PtrConst(
            Box::new(Type::value_named("Example.First", "PVOID")),
            1
        )]
    );
    assert_eq!(
        signature.return_type,
        Type::value_named("Example.First", "PVOID")
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn canonical_pointer_alias_dependency_keeps_the_consuming_partition_identity() {
    helpers::ensure_libclang();

    let scratch = scratch("canonical-pointer-dependency-owner");
    let corhdr = scratch.join("CorHdr.h");
    let cor = scratch.join("cor.h");
    let corprof = scratch.join("corprof.h");
    std::fs::write(
        &corhdr,
        "#pragma once\n\
         typedef unsigned char COR_SIGNATURE;\n\
         typedef const COR_SIGNATURE* PCCOR_SIGNATURE;\n",
    )
    .unwrap();
    std::fs::write(
        &cor,
        format!(
            "#pragma once\n\
             #include \"{}\"\n\
             struct METADATA_MARKER {{ int value; }};\n\
             extern \"C\" void GetMetadataSignature(PCCOR_SIGNATURE* ppvSig);\n",
            corhdr.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &corprof,
        format!(
            "#pragma once\n\
             #include \"{}\"\n\
             typedef unsigned char BYTE;\n\
             typedef const BYTE* LPCBYTE;\n\
             typedef BYTE COR_SIGNATURE;\n\
             typedef const COR_SIGNATURE* PCCOR_SIGNATURE;\n\
             extern \"C\" void GetDynamicFunctionInfo(PCCOR_SIGNATURE* ppvSig);\n\
             extern \"C\" void GetFunctionFromIP3(LPCBYTE ip);\n",
            cor.to_string_lossy()
        ),
    )
    .unwrap();
    let snapshot = aggregate_snapshot(&scratch, &[&corhdr, &cor, &corprof]);
    let reverse = aggregate_snapshot(&scratch, &[&corprof, &cor, &corhdr]);
    let profiling = RootPartition::new("profiling", "Example.ClrProfiling")
        .with_library("GetDynamicFunctionInfo", "profiling.dll")
        .with_library("GetFunctionFromIP3", "profiling.dll");
    let profiling_only =
        HeaderPartitionPolicy::new().with_traversed_header(corprof.to_string_lossy(), profiling);
    let combined_policy = profiling_only
        .clone()
        .with_traversed_header(
            corhdr.to_string_lossy(),
            RootPartition::new("metadata", "Example.WinRT.Metadata"),
        )
        .with_traversed_header(
            cor.to_string_lossy(),
            RootPartition::new("metadata", "Example.WinRT.Metadata")
                .with_library("GetMetadataSignature", "metadata.dll"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let emit = |snapshot: &Snapshot, policy: &HeaderPartitionPolicy| {
        let plan = snapshot
            .plan_header_partitions(policy, &NamespaceAuthorities::new())
            .unwrap();
        assert!(plan.audit(&options).unwrap().is_clean());
        plan.emit_with_options(&options).unwrap()
    };
    let profiling_only = emit(&snapshot, &profiling_only);
    let profiling_only_rdl = output(&profiling_only, "Example.ClrProfiling");
    assert!(
        profiling_only_rdl.contains("type PCCOR_SIGNATURE = *const u8"),
        "{profiling_only:#?}"
    );
    assert!(
        profiling_only_rdl.contains("type LPCBYTE = *const u8"),
        "{profiling_only:#?}"
    );
    assert!(
        profiling_only_rdl.contains("fn GetDynamicFunctionInfo(ppvSig: *mut PCCOR_SIGNATURE)"),
        "{profiling_only:#?}"
    );
    assert!(
        profiling_only_rdl.contains("fn GetFunctionFromIP3(ip: LPCBYTE)"),
        "{profiling_only:#?}"
    );

    let combined = emit(&snapshot, &combined_policy);
    assert_eq!(combined, emit(&reverse, &combined_policy));
    let profiling_rdl = output(&combined, "Example.ClrProfiling");
    let metadata_rdl = combined
        .iter()
        .filter(|(partition, _)| partition.namespace == "Example.WinRT.Metadata")
        .map(|(_, rdl)| rdl.as_str())
        .collect::<String>();
    assert!(
        profiling_rdl.contains("type PCCOR_SIGNATURE = *const u8"),
        "{combined:#?}"
    );
    assert!(
        metadata_rdl.contains("type PCCOR_SIGNATURE = *const u8"),
        "{combined:#?}"
    );
    assert!(
        metadata_rdl.contains("fn GetMetadataSignature(ppvSig: *mut PCCOR_SIGNATURE)"),
        "{combined:#?}"
    );
    assert!(
        profiling_rdl.contains("fn GetDynamicFunctionInfo(ppvSig: *mut PCCOR_SIGNATURE)"),
        "{combined:#?}"
    );
    assert!(
        profiling_rdl.contains("fn GetFunctionFromIP3(ip: LPCBYTE)"),
        "{combined:#?}"
    );

    let winmd = scratch.join("canonical-pointer-dependency-owner.winmd");
    windows_rdl::reader()
        .input_texts(combined.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    for namespace in ["Example.ClrProfiling", "Example.WinRT.Metadata"] {
        assert_eq!(
            index.expect(namespace, "PCCOR_SIGNATURE").underlying_type(),
            Some(Type::PtrConst(Box::new(Type::U8), 1))
        );
    }
    assert_eq!(
        index
            .expect("Example.ClrProfiling", "LPCBYTE")
            .underlying_type(),
        Some(Type::PtrConst(Box::new(Type::U8), 1))
    );
    let Item::Fn(dynamic) = index.expect_item("Example.ClrProfiling", "GetDynamicFunctionInfo")
    else {
        panic!("GetDynamicFunctionInfo was not emitted as a function");
    };
    assert_eq!(
        dynamic.signature(&[]).types,
        [Type::PtrMut(
            Box::new(Type::value_named("Example.ClrProfiling", "PCCOR_SIGNATURE")),
            1
        )]
    );
    let Item::Fn(from_ip) = index.expect_item("Example.ClrProfiling", "GetFunctionFromIP3") else {
        panic!("GetFunctionFromIP3 was not emitted as a function");
    };
    assert_eq!(
        from_ip.signature(&[]).types,
        [Type::value_named("Example.ClrProfiling", "LPCBYTE")]
    );
    let Item::Fn(metadata) = index.expect_item("Example.WinRT.Metadata", "GetMetadataSignature")
    else {
        panic!("GetMetadataSignature was not emitted as a function");
    };
    assert_eq!(
        metadata.signature(&[]).types,
        [Type::PtrMut(
            Box::new(Type::value_named(
                "Example.WinRT.Metadata",
                "PCCOR_SIGNATURE"
            )),
            1
        )]
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn included_canonical_pointer_alias_is_retained_by_owned_callback() {
    helpers::ensure_libclang();

    let scratch = scratch("included-canonical-pointer-callback");
    let common = scratch.join("common.h");
    let base = scratch.join("minwinbase.h");
    let enclave = scratch.join("enclaveapi.h");
    std::fs::write(&common, "#pragma once\ntypedef void* LPVOID;\n").unwrap();
    std::fs::write(
        &base,
        format!(
            "#pragma once\n\
             #include \"{}\"\n\
             typedef LPVOID (*PENCLAVE_ROUTINE)(LPVOID lpThreadParameter);\n\
             typedef PENCLAVE_ROUTINE LPENCLAVE_ROUTINE;\n",
            common.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &enclave,
        format!(
            "#pragma once\n\
             #include \"{}\"\n\
             extern \"C\" int CallEnclave(\n\
                 LPENCLAVE_ROUTINE lpRoutine,\n\
                 LPVOID lpParameter,\n\
                 int fWaitForThread,\n\
                 LPVOID* lpReturnValue);\n",
            base.to_string_lossy()
        ),
    )
    .unwrap();
    let snapshot = aggregate_snapshot(&scratch, &[&base, &enclave]);
    let reverse = aggregate_snapshot(&scratch, &[&enclave, &base]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            base.to_string_lossy(),
            RootPartition::new("base", "Example.SystemServices"),
        )
        .with_traversed_header(
            enclave.to_string_lossy(),
            RootPartition::new("enclave", "Example.Environment")
                .with_library("CallEnclave", "enclave.dll"),
        );
    let references = BTreeMap::new();
    let functions = BTreeSet::from(["CallEnclave".to_string()]);
    let mut options = EmitOptions::new("Example.Common", &references);
    options.functions = Some(&functions);
    let emit = |snapshot: Snapshot| {
        let plan = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap();
        assert!(plan.audit(&options).unwrap().is_clean());
        plan.emit_with_options(&options).unwrap()
    };
    let partitions = emit(snapshot);
    assert_eq!(partitions, emit(reverse));

    let common = output(&partitions, "Example.Common");
    assert!(
        common.contains("type LPVOID = *mut void"),
        "{partitions:#?}"
    );
    let base = output(&partitions, "Example.SystemServices");
    assert!(
        base.contains(
            "fn PENCLAVE_ROUTINE(lpThreadParameter: Example::Common::LPVOID) -> \
             Example::Common::LPVOID"
        ),
        "{partitions:#?}"
    );
    assert!(
        base.contains("type LPENCLAVE_ROUTINE = PENCLAVE_ROUTINE"),
        "{partitions:#?}"
    );
    let enclave = output(&partitions, "Example.Environment");
    assert!(
        enclave.contains(
            "fn CallEnclave(lpRoutine: Example::SystemServices::LPENCLAVE_ROUTINE, \
             lpParameter: Example::Common::LPVOID, fWaitForThread: i32, \
             lpReturnValue: *mut Example::Common::LPVOID) -> i32"
        ),
        "{partitions:#?}"
    );

    let winmd = scratch.join("included-canonical-pointer-callback.winmd");
    windows_rdl::reader()
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    assert_eq!(
        index.expect("Example.Common", "LPVOID").underlying_type(),
        Some(Type::PtrMut(Box::new(Type::Void), 1))
    );
    let invoke = index
        .expect("Example.SystemServices", "PENCLAVE_ROUTINE")
        .methods()
        .find(|method| method.name() == "Invoke")
        .unwrap()
        .signature(&[]);
    assert_eq!(
        invoke.types,
        [Type::value_named("Example.Common", "LPVOID")]
    );
    assert_eq!(
        invoke.return_type,
        Type::value_named("Example.Common", "LPVOID")
    );
    assert_eq!(
        index
            .expect("Example.SystemServices", "LPENCLAVE_ROUTINE")
            .underlying_type(),
        Some(Type::class_named(
            "Example.SystemServices",
            "PENCLAVE_ROUTINE"
        ))
    );
    let Item::Fn(call_enclave) = index.expect_item("Example.Environment", "CallEnclave") else {
        panic!("Example.Environment.CallEnclave was not emitted as a function");
    };
    assert_eq!(
        call_enclave.signature(&[]).types,
        [
            Type::value_named("Example.SystemServices", "LPENCLAVE_ROUTINE"),
            Type::value_named("Example.Common", "LPVOID"),
            Type::I32,
            Type::PtrMut(Box::new(Type::value_named("Example.Common", "LPVOID")), 1),
        ]
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn included_canonical_pointer_alias_does_not_follow_another_input_owner() {
    helpers::ensure_libclang();

    let scratch = scratch("included-canonical-pointer");
    let tbs = scratch.join("tbs.h");
    let satellite = scratch.join("satellite.h");
    std::fs::write(
        &tbs,
        "typedef void* PVOID;\n\
         typedef PVOID* PTBS_HCONTEXT;\n\
         typedef PVOID TBS_HCONTEXT;\n\
         extern \"C\" void TbsUse(PTBS_HCONTEXT value, TBS_HCONTEXT context);\n",
    )
    .unwrap();
    std::fs::write(&satellite, "extern \"C\" void SatelliteUse(PVOID value);\n").unwrap();
    let roots = [scratch.to_string_lossy().to_string()];
    let snapshot = extract(
        [
            Input::new(
                "aggregate.cpp",
                format!("#include \"{}\"\n", tbs.to_string_lossy()),
            )
            .with_root_dirs(roots.clone()),
            Input::new(
                "satellite.cpp",
                format!(
                    "#include \"{}\"\n#include \"{}\"\n",
                    tbs.to_string_lossy(),
                    satellite.to_string_lossy()
                ),
            )
            .with_root_dirs(roots),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "aggregate.cpp",
            tbs.to_string_lossy(),
            RootPartition::new("tbs", "Example.Tbs").with_library("TbsUse", "tbs.dll"),
        )
        .with_traversed_header_for_input(
            "satellite.cpp",
            satellite.to_string_lossy(),
            RootPartition::new("satellite", "Example.Satellite")
                .with_library("SatelliteUse", "satellite.dll"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();

    let tbs_rdl = output(&partitions, "Example.Tbs");
    assert!(tbs_rdl.contains("type PVOID = *mut void"), "{tbs_rdl}");
    assert!(
        tbs_rdl.contains("type PTBS_HCONTEXT = *mut PVOID"),
        "{tbs_rdl}"
    );
    assert!(tbs_rdl.contains("type TBS_HCONTEXT = PVOID"), "{tbs_rdl}");
    let satellite_rdl = output(&partitions, "Example.Satellite");
    assert!(!satellite_rdl.contains("type PVOID"), "{satellite_rdl}");
    assert!(
        satellite_rdl.contains("fn SatelliteUse(value: *mut void)"),
        "{satellite_rdl}"
    );

    let winmd = scratch.join("included-canonical-pointer.winmd");
    let mut compiler = windows_rdl::reader();
    for rdl in partitions.values() {
        compiler.input_text(rdl);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    assert!(!index.contains("Example.Satellite", "PVOID"));
    let Item::Fn(satellite_use) = index.expect_item("Example.Satellite", "SatelliteUse") else {
        panic!("Example.Satellite.SatelliteUse was not emitted as a function");
    };
    assert_eq!(
        satellite_use.signature(&[]).types,
        [Type::PtrMut(Box::new(Type::Void), 1)]
    );
    assert_eq!(
        index
            .expect("Example.Tbs", "PTBS_HCONTEXT")
            .underlying_type(),
        Some(Type::PtrMut(
            Box::new(Type::value_named("Example.Tbs", "PVOID")),
            1
        ))
    );
    assert_eq!(
        index
            .expect("Example.Tbs", "TBS_HCONTEXT")
            .underlying_type(),
        Some(Type::value_named("Example.Tbs", "PVOID"))
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn macro_typedef_expansions_keep_declaration_specific_routes() {
    helpers::ensure_libclang();

    let scratch = scratch("macro-typedef-routes");
    let macros = scratch.join("macros.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    let other = scratch.join("other.h");
    std::fs::write(
        &macros,
        "#pragma once\n\
         #define C_ASSERT(e) typedef char __C_ASSERT__[(e) ? 1 : -1]\n",
    )
    .unwrap();
    for header in [&first, &second, &other] {
        std::fs::write(
            header,
            format!("#include \"{}\"\nC_ASSERT(1);\n", macros.to_string_lossy()),
        )
        .unwrap();
    }
    let forward = aggregate_snapshot(&scratch, &[&first, &second, &other]);
    let assertions = forward
        .facts()
        .iter()
        .filter(|fact| fact.name == "__C_ASSERT__")
        .collect::<Vec<_>>();
    assert_eq!(assertions.len(), 3, "{assertions:#?}");
    assert!(
        assertions
            .iter()
            .all(|fact| fact.spelling.file == macros.to_string_lossy().replace('\\', "/")),
        "{assertions:#?}"
    );
    assert_eq!(
        assertions
            .iter()
            .map(|fact| fact.expansion.file.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        3,
        "{assertions:#?}"
    );
    let reverse = aggregate_snapshot(&scratch, &[&other, &second, &first]);
    let first_partition = RootPartition::new("first", "Example.Assert");
    let second_partition = RootPartition::new("second", "Example.Assert");
    let other_partition = RootPartition::new("other", "Example.Other");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(first.to_string_lossy(), first_partition.clone())
        .with_traversed_header(second.to_string_lossy(), second_partition.clone())
        .with_traversed_header(other.to_string_lossy(), other_partition.clone());
    let reverse_policy = HeaderPartitionPolicy::new()
        .with_traversed_header(other.to_string_lossy(), other_partition)
        .with_traversed_header(second.to_string_lossy(), second_partition)
        .with_traversed_header(first.to_string_lossy(), first_partition);
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let emit = |snapshot: Snapshot, policy: &HeaderPartitionPolicy| {
        let plan = snapshot
            .plan_header_partitions(policy, &NamespaceAuthorities::new())
            .unwrap();
        assert!(plan.audit(&options).unwrap().is_clean());
        plan.emit_with_options(&options).unwrap()
    };
    let partitions = emit(forward, &policy);
    let reverse_partitions = emit(reverse, &reverse_policy);

    assert_eq!(partitions, reverse_partitions);
    assert_eq!(partitions.len(), 2, "{partitions:#?}");
    for namespace in ["Example.Assert", "Example.Other"] {
        assert!(
            output(&partitions, namespace).contains("type __C_ASSERT__ = [i8; 1]"),
            "{partitions:#?}"
        );
    }
    let winmd = scratch.join("macro-typedef-routes.winmd");
    let mut compiler = windows_rdl::reader();
    for rdl in partitions.values() {
        compiler.input_text(rdl);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    for namespace in ["Example.Assert", "Example.Other"] {
        assert_eq!(
            index.expect(namespace, "__C_ASSERT__").underlying_type(),
            Some(Type::ArrayFixed(Box::new(Type::I8), 1))
        );
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn macro_typedef_reference_with_multiple_routes_remains_diagnostic() {
    helpers::ensure_libclang();

    let scratch = scratch("macro-typedef-reference-conflict");
    let macros = scratch.join("macros.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    let api = scratch.join("api.h");
    std::fs::write(
        &macros,
        "#pragma once\n\
         #define C_ASSERT(e) typedef char __C_ASSERT__[(e) ? 1 : -1]\n",
    )
    .unwrap();
    for header in [&first, &second] {
        std::fs::write(
            header,
            format!("#include \"{}\"\nC_ASSERT(1);\n", macros.to_string_lossy()),
        )
        .unwrap();
    }
    std::fs::write(
        &api,
        "extern \"C\" void UseAssertion(__C_ASSERT__* value);\n",
    )
    .unwrap();
    let snapshot = aggregate_snapshot(&scratch, &[&first, &second, &api]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        )
        .with_traversed_header(
            api.to_string_lossy(),
            RootPartition::new("api", "Example.Api").with_library("UseAssertion", "example.dll"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let error = plan.audit(&options).unwrap_err().to_string();

    assert!(
        error.contains("unresolved local type `__C_ASSERT__`"),
        "{error}"
    );
    assert!(
        error.contains("header partition dependency closure found 1 blocker(s)"),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn included_macro_handle_uses_default_namespace_qualification() {
    helpers::ensure_libclang();

    let scratch = scratch("included-macro-handle");
    let macros = scratch.join("macros.h");
    let handles = scratch.join("handles.h");
    let graphics = scratch.join("graphics.h");
    std::fs::write(
        &macros,
        "#pragma once\n\
         #define DECLARE_HANDLE(name) \
         struct HWND__ { int unused; }; typedef struct HWND__ *HWND\n",
    )
    .unwrap();
    std::fs::write(
        &handles,
        format!(
            "#pragma once\n#include \"{}\"\nDECLARE_HANDLE(HWND);\n",
            macros.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &graphics,
        format!(
            "#include \"{}\"\n\
             struct TARGET_PROPERTIES {{ HWND hwnd; }};\n\
             extern \"C\" HWND PassWindow(HWND window);\n",
            handles.to_string_lossy()
        ),
    )
    .unwrap();
    let snapshot = aggregate_snapshot(&scratch, &[&graphics]);
    let hwnd = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == "HWND")
        .unwrap();
    assert_eq!(
        hwnd.spelling.file,
        macros.to_string_lossy().replace('\\', "/")
    );
    assert_eq!(
        hwnd.expansion.file,
        handles.to_string_lossy().replace('\\', "/")
    );
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        graphics.to_string_lossy(),
        RootPartition::new("graphics", "Example.Graphics")
            .with_library("PassWindow", "graphics.dll"),
    );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();

    let common = output(&partitions, "Example.Common");
    assert!(common.contains("type HWND = *mut HWND__"), "{common}");
    let graphics = output(&partitions, "Example.Graphics");
    assert!(
        graphics.contains("hwnd: Example::Common::HWND"),
        "{graphics}"
    );
    assert!(
        graphics.contains("fn PassWindow(window: Example::Common::HWND) -> Example::Common::HWND"),
        "{graphics}"
    );

    let winmd = scratch.join("included-macro-handle.winmd");
    let mut compiler = windows_rdl::reader();
    for rdl in partitions.values() {
        compiler.input_text(rdl);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    assert_eq!(
        index.expect("Example.Common", "HWND").underlying_type(),
        Some(Type::PtrMut(
            Box::new(Type::value_named("Example.Common", "HWND__")),
            1
        ))
    );
    assert_eq!(
        index
            .expect("Example.Graphics", "TARGET_PROPERTIES")
            .fields()
            .find(|field| field.name() == "hwnd")
            .unwrap()
            .ty(),
        Type::value_named("Example.Common", "HWND")
    );
    let Item::Fn(pass_window) = index.expect_item("Example.Graphics", "PassWindow") else {
        panic!("Example.Graphics.PassWindow was not emitted as a function");
    };
    let signature = pass_window.signature(&[]);
    assert_eq!(
        signature.return_type,
        Type::value_named("Example.Common", "HWND")
    );
    assert_eq!(
        signature.types,
        [Type::value_named("Example.Common", "HWND")]
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn property_key_types_follow_dependency_routes() {
    helpers::ensure_libclang();

    let scratch = scratch("property-key-routes");
    let guid = scratch.join("guid.h");
    let devpropdef = scratch.join("devpropdef.h");
    let wtypes = scratch.join("wtypes.h");
    let display = scratch.join("ntddvdeo.h");
    let discovery = scratch.join("functiondiscoverykeys.h");
    std::fs::write(
        &guid,
        "#pragma once\n\
         struct GUID {\n\
             unsigned long Data1;\n\
             unsigned short Data2;\n\
             unsigned short Data3;\n\
             unsigned char Data4[8];\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &devpropdef,
        format!(
            "#pragma once\n\
             #include \"{}\"\n\
             struct DEVPROPKEY {{ GUID fmtid; unsigned long pid; }};\n\
             #define DEFINE_DEVPROPKEY(name, ...)\n",
            guid.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &wtypes,
        format!(
            "#pragma once\n\
             #include \"{}\"\n\
             struct PROPERTYKEY {{ GUID fmtid; unsigned long pid; }};\n\
             #define DEFINE_PROPERTYKEY(name, ...)\n",
            guid.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &display,
        format!(
            "#include \"{}\"\n\
             DEFINE_DEVPROPKEY(DEVPKEY_Device_ActivityId, 0xc50a3f10, 0xaa5c, 0x4247, \
                 0xb8, 0x30, 0xd6, 0xa6, 0xf8, 0xea, 0xa3, 0x10, 4)\n",
            devpropdef.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &discovery,
        format!(
            "#include \"{}\"\n\
             DEFINE_PROPERTYKEY(PKEY_FunctionInstance, 0x08c0c253, 0xa154, 0x4746, \
                 0x90, 0x05, 0x82, 0xde, 0x53, 0x17, 0x14, 0x8b, 1)\n",
            wtypes.to_string_lossy()
        ),
    )
    .unwrap();
    let snapshot = aggregate_snapshot(&scratch, &[&display, &discovery]);
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            guid.to_string_lossy(),
            RootPartition::new("foundation", "Example.Foundation"),
        )
        .with_traversed_header(
            devpropdef.to_string_lossy(),
            RootPartition::new("properties", "Example.Devices.Properties"),
        )
        .with_traversed_header(
            wtypes.to_string_lossy(),
            RootPartition::new("system", "Example.System.SystemServices"),
        )
        .with_traversed_header(
            display.to_string_lossy(),
            RootPartition::new("display", "Example.Devices.Display"),
        )
        .with_traversed_header(
            discovery.to_string_lossy(),
            RootPartition::new("discovery", "Example.Devices.FunctionDiscovery"),
        );
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();

    let display_rdl = output(&partitions, "Example.Devices.Display");
    assert!(
        display_rdl.contains(
            "const DEVPKEY_Device_ActivityId: Example::Devices::Properties::DEVPROPKEY = 4"
        ),
        "{display_rdl}"
    );
    let discovery_rdl = output(&partitions, "Example.Devices.FunctionDiscovery");
    assert!(
        discovery_rdl.contains(
            "const PKEY_FunctionInstance: Example::System::SystemServices::PROPERTYKEY = 1"
        ),
        "{discovery_rdl}"
    );

    let default_policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            display.to_string_lossy(),
            RootPartition::new("display", "Example.Devices.Display"),
        )
        .with_traversed_header(
            discovery.to_string_lossy(),
            RootPartition::new("discovery", "Example.Devices.FunctionDiscovery"),
        );
    let default_partitions = snapshot
        .plan_header_partitions(&default_policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    assert!(
        output(&default_partitions, "Example.Devices.Display")
            .contains("const DEVPKEY_Device_ActivityId: Example::Common::DEVPROPKEY = 4"),
        "{default_partitions:#?}"
    );
    assert!(
        output(&default_partitions, "Example.Devices.FunctionDiscovery")
            .contains("const PKEY_FunctionInstance: Example::Common::PROPERTYKEY = 1"),
        "{default_partitions:#?}"
    );

    let remap_policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            guid.to_string_lossy(),
            RootPartition::new("foundation", "Example.Foundation"),
        )
        .with_traversed_header(
            devpropdef.to_string_lossy(),
            RootPartition::new("properties", "Example.Devices.Properties")
                .with_remap("DEVPROPKEY", "DEVICE_PROPERTY_KEY"),
        )
        .with_traversed_header(
            wtypes.to_string_lossy(),
            RootPartition::new("system", "Example.System.SystemServices")
                .with_remap("PROPERTYKEY", "PROPERTY_KEY"),
        )
        .with_traversed_header(
            display.to_string_lossy(),
            RootPartition::new("display", "Example.Devices.Display"),
        )
        .with_traversed_header(
            discovery.to_string_lossy(),
            RootPartition::new("discovery", "Example.Devices.FunctionDiscovery"),
        );
    let remapped = snapshot
        .plan_header_partitions(&remap_policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    assert!(
        output(&remapped, "Example.Devices.Display").contains(
            "const DEVPKEY_Device_ActivityId: \
             Example::Devices::Properties::DEVICE_PROPERTY_KEY = 4"
        ),
        "{remapped:#?}"
    );
    assert!(
        output(&remapped, "Example.Devices.FunctionDiscovery").contains(
            "const PKEY_FunctionInstance: Example::System::SystemServices::PROPERTY_KEY = 1"
        ),
        "{remapped:#?}"
    );

    let winmd = scratch.join("property-key-routes.winmd");
    windows_rdl::reader()
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    for (namespace, name) in [
        ("Example.Devices.Properties", "DEVPROPKEY"),
        ("Example.System.SystemServices", "PROPERTYKEY"),
    ] {
        let fields = index
            .expect(namespace, name)
            .fields()
            .map(|field| (field.name().to_string(), field.ty()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            fields["fmtid"],
            Type::value_named("Example.Foundation", "GUID")
        );
        assert_eq!(fields["pid"], Type::U32);
    }
    assert_property_key(
        &index,
        "Example.Devices.Display",
        "DEVPKEY_Device_ActivityId",
        "Example.Devices.Properties",
        "DEVPROPKEY",
        [
            Value::U32(0xc50a3f10),
            Value::U16(0xaa5c),
            Value::U16(0x4247),
            Value::U8(0xb8),
            Value::U8(0x30),
            Value::U8(0xd6),
            Value::U8(0xa6),
            Value::U8(0xf8),
            Value::U8(0xea),
            Value::U8(0xa3),
            Value::U8(0x10),
        ],
        4,
    );
    assert_property_key(
        &index,
        "Example.Devices.FunctionDiscovery",
        "PKEY_FunctionInstance",
        "Example.System.SystemServices",
        "PROPERTYKEY",
        [
            Value::U32(0x08c0c253),
            Value::U16(0xa154),
            Value::U16(0x4746),
            Value::U8(0x90),
            Value::U8(0x05),
            Value::U8(0x82),
            Value::U8(0xde),
            Value::U8(0x53),
            Value::U8(0x17),
            Value::U8(0x14),
            Value::U8(0x8b),
        ],
        1,
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

fn assert_property_key(
    index: &windows_metadata::reader::Index,
    namespace: &str,
    name: &str,
    type_namespace: &str,
    type_name: &str,
    guid: [Value; 11],
    pid: u32,
) {
    let Item::Const(key) = index.expect_item(namespace, name) else {
        panic!("{namespace}.{name} was not emitted as a constant");
    };
    assert_eq!(key.ty(), Type::value_named(type_namespace, type_name));
    assert_eq!(key.constant().unwrap().value(), Value::U32(pid));
    assert_eq!(
        key.find_attribute("GuidAttribute").unwrap().value(),
        guid.into_iter()
            .map(|value| (String::new(), value))
            .collect::<Vec<_>>()
    );
}

#[test]
fn property_key_type_with_multiple_routes_remains_diagnostic() {
    helpers::ensure_libclang();

    let scratch = scratch("property-key-route-conflict");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    let display = scratch.join("display.h");
    std::fs::write(&first, "typedef unsigned long DEVPROPKEY;\n").unwrap();
    std::fs::write(&second, "typedef unsigned long DEVPROPKEY;\n").unwrap();
    std::fs::write(
        &display,
        format!(
            "#include \"{}\"\n\
             #include \"{}\"\n\
             #define DEFINE_DEVPROPKEY(name, ...)\n\
             DEFINE_DEVPROPKEY(DEVPKEY_Test, 0x12345678, 0x1234, 0x5678, 0x90, 0xab, \
                 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 3)\n",
            first.to_string_lossy(),
            second.to_string_lossy()
        ),
    )
    .unwrap();
    let snapshot = aggregate_snapshot(&scratch, &[&display]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        )
        .with_traversed_header(
            display.to_string_lossy(),
            RootPartition::new("display", "Example.Display"),
        );
    let references = BTreeMap::new();
    let error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&EmitOptions::new("Example.Common", &references))
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("header partition dependency closure found 1 blocker(s)"),
        "{error}"
    );
    assert!(error.contains("`DEVPROPKEY`"), "{error}");

    std::fs::remove_dir_all(scratch).unwrap();
}
