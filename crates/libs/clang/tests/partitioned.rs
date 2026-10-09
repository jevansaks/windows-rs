use std::collections::{BTreeMap, BTreeSet};
use windows_clang::{
    EmitOptions, ExtractionOptions, Input, NamespaceAuthorities, RdlPartition, RootPartition,
    TypeReference, TypeReferenceKind, extract_partitioned, extract_partitioned_with_options,
};

#[test]
fn selected_non_flat_function_uses_owned_spelling_route() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-owned-non-flat-function-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let owned = scratch.join("owned.h");
    let primary = scratch.join("primary.cpp");
    let secondary = scratch.join("secondary.cpp");
    std::fs::write(
        &owned,
        "enum OwnedStatusTag { OwnedOk = 0 };\n\
         typedef OwnedStatusTag OwnedStatus;\n\
         struct OwnedArc { float value; };\n\
         extern \"C\" OwnedStatus __stdcall OwnedSelected(OwnedArc* arc);\n\
         extern \"C\" OwnedStatus __stdcall OwnedUnselected(OwnedArc* arc);\n",
    )
    .unwrap();
    let primary_source = format!(
        "namespace Graphics {{ namespace DllExports {{\n\
         #include \"{}\"\n\
         extern \"C\" int __stdcall UnownedSelected(int value);\n\
         }} }}\n",
        owned.display()
    );
    let inputs = vec![
        Input::new(primary.to_string_lossy(), primary_source)
            .partitioned("primary-input")
            .with_root_partition(
                owned.to_string_lossy(),
                RootPartition::new("graphics", "Example.Graphics")
                    .with_library("OwnedSelected", "graphics.dll"),
            ),
        Input::new(
            secondary.to_string_lossy(),
            "typedef unsigned SECONDARY_VALUE;\n",
        )
        .partitioned("secondary-input")
        .with_root(
            secondary.to_string_lossy(),
            "secondary",
            "Example.Secondary",
        ),
    ];
    let args = [
        "-x",
        "c++",
        "-fms-extensions",
        "--target=x86_64-pc-windows-msvc",
    ];
    let serial = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());

    let selected = serial
        .facts()
        .iter()
        .find(|fact| fact.name == "OwnedSelected")
        .unwrap();
    assert_eq!(
        selected.origin.tu,
        primary.to_string_lossy().replace('\\', "/")
    );
    assert_eq!(
        selected.spelling.file,
        owned.to_string_lossy().replace('\\', "/")
    );
    assert_eq!(selected.expansion, selected.spelling);
    assert!(selected.root);
    let parent = serial
        .facts()
        .iter()
        .find(|fact| Some(&fact.origin) == selected.parent.as_ref())
        .unwrap();
    let grandparent = serial
        .facts()
        .iter()
        .find(|fact| Some(&fact.origin) == parent.parent.as_ref())
        .unwrap();
    assert_eq!(parent.name, "DllExports");
    assert_eq!(grandparent.name, "Graphics");

    let functions = BTreeSet::from(["OwnedSelected".to_string()]);
    let references = BTreeMap::from([
        (
            "EXTERNAL".to_string(),
            TypeReference::new("External", "EXTERNAL", TypeReferenceKind::Type),
        ),
        (
            "OwnedStatus".to_string(),
            TypeReference::new("External", "OwnedStatus", TypeReferenceKind::Type),
        ),
    ]);
    let mut options = EmitOptions::new("Example.Common", &references);
    options.functions = Some(&functions);
    assert_eq!(
        serial
            .clone()
            .emit_with_options(&options)
            .unwrap_err()
            .to_string(),
        "selected function `OwnedSelected` was not found"
    );
    let expected = serial
        .clone()
        .emit_partitioned_with_options(&options)
        .unwrap();
    assert_eq!(
        expected,
        parallel
            .clone()
            .emit_partitioned_with_options(&options)
            .unwrap()
    );
    let graphics = &expected[&RdlPartition {
        partition: "graphics".to_string(),
        namespace: "Example.Graphics".to_string(),
        header: owned.to_string_lossy().replace('\\', "/"),
    }];
    assert!(
        graphics.contains("#[library(\"graphics.dll\")]"),
        "{graphics}"
    );
    assert!(graphics.contains("enum OwnedStatusTag"), "{graphics}");
    assert!(
        graphics.contains("type OwnedStatus = OwnedStatusTag"),
        "{graphics}"
    );
    assert!(graphics.contains("struct OwnedArc"), "{graphics}");
    assert!(
        graphics.contains("fn OwnedSelected(arc: *mut OwnedArc) -> OwnedStatus"),
        "{graphics}"
    );
    assert!(!graphics.contains("OwnedUnselected"), "{graphics}");
    assert!(!graphics.contains("UnownedSelected"), "{graphics}");

    let unowned_functions = BTreeSet::from(["UnownedSelected".to_string()]);
    let mut unowned_options = EmitOptions::new("Example.Common", &references);
    unowned_options.functions = Some(&unowned_functions);
    let unowned_error = serial
        .emit_partitioned_with_options(&unowned_options)
        .unwrap_err()
        .to_string();
    assert_eq!(
        unowned_error,
        parallel
            .emit_partitioned_with_options(&unowned_options)
            .unwrap_err()
            .to_string()
    );
    assert_eq!(
        unowned_error,
        "selected function `UnownedSelected` was not found"
    );

    let mut reversed = inputs;
    reversed.reverse();
    let reversed = extract_partitioned_with_options(
        reversed,
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(
        expected,
        reversed
            .clone()
            .emit_partitioned_with_options(&options)
            .unwrap()
    );
    assert_eq!(
        unowned_error,
        reversed
            .emit_partitioned_with_options(&unowned_options)
            .unwrap_err()
            .to_string()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

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
fn legacy_typedef_target_exclusion_does_not_suppress_competing_owner() {
    helpers::ensure_libclang();

    let source = "typedef struct _SHARED { int value; } _SHARED;\n\
                  typedef _SHARED SHARED;\n\
                  typedef SHARED *PSHARED;\n";
    let snapshot = extract_partitioned(
        [
            Input::new("first.h", source)
                .partitioned("first-input")
                .with_root("first.h", "first", "Example.First"),
            Input::new("second.h", source)
                .partitioned("second-input")
                .with_root_partition(
                    "second.h",
                    RootPartition::new("second", "Example.Second").with_exclusion("_SHARED"),
                ),
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
            .contains("has ambiguous tagged root owners"),
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
fn exact_non_type_class_fact_falls_back_to_same_header_typedef() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-class-typedef-fallback-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let shared_header = scratch.join("shared.h");
    let alias_header = scratch.join("alias.h");
    let direct3d_main = scratch.join("direct3d-main.cpp");
    let media_main = scratch.join("media-main.cpp");
    std::fs::write(
        &shared_header,
        "struct __declspec(uuid(\"0cfbaf3a-9ff6-429a-99b3-a2796af8b89b\")) \
         IDirect3DSurface9;\n\
         typedef struct IDirect3DSurface9 IDirect3DSurface9;\n\
         struct __declspec(uuid(\"0cfbaf3a-9ff6-429a-99b3-a2796af8b89b\")) \
         IDirect3DSurface9 { virtual void Present() = 0; };\n",
    )
    .unwrap();
    std::fs::write(
        &alias_header,
        "struct __declspec(uuid(\"0cfbaf3a-9ff6-429a-99b3-a2796af8b89b\")) \
         IDirect3DSurface9;\n\
         typedef struct IDirect3DSurface9 IDirect3DSurface9;\n\
         struct __declspec(uuid(\"0cfbaf3a-9ff6-429a-99b3-a2796af8b89b\")) \
         IDirect3DSurface9 {};\n",
    )
    .unwrap();
    std::fs::write(&direct3d_main, "#include \"shared.h\"\n").unwrap();
    std::fs::write(
        &media_main,
        "#include \"alias.h\"\n\
         extern \"C\" void UseSurface(struct IDirect3DSurface9* value);\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned(
        [
            Input::new(
                direct3d_main.to_string_lossy(),
                std::fs::read_to_string(&direct3d_main).unwrap(),
            )
            .partitioned("direct3d-input")
            .with_root(
                shared_header.to_string_lossy(),
                "direct3d",
                "Example.Direct3D9",
            ),
            Input::new(
                media_main.to_string_lossy(),
                std::fs::read_to_string(&media_main).unwrap(),
            )
            .partitioned("media-input")
            .with_root(media_main.to_string_lossy(), "media", "Example.Media"),
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
fn canonical_interface_root_reconciles_header_and_helper_typedefs() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-interface-root-aliases-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let helper = scratch.join("d3d9helper.h");
    let shared = scratch.join("d3d9.h");
    let direct3d = scratch.join("direct3d.cpp");
    let media = scratch.join("media.cpp");
    let interfaces = [
        ("Device9", "DEVICE9", "d0223b96-bf7a-43fd-92bd-a43b0d82b9eb"),
        (
            "Surface9",
            "SURFACE9",
            "0cfbaf3a-9ff6-429a-99b3-a2796af8b89b",
        ),
    ];
    let mut helper_source = "#ifndef D3D9HELPER_H\n#define D3D9HELPER_H\n".to_string();
    let mut shared_source = "#ifndef D3D9_H\n#define D3D9_H\n".to_string();
    let mut media_source = "#include \"d3d9helper.h\"\n".to_string();
    for (suffix, alias_suffix, uuid) in interfaces {
        for source in [&mut helper_source, &mut shared_source] {
            source.push_str(&format!(
                "struct __declspec(uuid(\"{uuid}\")) IDirect3D{suffix};\n\
                 typedef struct IDirect3D{suffix} IDirect3D{suffix};\n\
                 typedef IDirect3D{suffix} *LPDIRECT3D{alias_suffix};\n\
                 typedef IDirect3D{suffix} *PDIRECT3D{alias_suffix};\n"
            ));
        }
        shared_source.push_str(&format!(
            "struct __declspec(uuid(\"{uuid}\")) IDirect3D{suffix} {{ \
             virtual void Present() = 0; }};\n"
        ));
        media_source.push_str(&format!(
            "extern \"C\" void Use{suffix}(IDirect3D{suffix}* value, \
             LPDIRECT3D{alias_suffix} legacy, PDIRECT3D{alias_suffix} pointer);\n"
        ));
    }
    helper_source.push_str("#endif\n");
    shared_source.push_str("#endif\n");
    std::fs::write(&helper, helper_source).unwrap();
    std::fs::write(&shared, shared_source).unwrap();
    std::fs::write(
        &direct3d,
        "#include \"d3d9helper.h\"\n#include \"d3d9.h\"\n",
    )
    .unwrap();
    std::fs::write(&media, media_source).unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned(
        [
            Input::new(
                direct3d.to_string_lossy(),
                std::fs::read_to_string(&direct3d).unwrap(),
            )
            .partitioned("direct3d-input")
            .with_root(shared.to_string_lossy(), "direct3d", "Example.Direct3D9")
            .with_root(helper.to_string_lossy(), "direct3d", "Example.Direct3D9"),
            Input::new(
                media.to_string_lossy(),
                std::fs::read_to_string(&media).unwrap(),
            )
            .partitioned("media-input")
            .with_root(media.to_string_lossy(), "media", "Example.Media"),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc", &include],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();
    let direct3d = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Direct3D9")
        .unwrap()
        .1;
    let media = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Media")
        .unwrap()
        .1;

    for (suffix, alias_suffix, _) in interfaces {
        assert_eq!(
            direct3d
                .matches(&format!("interface IDirect3D{suffix}"))
                .count(),
            1
        );
        assert_eq!(
            direct3d
                .matches(&format!("type LPDIRECT3D{alias_suffix}"))
                .count(),
            0
        );
        assert_eq!(
            direct3d
                .matches(&format!("type PDIRECT3D{alias_suffix}"))
                .count(),
            0
        );
        assert!(
            media.contains(&format!(
                "fn Use{suffix}(value: *mut Example::Direct3D9::IDirect3D{suffix}, \
                 legacy: Example::Direct3D9::IDirect3D{suffix}, \
                 pointer: Example::Direct3D9::IDirect3D{suffix})"
            )),
            "{media}"
        );
    }

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

    assert!(
        error.contains(
            "owner-excluded local type `HIDDEN` in partition `hidden` namespace \
             `Example.Hidden` is required without a retained public alias"
        ),
        "{error}"
    );
}

#[test]
fn excluded_native_tag_can_back_retained_public_aliases() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new(
            "dvdmedia.h",
            "typedef unsigned char UCHAR;\n\
             typedef struct _DVD_REGION { UCHAR copy; UCHAR system; } \
             DVD_REGION, *PDVD_REGION;\n",
        )
        .partitioned("media-input")
        .with_root_partition(
            "dvdmedia.h",
            RootPartition::new("media", "Example.Media").with_exclusion("_DVD_REGION"),
        )],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let partition = partitions.values().next().unwrap();

    assert!(partition.contains("struct DVD_REGION"), "{partition}");
    assert!(
        partition.contains("type PDVD_REGION = *mut DVD_REGION"),
        "{partition}"
    );
    assert!(!partition.contains("struct _DVD_REGION"), "{partition}");
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

#[test]
fn owner_u32_type_override_does_not_change_common_emission() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new("value.h", "typedef int FORCED_UINT;\n")
            .partitioned("value-input")
            .with_root_partition(
                "value.h",
                RootPartition::new("value", "Example.Value").with_u32_type("FORCED_UINT"),
            )],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let common = snapshot.emit_with_options(&options).unwrap();
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();
    let partition = partitions.values().next().unwrap();

    assert!(common.contains("type FORCED_UINT = i32"), "{common}");
    assert!(partition.contains("type FORCED_UINT = u32"), "{partition}");
}

#[test]
fn owner_u32_type_override_updates_unowned_same_source_copies() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-u32-shared-source-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let types = scratch.join("d3d9types.h");
    let direct3d = scratch.join("direct3d.cpp");
    let media = scratch.join("media.cpp");
    std::fs::write(&types, "typedef int D3DFORMAT;\n").unwrap();
    std::fs::write(&direct3d, "#include \"d3d9types.h\"\n").unwrap();
    std::fs::write(
        &media,
        "#include \"d3d9types.h\"\nextern \"C\" void UseFormat(D3DFORMAT value);\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract_partitioned(
        [
            Input::new(
                direct3d.to_string_lossy(),
                std::fs::read_to_string(&direct3d).unwrap(),
            )
            .partitioned("direct3d-input")
            .with_root_partition(
                types.to_string_lossy(),
                RootPartition::new("direct3d", "Example.Direct3D9").with_u32_type("D3DFORMAT"),
            ),
            Input::new(
                media.to_string_lossy(),
                std::fs::read_to_string(&media).unwrap(),
            )
            .partitioned("media-input")
            .with_root(media.to_string_lossy(), "media", "Example.Media"),
        ],
        &["-x", "c++", &include],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();
    let direct3d = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Direct3D9")
        .unwrap()
        .1;
    let media = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Media")
        .unwrap()
        .1;

    assert!(direct3d.contains("type D3DFORMAT = u32"), "{direct3d}");
    assert!(
        media.contains("fn UseFormat(value: Example::Direct3D9::D3DFORMAT)"),
        "{media}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn owner_flags_force_unsigned_flag_enum_projection() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new(
            "flags.h",
            "typedef enum FORCED_FLAGS { FORCED_NONE = 0, FORCED_ALL = -1 } FORCED_FLAGS;\n",
        )
        .partitioned("flags-input")
        .with_root_partition(
            "flags.h",
            RootPartition::new("flags", "Example.Flags")
                .with_flags("FORCED_FLAGS")
                .with_remap("FORCED_FLAGS", "FLAGS"),
        )],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let partition = partitions.values().next().unwrap();

    assert!(
        partition.contains("#[repr(u32)]\n        #[flags]\n        enum FLAGS"),
        "{partition}"
    );
    assert!(partition.contains("FORCED_ALL = 4294967295"), "{partition}");
}

#[test]
fn owner_preserves_automatic_function_pointer_level() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new(
            "identity.h",
            "typedef void IDENTITY_CALLBACK(int value);\n\
             extern \"C\" void SetIdentity(IDENTITY_CALLBACK callback);\n",
        )
        .partitioned("identity-input")
        .with_root_partition(
            "identity.h",
            RootPartition::new("identity", "Example.Identity")
                .with_preserved_auto_function_pointer_level("IDENTITY_CALLBACK"),
        )],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("identity.dll");
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();
    let partition = partitions.values().next().unwrap();

    assert!(
        partition.contains("fn SetIdentity(callback: *mut IDENTITY_CALLBACK)"),
        "{partition}"
    );
}

#[test]
fn owner_excludes_empty_records_only_in_partitioned_emission() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new(
            "records.h",
            "struct EMPTY_RECORD {};\nstruct FULL_RECORD { int value; };\n",
        )
        .partitioned("records-input")
        .with_root_partition(
            "records.h",
            RootPartition::new("records", "Example.Records").exclude_empty_records(),
        )],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let common = snapshot.emit_with_options(&options).unwrap();
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();
    let partition = partitions.values().next().unwrap();

    assert!(common.contains("struct EMPTY_RECORD"), "{common}");
    assert!(!partition.contains("struct EMPTY_RECORD"), "{partition}");
    assert!(partition.contains("struct FULL_RECORD"), "{partition}");
}

