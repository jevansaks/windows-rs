use super::*;

fn snapshot(facts: Vec<Fact>) -> Snapshot {
    Snapshot {
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
        pointer_callback_aliases: BTreeSet::new(),
        pointer_only_class_layouts: BTreeMap::new(),
        embeddable_class_layouts: BTreeSet::new(),
        clang_flag_enums: BTreeSet::new(),
        root_owners: BTreeMap::new(),
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
        header_partition_policy: false,
        header_authority_partition: None,
        timing_target: None,
    }
}

fn alias(local: u32, name: &str, file: &str, offset: u32, target: TypeRef) -> Fact {
    test_fact(
        local,
        None,
        FactKind::Typedef,
        name,
        Location {
            file: file.to_string(),
            offset,
        },
        FactData::Typedef { target },
    )
}

fn function(tu: &str, local: u32, name: &str, file: &str, parameter: &str, input: bool) -> Fact {
    test_fact_in_tu(
        tu,
        local,
        None,
        FactKind::Function,
        name,
        Location {
            file: file.to_string(),
            offset: local,
        },
        FactData::Function {
            link_name: name.to_string(),
            convention: CallingConvention::Platform,
            params: vec![Parameter {
                name: parameter.to_string(),
                ty: TypeRef::Scalar(Scalar::U32),
                annotation: ParamAnnotation {
                    input,
                    ..Default::default()
                },
            }],
            result: TypeRef::Scalar(Scalar::I32),
            variadic: false,
            noreturn: false,
        },
    )
}

fn named(fact: &Fact) -> TypeRef {
    TypeRef::Named {
        name: fact.name.clone(),
        declaration: fact.spelling.clone(),
    }
}

fn authority<'a>(snapshot: &'a Snapshot, fact: &Fact) -> Option<&'a str> {
    snapshot
        .fact_authority_namespace(fact, &DeclarationIndex::new(&snapshot.facts))
        .map(String::as_str)
}

fn suppressed(snapshot: &Snapshot, fact: &Fact) -> bool {
    snapshot.type_projection_suppressed(
        fact,
        &DeclarationIndex::new(&snapshot.facts),
        &mut BTreeSet::new(),
    )
}

#[test]
fn sal_count_observations_inspect_only_the_selected_source_bucket() {
    const LOOKUPS: usize = 256;
    for unrelated in [0, 1_000, 20_000] {
        let first = function("first", 1, "Selected", "common.h", "value", true);
        let mut second = first.clone();
        second.origin.tu = "second".to_string();
        let target = AnnotationTarget::Parameter {
            declaration: first.origin.clone(),
            index: 0,
        };
        let mut facts = vec![first, second];
        facts.extend((0..unrelated).map(|index| {
            function(
                "first",
                index + 10,
                "Unrelated",
                "unrelated.h",
                "value",
                true,
            )
        }));
        let mut snapshot = snapshot(facts);
        for fact in &snapshot.facts {
            snapshot.sal_constant_sizes.insert(
                AnnotationTarget::Parameter {
                    declaration: fact.origin.clone(),
                    index: 0,
                },
                Ok(SalSize {
                    bytes: false,
                    value: SalSizeValue::Constant(8),
                }),
            );
        }
        let facts = snapshot
            .facts
            .iter()
            .map(|fact| (&fact.origin, fact))
            .collect();
        let start = std::time::Instant::now();
        let observations = SalConstantObservations::new(&snapshot.sal_constant_sizes, &facts);
        let build_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(observations.by_use.len(), unrelated as usize + 1);
        let size = snapshot.sal_constant_sizes.get(&target).unwrap();
        let start = std::time::Instant::now();
        for _ in 0..LOOKUPS {
            assert_eq!(
                snapshot
                    .selected_sal_constant_size(&target, size, &facts, &observations)
                    .unwrap()
                    .as_ref()
                    .unwrap(),
                size.as_ref().unwrap(),
            );
        }
        assert_eq!(observations.inspected.get(), 2 * LOOKUPS);
        eprintln!(
            "sal-count-lookup observations={} bucket=2 lookups={LOOKUPS} inspected={} \
             build_ms={build_ms:.3} lookup_ms={:.3}",
            snapshot.sal_constant_sizes.len(),
            observations.inspected.get(),
            start.elapsed().as_secs_f64() * 1000.0,
        );
    }
}

