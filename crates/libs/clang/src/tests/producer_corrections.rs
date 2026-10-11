use super::*;

#[test]
fn shared_literal_enum_single_unowned_dependency() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("shared_enum");
    let foreign = root.join("foreign.hpp");
    let input = Input::new(
        foreign.to_str().unwrap(),
        std::fs::read_to_string(&foreign).unwrap(),
    )
    .with_roots([foreign.to_str().unwrap()])
    .partitioned("foreign.hpp");
    let mut policy = HeaderPartitionPolicy::new();
    policy.add_traversed_header_for_input(
        input.input.name.clone(),
        foreign.to_str().unwrap(),
        RootPartition::new("Foreign", "Test.Foreign"),
    );
    let mut outputs = Vec::new();
    for options in [
        ExtractionOptions::new(),
        ExtractionOptions::new().with_module_free_headers(),
    ] {
        let snapshot = extract_partitioned_with_options([input.clone()], ARGS, &options).unwrap();
        let enumeration = snapshot
            .facts
            .iter()
            .find(|fact| matches!(fact.data, FactData::Enum { .. }))
            .unwrap();
        assert!(!enumeration.root);
        assert!(!snapshot.root_owners.contains_key(&enumeration.origin));
        assert_eq!(
            snapshot
                .shared_enum_identities
                .contains_key(&enumeration.origin),
            options.module_free_headers
        );
        let output = snapshot
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap()
            .emit_with_options(&EmitOptions::new("Test", &BTreeMap::new()))
            .unwrap();
        assert_eq!(
            output
                .values()
                .filter(|rdl| rdl.contains("enum SHARED_CURVE"))
                .count(),
            1
        );
        assert!(output.iter().any(|(route, rdl)| {
            route.partition == "Foreign" && rdl.contains("struct SHARED_CURVE_REF")
        }));
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
    println!("shared_literal_enum single_unowned default=opt_in assertions=passed");
}