#[test]
fn partitioned_input_can_enable_cpp20() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new(
            "cpp20.h",
            "#if __cplusplus < 202002L\n#error C++20 required\n#endif\n\
             typedef unsigned CPP20_VALUE;\n",
        )
        .partitioned("cpp20-input")
        .with_cpp20()
        .with_root("cpp20.h", "cpp20", "Example.Cpp20")],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert!(
        partitions
            .values()
            .next()
            .unwrap()
            .contains("type CPP20_VALUE = u32")
    );
}

#[test]
fn partitioned_input_can_add_include_directory() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-partition-include-{}",
        std::process::id()
    ));
    let include = scratch.join("dxcore");
    std::fs::create_dir_all(&include).unwrap();
    let header = include.join("dxcore.h");
    std::fs::write(
        &header,
        "typedef struct DXCORE_VALUE { unsigned value; } DXCORE_VALUE;\n",
    )
    .unwrap();
    let snapshot = extract_partitioned(
        [Input::new("dxcore-main.cpp", "#include <dxcore.h>\n")
            .partitioned("dxcore-input")
            .with_include_directory(include.to_string_lossy())
            .with_root(
                header.to_string_lossy(),
                "dxcore",
                "Example.Graphics.Dxcore",
            )],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert!(
        partitions
            .values()
            .next()
            .unwrap()
            .contains("struct DXCORE_VALUE")
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn namespace_authorities_route_opposite_owner_policies() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [
            Input::new(
                "ddraw.h",
                "typedef struct _DDPIXELFORMAT { int directdraw; } \
                 DDPIXELFORMAT, *LPDDPIXELFORMAT;\n\
                 typedef struct _DDVIDEOPORTCONNECT { int directdraw; } \
                 DDVIDEOPORTCONNECT, *LPDDVIDEOPORTCONNECT;\n",
            )
            .partitioned("directdraw-input")
            .with_root("ddraw.h", "directdraw", "Example.DirectDraw"),
            Input::new(
                "RecompiledIdlHeaders/shared/ksmedia.h",
                "typedef struct _DDPIXELFORMAT { long long kernel; } \
                 DDPIXELFORMAT, *LPDDPIXELFORMAT;\n\
                 typedef struct _DDVIDEOPORTCONNECT { long long kernel; } \
                 DDVIDEOPORTCONNECT, *LPDDVIDEOPORTCONNECT;\n\
                 struct __declspec(uuid(\"28f54685-06fd-11d2-b27a-00a0c9223196\")) \
                 IKsControl { virtual void Kernel() = 0; };\n",
            )
            .partitioned("kernel-input")
            .with_root_partition(
                "shared/ksmedia.h",
                RootPartition::new("kernel", "Example.KernelStreaming")
                    .with_exclusion("_DDPIXELFORMAT")
                    .with_exclusion("_DDVIDEOPORTCONNECT")
                    .with_exclusion("IKsControl"),
            ),
            Input::new(
                "audio.h",
                "struct __declspec(uuid(\"28f54685-06fd-11d2-b27a-00a0c9223196\")) \
                 IKsControl { virtual void Audio() = 0; };\n\
                 extern \"C\" void UseKs(IKsControl* value);\n",
            )
            .partitioned("audio-input")
            .with_root("audio.h", "audio", "Example.Audio"),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let authorities =
        NamespaceAuthorities::new().with_exact("IKsControl", "Example.KernelStreaming");
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let partitions = snapshot
        .emit_partitioned_with_options_and_authorities(&options, &authorities)
        .unwrap();
    let directdraw = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.DirectDraw")
        .unwrap()
        .1;
    let kernel = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.KernelStreaming")
        .unwrap()
        .1;
    let audio = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Audio")
        .unwrap()
        .1;

    assert!(directdraw.contains("struct DDPIXELFORMAT"), "{directdraw}");
    assert!(
        directdraw.contains("struct DDVIDEOPORTCONNECT"),
        "{directdraw}"
    );
    assert!(!directdraw.contains("kernel: i64"), "{directdraw}");
    assert!(!kernel.contains("DDPIXELFORMAT"), "{kernel}");
    assert!(!kernel.contains("DDVIDEOPORTCONNECT"), "{kernel}");
    assert!(kernel.contains("interface IKsControl"), "{kernel}");
    assert!(
        audio.contains("fn UseKs(value: Example::KernelStreaming::IKsControl)"),
        "{audio}"
    );
}

#[test]
fn tagged_source_fact_wins_over_transitive_copy_before_shape_selection() {
    helpers::ensure_libclang();

    let scratch =
        std::env::temp_dir().join(format!("windows-clang-owned-source-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let header = scratch.join("ddraw.h");
    std::fs::write(
        &header,
        "typedef struct _MDL { unsigned value; } _MDL;\n\
         typedef struct _DDSURFACEDESC { _MDL mdl; } _DDSURFACEDESC;\n",
    )
    .unwrap();
    let include = format!("#include \"{}\"\n", header.to_string_lossy());
    let snapshot = extract_partitioned(
        [
            Input::new("owned.cpp", &include)
                .partitioned("directdraw-input")
                .with_root_partition(
                    header.to_string_lossy(),
                    RootPartition::new("directdraw", "Example.DirectDraw")
                        .with_remap("_MDL", "DDMDL")
                        .with_remap("_DDSURFACEDESC", "DDSURFACEDESC"),
                ),
            Input::new("transitive.cpp", &include).partitioned("transitive-input"),
        ],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let directdraw = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.DirectDraw")
        .unwrap()
        .1;

    assert!(directdraw.contains("struct DDSURFACEDESC"), "{directdraw}");
    assert!(directdraw.contains("mdl: DDMDL"), "{directdraw}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn unowned_scalar_typedef_dependencies_are_inlined() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-internal-alias-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let common = scratch.join("common.h");
    let api = scratch.join("api.h");
    std::fs::write(
        &common,
        "typedef unsigned long ACCESS_MASK;\n\
         typedef ACCESS_MASK *PACCESS_MASK;\n",
    )
    .unwrap();
    std::fs::write(
        &api,
        format!(
            "#include \"{}\"\n\
             typedef struct OWNED_ACCESS {{ ACCESS_MASK access; PACCESS_MASK pointer; }} \
             OWNED_ACCESS;\n",
            common.to_string_lossy()
        ),
    )
    .unwrap();
    let snapshot = extract_partitioned(
        [Input::new(
            "owned.cpp",
            format!("#include \"{}\"\n", api.to_string_lossy()),
        )
        .partitioned("owned-input")
        .with_root(api.to_string_lossy(), "owned", "Example.Owned")],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let owned = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Owned")
        .unwrap()
        .1;

    assert!(owned.contains("access: u32"), "{owned}");
    assert!(owned.contains("pointer: *mut u32"), "{owned}");
    assert!(!owned.contains("type ACCESS_MASK"), "{owned}");
    assert!(!owned.contains("type PACCESS_MASK"), "{owned}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn macro_generated_declaration_uses_expansion_root_owner() {
    helpers::ensure_libclang();

    let scratch =
        std::env::temp_dir().join(format!("windows-clang-macro-owner-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let macros = scratch.join("macros.h");
    let api = scratch.join("BdaTypes.h");
    let configured_root = scratch.join("bdatypes.h");
    std::fs::write(&macros, "#define ENUM enum\n").unwrap();
    std::fs::write(
        &api,
        format!(
            "#include \"{}\"\n\
             ENUM ApplicationTypeType {{ ApplicationTypeNone = 0 }};\n\
             typedef struct ApplicationHolder {{ ApplicationTypeType value; }} \
             ApplicationHolder;\n",
            macros.to_string_lossy()
        ),
    )
    .unwrap();
    let snapshot = extract_partitioned(
        [Input::new(
            "dshow.cpp",
            format!("#include \"{}\"\n", api.to_string_lossy()),
        )
        .partitioned("dshow-input")
        .with_root(
            configured_root.to_string_lossy(),
            "dshow",
            "Example.Media.DirectShow",
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let dshow = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Media.DirectShow")
        .unwrap()
        .1;

    assert!(dshow.contains("enum ApplicationTypeType"), "{dshow}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn namespace_authorities_support_wildcards_and_namespaces_without_partitions() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new(
            "dpi.h",
            "typedef unsigned DPI_VALUE;\n\
             #define DPI_DEFAULT 96\n\
             extern \"C\" DPI_VALUE DPI_GetValue(void);\n",
        )
        .partitioned("dpi-input")
        .with_root("dpi.h", "ui", "Example.UI")],
        &["-x", "c++"],
    )
    .unwrap();
    let authorities = NamespaceAuthorities::new()
        .with_exact("DPI_VALUE", "Example.UI.HiDpi")
        .with_wildcard("DPI_*", "Example.UI.HiDpi");
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("dpi.dll");
    let partitions = snapshot
        .emit_partitioned_with_options_and_authorities(&options, &authorities)
        .unwrap();
    let dpi = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.UI.HiDpi")
        .unwrap()
        .1;

    assert!(dpi.contains("type DPI_VALUE = u32"), "{dpi}");
    assert!(dpi.contains("const DPI_DEFAULT"), "{dpi}");
    assert!(dpi.contains("fn DPI_GetValue() -> DPI_VALUE"), "{dpi}");
}

#[test]
fn namespace_authority_routes_untagged_transitive_type() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-authority-input-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let common = scratch.join("minwinbase.h");
    let api = scratch.join("api.h");
    std::fs::write(
        &common,
        "typedef struct CRITICAL_SECTION { unsigned lock_count; } CRITICAL_SECTION;\n",
    )
    .unwrap();
    std::fs::write(
        &api,
        format!(
            "#include \"{}\"\n\
             typedef struct OWNED_LOCK {{ CRITICAL_SECTION section; }} OWNED_LOCK;\n",
            common.to_string_lossy()
        ),
    )
    .unwrap();
    let snapshot = extract_partitioned(
        [Input::new(
            "dshow.cpp",
            format!("#include \"{}\"\n", api.to_string_lossy()),
        )
        .partitioned("dshow-input")
        .with_root(api.to_string_lossy(), "dshow", "Example.Media.DirectShow")],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let authorities =
        NamespaceAuthorities::new().with_exact("CRITICAL_SECTION", "Example.System.Threading");
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options_and_authorities(
            &EmitOptions::new("Example.Common", &references),
            &authorities,
        )
        .unwrap();
    let (partition, threading) = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.System.Threading")
        .unwrap();

    assert_eq!(partition.partition, "dshow-input");
    assert_eq!(
        partition.header,
        common.to_string_lossy().replace('\\', "/")
    );
    assert!(threading.contains("struct CRITICAL_SECTION"), "{threading}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn midl_helper_functions_use_input_order_across_headers() {
    helpers::ensure_libclang();

    let scratch =
        std::env::temp_dir().join(format!("windows-clang-midl-helpers-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let wtypes = scratch.join("wtypes.h");
    std::fs::write(&wtypes, "typedef unsigned short *BSTR;\n").unwrap();
    let inputs = ["azroles.h", "bitscfg.h", "qmgr.h"]
        .into_iter()
        .enumerate()
        .map(|(index, header)| {
            let path = scratch.join(header);
            std::fs::write(
                &path,
                format!(
                    "#include \"{}\"\n\
                     extern \"C\" void BSTR_UserFree(unsigned long *flags, BSTR *value);\n\
                     extern \"C\" unsigned long BSTR_UserSize(unsigned long *flags, \
                     unsigned long offset, BSTR *value);\n",
                    wtypes.to_string_lossy()
                ),
            )
            .unwrap();
            Input::new(
                format!("{header}.cpp"),
                format!("#include \"{}\"\n", path.to_string_lossy()),
            )
            .partitioned(format!("{header}-input"))
            .with_root(
                path.to_string_lossy(),
                format!("{header}-partition"),
                format!("Example.Header{index}"),
            )
        })
        .collect::<Vec<_>>();
    let snapshot =
        extract_partitioned(inputs, &["-x", "c++", "--target=x86_64-pc-windows-msvc"]).unwrap();
    let authorities = NamespaceAuthorities::new()
        .with_exact("BSTR_UserFree", "Example.System.Com.Marshal")
        .with_exact("BSTR_UserSize", "Example.System.Com.Marshal");
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("oleaut32.dll");
    let partitions = snapshot
        .emit_partitioned_with_options_and_authorities(&options, &authorities)
        .unwrap();
    let marshal = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.System.Com.Marshal")
        .unwrap()
        .1;

    assert_eq!(marshal.matches("fn BSTR_UserFree").count(), 1, "{marshal}");
    assert_eq!(marshal.matches("fn BSTR_UserSize").count(), 1, "{marshal}");

    let differing = extract_partitioned(
        [
            Input::new(
                "first.h",
                "extern \"C\" void BSTR_UserFree(unsigned long *flags, void **first_value);\n",
            )
            .partitioned("first-input"),
            Input::new(
                "second.h",
                "extern \"C\" void BSTR_UserFree(unsigned long *flags, unsigned **second_value);\n",
            )
            .partitioned("second-input"),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap()
    .emit_partitioned_with_options_and_authorities(
        &options,
        &NamespaceAuthorities::new().with_exact("BSTR_UserFree", "Example.System.Com.Marshal"),
    )
    .unwrap();
    let marshal = differing
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.System.Com.Marshal")
        .unwrap()
        .1;
    let function = marshal
        .lines()
        .find(|line| line.contains(" fn BSTR_UserFree("))
        .unwrap();

    assert!(function.contains("first_value"), "{marshal}");
    assert!(!function.contains("second_value"), "{marshal}");
    assert!(
        marshal.contains("declaration selected=true input=\"first.h\""),
        "{marshal}"
    );
    assert!(
        marshal.contains("declaration selected=false input=\"second.h\""),
        "{marshal}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn macro_aliased_functions_preserve_distinct_link_names() {
    helpers::ensure_libclang();

    let scratch =
        std::env::temp_dir().join(format!("windows-clang-psapi-alias-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let common = scratch.join("common.h");
    let header = scratch.join("psapi.h");
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
    let v1 = RootPartition::new("psapi1", "Example.System.ProcessStatus")
        .with_library("EmptyWorkingSet", "PSAPI.dll")
        .with_library("EnumProcesses", "PSAPI.dll");
    let v2 = RootPartition::new("psapi2", "Example.System.ProcessStatus")
        .with_library("K32EmptyWorkingSet", "KERNEL32.dll")
        .with_library("K32EnumProcesses", "KERNEL32.dll");
    let snapshot = extract_partitioned(
        [
            Input::new("psapi1.cpp", &include)
                .partitioned("psapi1-input")
                .with_root_partition(header.to_string_lossy(), v1),
            Input::new(
                "psapi2.cpp",
                format!(
                    "#define EmptyWorkingSet K32EmptyWorkingSet\n\
                     #define EnumProcesses K32EnumProcesses\n\
                     {include}"
                ),
            )
            .partitioned("psapi2-input")
            .with_root_partition(header.to_string_lossy(), v2),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
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
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let process_status = partitions.values().cloned().collect::<Vec<_>>().join("\n");

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
fn duplicate_macro_constants_follow_distinct_namespace_routes() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-locale-constant-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let ntdef = scratch.join("a-ntdef.h");
    let winnt = scratch.join("z-winnt.h");
    let source = "#define LOCALE_CUSTOM_DEFAULT 0x0c00\n";
    std::fs::write(&ntdef, source).unwrap();
    std::fs::write(&winnt, source).unwrap();
    let inputs = vec![
        Input::new(
            "intl.cpp",
            format!("#include \"{}\"\n", winnt.to_string_lossy()),
        )
        .partitioned("intl-input")
        .with_root(winnt.to_string_lossy(), "intl", "Example.Globalization"),
        Input::new(
            "kernel.cpp",
            format!("#include \"{}\"\n", ntdef.to_string_lossy()),
        )
        .partitioned("kernel-input")
        .with_root(ntdef.to_string_lossy(), "kernel", "Example.System.Kernel"),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let partitions = serial.emit_partitioned_with_options(&options).unwrap();
    assert_eq!(
        partitions,
        parallel.emit_partitioned_with_options(&options).unwrap()
    );
    let output = partitions.values().cloned().collect::<Vec<_>>().join("\n");

    assert_eq!(
        output.matches("const LOCALE_CUSTOM_DEFAULT").count(),
        2,
        "{output}"
    );
    assert!(
        partitions
            .keys()
            .any(|partition| partition.namespace == "Example.Globalization"),
        "{partitions:#?}"
    );
    assert!(
        partitions
            .keys()
            .any(|partition| partition.namespace == "Example.System.Kernel"),
        "{partitions:#?}"
    );
    let mut reversed = inputs;
    reversed.reverse();
    assert_eq!(
        partitions,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(2),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap()
    );

    let different = windows_clang::extract(
        [
            Input::new("first.h", "#define LOCALE_CUSTOM_DEFAULT 0x0c00\n"),
            Input::new("second.h", "#define LOCALE_CUSTOM_DEFAULT 0x1000\n"),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap()
    .emit("Example")
    .unwrap_err();

    assert!(
        different
            .to_string()
            .contains("ambiguous constant root `LOCALE_CUSTOM_DEFAULT`"),
        "{different}"
    );

    let different_type = windows_clang::extract(
        [
            Input::new("first.h", "#define DIFFERENT_TYPE ((unsigned long)1)\n"),
            Input::new(
                "second.h",
                "#define DIFFERENT_TYPE ((unsigned long long)1)\n",
            ),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap()
    .emit("Example")
    .unwrap_err();

    assert!(
        different_type
            .to_string()
            .contains("ambiguous constant root `DIFFERENT_TYPE`"),
        "{different_type}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn differing_constant_roots_follow_namespace_authority_and_remaps() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-routed-constant-roots-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let first_header = scratch.join("first.h");
    let second_header = scratch.join("second.h");
    std::fs::write(
        &first_header,
        "#define ROUTED_CONSTANT ((unsigned long)1)\n",
    )
    .unwrap();
    std::fs::write(
        &second_header,
        "#define ROUTED_CONSTANT ((unsigned long long)2)\n",
    )
    .unwrap();
    let inputs = vec![
        Input::new(
            "first.cpp",
            format!("#include \"{}\"\n", first_header.display()),
        )
        .partitioned("first-input")
        .with_root(first_header.to_string_lossy(), "first", "Example.First"),
        Input::new(
            "second.cpp",
            format!("#include \"{}\"\n", second_header.display()),
        )
        .partitioned("second-input")
        .with_root(second_header.to_string_lossy(), "second", "Example.Second"),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());

    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let expected = serial
        .clone()
        .emit_partitioned_with_options(&options)
        .unwrap();
    assert_eq!(
        expected,
        parallel
            .clone()
            .emit_partitioned_with_options(&options)
            .unwrap()
    );
    let first = expected
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.First")
        .map(|(_, rdl)| rdl)
        .unwrap();
    let second = expected
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Second")
        .map(|(_, rdl)| rdl)
        .unwrap();
    assert!(first.contains("const ROUTED_CONSTANT: u32 = 1;"), "{first}");
    assert!(
        second.contains("const ROUTED_CONSTANT: u64 = 2;"),
        "{second}"
    );

    let mut reversed = inputs;
    reversed.reverse();
    assert_eq!(
        expected,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(2),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap()
    );

    let authority = NamespaceAuthorities::new().with_exact("ROUTED_CONSTANT", "Example.Shared");
    let authority_error = serial
        .emit_partitioned_with_options_and_authorities(&options, &authority)
        .unwrap_err()
        .to_string();
    assert!(
        authority_error.contains("ambiguous constant root `ROUTED_CONSTANT`"),
        "{authority_error}"
    );
    assert_eq!(
        authority_error,
        parallel
            .emit_partitioned_with_options_and_authorities(&options, &authority)
            .unwrap_err()
            .to_string()
    );

    let remapped_inputs = vec![
        Input::new(
            "first.cpp",
            format!("#include \"{}\"\n", first_header.display()),
        )
        .partitioned("first-input")
        .with_root(first_header.to_string_lossy(), "shared", "Example.Shared"),
        Input::new(
            "second.cpp",
            format!("#include \"{}\"\n", second_header.display()),
        )
        .partitioned("second-input")
        .with_root_partition(
            second_header.to_string_lossy(),
            RootPartition::new("shared", "Example.Shared")
                .with_remap("ROUTED_CONSTANT", "RENAMED_CONSTANT"),
        ),
    ];
    let remapped = extract_partitioned_with_options(
        remapped_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap()
    .emit_partitioned_with_options(&options)
    .unwrap();
    let output = remapped.values().cloned().collect::<Vec<_>>().join("\n");
    assert!(
        output.contains("const ROUTED_CONSTANT: u32 = 1;"),
        "{output}"
    );
    assert!(
        output.contains("const RENAMED_CONSTANT: u64 = 2;"),
        "{output}"
    );
    let mut reversed = remapped_inputs;
    reversed.reverse();
    assert_eq!(
        remapped,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(2),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn same_namespace_constant_roots_coalesce_or_conflict() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-same-route-constant-roots-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let first_header = scratch.join("first.h");
    let second_header = scratch.join("second.h");
    std::fs::write(&first_header, "#define SHARED_CONSTANT 1\n").unwrap();
    std::fs::write(&second_header, "#define SHARED_CONSTANT 2\n").unwrap();
    let conflicting_inputs = vec![
        Input::new(
            "first.cpp",
            format!("#include \"{}\"\n", first_header.display()),
        )
        .partitioned("first-input")
        .with_root(first_header.to_string_lossy(), "shared", "Example.Shared"),
        Input::new(
            "second.cpp",
            format!("#include \"{}\"\n", second_header.display()),
        )
        .partitioned("second-input")
        .with_root(second_header.to_string_lossy(), "shared", "Example.Shared"),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        conflicting_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        conflicting_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let error = serial
        .emit_partitioned_with_options(&options)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("ambiguous constant root `SHARED_CONSTANT`"),
        "{error}"
    );
    assert_eq!(
        error,
        parallel
            .emit_partitioned_with_options(&options)
            .unwrap_err()
            .to_string()
    );
    let mut reversed = conflicting_inputs;
    reversed.reverse();
    assert_eq!(
        error,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(2),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap_err()
        .to_string()
    );

    let common_header = scratch.join("common.h");
    std::fs::write(&common_header, "#define COALESCED_CONSTANT 3\n").unwrap();
    let include = format!("#include \"{}\"\n", common_header.display());
    let equal_inputs = vec![
        Input::new("first.cpp", include.clone())
            .partitioned("first-input")
            .with_root(common_header.to_string_lossy(), "shared", "Example.Shared"),
        Input::new("second.cpp", include)
            .partitioned("second-input")
            .with_root(common_header.to_string_lossy(), "shared", "Example.Shared"),
    ];
    let serial = extract_partitioned_with_options(
        equal_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        equal_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());
    let expected = serial.emit_partitioned_with_options(&options).unwrap();
    assert_eq!(
        expected,
        parallel.emit_partitioned_with_options(&options).unwrap()
    );
    let output = expected.values().cloned().collect::<Vec<_>>().join("\n");
    assert_eq!(
        output.matches("const COALESCED_CONSTANT").count(),
        1,
        "{output}"
    );
    let mut reversed = equal_inputs;
    reversed.reverse();
    assert_eq!(
        expected,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(2),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn nonroot_header_constants_use_unique_input_owner_routes() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-fallback-constant-routes-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let dependency = scratch.join("dependency.h");
    let first_public = scratch.join("first-public.h");
    let second_public = scratch.join("second-public.h");
    std::fs::write(&dependency, "#define FALLBACK_CONSTANT 1\n").unwrap();
    std::fs::write(
        &first_public,
        format!(
            "#include \"{}\"\n\
             #define W32M(text) __attribute__((annotate(text)))\n\
             enum W32M(\"win32metadata:associated_constant=FALLBACK_CONSTANT\") \
                 FIRST_KIND : unsigned {{ FIRST_NONE = 0 }};\n",
            dependency.display()
        ),
    )
    .unwrap();
    std::fs::write(
        &second_public,
        format!(
            "#include \"{}\"\n\
             #define W32M(text) __attribute__((annotate(text)))\n\
             enum W32M(\"win32metadata:associated_constant=FALLBACK_CONSTANT\") \
                 SECOND_KIND : unsigned {{ SECOND_NONE = 0 }};\n",
            dependency.display()
        ),
    )
    .unwrap();
    let inputs = vec![
        Input::new(
            "first.cpp",
            format!("#include \"{}\"\n", first_public.display()),
        )
        .partitioned("first-input")
        .with_root(first_public.to_string_lossy(), "first", "Example.First"),
        Input::new(
            "second.cpp",
            format!("#include \"{}\"\n", second_public.display()),
        )
        .partitioned("second-input")
        .with_root(second_public.to_string_lossy(), "second", "Example.Second"),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let expected = serial.emit_partitioned_with_options(&options).unwrap();
    assert_eq!(
        expected,
        parallel.emit_partitioned_with_options(&options).unwrap()
    );
    let first = expected
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.First")
        .map(|(_, rdl)| rdl)
        .unwrap();
    let second = expected
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Second")
        .map(|(_, rdl)| rdl)
        .unwrap();
    assert!(
        first.contains("const FALLBACK_CONSTANT: i32 = 1;"),
        "{first}"
    );
    assert!(
        second.contains("const FALLBACK_CONSTANT: i32 = 1;"),
        "{second}"
    );
    let mut reversed = inputs;
    reversed.reverse();
    assert_eq!(
        expected,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(2),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn routed_constants_preserve_same_namespace_value_collisions() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-routed-constant-value-collision-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let function_header = scratch.join("function.h");
    let first_constant = scratch.join("first-constant.h");
    let second_constant = scratch.join("second-constant.h");
    std::fs::write(
        &function_header,
        "extern \"C\" int VALUE_COLLISION(void);\n",
    )
    .unwrap();
    std::fs::write(&first_constant, "#define VALUE_COLLISION 1\n").unwrap();
    std::fs::write(&second_constant, "#define VALUE_COLLISION 2\n").unwrap();
    let inputs = vec![
        Input::new(
            "function.cpp",
            format!("#include \"{}\"\n", function_header.display()),
        )
        .partitioned("function-input")
        .with_root(function_header.to_string_lossy(), "first", "Example.First"),
        Input::new(
            "first.cpp",
            format!("#include \"{}\"\n", first_constant.display()),
        )
        .partitioned("first-input")
        .with_root(first_constant.to_string_lossy(), "first", "Example.First"),
        Input::new(
            "second.cpp",
            format!("#include \"{}\"\n", second_constant.display()),
        )
        .partitioned("second-input")
        .with_root(
            second_constant.to_string_lossy(),
            "second",
            "Example.Second",
        ),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(3),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("example.dll");
    let error = serial
        .emit_partitioned_with_options(&options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("duplicate planned name"), "{error}");
    assert_eq!(
        error,
        parallel
            .emit_partitioned_with_options(&options)
            .unwrap_err()
            .to_string()
    );
    let mut reversed = inputs;
    reversed.reverse();
    assert_eq!(
        error,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(3),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap_err()
        .to_string()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn routed_associated_constants_survive_public_exclusion() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-routed-associated-constants-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let dependency = scratch.join("dependency.h");
    let first_header = scratch.join("first.h");
    let second_header = scratch.join("second.h");
    std::fs::write(&dependency, "#define ASSOCIATED_VALUE 1\n").unwrap();
    std::fs::write(
        &first_header,
        format!(
            "#include \"{}\"\n\
             #define W32M(text) __attribute__((annotate(text)))\n\
             enum W32M(\"win32metadata:associated_constant=ASSOCIATED_VALUE\") \
                 FIRST_KIND : unsigned {{ FIRST_NONE = 0 }};\n",
            dependency.display()
        ),
    )
    .unwrap();
    std::fs::write(
        &second_header,
        format!(
            "#include \"{}\"\n\
             #define W32M(text) __attribute__((annotate(text)))\n\
             enum W32M(\"win32metadata:associated_constant=ASSOCIATED_VALUE\") \
                 SECOND_KIND : unsigned {{ SECOND_NONE = 0 }};\n",
            dependency.display()
        ),
    )
    .unwrap();
    let inputs = vec![
        Input::new(
            "first.cpp",
            format!("#include \"{}\"\n", first_header.display()),
        )
        .partitioned("first-input")
        .with_root(first_header.to_string_lossy(), "first", "Example.First"),
        Input::new(
            "second.cpp",
            format!("#include \"{}\"\n", second_header.display()),
        )
        .partitioned("second-input")
        .with_root(second_header.to_string_lossy(), "second", "Example.Second"),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());
    let references = BTreeMap::new();
    let excluded = BTreeSet::from(["ASSOCIATED_VALUE".to_string()]);
    let mut options = EmitOptions::new("Example.Common", &references);
    options.excluded_constants = Some(&excluded);
    let expected = serial.emit_partitioned_with_options(&options).unwrap();
    assert_eq!(
        expected,
        parallel.emit_partitioned_with_options(&options).unwrap()
    );
    let first = expected
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.First")
        .map(|(_, rdl)| rdl)
        .unwrap();
    assert!(
        first.contains("const ASSOCIATED_VALUE: i32 = 1;"),
        "{first}"
    );
    let second = expected
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Second")
        .map(|(_, rdl)| rdl)
        .unwrap();
    assert!(
        second.contains("const ASSOCIATED_VALUE: i32 = 1;"),
        "{second}"
    );
    let mut reversed = inputs;
    reversed.reverse();
    assert_eq!(
        expected,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(2),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn unowned_input_source_constant_defers_to_owned_header_root() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-source-constant-override-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let header = scratch.join("public.h");
    std::fs::write(&header, "#define COMPILE_CONTROL 1\n").unwrap();
    let inputs = vec![
        Input::new("source.cpp", "#define COMPILE_CONTROL 0\n").partitioned("source-input"),
        Input::new("header.cpp", format!("#include \"{}\"\n", header.display()))
            .partitioned("header-input")
            .with_root(
                header.to_string_lossy(),
                "header",
                "Example.HeaderConstants",
            ),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());

    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let expected = serial.emit_partitioned_with_options(&options).unwrap();
    assert_eq!(
        expected,
        parallel.emit_partitioned_with_options(&options).unwrap()
    );
    let output = expected.values().cloned().collect::<Vec<_>>().join("\n");
    assert!(output.contains("const COMPILE_CONTROL"), "{output}");
    assert!(output.contains(" = 1;"), "{output}");
    assert!(!output.contains(" = 0;"), "{output}");
    assert!(
        expected
            .keys()
            .any(|partition| partition.namespace == "Example.HeaderConstants"),
        "{expected:#?}"
    );

    let mut reversed = inputs;
    reversed.reverse();
    let reversed = extract_partitioned_with_options(
        reversed,
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap()
    .emit_partitioned_with_options(&options)
    .unwrap();
    assert_eq!(expected, reversed);

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn unowned_input_source_constants_are_omitted_and_owned_source_is_retained() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-source-constant-eligibility-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let public_header = scratch.join("public.h");
    std::fs::write(&public_header, "typedef unsigned PUBLIC_TYPE;\n").unwrap();
    let profile_inputs = vec![
        Input::new(
            "first.cpp",
            format!(
                "#define PROFILE_SELECTOR 1\n#include \"{}\"\n",
                public_header.display()
            ),
        )
        .partitioned("first-input")
        .with_root(public_header.to_string_lossy(), "public", "Example.Public"),
        Input::new("second.cpp", "#define PROFILE_SELECTOR 2\n").partitioned("second-input"),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        profile_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        profile_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());

    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let expected = serial.emit_partitioned_with_options(&options).unwrap();
    assert_eq!(
        expected,
        parallel.emit_partitioned_with_options(&options).unwrap()
    );
    let output = expected.values().cloned().collect::<Vec<_>>().join("\n");
    assert!(output.contains("type PUBLIC_TYPE"), "{output}");
    assert!(!output.contains("PROFILE_SELECTOR"), "{output}");

    let mut reversed = profile_inputs;
    reversed.reverse();
    let reversed = extract_partitioned_with_options(
        reversed,
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap()
    .emit_partitioned_with_options(&options)
    .unwrap();
    assert_eq!(expected, reversed);

    let owned_inputs = vec![
        Input::new("owned.cpp", "const float OWNED_SOURCE_CONSTANT = 7.0f;\n")
            .partitioned("owned-input")
            .with_root_partition(
                "owned.cpp",
                RootPartition::new("owned", "Example.Owned")
                    .with_remap("OWNED_SOURCE_CONSTANT", "RENAMED_SOURCE_CONSTANT"),
            ),
        Input::new("noise.cpp", "#define UNOWNED_SOURCE_CONSTANT 9\n").partitioned("noise-input"),
    ];
    let owned = extract_partitioned_with_options(
        owned_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap()
    .emit_partitioned_with_options(&options)
    .unwrap();
    let output = owned.values().cloned().collect::<Vec<_>>().join("\n");
    assert!(
        output.contains("const RENAMED_SOURCE_CONSTANT: f32 = 7.0;"),
        "{output}"
    );
    assert!(!output.contains("const OWNED_SOURCE_CONSTANT"), "{output}");
    assert!(!output.contains("UNOWNED_SOURCE_CONSTANT"), "{output}");
    assert!(
        owned
            .keys()
            .any(|partition| partition.namespace == "Example.Owned"),
        "{owned:#?}"
    );
    let mut reversed = owned_inputs;
    reversed.reverse();
    assert_eq!(
        owned,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(2),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap()
    );

    let selected = extract_partitioned_with_options(
        [
            Input::new("selected.cpp", "#define SELECTED_SOURCE_CONSTANT 11\n")
                .partitioned("selected-input"),
        ],
        &args,
        &ExtractionOptions::new(),
    )
    .unwrap()
    .emit_partitioned_with_options_and_authorities(
        &options,
        &NamespaceAuthorities::new().with_exact("SELECTED_SOURCE_CONSTANT", "Example.Selected"),
    )
    .unwrap();
    let output = selected.values().cloned().collect::<Vec<_>>().join("\n");
    assert!(
        output.contains("const SELECTED_SOURCE_CONSTANT: i32 = 11;"),
        "{output}"
    );
    assert!(
        selected
            .keys()
            .any(|partition| partition.namespace == "Example.Selected"),
        "{selected:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn header_selected_nonroot_constant_uses_its_spelling_owner() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-nonroot-constant-owner-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let public_header = scratch.join("public.h");
    let other_header = scratch.join("other.h");
    std::fs::write(&public_header, "enum { ROUTED_CONSTANT = 1 };\n").unwrap();
    std::fs::write(&other_header, "typedef unsigned OTHER_TYPE;\n").unwrap();
    let inputs = vec![
        Input::new("source.cpp", "#define ROUTED_CONSTANT 0\n").partitioned("source-input"),
        Input::new(
            "headers.cpp",
            format!(
                "#include \"{}\"\n#include \"{}\"\n",
                public_header.display(),
                other_header.display()
            ),
        )
        .partitioned("headers-input")
        .with_root(public_header.to_string_lossy(), "public", "Example.Public")
        .with_root(other_header.to_string_lossy(), "other", "Example.Other"),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());

    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let expected = serial.emit_partitioned_with_options(&options).unwrap();
    assert_eq!(
        expected,
        parallel.emit_partitioned_with_options(&options).unwrap()
    );
    let output = expected.values().cloned().collect::<Vec<_>>().join("\n");
    assert!(
        output.contains("const ROUTED_CONSTANT: i32 = 1;"),
        "{output}"
    );
    assert!(
        expected
            .keys()
            .any(|partition| partition.namespace == "Example.Public"),
        "{expected:#?}"
    );
    assert!(
        !expected
            .keys()
            .any(|partition| partition.namespace == "Example.Other"
                && expected[partition].contains("ROUTED_CONSTANT")),
        "{expected:#?}"
    );

    let mut reversed = inputs;
    reversed.reverse();
    let reversed = extract_partitioned_with_options(
        reversed,
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap()
    .emit_partitioned_with_options(&options)
    .unwrap();
    assert_eq!(expected, reversed);

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn constant_source_filter_preserves_header_and_owned_source_conflicts() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-constant-conflicts-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let first_header = scratch.join("first.h");
    let second_header = scratch.join("second.h");
    std::fs::write(&first_header, "#define HEADER_CONFLICT 1\n").unwrap();
    std::fs::write(&second_header, "#define HEADER_CONFLICT 2\n").unwrap();
    let header_inputs = vec![
        Input::new(
            "first.cpp",
            format!("#include \"{}\"\n", first_header.display()),
        )
        .partitioned("first-input")
        .with_root(
            first_header.to_string_lossy(),
            "constants",
            "Example.Constants",
        ),
        Input::new(
            "second.cpp",
            format!("#include \"{}\"\n", second_header.display()),
        )
        .partitioned("second-input")
        .with_root(
            second_header.to_string_lossy(),
            "constants",
            "Example.Constants",
        ),
    ];
    let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
    let serial = extract_partitioned_with_options(
        header_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_partitioned_with_options(
        header_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(3),
    )
    .unwrap();
    assert_eq!(serial.dump(), parallel.dump());
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let serial_error = serial
        .emit_partitioned_with_options(&options)
        .unwrap_err()
        .to_string();
    let parallel_error = parallel
        .emit_partitioned_with_options(&options)
        .unwrap_err()
        .to_string();
    assert_eq!(serial_error, parallel_error);
    assert!(
        serial_error.contains("ambiguous constant root `HEADER_CONFLICT`"),
        "{serial_error}"
    );
    let mut reversed = header_inputs;
    reversed.reverse();
    let reversed_error = extract_partitioned_with_options(
        reversed,
        &args,
        &ExtractionOptions::new().with_parallelism(3),
    )
    .unwrap()
    .emit_partitioned_with_options(&options)
    .unwrap_err()
    .to_string();
    assert_eq!(serial_error, reversed_error);

    let source_inputs = vec![
        Input::new("first.cpp", "#define SOURCE_CONFLICT 1\n")
            .partitioned("first-input")
            .with_root("first.cpp", "constants", "Example.Constants"),
        Input::new("second.cpp", "#define SOURCE_CONFLICT 2\n")
            .partitioned("second-input")
            .with_root("second.cpp", "constants", "Example.Constants"),
    ];
    let source_serial = extract_partitioned_with_options(
        source_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let source_parallel = extract_partitioned_with_options(
        source_inputs.clone(),
        &args,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(source_serial.dump(), source_parallel.dump());
    let source_error = source_serial
        .emit_partitioned_with_options(&options)
        .unwrap_err()
        .to_string();
    assert_eq!(
        source_error,
        source_parallel
            .emit_partitioned_with_options(&options)
            .unwrap_err()
            .to_string()
    );
    assert!(
        source_error.contains("ambiguous constant root `SOURCE_CONFLICT`"),
        "{source_error}"
    );
    let mut reversed = source_inputs;
    reversed.reverse();
    assert_eq!(
        source_error,
        extract_partitioned_with_options(
            reversed,
            &args,
            &ExtractionOptions::new().with_parallelism(2),
        )
        .unwrap()
        .emit_partitioned_with_options(&options)
        .unwrap_err()
        .to_string()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn namespace_authorities_reject_duplicate_and_conflicting_routes() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new("value.h", "typedef unsigned VALUE;\n")
            .partitioned("value-input")
            .with_root("value.h", "value", "Example.Value")],
        &["-x", "c++"],
    )
    .unwrap();
    let duplicate = NamespaceAuthorities::new()
        .with_exact("VALUE", "Example.One")
        .with_exact("VALUE", "Example.One");
    let references = BTreeMap::new();
    let error = snapshot
        .clone()
        .emit_partitioned_with_options_and_authorities(
            &EmitOptions::new("Example.Common", &references),
            &duplicate,
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "duplicate namespace authority name `VALUE`"
    );

    let conflicting = NamespaceAuthorities::new()
        .with_wildcard("V*", "Example.One")
        .with_wildcard("*E", "Example.Two");
    let error = snapshot
        .emit_partitioned_with_options_and_authorities(
            &EmitOptions::new("Example.Common", &references),
            &conflicting,
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "namespace authority wildcards conflict for `VALUE`"
    );
}