#[test]
fn declaration_index_keeps_all_matches_in_snapshot_order() {
    let first = alias(8, "TARGET", "types.h", 7, TypeRef::Scalar(Scalar::U32));
    let mut second = first.clone();
    second.origin.local = 2;
    second.kind = FactKind::Struct;
    second.data = FactData::None;
    second.definition = false;
    second.root = false;
    let mut same_origin = first.clone();
    same_origin.data = FactData::None;
    let mut other_tu = first.clone();
    other_tu.origin.tu = "other".to_string();
    let mut other_name = first.clone();
    other_name.name = "OTHER".to_string();
    let mut other_location = first.clone();
    other_location.spelling.offset += 1;
    let mut facts = vec![
        first,
        other_tu,
        second,
        other_name,
        same_origin,
        other_location,
    ];
    for _ in 0..facts.len() {
        let index = DeclarationIndex::new(&facts);
        for query in &facts {
            let expected: Vec<_> = facts
                .iter()
                .filter(|fact| {
                    fact.name == query.name
                        && fact.origin.tu == query.origin.tu
                        && fact.spelling == query.spelling
                })
                .collect();
            assert_eq!(
                index.get(&query.origin.tu, &query.name, &query.spelling),
                expected
            );
        }
        assert!(
            index
                .get("missing", "TARGET", &facts[0].spelling)
                .is_empty()
        );
        facts.rotate_left(1);
    }
}

fn declaration_uuid_full_scan<'a>(
    snapshot: &'a Snapshot,
    name: &str,
    tu: &str,
    declaration: &Location,
) -> Option<&'a str> {
    let guids: BTreeSet<_> = snapshot
        .facts
        .iter()
        .filter(|fact| fact.name == name && fact.origin.tu == tu && fact.spelling == *declaration)
        .filter_map(|fact| snapshot.fact_uuid(fact))
        .collect();
    (guids.len() == 1).then(|| *guids.first().unwrap())
}