#[test]
fn shared_literal_enum_public_provider() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("shared_enum");
    for case in [
        "shared",
        "duplicate_provider",
        "values",
        "tokens",
        "macro",
        "internal",
        "anonymous",
        "excluded",
        "owners",
        "unproved",
        "default",
    ] {
        let b_header = match case {
            "values" => "Values.h",
            "tokens" => "Tokens.h",
            "macro" => "Macro.h",
            "internal" => "Internal.h",
            "anonymous" => "Anonymous.h",
            _ => "Curve.h",
        };
        for reversed in [false, true] {
            let a = root.join("a").join("Curve.h");
            let b = root.join("b").join(b_header);
            let foreign = root.join("foreign.hpp");
            let mut policy = HeaderPartitionPolicy::new();
            let mut inputs = Vec::new();
            for (wrapper, header, source, owner) in [
                (
                    "a.hpp",
                    &a,
                    "#include \"a/Curve.h\"\n".to_string(),
                    RootPartition::new("A", "Test.A").with_exclusion("SHARED_CURVE"),
                ),
                (
                    "b.hpp",
                    &b,
                    format!("#include \"b/{b_header}\"\n"),
                    if case == "excluded" {
                        RootPartition::new("B", "Test.B").with_exclusion("SHARED_CURVE")
                    } else {
                        RootPartition::new("B", "Test.B")
                    },
                ),
                (
                    "foreign.hpp",
                    &foreign,
                    std::fs::read_to_string(&foreign).unwrap(),
                    RootPartition::new("Foreign", "Test.Foreign"),
                ),
            ] {
                let path = root.join(wrapper);
                let input = Input::new(path.to_str().unwrap(), source)
                    .with_roots([
                        a.to_str().unwrap(),
                        b.to_str().unwrap(),
                        foreign.to_str().unwrap(),
                    ])
                    .partitioned(wrapper);
                policy.add_traversed_header_for_input(
                    input.input.name.clone(),
                    header.to_str().unwrap(),
                    owner,
                );
                inputs.push(input);
            }
            if matches!(case, "owners" | "duplicate_provider") {
                let input = Input::new(
                    root.join("c.hpp").to_str().unwrap(),
                    format!("#include \"b/{b_header}\"\n"),
                )
                .with_roots([b.to_str().unwrap()])
                .partitioned("c.hpp");
                policy.add_traversed_header_for_input(
                    input.input.name.clone(),
                    b.to_str().unwrap(),
                    if case == "duplicate_provider" {
                        RootPartition::new("B", "Test.B")
                    } else {
                        RootPartition::new("C", "Test.C")
                    },
                );
                inputs.push(input);
            }
            if reversed {
                inputs.swap(0, 1);
            }
            let options = if case == "default" {
                ExtractionOptions::new()
            } else {
                ExtractionOptions::new().with_module_free_headers()
            };
            let mut snapshot = extract_partitioned_with_options(inputs, ARGS, &options).unwrap();
            if case == "unproved" {
                snapshot.shared_enum_identities.clear();
            }
            let referrer = snapshot
                .facts
                .iter()
                .find(|fact| fact.name == "SHARED_CURVE_REF")
                .unwrap();
            let FactData::Record { fields, .. } = &referrer.data else {
                panic!()
            };
            let TypeRef::Named { name, declaration } = &fields[0].ty else {
                panic!()
            };
            let aliases: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| {
                    fact.origin.tu == referrer.origin.tu
                        && fact.name == *name
                        && fact.spelling == *declaration
                })
                .collect();
            let [alias] = aliases.as_slice() else {
                panic!("exact local alias missing")
            };
            let FactData::Typedef {
                target: TypeRef::Named { name, declaration },
            } = &alias.data
            else {
                panic!()
            };
            let enums: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| {
                    fact.origin.tu == alias.origin.tu
                        && fact.name == *name
                        && fact.spelling == *declaration
                        && matches!(fact.data, FactData::Enum { .. })
                })
                .collect();
            let [enumeration] = enums.as_slice() else {
                panic!("exact local enum missing")
            };
            assert_eq!(alias.spelling.file, normalize_name(a.to_str().unwrap()));
            assert_eq!(enumeration.spelling.file, alias.spelling.file);
            if matches!(case, "shared" | "duplicate_provider") {
                assert!(snapshot.shared_enum_identities.contains_key(&alias.origin));
                assert!(
                    snapshot
                        .shared_enum_identities
                        .contains_key(&enumeration.origin)
                );
                if case == "duplicate_provider" {
                    let providers: Vec<_> = snapshot
                        .facts
                        .iter()
                        .filter(|fact| {
                            matches!(fact.data, FactData::Enum { .. })
                                && fact.spelling.file == normalize_name(b.to_str().unwrap())
                        })
                        .collect();
                    assert_eq!(providers.len(), 2);
                    assert_ne!(providers[0].origin.tu, providers[1].origin.tu);
                    assert!(providers.iter().all(|fact| {
                        snapshot.shared_enum_identities.contains_key(&fact.origin)
                    }));
                }
            }
            let outcome = snapshot
                .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                .and_then(|plan| {
                    plan.emit_with_options(&EmitOptions::new("Test", &BTreeMap::new()))
                });
            if matches!(case, "shared" | "duplicate_provider") {
                let output = outcome.unwrap();
                let enums: Vec<_> = output
                    .iter()
                    .filter(|(_, rdl)| rdl.contains("enum SHARED_CURVE"))
                    .collect();
                assert_eq!(enums.len(), 1, "{output:#?}");
                assert_eq!(enums[0].0.partition, "B");
                assert!(
                    output
                        .iter()
                        .any(|(route, rdl)| route.partition == "Foreign"
                            && rdl.contains("curve: Test::B::SHARED_CURVE")),
                    "{output:#?}"
                );
                assert!(
                    !output
                        .values()
                        .any(|rdl| rdl.contains("Test::A::SHARED_CURVE"))
                );
            } else {
                assert!(outcome.is_err(), "{case} reversed={reversed}: {outcome:#?}");
            }
            println!("shared_literal_enum case={case} reversed={reversed} assertions=passed");
        }
    }
}

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

