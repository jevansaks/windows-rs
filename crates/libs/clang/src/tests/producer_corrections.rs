use super::*;

const ARGS: &[&str] = &[
    "-x",
    "c++",
    "-fms-extensions",
    "--target=x86_64-pc-windows-msvc",
    "-nostdinc",
];

fn snapshot(source: &str) -> Snapshot {
    helpers::ensure_libclang();
    extract([Input::new("producer-corrections.hpp", source)], ARGS).unwrap()
}

#[test]
fn template_owners_do_not_contribute_nested_declarations() {
    let snapshot = snapshot(
        "template<class T> struct Template { struct Hidden { T value; }; enum E { HiddenValue=1 }; };
         template<class T> struct Template<T*> { struct PartialHidden {}; };
         struct Hidden { int value; };
         struct Node { Node* next; };
         struct Outer { struct Nested { int value; }; Nested* child; };
         class Ordinary {};
         struct { int value; } anonymous_value;",
    );
    assert!(
        !snapshot
            .facts
            .iter()
            .any(|fact| { matches!(fact.name.as_str(), "Template" | "PartialHidden" | "E") })
    );
    assert!(
        !snapshot
            .constants
            .iter()
            .any(|constant| constant.name == "HiddenValue")
    );
    let hidden: Vec<_> = snapshot
        .facts
        .iter()
        .filter(|fact| fact.name == "Hidden")
        .collect();
    assert_eq!(hidden.len(), 1);
    assert!(hidden[0].parent.is_none());
    for name in ["Node", "Outer", "Nested", "Ordinary"] {
        assert!(
            snapshot.facts.iter().any(|fact| fact.name == name),
            "{name}"
        );
    }
    assert!(snapshot.observation_only_class_origins.is_empty());
}

#[test]
fn generated_named_enums_preserve_native_values_roots_and_scopedness() {
    let snapshot = snapshot(
        "enum __MIDL___Native_0001 { NativeMax=-1, Native249=249, Native250=250 };
         typedef unsigned NativeAlias;
         enum _NativeAlias { EqualOne=1 };
         enum class __MIDL___Scoped : int { ScopedOne=1 };
         const unsigned Independent=1;",
    );
    for name in ["__MIDL___Native_0001", "_NativeAlias", "__MIDL___Scoped"] {
        let fact = snapshot
            .facts
            .iter()
            .find(|fact| fact.name == name)
            .unwrap();
        assert!(fact.root);
        let FactData::Enum {
            variants, scoped, ..
        } = &fact.data
        else {
            panic!("{fact:?}")
        };
        assert_eq!(*scoped, name == "__MIDL___Scoped");
        if name == "__MIDL___Native_0001" {
            assert_eq!(
                variants
                    .iter()
                    .map(|v| (v.name.as_str(), v.value))
                    .collect::<Vec<_>>(),
                [("NativeMax", -1), ("Native249", 249), ("Native250", 250)]
            );
        }
    }
    let alias = snapshot
        .facts
        .iter()
        .find(|fact| fact.name == "NativeAlias")
        .unwrap();
    assert!(matches!(
        alias.data,
        FactData::Typedef {
            target: TypeRef::Scalar(Scalar::U32)
        }
    ));
    assert!(!snapshot.constants.iter().any(|c| c.name == "NativeMax"));
    assert!(snapshot.constants.iter().any(|c| c.name == "Independent"));
    let references = BTreeMap::new();
    let rdl = snapshot
        .emit_with_options(&EmitOptions::new("Test", &references))
        .unwrap();
    assert!(rdl.contains("NativeMax = -1"), "{rdl}");
}