#[test]
fn declaration_uuid_preserves_all_exact_observations() {
    const GUID: &str = "11111111-2222-3333-4455-66778899aabb";
    const OTHER_GUID: &str = "aaaaaaaa-bbbb-cccc-ddee-ff0011223344";
    let declaration = Location {
        file: "types.h".to_string(),
        offset: 7,
    };
    let first = test_fact(
        1,
        None,
        FactKind::Struct,
        "TARGET",
        declaration.clone(),
        FactData::None,
    );
    let mut wrong_tu = first.clone();
    wrong_tu.origin.tu = "other".to_string();
    let mut wrong_name = first.clone();
    wrong_name.name = "OTHER".to_string();
    let mut wrong_file = first.clone();
    wrong_file.spelling.file = "Types.h".to_string();
    let mut wrong_offset = first.clone();
    wrong_offset.spelling.offset += 1;
    let mut facts = vec![first.clone()];
    for (index, mut fact) in [wrong_tu, wrong_name, wrong_file, wrong_offset]
        .into_iter()
        .enumerate()
    {
        fact.origin.local = index as u32 + 2;
        fact.data = FactData::Class {
            guid: OTHER_GUID.to_string(),
        };
        facts.push(fact);
    }
    let mut snapshot = snapshot(facts);
    for (stage, expected) in [None, Some(GUID), Some(GUID), None].into_iter().enumerate() {
        if stage > 0 {
            let mut late = first.clone();
            late.origin.local = stage as u32 + 10;
            late.root = false;
            late.definition = false;
            late.system = true;
            late.expansion.file = "invocation.h".to_string();
            match stage {
                1 => {
                    late.kind = FactKind::Class;
                    late.data = FactData::Class {
                        guid: GUID.to_string(),
                    };
                    snapshot
                        .declaration_guids
                        .insert(late.origin.clone(), OTHER_GUID.to_string());
                }
                2 => {
                    late.data = FactData::Interface {
                        base: None,
                        guid: Some(GUID.to_string()),
                        methods: Vec::new(),
                    };
                    let mut sidecar = late.clone();
                    sidecar.origin.local = 20;
                    sidecar.data = FactData::None;
                    snapshot
                        .declaration_guids
                        .insert(sidecar.origin.clone(), GUID.to_string());
                    snapshot.facts.push(sidecar);
                }
                3 => {
                    snapshot
                        .declaration_guids
                        .insert(late.origin.clone(), OTHER_GUID.to_string());
                }
                _ => unreachable!(),
            }
            snapshot.facts.push(late);
        }
        for _ in 0..snapshot.facts.len() {
            let declarations = DeclarationIndex::new(&snapshot.facts);
            assert_eq!(
                declaration_uuid(&snapshot, &declarations, "TARGET", "tu", &declaration),
                expected
            );
            for fact in &snapshot.facts {
                assert_eq!(
                    declaration_uuid(
                        &snapshot,
                        &declarations,
                        &fact.name,
                        &fact.origin.tu,
                        &fact.spelling
                    ),
                    declaration_uuid_full_scan(
                        &snapshot,
                        &fact.name,
                        &fact.origin.tu,
                        &fact.spelling
                    )
                );
            }
            assert_eq!(
                declaration_uuid(&snapshot, &declarations, "MISSING", "tu", &declaration),
                None
            );
            snapshot.facts.rotate_left(1);
        }
    }
}

#[test]
fn declaration_uuid_inspects_only_the_exact_bucket() {
    const LOOKUPS: usize = 256;
    const GUID: &str = "11111111-2222-3333-4455-66778899aabb";
    for unrelated in [0, 1_000, 20_000] {
        let declaration = Location {
            file: "types.h".to_string(),
            offset: 7,
        };
        let first = test_fact(
            1,
            None,
            FactKind::Class,
            "TARGET",
            declaration.clone(),
            FactData::Class {
                guid: GUID.to_string(),
            },
        );
        let mut alternate = first.clone();
        alternate.origin.local = 2;
        alternate.root = false;
        let mut facts = vec![first];
        facts.extend((0..unrelated).map(|index| {
            test_fact(
                index + 10,
                None,
                FactKind::Class,
                "UNRELATED",
                Location {
                    file: "unrelated.h".to_string(),
                    offset: index,
                },
                FactData::Class {
                    guid: GUID.to_string(),
                },
            )
        }));
        facts.push(alternate);
        let snapshot = snapshot(facts);
        let declarations = DeclarationIndex::new(&snapshot.facts);
        let expected = declaration_uuid_full_scan(&snapshot, "TARGET", "tu", &declaration);
        assert_eq!(expected, Some(GUID));
        for _ in 0..LOOKUPS {
            assert_eq!(
                declaration_uuid(&snapshot, &declarations, "TARGET", "tu", &declaration),
                expected
            );
        }
        assert_eq!(declarations.inspected_candidates.get(), 2 * LOOKUPS);
        eprintln!(
            "declaration-uuid facts={} bucket=2 lookups={LOOKUPS} inspected={}",
            snapshot.facts.len(),
            declarations.inspected_candidates.get()
        );
    }
}