#[test]
fn guarded_enum_input_qualified_owner_exclusion_baseline() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("guarded_enum");
    let first = root.join("a").join("Curve.h");
    let second = root.join("b").join("Curve.h");
    let references = BTreeMap::from([(
        "UnrelatedReference".to_string(),
        TypeReference::new(
            "Test.External",
            "UnrelatedReference",
            TypeReferenceKind::Type,
        ),
    )]);
    let options = EmitOptions::new("Test.Common", &references);
    let cases = [
        ("excluded", vec!["a.hpp", "b.hpp"]),
        ("excluded_foreign", vec!["a.hpp", "b.hpp", "foreign.hpp"]),
        ("distinct_colors", vec!["a.hpp", "b.hpp", "foreign.hpp"]),
        ("same_color", vec!["a.hpp", "b.hpp"]),
        ("macro_conflict", vec!["a.hpp", "b.hpp", "b_conflict.hpp"]),
    ];
    let mut evidence = format!(
        "pid={}\nruntime={:?}\nargs={ARGS:?}\nfirst={first:?}\nsecond={second:?}\n",
        std::process::id(),
        std::env::var_os("LIBCLANG_PATH"),
    );
    let mut results = Vec::new();
    let mut calls = 0;
    let mut tu_visits = 0;
    for (case, wrappers, reversed) in cases
        .into_iter()
        .flat_map(|(case, wrappers)| [false, true].map(move |r| (case, wrappers.clone(), r)))
    {
        let mut wrappers = wrappers;
        if reversed {
            wrappers.reverse();
        }
        let excludes = matches!(case, "excluded" | "excluded_foreign" | "macro_conflict");
        let shared_color = case == "same_color";
        let first_owner = RootPartition::new(
            "A",
            if shared_color {
                "Test.Shared"
            } else {
                "Test.A"
            },
        );
        let first_owner = if excludes {
            first_owner.with_exclusion("AUDIO_CURVE_TYPE")
        } else {
            first_owner
        };
        let second_owner = RootPartition::new(
            "B",
            if shared_color {
                "Test.Shared"
            } else {
                "Test.B"
            },
        );
        let mut policy = HeaderPartitionPolicy::new();
        let mut inputs = Vec::new();
        writeln!(evidence, "\nCASE={case} reversed={reversed}").unwrap();
        for wrapper in &wrappers {
            let path = root.join(wrapper);
            let input = Input::new(
                path.to_str().unwrap(),
                std::fs::read_to_string(&path).unwrap(),
            )
            .with_roots([first.to_str().unwrap(), second.to_str().unwrap()])
            .partitioned(*wrapper);
            let (header, owner) = if wrapper.starts_with('b') {
                (&second, &second_owner)
            } else {
                (&first, &first_owner)
            };
            policy.add_traversed_header_for_input(
                input.input.name.clone(),
                header.to_str().unwrap(),
                owner.clone(),
            );
            writeln!(
                evidence,
                "raw_absolute_wrapper={path:?} qualifier={:?} header={header:?} \
                 caller={wrapper:?} original_roots={:?}",
                input.input.name, input.roots,
            )
            .unwrap();
            inputs.push(input);
        }
        writeln!(evidence, "inputs={inputs:#?}\npolicy={policy:#?}").unwrap();
        calls += 1;
        tu_visits += inputs.len();
        let extracted = extract_partitioned_with_options(
            inputs,
            ARGS,
            &ExtractionOptions::new().with_parallelism(1),
        );
        let mut aliases = 0;
        let mut exact_aliases = 0;
        let mut suppressed_a = 0;
        let mut retained_b = 0;
        let mut associated_slots = 0;
        let (flat, colored) = match extracted {
            Err(error) => {
                let message = error.to_string();
                writeln!(evidence, "extraction_error={message}").unwrap();
                (Err(message.clone()), Err(message))
            }
            Ok(snapshot) => {
                writeln!(evidence, "raw_native_snapshot={snapshot:#?}").unwrap();
                for alias in snapshot
                    .facts
                    .iter()
                    .filter(|fact| fact.name == "AUDIO_CURVE_TYPE")
                    .filter(|fact| matches!(fact.data, FactData::Typedef { .. }))
                {
                    aliases += 1;
                    if let FactData::Typedef {
                        target: TypeRef::Named { name, declaration },
                    } = &alias.data
                    {
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
                        exact_aliases += usize::from(targets.len() == 1);
                        writeln!(
                            evidence,
                            "native_alias={alias:#?}\nexact_local_enum_targets={targets:#?}"
                        )
                        .unwrap();
                    }
                }
                associated_slots = snapshot
                    .annotations
                    .iter()
                    .filter(|(_, annotations)| {
                        annotations.iter().any(|annotation| {
                            matches!(
                                annotation,
                                Annotation::AssociatedEnum(name) if name == "AUDIO_CURVE_TYPE"
                            )
                        })
                    })
                    .count();
                let flat = snapshot
                    .emit_with_options(&options)
                    .map_err(|e| e.to_string());
                let colored = snapshot
                    .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                    .map_err(|e| e.to_string())
                    .and_then(|plan| {
                        writeln!(
                            evidence,
                            "header_plan_snapshot={:#?}\nroot_conflicts={:#?}",
                            plan.snapshot, plan.root_conflicts,
                        )
                        .unwrap();
                        let (prepared, display_names, source_names) = plan
                            .snapshot
                            .clone()
                            .into_partitioned_planning_snapshot(&options);
                        writeln!(
                            evidence,
                            "prepared_snapshot={prepared:#?}\ndisplay_names={display_names:#?}\n\
                             source_fact_names={:#?}\nsource_constant_names={:#?}",
                            source_names.facts, source_names.constants,
                        )
                        .unwrap();
                        for fact in &prepared.facts {
                            if !matches!(
                                fact.data,
                                FactData::Enum { .. } | FactData::Typedef { .. }
                            ) {
                                continue;
                            }
                            let owner = prepared.root_owners.get(&fact.origin);
                            let suppressed =
                                prepared.suppressed_type_origins.contains(&fact.origin);
                            writeln!(
                                evidence,
                                "prepared_type={fact:#?} owner={owner:#?} suppressed={suppressed}"
                            )
                            .unwrap();
                            if owner.is_some_and(|owner| owner.partition == "A")
                                && suppressed
                                && !fact.root
                            {
                                suppressed_a += 1;
                            }
                            if owner.is_some_and(|owner| owner.partition == "B")
                                && !suppressed
                                && fact.root
                            {
                                retained_b += 1;
                            }
                        }
                        let selection = prepared
                            .plan_partitioned(&options, &display_names, &source_names)
                            .map(|plan| {
                                plan.types
                                    .into_iter()
                                    .map(|planned| (planned.name, planned.fact))
                                    .collect::<Vec<_>>()
                            })
                            .map_err(|e| e.to_string());
                        writeln!(evidence, "actual_prepared_type_selection={selection:#?}")
                            .unwrap();
                        plan.emit_with_options(&options).map_err(|e| e.to_string())
                    });
                (flat, colored)
            }
        };
        writeln!(
            evidence,
            "flat={flat:#?}\ncolored={colored:#?}\naliases={aliases} \
             exact_aliases={exact_aliases} suppressed_a={suppressed_a} retained_b={retained_b} \
             associated_slots={associated_slots}"
        )
        .unwrap();
        results.push((
            case,
            reversed,
            wrappers.len(),
            flat,
            colored,
            aliases,
            exact_aliases,
            suppressed_a,
            retained_b,
            associated_slots,
        ));
    }
    writeln!(
        evidence,
        "native_calls={calls} authored_tu_visits={tu_visits}"
    )
    .unwrap();
    println!("{evidence}");
    assert_eq!(calls, 10);
    assert_eq!(tu_visits, 26);
    assert_ne!(
        std::fs::canonicalize(&first).unwrap(),
        std::fs::canonicalize(&second).unwrap(),
    );
    for (case, _, tus, _, colored, aliases, joins, _, _, slots) in &results {
        assert_eq!(aliases, tus, "{case}: missing native typedefs");
        assert_eq!(
            joins, tus,
            "{case}: missing exact TU-local native enum targets"
        );
        assert!(*slots > 0, "{case}: missing actual selected B annotation");
        if matches!(*case, "same_color" | "macro_conflict") {
            let error = colored.as_ref().unwrap_err();
            assert!(error.contains("ambiguous type root"), "{case}: {error}");
            assert!(error.contains("AUDIO_CURVE_TYPE"), "{case}: {error}");
            assert!(error.contains("Enum/Enum"), "{case}: {error}");
        }
    }
    let mut excluded_output = None;
    for (case, _, tus, _, colored, _, _, suppressed, retained, _) in &results {
        if matches!(*case, "same_color" | "macro_conflict") {
            continue;
        }
        let colored = colored
            .as_ref()
            .unwrap_or_else(|error| panic!("{case}: {error}"));
        let enums: Vec<_> = colored
            .iter()
            .filter(|(_, rdl)| rdl.contains("enum AUDIO_CURVE_TYPE"))
            .collect();
        if *case == "distinct_colors" {
            assert_eq!(enums.len(), 2, "{case}: {colored:#?}");
            assert!(enums.iter().any(|(route, _)| route.partition == "A"));
            assert!(enums.iter().any(|(route, _)| route.partition == "B"));
        } else {
            assert_eq!(*suppressed, 2 * (tus - 1), "{case}");
            assert_eq!(*retained, 2, "{case}");
            assert_eq!(enums.len(), 1, "{case}: {colored:#?}");
            assert_eq!(enums[0].0.partition, "B");
            assert_eq!(enums[0].0.namespace, "Test.B");
            assert_eq!(enums[0].0.header, normalize_name(second.to_str().unwrap()));
            if let Some(expected) = excluded_output {
                assert_eq!(colored, expected, "{case}");
            } else {
                excluded_output = Some(colored);
            }
        }
        for (_, rdl) in enums {
            assert!(rdl.contains("AUDIO_CURVE_TYPE_NONE = 0"), "{rdl}");
            assert!(rdl.contains("AUDIO_CURVE_TYPE_WINDOWS_FADE = 1"), "{rdl}");
        }
        let b = colored
            .iter()
            .find(|(route, _)| route.partition == "B")
            .unwrap()
            .1;
        assert!(b.contains("struct B_ROOT"), "{b}");
        assert!(b.contains("associated_enum(\"AUDIO_CURVE_TYPE\")"), "{b}");
    }
    for pair in results.chunks_exact(2) {
        assert_eq!(pair[0].4, pair[1].4, "{}: reversed output/error", pair[0].0);
    }
}

