use std::collections::BTreeMap;
use windows_clang::{EmitOptions, Input, RdlPartition, extract_partitioned};

#[test]
fn partitioned_emission_routes_owners_and_qualifies_types() {
    helpers::ensure_libclang();

    let scratch =
        std::env::temp_dir().join(format!("windows-clang-partitioned-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let types = scratch.join("types.h");
    let functions = scratch.join("functions.h");
    std::fs::write(
        &types,
        "typedef struct ITEM { int value; } ITEM;\n\
         typedef void (*ITEM_CALLBACK)(ITEM value);\n\
         typedef struct HOLDER { ITEM item; ITEM_CALLBACK callback; } HOLDER;\n\
         typedef unsigned COUNT;\n\
         #define ITEM_COUNT 4\n\
         #define DEFAULT_COUNT ((COUNT)4)\n",
    )
    .unwrap();
    std::fs::write(
        &functions,
        "#include \"types.h\"\n\
         extern \"C\" ITEM UseItem(ITEM value);\n\
         extern \"C\" void UseHolder(HOLDER value, ITEM_CALLBACK callback);\n",
    )
    .unwrap();

    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned(
        [
            Input::new(
                types.to_string_lossy(),
                std::fs::read_to_string(&types).unwrap(),
            )
            .partitioned("types-input")
            .with_root(types.to_string_lossy(), "types", "Example.Types"),
            Input::new(
                functions.to_string_lossy(),
                std::fs::read_to_string(&functions).unwrap(),
            )
            .partitioned("functions-input")
            .with_root(
                functions.to_string_lossy(),
                "functions",
                "Example.Functions",
            ),
        ],
        &["-x", "c++", &include],
    )
    .unwrap();

    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();
    let types_rdl = &partitions[&RdlPartition {
        partition: "types".to_string(),
        namespace: "Example.Types".to_string(),
        header: types.to_string_lossy().replace('\\', "/"),
    }];
    let functions_rdl = &partitions[&RdlPartition {
        partition: "functions".to_string(),
        namespace: "Example.Functions".to_string(),
        header: functions.to_string_lossy().replace('\\', "/"),
    }];

    assert!(types_rdl.contains("mod Types"));
    assert!(types_rdl.contains("struct ITEM"));
    assert!(types_rdl.contains("const ITEM_COUNT: i32 = 4"));
    assert!(types_rdl.contains("extern \"C\" fn ITEM_CALLBACK(value: ITEM)"));
    assert!(types_rdl.contains("struct HOLDER"));
    assert!(types_rdl.contains("item: ITEM"));
    assert!(types_rdl.contains("callback: ITEM_CALLBACK"));
    assert!(types_rdl.contains("const DEFAULT_COUNT: COUNT = 4"));
    assert!(
        functions_rdl.contains("fn UseItem(value: Example::Types::ITEM) -> Example::Types::ITEM"),
        "{functions_rdl}"
    );
    assert!(
        functions_rdl.contains(
            "fn UseHolder(value: Example::Types::HOLDER, callback: Example::Types::ITEM_CALLBACK)"
        ),
        "{functions_rdl}"
    );

    let common = snapshot.emit_with_options(&options).unwrap();
    assert!(common.contains("mod Common"));
    assert!(common.contains("fn UseItem(value: ITEM) -> ITEM"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn equivalent_declarations_require_one_partition_owner() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [
            Input::new("first.h", "typedef unsigned VALUE;\n")
                .partitioned("first-input")
                .with_root("first.h", "first", "Example.First"),
            Input::new("second.h", "typedef unsigned VALUE;\n")
                .partitioned("second-input")
                .with_root("second.h", "second", "Example.Second"),
        ],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let error = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("type `VALUE` has ambiguous tagged root owners"),
        "{error}"
    );
}

#[test]
fn equivalent_declarations_with_same_partition_are_deterministic() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [
            Input::new("first.h", "typedef unsigned VALUE;\n")
                .partitioned("first-input")
                .with_root("first.h", "shared", "Example.Shared"),
            Input::new("second.h", "typedef unsigned VALUE;\n")
                .partitioned("second-input")
                .with_root("second.h", "shared", "Example.Shared"),
        ],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert_eq!(partitions.len(), 1);
    let (partition, rdl) = partitions.first_key_value().unwrap();
    assert_eq!(partition.partition, "shared");
    assert_eq!(partition.namespace, "Example.Shared");
    assert_eq!(partition.header, "first.h");
    assert!(rdl.contains("type VALUE = u32"));
}

#[test]
fn partitioned_emission_rejects_invalid_namespace() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new("value.h", "typedef unsigned VALUE;\n")
            .partitioned("value-input")
            .with_root("value.h", "value", "Example..Value")],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let error = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();
    assert_eq!(error.to_string(), "invalid namespace `Example..Value`");
}

#[test]
fn partitioned_emission_rejects_untagged_selected_root() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new("value.h", "typedef unsigned VALUE;\n").partitioned("value-input")],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let error = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "selected type `VALUE` has no tagged root owner"
    );
}