#[test]
fn authority_lookup_preserves_exact_identity_and_fact_order() {
    let target = alias(10, "TARGET", "types.h", 7, TypeRef::Scalar(Scalar::U32));
    let root = alias(20, "ROOT", "public.h", 1, named(&target));
    let mut wrong_tu = target.clone();
    wrong_tu.origin.tu = "other".to_string();
    let mut wrong_name = target.clone();
    wrong_name.name = "OTHER".to_string();
    wrong_name.origin.local = 11;
    let mut wrong_file = target.clone();
    wrong_file.spelling.file = "Types.h".to_string();
    wrong_file.origin.local = 12;
    let mut wrong_offset = target.clone();
    wrong_offset.spelling.offset += 1;
    wrong_offset.origin.local = 13;
    let mut first = target.clone();
    first.origin.local = 9;
    first.expansion.file = "macro-invocation.h".to_string();
    let mut second = target.clone();
    second.origin.local = 1;

    let mut snapshot = snapshot(vec![
        wrong_tu,
        wrong_name,
        wrong_file,
        wrong_offset,
        target,
        first,
        second,
    ]);
    for fact in &snapshot.facts {
        if fact.origin.local != 10 || fact.origin.tu != "tu" {
            snapshot.fact_namespace_authorities.insert(
                fact.origin.clone(),
                format!("Example.N{}", fact.origin.local),
            );
        }
    }
    assert_eq!(authority(&snapshot, &root), Some("Example.N9"));
    snapshot.facts.swap(5, 6);
    assert_eq!(authority(&snapshot, &root), Some("Example.N1"));
    snapshot.facts.truncate(5);
    assert_eq!(authority(&snapshot, &root), None);
    assert!(!suppressed(&snapshot, &root));
    snapshot.suppressed_type_origins = snapshot.facts[..4]
        .iter()
        .map(|fact| fact.origin.clone())
        .collect();
    assert!(!suppressed(&snapshot, &root));
    snapshot.facts.pop();
    assert_eq!(authority(&snapshot, &root), None);
    assert!(!suppressed(&snapshot, &root));
    snapshot
        .fact_namespace_authorities
        .insert(root.origin.clone(), "Example.Direct".to_string());
    assert_eq!(authority(&snapshot, &root), Some("Example.Direct"));
}

#[test]
fn projection_lookup_preserves_alias_cycles_and_shared_location_visits() {
    let mut first = alias(1, "FIRST", "types.h", 1, TypeRef::Void);
    let second = alias(2, "SECOND", "types.h", 2, named(&first));
    first.data = FactData::Typedef {
        target: named(&second),
    };
    let root = alias(
        3,
        "ROOT",
        "public.h",
        1,
        TypeRef::Pointer {
            mutable: true,
            target: Box::new(TypeRef::Reference {
                mutable: false,
                target: Box::new(TypeRef::Array {
                    target: Box::new(named(&first)),
                    len: 2,
                }),
            }),
        },
    );
    let mut snapshot = snapshot(vec![first.clone(), second]);
    assert!(!suppressed(&snapshot, &root));
    snapshot.fact_namespace_authorities.insert(
        snapshot.facts[1].origin.clone(),
        "Example.Second".to_string(),
    );
    assert_eq!(authority(&snapshot, &first), Some("Example.Second"));
    assert_eq!(authority(&snapshot, &root), None);
    let indirect = alias(6, "INDIRECT", "public.h", 2, named(&first));
    assert_eq!(authority(&snapshot, &indirect), None);

    let mut duplicate = first.clone();
    duplicate.origin.local = 4;
    snapshot
        .suppressed_type_origins
        .insert(duplicate.origin.clone());
    snapshot.facts.push(duplicate);
    assert!(suppressed(&snapshot, &root));

    let mut same_location = first;
    same_location.origin.local = 5;
    same_location.name = "DIFFERENT_NAME".to_string();
    snapshot
        .suppressed_type_origins
        .insert(same_location.origin.clone());
    snapshot.facts[0].data = FactData::Typedef {
        target: named(&same_location),
    };
    snapshot.facts.pop();
    snapshot.facts.push(same_location);
    // The cycle guard keys on TU and location, not name.
    assert!(!suppressed(&snapshot, &root));
}