#[test]
fn guarded_enum_exact_source_dependency_exclusion() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("guarded_enum");
    let a = root.join("a").join("Curve.h");
    let b = root.join("b").join("Curve.h");
    let references = BTreeMap::new();
    let excluded_types = BTreeSet::from(["B_ROOT".to_string()]);
    let mut options = EmitOptions::new("Test.Common", &references);
    options.excluded_types = Some(&excluded_types);
    for case in ["excluded_a", "retained_a", "native_b"] {
        for reversed in [false, true] {
            let foreign_wrapper = if case == "native_b" {
                "foreign_b_ref.hpp"
            } else {
                "foreign_ref.hpp"
            };
            let foreign = root.join(foreign_wrapper);
            let mut wrappers = ["a.hpp", "b.hpp", foreign_wrapper];
            if reversed {
                wrappers.swap(0, 1);
            }
            let mut inputs = Vec::new();
            let mut qualifiers = BTreeMap::new();
            let mut policy = HeaderPartitionPolicy::new();
            for wrapper in wrappers {
                let path = root.join(wrapper);
                let input = Input::new(
                    path.to_str().unwrap(),
                    std::fs::read_to_string(&path).unwrap(),
                )
                .with_roots([
                    a.to_str().unwrap(),
                    b.to_str().unwrap(),
                    foreign.to_str().unwrap(),
                ])
                .partitioned(wrapper);
                let (header, mut owner) = match wrapper {
                    "a.hpp" => (&a, RootPartition::new("A", "Test.A")),
                    "b.hpp" => (&b, RootPartition::new("B", "Test.B")),
                    _ => (&foreign, RootPartition::new("Foreign", "Test.Foreign")),
                };
                if (wrapper == "a.hpp" && case != "retained_a")
                    || (wrapper == "b.hpp" && case == "retained_a")
                {
                    owner = owner.with_exclusion("AUDIO_CURVE_TYPE");
                }
                qualifiers.insert(wrapper, input.input.name.clone());
                policy.add_traversed_header_for_input(
                    input.input.name.clone(),
                    header.to_str().unwrap(),
                    owner,
                );
                inputs.push(input);
            }
            let snapshot = extract_partitioned_with_options(
                inputs,
                ARGS,
                &ExtractionOptions::new().with_parallelism(1),
            )
            .unwrap();
            let referrers: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| fact.name == "FOREIGN_CURVE_REF")
                .collect();
            let [referrer] = referrers.as_slice() else {
                panic!("missing exact foreign referrer");
            };
            let FactData::Record { fields, .. } = &referrer.data else {
                panic!("foreign referrer is not a record");
            };
            let TypeRef::Named { name, declaration } = &fields[0].ty else {
                panic!("foreign field is not a native named dependency");
            };
            let aliases: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| {
                    fact.origin.tu == referrer.origin.tu
                        && fact.name == *name
                        && fact.spelling == *declaration
                })
                .collect();
            let [alias] = aliases.as_slice() else {
                panic!("foreign field does not have one exact native alias");
            };
            let FactData::Typedef {
                target: TypeRef::Named { name, declaration },
            } = &alias.data
            else {
                panic!("foreign alias does not have a native declaration target");
            };
            let targets: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| {
                    fact.origin.tu == alias.origin.tu
                        && fact.name == *name
                        && fact.spelling == *declaration
                        && matches!(fact.data, FactData::Enum { .. })
                })
                .collect();
            let [target] = targets.as_slice() else {
                panic!("foreign alias does not have one exact native enum target");
            };
            let provider = if case == "native_b" { "b.hpp" } else { "a.hpp" };
            assert!(snapshot.facts.iter().any(|fact| {
                fact.origin.tu == qualifiers[provider] && same_source_declaration(target, fact)
            }));
            let referrer_origin = referrer.origin.clone();
            let referrer_location = referrer.spelling.clone();
            let alias_origin = alias.origin.clone();
            let target_origin = target.origin.clone();
            let alias_location = alias.spelling.clone();
            let enum_location = target.spelling.clone();
            let header_plan = snapshot
                .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                .unwrap();
            assert!(
                !header_plan
                    .snapshot
                    .root_owners
                    .contains_key(&target_origin)
            );
            let outcome = header_plan.emit_with_options(&options);
            let evidence = if case == "excluded_a" {
                let error = outcome.unwrap_err().to_string();
                assert!(
                    error.contains("owner-excluded native dependency `AUDIO_CURVE_TYPE`"),
                    "{error}"
                );
                assert!(
                    error.contains("partition `A` namespace `Test.A`"),
                    "{error}"
                );
                assert!(error.contains("FOREIGN_CURVE_REF"), "{error}");
                assert!(error.contains("exact dependency path"), "{error}");
                assert!(
                    error.contains(&format!(
                        "{}:{}",
                        alias_location.file, alias_location.offset
                    )),
                    "{error}"
                );
                assert!(
                    error.contains(&format!("{}:{}", enum_location.file, enum_location.offset)),
                    "{error}"
                );
                assert!(
                    error.starts_with("header partition dependency closure found "),
                    "{error}"
                );
                assert!(
                    error.ends_with(
                        "later layout, ownership, and RDL validation has not run for this failed plan"
                    ),
                    "{error}"
                );
                assert!(!error.contains("ambiguous type root"), "{error}");
                format!("Err({error})")
            } else {
                let output = outcome.unwrap();
                let provider = if case == "native_b" { "B" } else { "A" };
                let enums: Vec<_> = output
                    .iter()
                    .filter(|(_, rdl)| rdl.contains("enum AUDIO_CURVE_TYPE"))
                    .collect();
                assert_eq!(enums.len(), 1, "{output:#?}");
                assert_eq!(enums[0].0.partition, provider);
                assert!(output.iter().any(|(route, rdl)| {
                    route.partition == "Foreign" && rdl.contains("struct FOREIGN_CURVE_REF")
                }));
                format!("Ok({output:#?})")
            };
            println!(
                "\nDEPENDENCY_CASE={case} reversed={reversed}\n\
                 native_referrer={referrer_origin:?} at {}:{} -> \
                 native_alias={alias_origin:?} at {}:{} -> \
                 native_enum={target_origin:?} at {}:{}\n\
                 actual_outcome={evidence}",
                referrer_location.file,
                referrer_location.offset,
                alias_location.file,
                alias_location.offset,
                enum_location.file,
                enum_location.offset,
            );
        }
    }
}

