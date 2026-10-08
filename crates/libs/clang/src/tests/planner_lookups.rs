use super::*;

fn snapshot(facts: Vec<Fact>) -> Snapshot {
    Snapshot {
        facts,
        constants: Vec::new(),
        included_files: Vec::new(),
        declare_handles: Vec::new(),
        annotations: BTreeMap::new(),
        declaration_guids: BTreeMap::new(),
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