fn fixture(case: &str) -> (Input, HeaderPartitionPolicy, Vec<String>) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections");
    let friend = root.join(if case == "friend-uuid" {
        "friend_uuid.h"
    } else {
        "friend.h"
    });
    let ordinary = root.join(if case == "ordinary-uuid" {
        "ordinary_uuid.h"
    } else {
        "ordinary.h"
    });
    let friend_include = if case == "friend-uuid" {
        "friend_uuid.h"
    } else {
        "friend.h"
    };
    let ordinary_include = if case == "ordinary-uuid" {
        "ordinary_uuid.h"
    } else {
        "ordinary.h"
    };
    let source = match case {
        "friend-only" | "friend-uuid" => format!("#include \"{friend_include}\"\n"),
        "ordinary-first" | "ordinary-uuid" | "ordinary-nonroot" => {
            format!("#include \"{ordinary_include}\"\n#include \"{friend_include}\"\n")
        }
        _ => format!("#include \"{friend_include}\"\n#include \"{ordinary_include}\"\n"),
    };
    let mut input = Input::new(root.join(format!("{case}.hpp")).to_str().unwrap(), source)
        .with_roots([friend.to_str().unwrap(), ordinary.to_str().unwrap()]);
    if case == "ordinary-nonroot" {
        input.roots.clear();
    }
    if case == "source-excluded" {
        input = input.with_excluded_source_dirs([friend.to_str().unwrap()]);
    }
    let owner = RootPartition::new("Owner", "Test").with_exclusion("Effect");
    let mut friend_owner = owner.clone();
    if case == "owner-excluded" {
        friend_owner.exclusions.insert("Graphics".into());
    }
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(friend.to_str().unwrap(), friend_owner)
        .with_traversed_header(ordinary.to_str().unwrap(), owner);
    let mut args: Vec<_> = ARGS.iter().map(|arg| (*arg).to_string()).collect();
    args.push(format!("-I{}", root.display()));
    (input, policy, args)
}

#[test]
fn friend_observation_preserves_identity_without_root_eligibility() {
    helpers::ensure_libclang();
    for case in [
        "friend-only",
        "friend-uuid",
        "friend-first",
        "owner-excluded",
        "ordinary-first",
        "ordinary-uuid",
        "ordinary-nonroot",
    ] {
        let (input, policy, args) = fixture(case);
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        let snapshot = extract([input], &args).unwrap();
        let observation_only = matches!(
            case,
            "friend-only" | "friend-uuid" | "friend-first" | "owner-excluded"
        );
        assert_eq!(
            snapshot.observation_only_class_origins.len(),
            usize::from(observation_only)
        );
        if !observation_only {
            assert!(format!("{snapshot:?}").contains("observation_only_class_origins: {}"));
        }
        if case == "ordinary-nonroot" {
            let graphics = snapshot
                .facts
                .iter()
                .find(|fact| fact.name == "Graphics")
                .unwrap();
            assert!(!graphics.root);
        }
        let plan = snapshot
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap();
        if case == "ordinary-nonroot" {
            let graphics = plan
                .snapshot
                .facts
                .iter()
                .find(|fact| fact.name == "Graphics")
                .unwrap();
            assert!(graphics.root);
        }
        for anchor in &plan.snapshot.observation_only_class_origins {
            let fact = plan
                .snapshot
                .facts
                .iter()
                .find(|fact| fact.origin == *anchor)
                .unwrap();
            assert!(!fact.root);
            assert!(plan.snapshot.root_owners.contains_key(anchor));
            assert_eq!(
                plan.snapshot.class_canonical_origins[anchor],
                CanonicalRecord::Source(anchor.clone())
            );
            let semantic_parent = plan
                .snapshot
                .facts
                .iter()
                .find(|candidate| Some(&candidate.origin) == fact.parent.as_ref())
                .unwrap();
            assert_eq!(semantic_parent.kind, FactKind::Namespace);
            assert_eq!(semantic_parent.name, "N");
        }
        if matches!(case, "friend-first" | "owner-excluded") {
            let graphics: Vec<_> = plan
                .snapshot
                .facts
                .iter()
                .filter(|f| f.name == "Graphics")
                .collect();
            assert_eq!(graphics.len(), 2);
            assert!(!graphics[0].root);
            assert!(graphics[1].root);
            assert_eq!(
                plan.snapshot.class_canonical_origins[&graphics[1].origin],
                CanonicalRecord::Source(graphics[0].origin.clone())
            );
            if case == "owner-excluded" {
                assert!(
                    plan.snapshot.root_owners[&graphics[0].origin]
                        .exclusions
                        .contains("Graphics")
                );
            }
        }
        let references = BTreeMap::new();
        let output = plan
            .emit_with_options(&EmitOptions::new("Test", &references))
            .unwrap();
        if matches!(case, "friend-only" | "friend-uuid") {
            assert!(output.is_empty(), "{output:?}");
        }
        if case == "ordinary-uuid" {
            assert!(
                output
                    .values()
                    .any(|rdl| rdl.contains("const Graphics: GUID")),
                "{output:?}"
            );
        }
    }
}

#[test]
fn excluded_friend_source_is_fatal_with_native_canonical_context() {
    helpers::ensure_libclang();
    let (mut input, _, args) = fixture("source-excluded");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections");
    input.excluded_roots.clear();
    input = input.with_excluded_source_dirs([root.join("excluded").to_str().unwrap()]);
    input.source = "#include \"excluded/friend.h\"\n#include \"ordinary.h\"\n".into();
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let error = extract([input], &args).unwrap_err().to_string();
    for text in [
        "missing exact native class canonical origin",
        "record_name=",
        "canonical_name=",
        "canonical_equal_record=",
        "bucket_equality=",
    ] {
        assert!(error.contains(text), "{error}");
    }
}