mod excluded_enum_raw_context {
    use super::*;

    fn record() -> FactData {
        FactData::Record {
            base: None,
            fields: Vec::new(),
            size: 4,
            align: 4,
            packing: None,
            alignment: None,
            union: false,
        }
    }

    fn owner(input: &str, excluded: bool) -> RootOwner {
        RootOwner {
            input: input.to_string(),
            root: "enum.h".to_string(),
            partition: input.to_string(),
            namespace: format!("Test.{input}"),
            remaps: BTreeMap::new(),
            exclusions: if excluded {
                BTreeSet::from(["E".to_string()])
            } else {
                BTreeSet::new()
            },
            libraries: BTreeMap::new(),
            u32_types: BTreeSet::new(),
            flags: BTreeSet::new(),
            preserved_auto_function_pointer_levels: BTreeSet::new(),
            exclude_empty_records: false,
        }
    }

    fn sample() -> Snapshot {
        let mut facts = Vec::new();
        for tu in ["owned", "foreign"] {
            let parent = test_fact_in_tu(
                tu,
                1,
                None,
                FactKind::Struct,
                "Parent",
                Location {
                    file: "parent.h".to_string(),
                    offset: 4,
                },
                record(),
            );
            let mut enumeration = test_fact_in_tu(
                tu,
                2,
                Some(parent.origin.clone()),
                FactKind::Enum,
                "E",
                Location {
                    file: "enum.h".to_string(),
                    offset: 10,
                },
                FactData::Enum {
                    repr: Scalar::I32,
                    variants: Vec::new(),
                    fixed: false,
                    scoped: false,
                },
            );
            enumeration.root = tu == "owned";
            facts.extend([parent, enumeration]);
        }
        Snapshot {
            root_owners: BTreeMap::from([(facts[1].origin.clone(), owner("owned", true))]),
            facts,
            constants: Vec::new(),
            value_declarations: Vec::new(),
            included_files: Vec::new(),
            declare_handles: Vec::new(),
            annotations: BTreeMap::new(),
            source_annotations: BTreeMap::new(),
            function_annotations: BTreeMap::new(),
            sal_constant_sizes: BTreeMap::new(),
            declaration_guids: BTreeMap::new(),
            class_canonical_origins: BTreeMap::new(),
            class_canonical_anchors: BTreeMap::new(),
            shared_enum_identities: BTreeMap::new(),
            observation_only_class_origins: BTreeSet::new(),
            pointer_callback_aliases: BTreeSet::new(),
            pointer_only_class_layouts: BTreeMap::new(),
            embeddable_class_layouts: BTreeSet::new(),
            clang_flag_enums: BTreeSet::new(),
            constant_root_owners: BTreeMap::new(),
            root_partitions: BTreeMap::new(),
            partition_inputs: BTreeMap::new(),
            input_order: BTreeMap::new(),
            partition_exclusions: Vec::new(),
            forced_flags: BTreeSet::new(),
            suppressed_type_origins: BTreeSet::new(),
            projected_type_names: BTreeMap::new(),
            namespace_authorities: BTreeMap::new(),
            fact_namespace_authorities: BTreeMap::new(),
            constant_namespace_authorities: BTreeMap::new(),
            header_partition_policy: true,
            header_authority_partition: None,
            timing_target: None,
        }
    }

