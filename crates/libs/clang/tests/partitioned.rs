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
    let common = snapshot.emit_with_options(&options).unwrap();
    assert!(common.contains("mod Common"));
    assert!(common.contains("fn UseItem(value: ITEM) -> ITEM"));
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

#[test]
fn same_interface_name_can_emit_in_distinct_namespaces() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-same-interface-name-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let media_header = scratch.join("media.h");
    let dtc_header = scratch.join("dtc.h");
    let api_header = scratch.join("api.h");
    std::fs::write(
        &media_header,
        "struct __declspec(uuid(\"56a868ac-0ad4-11ce-b03a-0020af0ba770\")) \
         IResourceManager { virtual void Media() = 0; };\n",
    )
    .unwrap();
    std::fs::write(
        &dtc_header,
        "struct __declspec(uuid(\"13741d21-87eb-11ce-8081-0080c758527e\")) \
         IResourceManager { virtual void Dtc() = 0; };\n",
    )
    .unwrap();
    std::fs::write(
        &api_header,
        "#include \"media.h\"\nextern \"C\" void UseMedia(IResourceManager* value);\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned(
        [
            Input::new(
                media_header.to_string_lossy(),
                std::fs::read_to_string(&media_header).unwrap(),
            )
            .partitioned("media-input")
            .with_root(media_header.to_string_lossy(), "media", "Example.Media"),
            Input::new(
                dtc_header.to_string_lossy(),
                std::fs::read_to_string(&dtc_header).unwrap(),
            )
            .partitioned("dtc-input")
            .with_root(dtc_header.to_string_lossy(), "dtc", "Example.Dtc"),
            Input::new(
                api_header.to_string_lossy(),
                std::fs::read_to_string(&api_header).unwrap(),
            )
            .partitioned("api-input")
            .with_root(api_header.to_string_lossy(), "api", "Example.Api"),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc", &include],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let common_error = snapshot.emit_with_options(&options).unwrap_err();
    assert_eq!(
        common_error.to_string(),
        "interface `IResourceManager` has conflicting UUID attributes"
    );
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();

    assert_eq!(partitions.len(), 3);
    let media = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Media")
        .unwrap()
        .1;
    let dtc = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Dtc")
        .unwrap()
        .1;
    let api = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Api")
        .unwrap()
        .1;
    assert!(media.contains("interface IResourceManager"));
    assert!(media.contains("0x56a868ac_0ad4_11ce_b03a_0020af0ba770"));
    assert!(dtc.contains("interface IResourceManager"));
    assert!(dtc.contains("0x13741d21_87eb_11ce_8081_0080c758527e"));
    assert!(
        api.contains("fn UseMedia(value: Example::Media::IResourceManager)"),
        "{api}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn same_namespace_interface_uuid_conflict_is_still_an_error() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [
            Input::new(
                "first.h",
                "struct __declspec(uuid(\"56a868ac-0ad4-11ce-b03a-0020af0ba770\")) \
                 IResourceManager { virtual void First() = 0; };\n",
            )
            .partitioned("first-input")
            .with_root("first.h", "first", "Example.Shared"),
            Input::new(
                "second.h",
                "struct __declspec(uuid(\"13741d21-87eb-11ce-8081-0080c758527e\")) \
                 IResourceManager { virtual void Second() = 0; };\n",
            )
            .partitioned("second-input")
            .with_root("second.h", "second", "Example.Shared"),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let error = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "interface `IResourceManager` has conflicting UUID attributes"
    );
}

#[test]
fn collision_planning_preserves_unrelated_shared_type_references() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-shared-collision-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let shared_header = scratch.join("shared.h");
    let media_header = scratch.join("media.h");
    let dtc_header = scratch.join("dtc.h");
    std::fs::write(
        &shared_header,
        "struct IDirect3DSurface9;\n\
         struct __declspec(uuid(\"0cfbaf3a-9ff6-429a-99b3-a2796af8b89b\")) \
         IDirect3DSurface9 { virtual void Surface() = 0; };\n\
         typedef struct IDirect3DSurface9 IDirect3DSurface9;\n",
    )
    .unwrap();
    std::fs::write(
        &media_header,
        "#include \"shared.h\"\n\
         struct __declspec(uuid(\"56a868ac-0ad4-11ce-b03a-0020af0ba770\")) \
         IResourceManager { virtual void Media() = 0; };\n\
         extern \"C\" void UseSurface(IDirect3DSurface9* value);\n",
    )
    .unwrap();
    std::fs::write(
        &dtc_header,
        "struct __declspec(uuid(\"13741d21-87eb-11ce-8081-0080c758527e\")) \
         IResourceManager { virtual void Dtc() = 0; };\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned(
        [
            Input::new(
                shared_header.to_string_lossy(),
                std::fs::read_to_string(&shared_header).unwrap(),
            )
            .partitioned("shared-input")
            .with_root(
                shared_header.to_string_lossy(),
                "shared",
                "Example.Direct3D9",
            ),
            Input::new(
                media_header.to_string_lossy(),
                std::fs::read_to_string(&media_header).unwrap(),
            )
            .partitioned("media-input")
            .with_root(media_header.to_string_lossy(), "media", "Example.Media"),
            Input::new(
                dtc_header.to_string_lossy(),
                std::fs::read_to_string(&dtc_header).unwrap(),
            )
            .partitioned("dtc-input")
            .with_root(dtc_header.to_string_lossy(), "dtc", "Example.Dtc"),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc", &include],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();
    let media = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Media")
        .unwrap()
        .1;
    assert!(
        media.contains("fn UseSurface(value: Example::Direct3D9::IDirect3DSurface9)"),
        "{media}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}