fn fixture(unrelated: u32, roots: u32) -> (Snapshot, HeaderPartitionPolicy, NamespaceAuthorities) {
    let mut facts = Vec::new();
    for local in 0..unrelated {
        let mut fact = alias(
            local,
            &format!("UNRELATED_{local:05}"),
            "unrelated.h",
            local,
            TypeRef::Scalar(Scalar::U32),
        );
        fact.root = false;
        facts.push(fact);
    }
    let mut authorities = NamespaceAuthorities::new();
    for index in 0..roots {
        let local = unrelated + index * 2;
        let mut target = alias(
            local,
            &format!("TARGET_{index:05}"),
            "dependency.h",
            index,
            TypeRef::Scalar(Scalar::U32),
        );
        target.root = false;
        let root = alias(
            local + 1,
            &format!("ALIAS_{index:05}"),
            "public.h",
            index,
            named(&target),
        );
        if index % 2 == 0 {
            authorities = authorities.with_exact(&target.name, "Example.Public");
        }
        facts.extend([target, root]);
    }
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header("public.h", RootPartition::new("public", "Example.Public"));
    (snapshot(facts), policy, authorities)
}

fn references() -> BTreeMap<String, TypeReference> {
    BTreeMap::from([(
        "EXTERNAL".to_string(),
        TypeReference::new("Example.External", "EXTERNAL", TypeReferenceKind::Type),
    )])
}

#[test]
fn planner_fixture_preserves_output_and_conflict_order() {
    let (snapshot, policy, authorities) = fixture(32, 4);
    let references = references();
    let options = EmitOptions::new("Windows.Win32", &references);
    let output = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    assert_eq!(output.len(), 2);
    assert_eq!(
        output[&RdlPartition {
            partition: "public".to_string(),
            namespace: "Example.Public".to_string(),
            header: "public.h".to_string(),
        }],
        "#[win32]\nmod Example {\n    mod Public {\n\
         \x20       type ALIAS_00000 = TARGET_00000;\n\
         \x20       type ALIAS_00001 = u32;\n\
         \x20       type ALIAS_00002 = TARGET_00002;\n\
         \x20       type ALIAS_00003 = u32;\n    }\n}\n"
    );
    let ambiguous =
        policy.with_traversed_header("public.h", RootPartition::new("other", "Example.Other"));
    let plan = snapshot
        .plan_header_partitions(&ambiguous, &authorities)
        .unwrap();
    let audit = plan.audit(&options).unwrap();
    assert_eq!(
        audit
            .conflicts()
            .iter()
            .map(|conflict| conflict.name.as_str())
            .collect::<Vec<_>>(),
        ["ALIAS_00000", "ALIAS_00001", "ALIAS_00002", "ALIAS_00003"]
    );
    assert_eq!(
        plan.emit_with_options(&options).unwrap_err().to_string(),
        audit.to_string()
    );
}