    fn conflicting(snapshot: &Snapshot) -> bool {
        PlanningSourceNames::new(snapshot).excluded_native_dependencies[&Origin {
            tu: "foreign".to_string(),
            local: 2,
        }]
            .conflicting
    }

    #[test]
    fn strict_match() {
        let snapshot = sample();
        let names = PlanningSourceNames::new(&snapshot);
        assert_eq!(names.exclusion_index_sizes, (1, 2));
        assert!(!conflicting(&snapshot));
        assert_eq!(names.facts.len(), snapshot.facts.len());
    }

    #[test]
    fn named_parent_source_and_data_disagreement() {
        for changed in ["source", "data", "definition", "kind"] {
            let mut snapshot = sample();
            let parent = &mut snapshot.facts[2];
            match changed {
                "source" => parent.spelling.offset += 1,
                "data" => {
                    parent.data = FactData::Class {
                        guid: "different".to_string(),
                    }
                }
                "definition" => parent.definition = false,
                "kind" => parent.kind = FactKind::Class,
                _ => unreachable!(),
            }
            assert!(conflicting(&snapshot), "{changed}");
        }
    }

    #[test]
    fn missing_cyclic_anonymous_and_cross_tu() {
        for changed in ["missing", "cycle", "anonymous", "cross_tu", "duplicate"] {
            let mut snapshot = sample();
            match changed {
                "missing" => snapshot.facts.retain(|fact| fact.kind == FactKind::Enum),
                "cycle" => {
                    for index in [0, 2] {
                        snapshot.facts[index].parent = Some(snapshot.facts[index].origin.clone());
                    }
                }
                "anonymous" => {
                    for index in [0, 2] {
                        snapshot.facts[index].name.clear();
                    }
                }
                "cross_tu" => snapshot.facts[3].parent = Some(snapshot.facts[0].origin.clone()),
                "duplicate" => snapshot.facts.push(snapshot.facts[2].clone()),
                _ => unreachable!(),
            }
            assert!(conflicting(&snapshot), "{changed}");
        }
    }

