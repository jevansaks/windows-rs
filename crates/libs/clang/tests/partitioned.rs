use std::collections::BTreeMap;
use windows_clang::{EmitOptions, Input, RdlPartition, RootPartition, extract_partitioned};

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
    let alias_header = scratch.join("alias.h");
    let direct3d_main = scratch.join("direct3d-main.cpp");
    let media_header = scratch.join("media.h");
    let dtc_header = scratch.join("dtc.h");
    std::fs::write(
        &shared_header,
        "#ifndef SHARED_H\n#define SHARED_H\n\
         struct IDirect3DSurface9 { int value; };\n\
         typedef struct IDirect3DSurface9 IDirect3DSurface9;\n\
         #endif\n",
    )
    .unwrap();
    std::fs::write(
        &alias_header,
        "#ifndef DIRECT3D_FULL\nstruct IDirect3DSurface9;\n#endif\n\
         #include \"shared.h\"\n",
    )
    .unwrap();
    std::fs::write(
        &direct3d_main,
        "#define DIRECT3D_FULL\n#include \"shared.h\"\n#include \"alias.h\"\n",
    )
    .unwrap();
    std::fs::write(
        &media_header,
        "#include \"alias.h\"\n\
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
                direct3d_main.to_string_lossy(),
                std::fs::read_to_string(&direct3d_main).unwrap(),
            )
            .partitioned("shared-input")
            .with_root(
                shared_header.to_string_lossy(),
                "shared",
                "Example.Direct3D9",
            )
            .with_root(
                alias_header.to_string_lossy(),
                "shared",
                "Example.Direct3D9",
            ),
            Input::new(
                media_header.to_string_lossy(),
                std::fs::read_to_string(&media_header).unwrap(),
            )
            .partitioned("media-input")
            .with_root(media_header.to_string_lossy(), "media", "Example.Media")
            .with_root(
                alias_header.to_string_lossy(),
                "shared",
                "Example.Direct3D9",
            ),
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
        media.contains("fn UseSurface(value: *mut Example::Direct3D9::IDirect3DSurface9)"),
        "{media}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn owner_remap_applies_before_namespace_collision_planning() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [
            Input::new(
                "first.h",
                "typedef struct _PIN_INFO { int first; } _PIN_INFO;\n\
                 extern \"C\" void UsePin(_PIN_INFO value);\n",
            )
            .partitioned("first-input")
            .with_root_partition(
                "first.h",
                RootPartition::new("first", "Example.First").with_remap("_PIN_INFO", "PIN_INFO"),
            ),
            Input::new(
                "second.h",
                "typedef struct PIN_INFO { long long second; } PIN_INFO;\n",
            )
            .partitioned("second-input")
            .with_root("second.h", "second", "Example.Second"),
        ],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();
    let first = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.First")
        .unwrap()
        .1;
    let second = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Second")
        .unwrap()
        .1;
    assert!(first.contains("struct PIN_INFO"), "{first}");
    assert!(first.contains("fn UsePin(value: PIN_INFO)"), "{first}");
    assert!(!first.contains("_PIN_INFO"), "{first}");
    assert!(second.contains("struct PIN_INFO"), "{second}");
}

#[test]
fn owner_exclusions_apply_before_remaps_and_only_to_the_selected_partition() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [
            Input::new(
                "first.h",
                "typedef struct _PIN_INFO { int first; } _PIN_INFO;\n\
                 extern \"C\" void ExcludedFunction(void);\n\
                 #define EXCLUDED_CONSTANT 1\n",
            )
            .partitioned("first-input")
            .with_root_partition(
                "first.h",
                RootPartition::new("first", "Example.First")
                    .with_remap("_PIN_INFO", "PIN_INFO")
                    .with_exclusion("_PIN_INFO")
                    .with_exclusion("ExcludedFunction")
                    .with_exclusion("EXCLUDED_CONSTANT"),
            ),
            Input::new(
                "second.h",
                "typedef struct PIN_INFO { long long second; } PIN_INFO;\n",
            )
            .partitioned("second-input")
            .with_root("second.h", "second", "Example.Second"),
        ],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();

    assert_eq!(partitions.len(), 1);
    let second = partitions.values().next().unwrap();
    assert!(second.contains("struct PIN_INFO"), "{second}");
    assert!(!second.contains("ExcludedFunction"), "{second}");
    assert!(!second.contains("EXCLUDED_CONSTANT"), "{second}");
}

#[test]
fn owner_exclusion_does_not_suppress_same_short_name_in_another_partition() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [
            Input::new(
                "first.h",
                "typedef struct PIN_INFO { int first; } PIN_INFO;\n",
            )
            .partitioned("first-input")
            .with_root_partition(
                "first.h",
                RootPartition::new("first", "Example.First").with_exclusion("PIN_INFO"),
            ),
            Input::new(
                "second.h",
                "typedef struct PIN_INFO { long long second; } PIN_INFO;\n",
            )
            .partitioned("second-input")
            .with_root("second.h", "second", "Example.Second"),
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
    assert_eq!(partition.namespace, "Example.Second");
    assert!(rdl.contains("struct PIN_INFO"), "{rdl}");
    assert!(rdl.contains("second: i64"), "{rdl}");
}

#[test]
fn required_owner_excluded_type_reports_reference_and_owner_diagnostics() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new(
            "hidden.h",
            "typedef struct HIDDEN { int value; } HIDDEN;\n\
             extern \"C\" HIDDEN UseHidden(HIDDEN value);\n",
        )
        .partitioned("hidden-input")
        .with_root_partition(
            "hidden.h",
            RootPartition::new("hidden", "Example.Hidden").with_exclusion("HIDDEN"),
        )],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let error = snapshot
        .emit_partitioned_with_options(&options)
        .unwrap_err();
    let error = error.to_string();

    assert!(error.contains("unresolved local type `HIDDEN`"), "{error}");
    assert!(error.contains("hidden.h:"), "{error}");
    assert!(
        error.contains("normalized declaration path `hidden.h`"),
        "{error}"
    );
    assert!(error.contains("partition=\"hidden\""), "{error}");
    assert!(error.contains("namespace=\"Example.Hidden\""), "{error}");
    assert!(error.contains("excluded by owner setting"), "{error}");
}

#[test]
fn owner_libraries_distinguish_same_function_name_between_partitions() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [
            Input::new("audio.h", "extern \"C\" int GetDeviceID(int device);\n")
                .partitioned("audio-input")
                .with_root_partition(
                    "audio.h",
                    RootPartition::new("audio", "Example.Audio")
                        .with_library("GetDeviceID", "DSOUND.dll"),
                ),
            Input::new(
                "tbs.h",
                "extern \"C\" long long GetDeviceID(long long device);\n",
            )
            .partitioned("tbs-input")
            .with_root_partition(
                "tbs.h",
                RootPartition::new("tbs", "Example.Tbs").with_library("GetDeviceID", "tbs.dll"),
            ),
        ],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let audio = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Audio")
        .unwrap()
        .1;
    let tbs = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Tbs")
        .unwrap()
        .1;

    assert!(audio.contains("#[library(\"DSOUND.dll\""), "{audio}");
    assert!(audio.contains("fn GetDeviceID"), "{audio}");
    assert!(tbs.contains("#[library(\"tbs.dll\""), "{tbs}");
    assert!(tbs.contains("fn GetDeviceID"), "{tbs}");
}