#[test]
fn owned_planner_matches_borrowed_output_and_audit() {
    let (snapshot, policy, authorities) = fixture(32, 4);
    let references = references();
    let options = EmitOptions::new("Windows.Win32", &references);
    let borrowed_output = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let owned_output = snapshot
        .clone()
        .into_header_partition_plan(&policy, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    assert_eq!(owned_output, borrowed_output);

    let ambiguous =
        policy.with_traversed_header("public.h", RootPartition::new("other", "Example.Other"));
    let borrowed_audit = snapshot
        .plan_header_partitions(&ambiguous, &authorities)
        .unwrap()
        .audit(&options)
        .unwrap();
    let owned_audit = snapshot
        .into_header_partition_plan(&ambiguous, &authorities)
        .unwrap()
        .audit(&options)
        .unwrap();
    assert_eq!(owned_audit, borrowed_audit);
}

#[test]
fn owned_planner_reuses_snapshot_fact_storage() {
    let (snapshot, policy, authorities) = fixture(32, 4);
    let facts = snapshot.facts.as_ptr();
    let fact_capacity = snapshot.facts.capacity();
    let plan = snapshot
        .into_header_partition_plan(&policy, &authorities)
        .unwrap();
    assert_eq!(plan.snapshot.facts.as_ptr(), facts);
    assert_eq!(plan.snapshot.facts.capacity(), fact_capacity);
}

#[test]
fn function_ambiguity_inventory_is_complete_and_stable() {
    let mut snapshot = snapshot(vec![
        function("first.cpp", 1, "Alpha", "first.h", "alpha_first", false),
        function("second.cpp", 2, "Alpha", "second.h", "alpha_second", true),
        function("first.cpp", 3, "Beta", "first.h", "beta_first", false),
        function("second.cpp", 4, "Beta", "second.h", "beta_second", true),
    ]);
    snapshot.input_order =
        BTreeMap::from([("first.cpp".to_string(), 0), ("second.cpp".to_string(), 1)]);
    let references = BTreeMap::new();
    let plan = snapshot
        .plan(PlanningOptions {
            references: &references,
            excluded_types: None,
            excluded_functions: None,
            excluded_constants: None,
            selected_functions: None,
            display_names: None,
            source_names: None,
            mutable_string_aliases: false,
        })
        .unwrap();
    let ambiguities = plan.function_ambiguities().collect::<Vec<_>>();

    assert_eq!(
        ambiguities
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        ["Alpha", "Beta"]
    );
    assert!(ambiguities.iter().all(|(_, ambiguity)| {
        ambiguity.declarations.len() == 2
            && ambiguity.declarations[0].selected
            && ambiguity.declarations[0].fact.origin.tu == "first.cpp"
    }));
    assert_eq!(
        plan.function_ambiguity_summary().as_deref(),
        Some(
            "windows-clang: function redeclarations count=2 \
             inventory=\"Alpha\"[selected=true input=\"first.cpp\" source=\"first.h:1\", \
             selected=false input=\"second.cpp\" source=\"second.h:2\"]; \
             \"Beta\"[selected=true input=\"first.cpp\" source=\"first.h:3\", \
             selected=false input=\"second.cpp\" source=\"second.h:4\"]"
        )
    );
}

#[test]
#[ignore = "bounded planner timing fixture; run with --release --ignored --nocapture"]
fn planner_lookup_scaling() {
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::time::Instant;

    let references = references();
    let options = EmitOptions::new("Windows.Win32", &references);
    let (snapshot, policy, authorities) = fixture(32, 4);
    let policy =
        policy.with_traversed_header("public.h", RootPartition::new("other", "Example.Other"));
    let diagnostic = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap_err()
        .to_string();
    if let Ok(directory) = std::env::var("WINDOWS_CLANG_LOOKUP_EVIDENCE") {
        std::fs::write(
            std::path::Path::new(&directory).join("diagnostic.txt"),
            diagnostic,
        )
        .unwrap();
    }
    for unrelated in [8_000, 32_000] {
        let (mut snapshot, policy, authorities) = fixture(unrelated, 1_000);
        snapshot.timing_target = Some(format!("lookup-{unrelated}"));
        let mut previous = None;
        for sample in 0..3 {
            let start = Instant::now();
            let output = snapshot
                .plan_header_partitions(&policy, &authorities)
                .unwrap()
                .emit_with_options(&options)
                .unwrap();
            let elapsed = start.elapsed();
            let text = format!("{output:#?}");
            if let Some(previous) = &previous {
                assert_eq!(&text, previous);
            }
            let mut hash = DefaultHasher::new();
            text.hash(&mut hash);
            println!(
                "planner-lookup facts={} roots=1000 sample={sample} elapsed_ms={:.3} bytes={} output_hash={:016x}",
                snapshot.facts.len(),
                elapsed.as_secs_f64() * 1_000.0,
                text.len(),
                hash.finish(),
            );
            if let Ok(directory) = std::env::var("WINDOWS_CLANG_LOOKUP_EVIDENCE") {
                std::fs::write(
                    std::path::Path::new(&directory).join(format!("output-{unrelated}.txt")),
                    &text,
                )
                .unwrap();
            }
            previous = Some(text);
        }
    }
}