    #[test]
    fn private_specializations() {
        for generated in [false, true] {
            let mut snapshot = sample();
            for index in [0, 2] {
                let parent = &snapshot.facts[index];
                let anchor = Origin {
                    tu: parent.origin.tu.clone(),
                    local: 99,
                };
                let canonical = if generated {
                    CanonicalRecord::CompilerGenerated {
                        anchor,
                        kind: CanonicalRecordKind::Struct,
                    }
                } else {
                    snapshot.class_canonical_anchors.insert(
                        anchor.clone(),
                        ClassCanonicalAnchor {
                            kind: CanonicalRecordKind::Struct,
                            parent: None,
                            name: parent.name.clone(),
                            spelling: parent.spelling.clone(),
                            expansion: parent.expansion.clone(),
                            main_file: false,
                            system: false,
                            template_name: "Template".to_string(),
                            template_spelling: parent.spelling.clone(),
                            template_expansion: parent.expansion.clone(),
                            template_partial: false,
                        },
                    );
                    CanonicalRecord::Source(anchor)
                };
                snapshot
                    .class_canonical_origins
                    .insert(parent.origin.clone(), canonical);
            }
            assert!(conflicting(&snapshot));
        }
    }

    #[test]
    fn inactive_work() {
        for active in [false, true] {
            let mut snapshot = sample();
            snapshot.header_partition_policy = active;
            if active {
                snapshot
                    .root_owners
                    .values_mut()
                    .for_each(|owner| owner.exclusions.clear());
            }
            let names = PlanningSourceNames::new(&snapshot);
            assert_eq!(names.exclusion_index_sizes, (0, 0));
            assert!(names.excluded_native_dependencies.is_empty());
            assert_eq!(names.facts.len(), snapshot.facts.len());
        }
    }