#[test]
fn friend_uuid_alias_preserves_real_ordinary_eligibility() {
    helpers::ensure_libclang();
    let source = "namespace N { class Effect { friend class C; }; typedef class C Alias;
        class __declspec(uuid(\"11111111-2222-3333-4455-66778899aabb\")) C; }";
    let inputs: Vec<_> = (0..1)
        .map(|i| Input::new(format!("friend{i}.hpp"), source))
        .collect();
    let serial = extract_with_options(
        inputs.clone(),
        ARGS,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_with_options(
        inputs.clone(),
        ARGS,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial, parallel);
    assert_eq!(parallel.clone(), parallel);
    assert_eq!(parallel.observation_only_class_origins.len(), 1);
    let aliases = parallel.coclass_aliases().unwrap();
    assert!(!aliases.is_empty());
    assert!(aliases.values().all(|alias| {
        !alias.guids.is_empty()
            && parallel
                .observation_only_class_origins
                .contains(alias.canonical.anchor())
    }));
    let mut policy = HeaderPartitionPolicy::new();
    for input in &inputs {
        policy.add_traversed_header(
            &input.name,
            RootPartition::new("Owner", "Test").with_exclusion("Effect"),
        );
    }
    let plan = parallel
        .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert_eq!(plan.snapshot.observation_only_class_origins.len(), 1);
    let references = BTreeMap::new();
    let output = plan
        .emit_with_options(&EmitOptions::new("Test", &references))
        .unwrap();
    assert!(
        output.values().any(|rdl| rdl.contains("const C: GUID")),
        "{output:?}"
    );
}

#[test]
fn owned_associated_enum_remaps_update_annotations_and_reject_disagreement() {
    helpers::ensure_libclang();
    let source = "enum NativeEnum { Value=1 };
        struct Root { __attribute__((annotate(\"win32metadata:associated_enum=NativeEnum\"))) int value; };";
    let first = Input::new("associated-first.hpp", source);
    let snapshot = extract([first.clone()], ARGS).unwrap();
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        &first.name,
        RootPartition::new("First", "Test").with_remap("NativeEnum", "RoutedEnum"),
    );
    let plan = snapshot
        .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let references = BTreeMap::new();
    let output = plan
        .emit_with_options(&EmitOptions::new("Test", &references))
        .unwrap();
    assert!(
        output.values().any(|rdl| rdl.contains("enum RoutedEnum")),
        "{output:?}"
    );
    assert!(
        output
            .values()
            .any(|rdl| rdl.contains("associated_enum(\"RoutedEnum\")")),
        "{output:?}"
    );

    let second = Input::new("associated-second.hpp", source);
    let snapshot = extract([first, second.clone()], ARGS).unwrap();
    let policy = policy.with_traversed_header(
        &second.name,
        RootPartition::new("Second", "Other").with_remap("NativeEnum", "OtherEnum"),
    );
    let error = snapshot
        .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("conflicting routed provider remaps"),
        "{error}"
    );
}

#[test]
fn friend_only_uuid_role_remains_nonemittable_after_parallel_merge() {
    helpers::ensure_libclang();
    let source = "namespace N { class Effect { friend class __declspec(uuid(\"11111111-2222-3333-4455-66778899aabb\")) C; }; }";
    let inputs: Vec<_> = (0..3)
        .map(|i| Input::new(format!("observation{i}.hpp"), source))
        .collect();
    let serial = extract_with_options(
        inputs.clone(),
        ARGS,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_with_options(
        inputs.clone(),
        ARGS,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial, parallel);
    let mut policy = HeaderPartitionPolicy::new();
    for input in inputs {
        policy.add_traversed_header(
            &input.name,
            RootPartition::new("Owner", "Test").with_exclusion("Effect"),
        );
    }
    let plan = parallel
        .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert_eq!(plan.snapshot.observation_only_class_origins.len(), 3);
    let references = BTreeMap::new();
    assert!(
        plan.emit_with_options(&EmitOptions::new("Test", &references))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn earlier_specialization_anchors_are_private_identity_not_layouts() {
    for source in [
        "namespace N { template<class... T> class Tuple; Tuple<>* before;
             template<> class Tuple<> { public: int value; }; }",
        "namespace N { template<class K, class V> struct Map { int hidden; };
             Map<int,int>* before; template<> struct Map<int,int> { int value; }; }",
    ] {
        let snapshot = snapshot(source);
        assert_eq!(snapshot.class_canonical_anchors.len(), 1);
        let (origin, anchor) = snapshot.class_canonical_anchors.first_key_value().unwrap();
        assert_eq!(
            snapshot.class_canonical_origins[origin],
            CanonicalRecord::Source(origin.clone())
        );
        assert!(anchor.main_file);
        assert!(!anchor.system);
        assert!(!anchor.template_partial);
        assert_eq!(anchor.spelling, anchor.template_spelling);
        let parent = snapshot
            .facts
            .iter()
            .find(|fact| Some(&fact.origin) == anchor.parent.as_ref())
            .unwrap();
        assert_eq!(
            (parent.name.as_str(), parent.kind),
            ("N", FactKind::Namespace)
        );
        assert!(!snapshot.facts.iter().any(|fact| fact.origin == *origin));
        assert!(!snapshot.pointer_only_class_layouts.contains_key(origin));
        assert!(!snapshot.declaration_guids.contains_key(origin));
        assert!(!snapshot.root_owners.contains_key(origin));
        let record = snapshot
            .facts
            .iter()
            .find(|fact| fact.name == anchor.name)
            .unwrap();
        assert_ne!(record.origin, *origin);
        assert_eq!(
            snapshot.class_canonical_origins[&record.origin],
            CanonicalRecord::Source(origin.clone())
        );
        assert_eq!(snapshot, snapshot.clone());
        snapshot.coclass_aliases().unwrap();
        let rdl = snapshot.emit_with_library("Test", "test.dll").unwrap();
        assert!(rdl.contains("value: i32"), "{rdl}");
        assert!(!rdl.contains("hidden"), "{rdl}");
    }
}

#[test]
fn distinct_same_location_anchors_survive_owned_merge_and_colored_selection() {
    helpers::ensure_libclang();
    let source = "namespace N { template<class T> struct Item;
            Item<int>* first; Item<double>* second;
            template<> struct Item<int> { int value; };
            template<> struct Item<double> { double value; };
            struct Ordinary { int value; }; }";
    let inputs: Vec<_> = (0..3)
        .map(|i| Input::new(format!("specialization{i}.hpp"), source))
        .collect();
    let serial = extract_with_options(
        inputs.clone(),
        ARGS,
        &ExtractionOptions::new().with_parallelism(1),
    )
    .unwrap();
    let parallel = extract_with_options(
        inputs.clone(),
        ARGS,
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();
    assert_eq!(serial, parallel);
    assert_eq!(parallel, parallel.clone());
    assert_eq!(parallel.class_canonical_anchors.len(), 6);
    for input in &inputs {
        let anchors: Vec<_> = parallel
            .class_canonical_anchors
            .iter()
            .filter(|(origin, _)| origin.tu == input.name)
            .collect();
        assert_eq!(anchors.len(), 2);
        assert_ne!(anchors[0].0, anchors[1].0);
        assert_eq!(anchors[0].1.spelling, anchors[1].1.spelling);
        assert_eq!(
            anchors[0].1.template_spelling,
            anchors[1].1.template_spelling
        );
        assert_ne!(
            parallel.class_canonical_origins[anchors[0].0],
            parallel.class_canonical_origins[anchors[1].0]
        );
    }
    parallel.coclass_aliases().unwrap();
    let input = inputs.into_iter().next().unwrap();
    let snapshot = extract([input.clone()], ARGS).unwrap();
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        &input.name,
        RootPartition::new("Owner", "Test").with_exclusion("Item"),
    );
    let plan = snapshot
        .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert_eq!(plan.snapshot.class_canonical_anchors.len(), 2);
    plan.snapshot.coclass_aliases().unwrap();
    let references = BTreeMap::new();
    let output = plan
        .emit_with_options(&EmitOptions::new("Test", &references))
        .unwrap();
    assert!(
        output.values().any(|rdl| rdl.contains("struct Ordinary")),
        "{output:?}"
    );
    assert!(
        output.values().all(|rdl| !rdl.contains("struct Item")),
        "{output:?}"
    );
}

#[test]
fn excluded_specialization_canonical_source_remains_fatal() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections");
    let input = Input::new(
            root.join("excluded-specialization.hpp").to_str().unwrap(),
            "#include \"excluded/template.h\"\nnamespace N { template<> struct Item<int> { int value; }; }",
        )
        .with_excluded_source_dirs([root.join("excluded").to_str().unwrap()]);
    let include = format!("-I{}", root.display());
    let mut args = ARGS.to_vec();
    args.push(&include);
    let error = extract([input], &args).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("excluded native class canonical specialization source"),
        "{error}"
    );
}

#[test]
fn referenced_uuid_generic_specialization_keeps_flat_and_colored_contracts() {
    helpers::ensure_libclang();
    let prefix = "struct HSTRING__; typedef HSTRING__* HSTRING; struct IInspectable;
            template<typename K, typename V> struct IMapView {};";
    let alias = "typedef IMapView<HSTRING,IInspectable*> MAP;";
    let specialization =
        "template<> struct __declspec(uuid(\"12345678-1234-5678-90ab-cdef12345678\"))
            IMapView<HSTRING,IInspectable*> { virtual int __stdcall Read(int* value) = 0; };";
    let function = "extern \"C\" void GetMap(MAP** value);";
    let references = BTreeMap::from([(
        "IMapView".to_string(),
        TypeReference::new(
            "Windows.Foundation.Collections",
            "IMapView",
            TypeReferenceKind::Interface,
        ),
    )]);
    let mut options = EmitOptions::new("WinrtGenericInterface", &references);
    options.library = Some("api.dll");
    let mut flat = Vec::new();
    let mut colored = Vec::new();
    for (index, source) in [
        format!("{prefix}{specialization}{alias}{function}"),
        format!("{prefix}{alias}{specialization}{function}"),
    ]
    .into_iter()
    .enumerate()
    {
        let input = Input::new("generic-specialization.hpp", source);
        let snapshot = extract([input.clone()], ARGS).unwrap();
        assert_eq!(snapshot.class_canonical_anchors.len(), index);
        let record = snapshot
            .facts
            .iter()
            .find(|fact| fact.name == "IMapView")
            .unwrap();
        assert_eq!(
            snapshot.declaration_guids[&record.origin],
            "12345678-1234-5678-90ab-cdef12345678"
        );
        assert!(matches!(&record.data, FactData::Interface { methods, .. } if methods.len() == 1));
        let alias = snapshot
            .facts
            .iter()
            .find(|fact| fact.name == "MAP")
            .unwrap();
        assert!(matches!(
            &alias.data,
            FactData::Typedef { target: TypeRef::Generic { name, args, .. } }
                if name == "IMapView" && args.len() == 2
        ));
        flat.push(snapshot.emit_with_options(&options).unwrap());
        let policy = HeaderPartitionPolicy::new().with_traversed_header(
            &input.name,
            RootPartition::new("Owner", "WinrtGenericInterface"),
        );
        let plan = snapshot
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap();
        for origin in plan.snapshot.class_canonical_anchors.keys() {
            assert!(!plan.snapshot.root_owners.contains_key(origin));
            assert!(!plan.snapshot.declaration_guids.contains_key(origin));
        }
        colored.push(plan.emit_with_options(&options).unwrap());
    }
    assert_eq!(flat[0], flat[1]);
    assert_eq!(colored[0], colored[1]);
    assert!(
        flat[1].contains("type MAP = IMapView<String, Object>"),
        "{:?}",
        flat[1]
    );
    assert!(flat[1].contains("GetMap(value: *mut MAP)"), "{:?}", flat[1]);
}

#[test]
fn real_file_enum_root_identity_baseline() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("enum_roots");
    let shared = root.join("SharedFlags.h");
    let lower = root.join(if cfg!(windows) {
        "sharedflags.h"
    } else {
        "SharedFlags.h"
    });
    let separate = root.join("distinct").join("SharedFlags.h");
    let same_file = std::fs::canonicalize(&shared);
    let lower_file = std::fs::canonicalize(&lower);
    let separate_file = std::fs::canonicalize(&separate);
    let owner = RootPartition::new("Audio.Apo", "Test.Audio.Apo");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(shared.to_str().unwrap(), owner.clone())
        .with_traversed_header(separate.to_str().unwrap(), owner);
    let references = BTreeMap::new();
    let options = EmitOptions::new("Test.Audio.Apo", &references);
    let include = format!("-I{}", root.display());
    let mut args = ARGS.to_vec();
    args.extend(["-std=c++20", &include]);
    if cfg!(windows) {
        args.push("-DCASE_VARIANT_INCLUDE");
    }
    let mut evidence = format!(
        "pid={}\nruntime={:?}\nargs={args:?}\npolicy={policy:#?}\n\
         upper={same_file:?}\nlower={lower_file:?}\nseparate={separate_file:?}\n",
        std::process::id(),
        std::env::var_os("LIBCLANG_PATH"),
    );
    let mut cases = vec![
        ("identical", ["identical_first.hpp", "identical_second.hpp"]),
        (
            "portable_alias",
            ["identical_first.hpp", "alias_second.hpp"],
        ),
        (
            "distinct_files",
            ["distinct_first.hpp", "distinct_second.hpp"],
        ),
        ("macro_conflict", ["macro_first.hpp", "macro_second.hpp"]),
    ];
    if cfg!(windows) {
        cases.push(("case_variant", ["case_first.hpp", "case_second.hpp"]));
    }
    writeln!(evidence, "windows_case_variant_exercised={}", cfg!(windows)).unwrap();
    let mut results = Vec::new();
    for (case, wrappers, reversed) in cases
        .into_iter()
        .flat_map(|(case, wrappers)| [false, true].map(|reversed| (case, wrappers, reversed)))
    {
        let mut wrappers = wrappers;
        if reversed {
            wrappers.reverse();
        }
        let inputs: Vec<_> = wrappers
            .into_iter()
            .map(|name| {
                let path = root.join(name);
                Input::new(
                    path.to_str().unwrap(),
                    std::fs::read_to_string(&path).unwrap(),
                )
                .with_roots([
                    shared.to_str().unwrap(),
                    lower.to_str().unwrap(),
                    separate.to_str().unwrap(),
                ])
                .partitioned(name)
                .with_root(
                    path.to_str().unwrap(),
                    if name.contains("first") {
                        "Audio.Apo"
                    } else {
                        "TermServ"
                    },
                    if name.contains("first") {
                        "Test.Audio.Apo"
                    } else {
                        "Test.System.RemoteDesktop"
                    },
                )
            })
            .collect();
        writeln!(
            evidence,
            "\nCASE={case} reversed={reversed}\ninputs={inputs:#?}"
        )
        .unwrap();
        let extracted = extract_partitioned_with_options(
            inputs,
            &args,
            &ExtractionOptions::new().with_parallelism(1),
        );
        let (flat, colored, exact_aliases) = match extracted {
            Ok(snapshot) => {
                writeln!(evidence, "native_snapshot={snapshot:#?}").unwrap();
                let mut exact_aliases = 0;
                for alias in snapshot
                    .facts
                    .iter()
                    .filter(|fact| fact.name == "BufferFlags")
                {
                    let FactData::Typedef {
                        target: TypeRef::Named { name, declaration },
                    } = &alias.data
                    else {
                        continue;
                    };
                    let targets: Vec<_> = snapshot
                        .facts
                        .iter()
                        .filter(|target| {
                            target.origin.tu == alias.origin.tu
                                && target.name == *name
                                && target.spelling == *declaration
                                && matches!(target.data, FactData::Enum { .. })
                        })
                        .collect();
                    writeln!(
                        evidence,
                        "local_alias_origin={:?} target={declaration:?} matches={targets:#?}",
                        alias.origin
                    )
                    .unwrap();
                    for target in &targets {
                        let FactData::Enum {
                            repr,
                            variants,
                            fixed,
                            scoped,
                        } = &target.data
                        else {
                            unreachable!()
                        };
                        assert_eq!(*repr, Scalar::I32);
                        assert!(!fixed);
                        assert!(!scoped);
                        let silent = if case == "macro_conflict"
                            && alias.origin.tu.ends_with("macro_second.hpp")
                        {
                            3
                        } else {
                            2
                        };
                        assert_eq!(
                            variants
                                .iter()
                                .map(|v| (v.name.as_str(), v.value))
                                .collect::<Vec<_>>(),
                            [
                                ("BUFFER_INVALID", 0),
                                ("BUFFER_VALID", 1),
                                ("BUFFER_SILENT", silent),
                            ]
                        );
                    }
                    exact_aliases += usize::from(targets.len() == 1);
                }
                let flat = snapshot
                    .emit_with_options(&options)
                    .map_err(|e| e.to_string());
                let colored = snapshot
                    .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                    .map_err(|e| e.to_string())
                    .and_then(|plan| {
                        writeln!(
                            evidence,
                            "effective_snapshot={:#?}\nroot_conflicts={:#?}",
                            plan.snapshot, plan.root_conflicts,
                        )
                        .unwrap();
                        for fact in plan
                            .snapshot
                            .facts
                            .iter()
                            .filter(|f| f.name == "BufferFlags")
                        {
                            let owner = &plan.snapshot.root_owners[&fact.origin];
                            assert_eq!(owner.partition, "Audio.Apo");
                            assert_eq!(owner.namespace, "Test.Audio.Apo");
                        }
                        plan.emit_with_options(&options).map_err(|e| e.to_string())
                    });
                (flat, colored, exact_aliases)
            }
            Err(error) => {
                let error = error.to_string();
                writeln!(evidence, "extraction_error={error}").unwrap();
                (Err(error.clone()), Err(error), 0)
            }
        };
        writeln!(
            evidence,
            "flat={flat:#?}\ncolored={colored:#?}\nexact_local_aliases={exact_aliases}"
        )
        .unwrap();
        results.push((case, reversed, flat, colored, exact_aliases));
    }
    println!("{evidence}");
    if let Some(path) = std::env::var_os("WINDOWS_CLANG_ENUM_ROOT_CAPTURE") {
        std::fs::write(path, &evidence).unwrap();
    }

    assert_eq!(
        same_file.as_ref().unwrap(),
        lower_file.as_ref().unwrap(),
        "case-variant fixture must resolve to the same physical file"
    );
    assert_ne!(
        same_file.as_ref().unwrap(),
        separate_file.as_ref().unwrap(),
        "equal-content negative must use a distinct real file"
    );
    for (case, _, flat, colored, aliases) in &results {
        assert_eq!(
            *aliases, 2,
            "{case}: missing exact TU-local native typedef joins"
        );
        if matches!(*case, "distinct_files" | "macro_conflict") {
            assert!(flat.is_err(), "{case}: {flat:?}");
            assert!(colored.is_err(), "{case}: {colored:?}");
        }
    }
    let mut expected = None;
    for (case, _, flat, colored, _) in results
        .iter()
        .filter(|(case, ..)| !matches!(*case, "distinct_files" | "macro_conflict"))
    {
        let flat = flat
            .as_ref()
            .unwrap_or_else(|error| panic!("{case}: {error}"));
        let colored = colored
            .as_ref()
            .unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!(colored.len(), 1, "{case}: {colored:?}");
        if let Some((expected_flat, expected_colored)) = expected {
            assert_eq!(flat, expected_flat, "{case}");
            assert_eq!(colored, expected_colored, "{case}");
        } else {
            expected = Some((flat, colored));
        }
        for output in std::iter::once(flat).chain(colored.values()) {
            assert_eq!(output.matches("enum BufferFlags").count(), 1, "{output}");
            for value in [
                "BUFFER_INVALID = 0",
                "BUFFER_VALID = 1",
                "BUFFER_SILENT = 2",
            ] {
                assert!(output.contains(value), "{case}: {output}");
            }
        }
    }
}

#[test]
fn native_file_entry_paths_resolve_aliases_and_preserve_virtual_names() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("enum_roots");
    let include = format!("-I{}", root.display());
    let mut args = ARGS.to_vec();
    args.push(&include);
    let mut paths = vec!["SharedFlags.h", "distinct/../SharedFlags.h"];
    if cfg!(windows) {
        paths.push("sharedflags.h");
    }
    let mut expected = None;
    for (index, path) in paths.into_iter().enumerate() {
        let input = Input::new(
            root.join(format!("path-witness-{index}.hpp"))
                .to_str()
                .unwrap(),
            format!("#include \"{path}\"\n"),
        );
        let witness =
            extract::source_file_path_witness(&input, &args, root.join(path).to_str().unwrap())
                .unwrap();
        println!("native_path_witness={witness:?}");
        assert!(
            !witness.1.is_empty(),
            "real file has no native real path: {witness:?}"
        );
        assert_eq!(witness.2, normalize_name(&witness.1));
        if let Some(expected) = &expected {
            assert_eq!(&witness.2, expected);
        } else {
            expected = Some(witness.2);
        }
    }
    let source = "typedef enum VirtualFlags { VirtualValue=1 } VirtualFlags;";
    let inputs: Vec<_> = ["Virtual.hpp", "virtual.hpp"]
        .into_iter()
        .map(|name| {
            let path = root.join("nonexistent").join(name);
            assert!(!path.exists());
            Input::new(path.to_str().unwrap(), source)
        })
        .collect();
    for input in &inputs {
        let witness = extract::source_file_path_witness(input, ARGS, &input.name).unwrap();
        println!("virtual_path_witness={witness:?}");
        assert!(witness.1.is_empty(), "{witness:?}");
        assert_eq!(witness.2, input.name);
    }
    let snapshot = extract_with_options(inputs.clone(), ARGS, &ExtractionOptions::new()).unwrap();
    for input in &inputs {
        let facts: Vec<_> = snapshot
            .facts
            .iter()
            .filter(|f| f.name == "VirtualFlags" && f.origin.tu == input.name)
            .collect();
        assert_eq!(facts.len(), 2);
        assert!(
            facts
                .iter()
                .all(|f| f.spelling.file == input.name && f.expansion.file == input.name)
        );
    }
    assert_ne!(inputs[0].name, inputs[1].name);
    assert!(
        snapshot
            .emit("Virtual")
            .unwrap_err()
            .to_string()
            .contains("ambiguous type root")
    );
}

#[test]
fn independent_root_failures_are_fatal_and_ordered_before_dependencies() {
    helpers::ensure_libclang();
    let first = Input::new(
        "inventory-first.hpp",
        "#define CConstant 1\n\
         struct ACollision { int value; }; extern \"C\" int ACollision(int value);\n\
         enum BType { BValue=1 };\n\
         class __declspec(uuid(\"11111111-2222-3333-4455-66778899aabb\")) DValue;\n\
         namespace Hidden { enum EAssociated { EValue=1 }; }\n\
         struct ValidRoot { __attribute__((annotate(\"win32metadata:associated_enum=EAssociated\"))) int value; };\n\
         extern \"C\" int ValidCall(int value);",
    );
    let second = Input::new(
        "inventory-second.hpp",
        "#define CConstant 2\n\
         enum BType { BValue=2 };\n\
         class __declspec(uuid(\"22222222-3333-4444-5566-778899aabbcc\")) DValue;\n\
         namespace Hidden { enum EAssociated { EValue=2 }; }",
    );
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(&first.name, RootPartition::new("Owner", "Test.Inventory"))
        .with_traversed_header(&second.name, RootPartition::new("Owner", "Test.Inventory"));
    let references = BTreeMap::from([(
        "UnusedReference".to_string(),
        TypeReference::new("Other", "UnusedReference", TypeReferenceKind::Interface),
    )]);
    let selected = BTreeSet::from([
        "ACollision".to_string(),
        "FMissing".to_string(),
        "ValidCall".to_string(),
    ]);
    let mut options = EmitOptions::new("Test.Inventory", &references);
    options.library = Some("test.dll");
    options.functions = Some(&selected);
    let mut errors = Vec::new();
    for inputs in [[first.clone(), second.clone()], [second, first]] {
        let snapshot = extract_with_options(inputs, ARGS, &ExtractionOptions::new()).unwrap();
        let error = snapshot
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap()
            .emit_with_options(&options)
            .unwrap_err()
            .to_string();
        println!("root_inventory={error}");
        for message in [
            "type and function roots collide on `ACollision`",
            "ambiguous type root `BType`",
            "ambiguous constant root `CConstant`",
            "ambiguous value root `DValue`",
            "associated enum `EAssociated`",
            "selected function `FMissing` was not found",
        ] {
            assert!(error.contains(message), "{error}");
        }
        assert!(
            !error.contains("selected function `ACollision` was not found"),
            "{error}"
        );
        assert!(!error.contains("ValidCall"), "{error}");
        assert!(!error.contains("#[win32]"), "{error}");
        assert!(error.contains("inventory-first.hpp#"), "{error}");
        assert!(error.contains("namespace: \"Test.Inventory\""), "{error}");
        let positions: Vec<_> = [
            "ACollision",
            "BType",
            "CConstant",
            "DValue",
            "EAssociated",
            "FMissing",
        ]
        .map(|name| error.find(&format!("`{name}`")).unwrap())
        .into_iter()
        .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "{error}"
        );
        errors.push(error);
    }
    assert_eq!(errors[0], errors[1]);
    assert!(
        snapshot("struct ValidRoot { int value; };")
            .emit("Valid")
            .unwrap()
            .contains("struct ValidRoot")
    );
}

#[test]
fn root_inventory_preserves_single_and_function_only_error_text() {
    let error =
        snapshot("struct SingleCollision { int value; }; extern \"C\" int SingleCollision(int);")
            .emit_with_library("Test", "test.dll")
            .unwrap_err();
    assert_eq!(
        error.to_string(),
        "type and function roots collide on `SingleCollision`"
    );
    let errors = ["first function failure", "second function failure"]
        .map(|message| RootSelectionError {
            name: "Function".to_string(),
            category: "function",
            error: Error(message.to_string()),
            candidates: BTreeSet::new(),
        })
        .into_iter()
        .collect();
    assert_eq!(
        aggregate_root_selection_errors(errors).to_string(),
        "ambiguous function roots (2):\n  first function failure\n  second function failure",
    );
}