    #[test]
    fn retains_nonexcluding_owned_candidates() {
        let mut snapshot = sample();
        let mut parent = snapshot.facts[0].clone();
        parent.origin.tu = "retained".to_string();
        let mut enumeration = snapshot.facts[1].clone();
        enumeration.origin.tu = "retained".to_string();
        enumeration.parent = Some(parent.origin.clone());
        snapshot
            .root_owners
            .insert(enumeration.origin.clone(), owner("retained", false));
        snapshot.facts.extend([parent, enumeration]);
        let names = PlanningSourceNames::new(&snapshot);
        assert_eq!(names.exclusion_index_sizes, (2, 3));
        assert!(conflicting(&snapshot));
        assert_eq!(
            names
                .excluded_native_dependencies
                .values()
                .next()
                .unwrap()
                .authorities
                .len(),
            2
        );
    }

    #[test]
    fn capture_precedes_pointer_class_promotion() {
        let mut snapshot = sample();
        let parent = snapshot.facts[0].clone();
        let foreign_parent_origin = snapshot.facts[2].origin.clone();
        snapshot.facts[0].data = FactData::Unsupported {
            reason: "raw parent".to_string(),
        };
        snapshot
            .pointer_only_class_layouts
            .insert(parent.origin.clone(), record());
        let function = test_fact_in_tu(
            "owned",
            3,
            None,
            FactKind::Function,
            "Selected",
            Location {
                file: "function.h".to_string(),
                offset: 1,
            },
            FactData::Function {
                link_name: "Selected".to_string(),
                convention: CallingConvention::Platform,
                params: vec![Parameter {
                    name: "parent".to_string(),
                    ty: TypeRef::Pointer {
                        mutable: true,
                        target: Box::new(TypeRef::Named {
                            name: parent.name.clone(),
                            declaration: parent.spelling.clone(),
                        }),
                    },
                    annotation: ParamAnnotation::default(),
                }],
                result: TypeRef::Void,
                variadic: false,
                noreturn: false,
            },
        );
        snapshot
            .root_owners
            .insert(function.origin.clone(), owner("owned", false));
        snapshot.facts.push(function);
        assert!(conflicting(&snapshot));
        let references = BTreeMap::new();
        let functions = BTreeSet::from(["Selected".to_string()]);
        let mut options = EmitOptions::new("Test", &references);
        options.functions = Some(&functions);
        let (prepared, _, names) = snapshot.into_partitioned_planning_snapshot(&options);
        let promoted = prepared
            .facts
            .iter()
            .find(|fact| fact.origin == parent.origin)
            .unwrap();
        let foreign = prepared
            .facts
            .iter()
            .find(|fact| fact.origin == foreign_parent_origin)
            .unwrap();
        assert_eq!(promoted.data, foreign.data);
        assert!(
            names
                .excluded_native_dependencies
                .values()
                .next()
                .unwrap()
                .conflicting
        );
        assert_eq!(names.facts[&parent.origin], parent.name);
    }
}
