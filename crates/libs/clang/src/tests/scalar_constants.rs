use super::*;

const ARGS: [&str; 6] = [
    "-x",
    "c++",
    "-std=c++17",
    "--target=x86_64-pc-windows-msvc",
    "--no-default-config",
    "-nostdinc",
];

fn header() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("scalar_constants.hpp")
        .to_str()
        .unwrap()
        .to_string()
}

fn capture(reversed: bool) -> Snapshot {
    capture_with_other(reversed, false, false)
}

fn capture_with_other(reversed: bool, other: bool, anchors: bool) -> Snapshot {
    helpers::ensure_libclang();
    let header = header();
    let mut inputs: Vec<_> = [
        ("scalar-first.cpp", "First", 7, "int", ""),
        (
            "scalar-second.cpp",
            "Second",
            13,
            "unsigned int",
            "#define REPORT_ALTERNATIVE\n",
        ),
        ("scalar-equal.cpp", "Equal", 7, "int", ""),
    ]
    .into_iter()
    .map(|(input, partition, value, ty, prefix)| {
        Input::new(
            input,
            format!(
                "{}{prefix}#define REPORT_VALUE {value}\n#define REPORT_TYPE {ty}\n\
                 #include {header:?}\n",
                if anchors {
                    "#define REPORT_SCOPING_ANCHORS\n"
                } else {
                    ""
                }
            ),
        )
        .with_roots([header.as_str()])
        .partitioned(partition)
    })
    .collect();
    if other {
        inputs.push(
            Input::new(
                "scalar-other.hpp",
                format!(
                    "{}const int RENAMED = 99;\n",
                    if anchors {
                        "struct RENAMED { unsigned int other_member; };\n"
                    } else {
                        ""
                    }
                ),
            )
            .partitioned("Other"),
        );
    }
    if reversed {
        inputs.reverse();
    }
    extract_partitioned(inputs, &ARGS).unwrap()
}

fn policy(flipped: bool) -> HeaderPartitionPolicy {
    let header = header();
    HeaderPartitionPolicy::new()
        .with_partition_authority("First", usize::from(flipped), "scalar-first.cpp")
        .with_partition_authority("Second", usize::from(!flipped), "scalar-second.cpp")
        .with_partition_authority("Equal", 2, "scalar-equal.cpp")
        .with_traversed_header(&header, RootPartition::new("First", "Test.First"))
        .with_traversed_header(&header, RootPartition::new("Second", "Test.Second"))
        .with_traversed_header(&header, RootPartition::new("Equal", "Test.Equal"))
}

fn plan(snapshot: Snapshot, flipped: bool) -> HeaderPartitionPlan {
    snapshot
        .into_header_partition_plan(&policy(flipped), &NamespaceAuthorities::new())
        .unwrap()
}

fn emit(plan: HeaderPartitionPlan) -> HeaderPartitionReport {
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("scalar.dll");
    plan.emit_with_report(&options).unwrap()
}

fn without_notes(plan: HeaderPartitionPlan) -> BTreeMap<RdlPartition, String> {
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("scalar.dll");
    let (mut snapshot, display_names, source_names) =
        plan.snapshot.into_partitioned_planning_snapshot(&options);
    snapshot.project_suppressed_declare_handles();
    let mut planned = snapshot
        .plan_partitioned(&options, &display_names, &source_names)
        .unwrap();
    let candidates = snapshot
        .partition_route_candidates(&planned, &source_names, &options)
        .unwrap();
    let routes = snapshot.resolve_partition_routes(candidates).unwrap();
    planned.definition_notes.clear();
    planned.constant_notes.clear();
    snapshot
        .format_partitioned_plan(planned, &options, &routes, &display_names, None)
        .unwrap()
}

fn assert_notes(report: &HeaderPartitionReport, without: &BTreeMap<RdlPartition, String>) {
    let all = report.partitions.values().cloned().collect::<String>();
    let mut stripped = report.partitions.clone();
    for row in &report.definition_evidence {
        let line = format!("// windows-clang: {}\n", row.message);
        let different = matches!(row.outcome, DefinitionComparison::Different { .. });
        assert_eq!(
            all.matches(&line).count(),
            usize::from(different),
            "{row:?}"
        );
        if different {
            for text in stripped.values_mut() {
                let exact_lines: Vec<_> = text
                    .split_inclusive('\n')
                    .filter(|candidate| candidate.trim_start() != line)
                    .collect();
                *text = exact_lines.concat();
            }
        }
    }
    assert_eq!(&stripped, without);
}

fn constant_rows<'a>(report: &'a HeaderPartitionReport, name: &str) -> Vec<&'a DefinitionEvidence> {
    report
        .definition_evidence
        .iter()
        .filter(|row| row.body_kind == DefinitionBodyKind::Constant && row.identity.name == name)
        .collect()
}

fn assert_metadata(report: &HeaderPartitionReport, flipped: bool) {
    let path = std::env::temp_dir().join(format!(
        "scalar-report-{}-{:?}.winmd",
        std::process::id(),
        std::thread::current().id()
    ));
    windows_rdl::reader()
        .input_texts(report.partitions.values())
        .reference_default()
        .output(&path)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&path).unwrap();
    let namespace = if flipped { "Test.Second" } else { "Test.First" };
    let apis = index.expect(namespace, "Apis");
    let value = apis.fields().find(|field| field.name() == "VALUE").unwrap();
    assert_eq!(value.ty(), windows_metadata::Type::I32);
    assert_eq!(
        value.constant().unwrap().value(),
        windows_metadata::Value::I32(if flipped { 13 } else { 7 })
    );
    let typed = apis.fields().find(|field| field.name() == "TYPED").unwrap();
    assert_eq!(
        typed.ty(),
        if flipped {
            windows_metadata::Type::U32
        } else {
            windows_metadata::Type::I32
        }
    );
    let float = apis.fields().find(|field| field.name() == "FLOAT").unwrap();
    assert_eq!(float.ty(), windows_metadata::Type::F32);
    assert_eq!(
        float.constant().unwrap().value(),
        windows_metadata::Value::F32(if flipped { 13.0 } else { 7.0 })
    );
    let function = apis
        .methods()
        .find(|method| method.name() == "UseScalar")
        .unwrap();
    assert_eq!(
        function.impl_map().unwrap().import_scope().name(),
        "scalar.dll"
    );
    assert!(index.contains(namespace, "Named"));
    assert!(!index.contains(if flipped { "Test.First" } else { "Test.Second" }, "Apis"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn scalar_constant_rank_inversion_orders_equal_scopes_and_exact_notes() {
    let mut baselines = BTreeMap::new();
    for reversed in [false, true] {
        let snapshot = capture(reversed);
        let values: Vec<_> = snapshot
            .value_declarations
            .iter()
            .filter(|declaration| declaration.name == "OUT")
            .collect();
        assert_eq!(values.len(), 3);
        let first = values
            .iter()
            .find(|value| value.origin.tu == "scalar-first.cpp")
            .unwrap();
        let second = values
            .iter()
            .find(|value| value.origin.tu == "scalar-second.cpp")
            .unwrap();
        assert!(!first.definition);
        assert!(second.definition);
        assert_ne!(first.spelling.offset, second.spelling.offset);
        let facts = snapshot
            .facts
            .iter()
            .map(|fact| (&fact.origin, fact))
            .collect();
        for declaration in [first, second] {
            let scope = com_pointer::checked_semantic_scope(
                &snapshot,
                &facts,
                &declaration.origin,
                |fact| fact.name.clone(),
                || format!("OUT in {:?}", declaration.origin.tu),
            )
            .unwrap();
            assert_eq!(
                scope,
                vec![
                    (SemanticTypeRole::Namespace, "A".into()),
                    (SemanticTypeRole::Record, "C".into()),
                ]
            );
        }
        for flipped in [false, true] {
            let admitted = without_notes(plan(snapshot.clone(), flipped));
            let report = emit(plan(snapshot.clone(), flipped));
            assert_notes(&report, &admitted);
            assert_metadata(&report, flipped);
            let rows = constant_rows(&report, "VALUE");
            assert_eq!(rows.len(), if flipped { 2 } else { 1 });
            for row in rows {
                assert_eq!(
                    row.identity.scope,
                    vec![(DefinitionTypeRole::Namespace, "Scope".into())]
                );
                assert_eq!(row.identity.role, DefinitionTypeRole::Variable);
                assert_eq!(row.selected.rank, Some(0));
                assert_eq!(row.emitted_provider, None);
                assert_eq!(row.selected_emission, None);
                let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
                    panic!("{row:?}");
                };
                assert!(unknown.is_empty());
                assert_eq!(
                    keys,
                    &[DefinitionDifference {
                        path: "value".into(),
                        selected: if flipped { "13" } else { "7" }.into(),
                        alternative: if flipped { "7" } else { "13" }.into(),
                    }]
                );
            }
            for name in [
                "EQUAL",
                "SECOND",
                "SCOPED",
                "OUT",
                "PROJECTED_MEMBER",
                "ABSENT",
                "POINTER",
            ] {
                assert!(
                    constant_rows(&report, name).is_empty(),
                    "{name}: {report:?}"
                );
            }
            assert_eq!(
                constant_rows(&report, "TYPED").len(),
                if flipped { 2 } else { 1 }
            );
            assert_eq!(
                constant_rows(&report, "MACRO").len(),
                if flipped { 2 } else { 1 }
            );
            let floats = constant_rows(&report, "FLOAT");
            assert_eq!(floats.len(), if flipped { 2 } else { 1 });
            for row in floats {
                let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
                    panic!("{row:?}");
                };
                assert!(unknown.is_empty());
                assert_eq!(
                    keys,
                    &[DefinitionDifference {
                        path: "value".into(),
                        selected: format!(
                            "F32-bits:{:08x}",
                            if flipped { 13f32 } else { 7f32 }.to_bits()
                        ),
                        alternative: format!(
                            "F32-bits:{:08x}",
                            if flipped { 7f32 } else { 13f32 }.to_bits()
                        ),
                    }]
                );
            }
            assert!(report.constant_not_compared.is_empty(), "{report:?}");
            let ConstantEvidenceCoverage::CapturedScalars {
                selected_members,
                captured_observations,
                defining_native_bodies,
                matched_alternative_observations,
                unproved_observations,
            } = report.constant_evidence_coverage
            else {
                panic!("{report:?}")
            };
            assert_eq!(selected_members, 9);
            assert_eq!(
                matched_alternative_observations,
                if flipped { 16 } else { 17 }
            );
            assert_eq!(captured_observations, if flipped { 25 } else { 26 });
            // FIRST and SECOND share one native anonymous-enum defining origin per TU.
            assert!(defining_native_bodies < captured_observations);
            assert_eq!(unproved_observations, 0);
            if let Some(baseline) = baselines.get(&flipped) {
                assert_eq!(&report, baseline);
            } else {
                baselines.insert(flipped, report);
            }
        }
    }
}

#[test]
fn scalar_constant_local_unknown_keeps_independent_difference() {
    let mut snapshot = capture(false);
    for constant in &mut snapshot.constants {
        if constant.root.tu != "scalar-second.cpp" {
            continue;
        }
        match constant.name.as_str() {
            "TYPED" => constant.value = Value::Utf8("private unsupported alternative".into()),
            "VALUE" => constant.ty = TypeRef::Void,
            _ => {}
        }
    }
    let admitted = without_notes(plan(snapshot.clone(), false));
    let report = emit(plan(snapshot, false));
    assert_notes(&report, &admitted);
    assert_metadata(&report, false);
    for (name, difference, unknown_path) in [("TYPED", "type", "value"), ("VALUE", "value", "type")]
    {
        let rows = constant_rows(&report, name);
        assert_eq!(rows.len(), 1);
        let DefinitionComparison::Different { keys, unknown } = &rows[0].outcome else {
            panic!("{:?}", rows[0]);
        };
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].path, difference);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].path, unknown_path);
    }
}

#[test]
fn scalar_constant_unproved_identity_is_not_paired_or_a_veto() {
    let original = capture(false);
    for selected in [false, true] {
        let mut snapshot = original.clone();
        let declaration = snapshot
            .value_declarations
            .iter()
            .find(|declaration| {
                declaration.name == "VALUE"
                    && declaration.origin.tu
                        == if selected {
                            "scalar-first.cpp"
                        } else {
                            "scalar-second.cpp"
                        }
            })
            .unwrap();
        snapshot.semantic_parents.remove(&declaration.origin);
        let admitted = without_notes(plan(snapshot.clone(), false));
        let report = emit(plan(snapshot, false));
        assert_notes(&report, &admitted);
        assert_metadata(&report, false);
        assert!(constant_rows(&report, "VALUE").is_empty());
        let cases: Vec<_> = report
            .constant_not_compared
            .iter()
            .filter(|case| case.name == "VALUE")
            .collect();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].selected, selected);
        assert!(
            cases[0]
                .reason
                .contains("no captured native semantic-parent relation")
        );
        assert_eq!(
            cases[0].observation.input,
            if selected {
                "scalar-first.cpp"
            } else {
                "scalar-second.cpp"
            }
        );
        assert!(cases[0].observation.partition.is_some());
        assert!(
            cases[0]
                .observation
                .location
                .file
                .ends_with("scalar_constants.hpp")
        );
    }
}

#[test]
fn scalar_constant_original_enum_member_survives_actual_remap_and_scoping() {
    let snapshot = capture_with_other(false, true, true);
    for input in [
        "scalar-first.cpp",
        "scalar-second.cpp",
        "scalar-equal.cpp",
        "scalar-other.hpp",
    ] {
        let anchor = snapshot
            .facts
            .iter()
            .find(|fact| {
                fact.origin.tu == input && fact.name == "RENAMED" && fact.kind == FactKind::Struct
            })
            .unwrap();
        assert!(anchor.definition);
        let FactData::Record { fields, .. } = &anchor.data else {
            panic!("{anchor:?}")
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(
            fields[0].name,
            if input == "scalar-other.hpp" {
                "other_member"
            } else {
                "shared_member"
            }
        );
    }
    let policy = remap_policy();
    let make_plan = || {
        snapshot
            .clone()
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap()
    };
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("scalar.dll");
    let (prepared, display_names, source_names) = make_plan()
        .snapshot
        .into_partitioned_planning_snapshot(&options);
    let first = prepared
        .constants
        .iter()
        .find(|constant| {
            constant.root.tu == "scalar-first.cpp"
                && source_names
                    .constant_member(constant)
                    .is_some_and(|member| member.name == "FIRST")
        })
        .unwrap();
    assert!(first.name.starts_with("__partition_"));
    assert_eq!(display_names[&first.name], "RENAMED");
    assert_eq!(source_names.constant_name(first), first.name);
    assert_eq!(source_names.constant_member(first).unwrap().name, "FIRST");
    for conflict in ["identical", "owner", "pin"] {
        let mut private = make_plan().snapshot;
        let original_owner =
            private.constant_root_owners[&(first.root.clone(), "FIRST".into())].clone();
        let mut occupied_owner = original_owner.clone();
        if conflict == "owner" {
            occupied_owner
                .as_mut()
                .unwrap()
                .libraries
                .insert("__metadata_probe".into(), "occupied.dll".into());
        }
        // Private metadata at the genuine scoped destination; native declarations are unchanged.
        private
            .constant_root_owners
            .insert((first.root.clone(), first.name.clone()), occupied_owner);
        for (name, namespace) in [
            ("FIRST", "Test.First"),
            ("SECOND", "Test.First"),
            (
                first.name.as_str(),
                if conflict == "pin" {
                    "Test.Conflict"
                } else {
                    "Test.First"
                },
            ),
        ] {
            private.constant_namespace_authorities.insert(
                (first.definition.clone(), name.into()),
                Ok(namespace.into()),
            );
        }
        let (scoped, names, sources) = private.into_partitioned_planning_snapshot(&options);
        assert_eq!(
            scoped.constant_namespace_authorities[&(first.definition.clone(), "SECOND".into())],
            Ok("Test.First".into())
        );
        let member = scoped
            .constants
            .iter()
            .find(|constant| constant.root == first.root && constant.name == first.name)
            .unwrap();
        assert_eq!(sources.constant_member(member).unwrap().name, "FIRST");
        if conflict == "identical" {
            assert_eq!(
                scoped.constant_root_owners[&(first.root.clone(), first.name.clone())],
                original_owner
            );
            assert_eq!(
                scoped.constant_namespace_authorities
                    [&(first.definition.clone(), first.name.clone())],
                Ok("Test.First".into())
            );
            scoped.plan_partitioned(&options, &names, &sources).unwrap();
        } else {
            let Err(error) = scoped.plan_partitioned(&options, &names, &sources) else {
                panic!("conflicting scoped metadata was accepted");
            };
            assert!(error.to_string().contains(if conflict == "owner" {
                "ambiguous propagated root owners"
            } else {
                "ambiguous propagated namespace pins"
            }));
            let excluded = BTreeSet::from([first.name.clone()]);
            let mut filtered = EmitOptions::new("Test", &references);
            filtered.library = Some("scalar.dll");
            filtered.excluded_constants = Some(&excluded);
            let omitted = scoped
                .plan_partitioned(&filtered, &names, &sources)
                .unwrap();
            assert!(
                omitted
                    .constants
                    .iter()
                    .all(|planned| planned.constant.name != first.name)
            );
        }
    }
    let planned = prepared
        .plan_partitioned(&options, &display_names, &source_names)
        .unwrap();
    let candidates = prepared
        .partition_route_candidates(&planned, &source_names, &options)
        .unwrap();
    assert_eq!(
        candidates[&(first.name.clone(), OutputKind::Value)].source_name,
        first.name
    );
    let other = prepared
        .constants
        .iter()
        .find(|constant| constant.root.tu == "scalar-other.hpp")
        .unwrap();
    assert!(other.name.starts_with("__partition_"));
    assert_ne!(first.name, other.name);
    assert_eq!(source_names.constant_name(other), "RENAMED");
    assert_eq!(source_names.constant_member(other).unwrap().name, "RENAMED");
    assert_eq!(
        candidates[&(other.name.clone(), OutputKind::Value)].source_name,
        "RENAMED"
    );
    let report = emit(make_plan());
    assert_notes(&report, &without_notes(make_plan()));
    let rows = constant_rows(&report, "FIRST");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].identity.role,
        DefinitionTypeRole::AnonymousEnumMember
    );
    assert!(constant_rows(&report, "SECOND").is_empty());
    let text = report
        .partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Test.First")
        .unwrap()
        .1;
    let first_note = format!("// windows-clang: {}\n", rows[0].message);
    assert!(text.contains(&format!("{first_note}        const RENAMED:")));
    let other_text = report
        .partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Test.Other")
        .unwrap()
        .1;
    assert!(other_text.contains("const RENAMED:"));
    assert!(!other_text.contains(&rows[0].message));
    let second = text
        .lines()
        .position(|line| line.contains("const SECOND:"))
        .unwrap();
    assert!(
        !text
            .lines()
            .nth(second.saturating_sub(1))
            .unwrap()
            .contains(&rows[0].message)
    );
}

fn remap_policy() -> HeaderPartitionPolicy {
    let header = header();
    HeaderPartitionPolicy::new()
        .with_partition_authority("First", 0, "scalar-first.cpp")
        .with_partition_authority("Second", 1, "scalar-second.cpp")
        .with_partition_authority("Equal", 2, "scalar-equal.cpp")
        .with_partition_authority("Other", 3, "scalar-other.hpp")
        .with_traversed_header(
            &header,
            RootPartition::new("First", "Test.First").with_remap("FIRST", "RENAMED"),
        )
        .with_traversed_header(
            &header,
            RootPartition::new("Second", "Test.Second").with_remap("FIRST", "RENAMED"),
        )
        .with_traversed_header(
            &header,
            RootPartition::new("Equal", "Test.Equal").with_remap("FIRST", "RENAMED"),
        )
        .with_traversed_header(
            "scalar-other.hpp",
            RootPartition::new("Other", "Test.Other"),
        )
}

#[test]
fn scalar_constant_only_collision_preserves_pre_observation_rejection() {
    let snapshot = capture_with_other(false, true, false);
    assert!(!snapshot.facts.iter().any(|fact| fact.name == "RENAMED"));
    let plan = snapshot
        .into_header_partition_plan(&remap_policy(), &NamespaceAuthorities::new())
        .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("scalar.dll");
    let observed = OBSERVATION_CALLS.get();
    let error = plan.emit_with_report(&options).unwrap_err();
    assert_eq!(error.to_string(), "ambiguous constant root `RENAMED`");
    assert_eq!(OBSERVATION_CALLS.get(), observed);
}

fn sibling_remap_policy(remaps: &[(&str, &str)]) -> HeaderPartitionPolicy {
    let header = header();
    let mut policy = HeaderPartitionPolicy::new();
    for (partition, input, rank) in [
        ("First", "scalar-first.cpp", 0),
        ("Second", "scalar-second.cpp", 1),
        ("Equal", "scalar-equal.cpp", 2),
    ] {
        policy.set_partition_authority(partition, rank, input);
        let mut owner = RootPartition::new(partition, format!("Test.{partition}"));
        for (source, target) in remaps {
            owner = owner.with_remap(*source, *target);
        }
        policy.add_traversed_header(&header, owner);
    }
    policy
}

fn assert_sibling_metadata(report: &HeaderPartitionReport, members: &[(&str, &Constant)]) {
    let path = std::env::temp_dir().join(format!(
        "scalar-siblings-{}-{:?}.winmd",
        std::process::id(),
        std::thread::current().id()
    ));
    windows_rdl::reader()
        .input_texts(report.partitions.values())
        .reference_default()
        .output(&path)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&path).unwrap();
    let apis = index.expect("Test.First", "Apis");
    for &(name, native) in members {
        let fields: Vec<_> = apis.fields().filter(|field| field.name() == name).collect();
        assert_eq!(fields.len(), 1, "{name}");
        let (ty, value) = match (&native.ty, &native.value) {
            (TypeRef::Scalar(Scalar::I32), Value::Signed(value)) => (
                windows_metadata::Type::I32,
                windows_metadata::Value::I32((*value).try_into().unwrap()),
            ),
            (TypeRef::Scalar(Scalar::U32), Value::Unsigned(value)) => (
                windows_metadata::Type::U32,
                windows_metadata::Value::U32((*value).try_into().unwrap()),
            ),
            _ => panic!("unexpected captured anonymous-enum scalar: {native:?}"),
        };
        assert_eq!(fields[0].ty(), ty);
        assert_eq!(fields[0].constant().unwrap().value(), value);
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn scalar_constant_occupied_target_chain_and_swap_keep_original_members() {
    let original = capture(false);
    let first = original
        .constants
        .iter()
        .find(|constant| constant.root.tu == "scalar-first.cpp" && constant.name == "FIRST")
        .unwrap();
    let second = original
        .constants
        .iter()
        .find(|constant| constant.root.tu == "scalar-first.cpp" && constant.name == "SECOND")
        .unwrap();
    assert_eq!(
        scalar_observation_value(&first.value),
        Ok(ScalarObservationValue::Integral(7))
    );
    assert_eq!(
        scalar_observation_value(&second.value),
        Ok(ScalarObservationValue::Integral(11))
    );
    let sibling_roots: BTreeSet<_> = original
        .constants
        .iter()
        .filter(|constant| matches!(constant.name.as_str(), "FIRST" | "SECOND"))
        .map(|constant| constant.root.clone())
        .collect();
    let authoritative_owner_keys = |snapshot: &Snapshot| {
        snapshot
            .constant_root_owners
            .keys()
            .filter(|(origin, _)| origin == &first.root)
            .cloned()
            .collect::<Vec<_>>()
    };
    for (case, remaps, second_name) in [
        ("chain", [("FIRST", "SECOND"), ("SECOND", "THIRD")], "THIRD"),
        ("swap", [("FIRST", "SECOND"), ("SECOND", "FIRST")], "FIRST"),
    ] {
        let mut baseline = None;
        for reversed in [false, true] {
            let mut snapshot = original.clone();
            let mut operations = remaps.to_vec();
            if reversed {
                snapshot.constants.reverse();
                operations.reverse();
            }
            let policy = sibling_remap_policy(&operations);
            let make_plan = || {
                snapshot
                    .clone()
                    .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                    .unwrap()
            };
            let references = BTreeMap::new();
            let mut options = EmitOptions::new("Test", &references);
            options.library = Some("scalar.dll");
            let before = make_plan();
            eprintln!(
                "occupied_target={case} reversed={reversed} authoritative_owner_keys_before={:?} fact_root_owner_before={}",
                authoritative_owner_keys(&before.snapshot),
                before.snapshot.root_owners.contains_key(&first.root),
            );
            let (prepared, display_names, source_names) =
                before.snapshot.into_partitioned_planning_snapshot(&options);
            eprintln!(
                "occupied_target={case} reversed={reversed} authoritative_owner_keys_after={:?} fact_root_owner_after={}",
                authoritative_owner_keys(&prepared),
                prepared.root_owners.contains_key(&first.root),
            );
            let mut prepared_members = prepared
                .constants
                .iter()
                .filter(|constant| sibling_roots.contains(&constant.root))
                .map(|constant| {
                    (
                        constant.root.tu.as_str(),
                        constant.name.as_str(),
                        source_names.constant_member(constant),
                        source_names
                            .constant_members
                            .get(&ConstantMember::from(constant)),
                        &constant.value,
                    )
                })
                .collect::<Vec<_>>();
            prepared_members.sort_by_key(|member| (member.0, member.1));
            eprintln!(
                "occupied_target={case} reversed={reversed} prepared_members={prepared_members:?}"
            );
            for input in ["scalar-first.cpp", "scalar-second.cpp", "scalar-equal.cpp"] {
                for (name, transformed) in [("FIRST", "SECOND"), ("SECOND", second_name)] {
                    let current = if input == "scalar-first.cpp" {
                        transformed
                    } else {
                        name
                    };
                    let constant = prepared
                        .constants
                        .iter()
                        .find(|constant| constant.root.tu == input && constant.name == current)
                        .unwrap();
                    assert_eq!(source_names.constant_member(constant).unwrap().name, name);
                    let native = original
                        .constants
                        .iter()
                        .find(|constant| constant.root.tu == input && constant.name == name)
                        .unwrap();
                    assert_eq!(
                        source_names.constant_member(constant),
                        Some(&ConstantMember::from(native))
                    );
                    assert_eq!(source_names.constant_name(constant), current);
                }
            }
            let planned = prepared
                .plan_partitioned(&options, &display_names, &source_names)
                .unwrap();
            eprintln!(
                "occupied_target={case} reversed={reversed} selected_members={:?}",
                planned
                    .constants
                    .iter()
                    .map(|planned| (
                        planned.constant.name.as_str(),
                        source_names
                            .constant_member(planned.constant)
                            .map(|member| member.name.as_str()),
                        &planned.constant.value,
                    ))
                    .collect::<Vec<_>>()
            );
            let candidates = prepared
                .partition_route_candidates(&planned, &source_names, &options)
                .unwrap();
            for name in ["SECOND", second_name] {
                assert!(
                    planned
                        .constants
                        .iter()
                        .any(|planned| planned.constant.name == name),
                    "ordinary admission omitted transformed sibling {name:?} in {case}, reversed={reversed}"
                );
                assert_eq!(
                    candidates[&(name.into(), OutputKind::Value)].source_name,
                    name
                );
            }
            let report = emit(make_plan());
            assert_notes(&report, &without_notes(make_plan()));
            assert_metadata(&report, false);
            assert_sibling_metadata(&report, &[("SECOND", first), (second_name, second)]);
            let rows = constant_rows(&report, "FIRST");
            assert_eq!(rows.len(), 1);
            let DefinitionComparison::Different { keys, unknown } = &rows[0].outcome else {
                panic!("{:?}", rows[0]);
            };
            assert!(unknown.is_empty());
            assert_eq!(
                keys,
                &[DefinitionDifference {
                    path: "value".into(),
                    selected: "7".into(),
                    alternative: "13".into(),
                }]
            );
            assert!(constant_rows(&report, "SECOND").is_empty());
            assert!(report.constant_not_compared.is_empty());
            let text = report
                .partitions
                .iter()
                .find(|(partition, _)| partition.namespace == "Test.First")
                .unwrap()
                .1;
            let exact_note = format!("// windows-clang: {}\n", rows[0].message);
            assert!(text.contains(&format!("{exact_note}        const SECOND:")));
            let second_line = text
                .lines()
                .position(|line| line.contains(&format!("const {second_name}:")))
                .unwrap();
            assert!(
                !text
                    .lines()
                    .nth(second_line.saturating_sub(1))
                    .unwrap()
                    .contains(&rows[0].message)
            );
            if let Some(baseline) = &baseline {
                assert_eq!(&report, baseline);
            } else {
                baseline = Some(report);
            }
        }
    }
}

#[test]
fn scalar_constant_metadata_owner_pin_chains_swaps_and_demanded_conflicts() {
    let original = capture(false);
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("scalar.dll");
    for (remaps, second_name) in [
        ([("FIRST", "SECOND"), ("SECOND", "THIRD")], "THIRD"),
        ([("FIRST", "SECOND"), ("SECOND", "FIRST")], "FIRST"),
    ] {
        for reversed in [false, true] {
            let mut snapshot = original.clone();
            let mut operations = remaps.to_vec();
            if reversed {
                snapshot.constants.reverse();
                operations.reverse();
            }
            let mut before = snapshot
                .into_header_partition_plan(
                    &sibling_remap_policy(&operations),
                    &NamespaceAuthorities::new(),
                )
                .unwrap()
                .snapshot;
            let root = before
                .constants
                .iter()
                .find(|constant| constant.root.tu == "scalar-first.cpp" && constant.name == "FIRST")
                .unwrap()
                .root
                .clone();
            let mut expected = BTreeMap::new();
            // Private distinct settings identify each metadata entry without changing native facts.
            for (name, current, namespace) in [
                ("FIRST", "SECOND", "Test.PinFirst"),
                ("SECOND", second_name, "Test.PinSecond"),
            ] {
                let owner = before
                    .constant_root_owners
                    .get_mut(&(root.clone(), name.into()))
                    .unwrap();
                owner
                    .as_mut()
                    .unwrap()
                    .libraries
                    .insert("__metadata_probe".into(), name.into());
                expected.insert(current, (owner.clone(), namespace));
                before
                    .constant_namespace_authorities
                    .insert((root.clone(), name.into()), Ok(namespace.into()));
            }
            let untouched = before.constant_root_owners[&(root.clone(), "FIRST".into())].clone();
            before
                .constant_root_owners
                .insert((root.clone(), "__unrequested".into()), untouched.clone());
            before.constant_namespace_authorities.insert(
                (root.clone(), "__unrequested".into()),
                Ok("Test.Untouched".into()),
            );
            let (prepared, names, sources) = before.into_partitioned_planning_snapshot(&options);
            assert_eq!(
                prepared.constant_root_owners[&(root.clone(), "__unrequested".into())],
                untouched
            );
            assert_eq!(
                prepared.constant_namespace_authorities[&(root.clone(), "__unrequested".into())],
                Ok("Test.Untouched".into())
            );
            let plan = prepared
                .plan_partitioned(&options, &names, &sources)
                .unwrap();
            let routes = prepared
                .partition_route_candidates(&plan, &sources, &options)
                .unwrap();
            for (current, (owner, namespace)) in expected {
                assert_eq!(
                    prepared.constant_root_owners[&(root.clone(), current.into())],
                    owner
                );
                assert_eq!(
                    prepared.constant_namespace_authorities[&(root.clone(), current.into())],
                    Ok(namespace.into())
                );
                assert_eq!(
                    routes[&(current.into(), OutputKind::Value)]
                        .namespace
                        .as_deref(),
                    Some(namespace)
                );
            }
        }
    }
    for conflict in ["owner", "pin"] {
        for reversed in [false, true] {
            let mut snapshot = original.clone();
            let values: BTreeMap<_, _> = snapshot
                .constants
                .iter()
                .filter(|constant| constant.name == "FIRST")
                .map(|constant| (constant.root.tu.clone(), constant.value.clone()))
                .collect();
            // Private equal-body convergence retains the genuine captured origins and types.
            for constant in &mut snapshot.constants {
                if constant.name == "SECOND" {
                    constant.value = values[&constant.root.tu].clone();
                }
            }
            let mut remaps = [("FIRST", "TARGET"), ("SECOND", "TARGET")];
            if reversed {
                snapshot.constants.reverse();
                remaps.reverse();
            }
            let mut before = snapshot
                .into_header_partition_plan(
                    &sibling_remap_policy(&remaps),
                    &NamespaceAuthorities::new(),
                )
                .unwrap()
                .snapshot;
            let root = before
                .constants
                .iter()
                .find(|constant| constant.root.tu == "scalar-first.cpp" && constant.name == "FIRST")
                .unwrap()
                .root
                .clone();
            if conflict == "owner" {
                before
                    .constant_root_owners
                    .get_mut(&(root.clone(), "SECOND".into()))
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .libraries
                    .insert("__metadata_probe".into(), "different.dll".into());
            }
            for name in ["FIRST", "SECOND"] {
                before.constant_namespace_authorities.insert(
                    (root.clone(), name.into()),
                    Ok(if conflict == "pin" && name == "SECOND" {
                        "Test.Conflict"
                    } else {
                        "Test.First"
                    }
                    .into()),
                );
            }
            let (mut prepared, names, sources) =
                before.into_partitioned_planning_snapshot(&options);
            if conflict == "owner" {
                assert_eq!(
                    prepared.constant_root_owners[&(root.clone(), "TARGET".into())]
                        .as_ref()
                        .unwrap_err()
                        .len(),
                    2
                );
            } else {
                assert_eq!(
                    prepared.constant_namespace_authorities[&(root.clone(), "TARGET".into())],
                    Err(BTreeSet::from([
                        "Test.First".into(),
                        "Test.Conflict".into()
                    ]))
                );
            }
            let observed = OBSERVATION_CALLS.get();
            let Err(error) = prepared.plan_partitioned(&options, &names, &sources) else {
                panic!("conflicting remapped metadata was accepted");
            };
            assert_eq!(OBSERVATION_CALLS.get(), observed);
            assert!(error.to_string().contains(if conflict == "owner" {
                "ambiguous propagated root owners"
            } else {
                "ambiguous propagated namespace pins"
            }));
            let excluded = BTreeSet::from(["TARGET".into()]);
            let mut filtered = EmitOptions::new("Test", &references);
            filtered.library = Some("scalar.dll");
            filtered.excluded_constants = Some(&excluded);
            let plan = prepared
                .plan_partitioned(&filtered, &names, &sources)
                .unwrap();
            assert!(
                plan.constants
                    .iter()
                    .all(|planned| planned.constant.name != "TARGET")
            );
            // Private unrequested state retains conflicting metadata but removes its value candidates.
            prepared
                .constants
                .retain(|constant| constant.name != "TARGET");
            prepared
                .plan_partitioned(&options, &names, &sources)
                .unwrap();
        }
    }
}

#[test]
fn scalar_constant_converging_tokens_are_unproved_not_borrowed_or_omitted() {
    let original = capture(false);
    let first = original
        .constants
        .iter()
        .find(|constant| constant.root.tu == "scalar-first.cpp" && constant.name == "FIRST")
        .unwrap();
    assert_eq!(
        scalar_observation_value(&first.value),
        Ok(ScalarObservationValue::Integral(7))
    );
    let remaps = [("FIRST", "TARGET"), ("SECOND", "TARGET")];
    let ordinary = original
        .clone()
        .into_header_partition_plan(&sibling_remap_policy(&remaps), &NamespaceAuthorities::new())
        .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("scalar.dll");
    let calls = OBSERVATION_CALLS.get();
    assert_eq!(
        ordinary.emit_with_report(&options).unwrap_err().to_string(),
        "ambiguous constant root `TARGET`"
    );
    assert_eq!(OBSERVATION_CALLS.get(), calls);
    let mut baseline = None;
    for reversed in [false, true] {
        let mut snapshot = original.clone();
        let first_values: BTreeMap<_, _> = snapshot
            .constants
            .iter()
            .filter(|constant| constant.name == "FIRST")
            .map(|constant| (constant.root.tu.clone(), constant.value.clone()))
            .collect();
        // Private equal-body convergence, preserving native origins and ordinary admission.
        for constant in &mut snapshot.constants {
            if constant.name == "SECOND" {
                constant.value = first_values[&constant.root.tu].clone();
            }
        }
        let mut operations = remaps.to_vec();
        if reversed {
            snapshot.constants.reverse();
            operations.reverse();
        }
        let policy = sibling_remap_policy(&operations);
        let make_plan = || {
            snapshot
                .clone()
                .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                .unwrap()
        };
        let (prepared, display_names, mut source_names) = make_plan()
            .snapshot
            .into_partitioned_planning_snapshot(&options);
        let planned = prepared
            .plan_partitioned(&options, &display_names, &source_names)
            .unwrap();
        let selected = planned
            .constants
            .iter()
            .find(|planned| planned.constant.name == "TARGET")
            .unwrap()
            .constant;
        assert!(source_names.constant_member(selected).is_none());
        let candidates = &source_names.constant_members[&ConstantMember::from(selected)];
        assert_eq!(
            candidates
                .iter()
                .map(|member| member.name.as_str())
                .collect::<Vec<_>>(),
            ["FIRST", "SECOND"]
        );
        assert_eq!(source_names.constant_name(selected), "TARGET");
        let report = emit(make_plan());
        assert_notes(&report, &without_notes(make_plan()));
        assert_sibling_metadata(&report, &[("TARGET", first)]);
        for name in ["FIRST", "SECOND", "TARGET"] {
            assert!(constant_rows(&report, name).is_empty());
        }
        let cases: Vec<_> = report
            .constant_not_compared
            .iter()
            .filter(|case| case.name == "TARGET")
            .collect();
        assert_eq!(cases.len(), 1);
        assert!(cases[0].selected);
        assert_eq!(
            cases[0].reason,
            "ambiguous original-member token: \"FIRST\", \"SECOND\""
        );
        assert_eq!(cases[0].observation.input, "scalar-first.cpp");
        assert_eq!(cases[0].observation.partition.as_deref(), Some("First"));
        assert_eq!(cases[0].observation.rank, Some(0));
        assert_eq!(cases[0].observation.location, selected.spelling);
        assert_eq!(
            cases[0].observation.defining_locations,
            vec![selected.spelling.clone()]
        );
        let ConstantEvidenceCoverage::CapturedScalars {
            selected_members,
            captured_observations,
            matched_alternative_observations,
            unproved_observations,
            ..
        } = report.constant_evidence_coverage
        else {
            panic!("{report:?}")
        };
        assert_eq!(
            (
                selected_members,
                captured_observations,
                matched_alternative_observations,
                unproved_observations
            ),
            (8, 22, 13, 1)
        );
        source_names
            .constant_members
            .remove(&ConstantMember::from(selected));
        let missing = observe_constants(&prepared, &planned.constants, Some(&source_names));
        assert!(missing.not_compared.iter().any(|case| case.name == "TARGET"
            && case.selected
            && case.reason == "missing original-member token"));
        assert!(
            !missing
                .members
                .contains_key(&ConstantMember::from(selected))
        );
        if let Some(baseline) = &baseline {
            assert_eq!(&report, baseline);
        } else {
            baseline = Some(report);
        }
    }
}

#[test]
fn scalar_constant_duplicate_losing_tokens_coalesce_without_property_winners() {
    let original = capture(false);
    let admitted = without_notes(plan(original.clone(), false));
    let mut baseline = None;
    for reversed in [false, true] {
        let mut snapshot = original.clone();
        let mut duplicates = Vec::new();
        for constant in &mut snapshot.constants {
            if constant.root.tu != "scalar-second.cpp" {
                continue;
            }
            // Private contradictory observations, not additional native evaluations.
            match constant.name.as_str() {
                "TYPED" => {
                    constant.value = Value::Unsigned(13);
                    let mut duplicate = constant.clone();
                    duplicate.ty = TypeRef::Scalar(Scalar::I32);
                    duplicates.push(duplicate);
                }
                "VALUE" => {
                    constant.ty = TypeRef::Scalar(Scalar::U32);
                    let mut duplicate = constant.clone();
                    duplicate.value = Value::Signed(17);
                    duplicates.push(duplicate);
                }
                "EQUAL" => duplicates.push(constant.clone()),
                _ => {}
            }
        }
        assert_eq!(duplicates.len(), 3);
        snapshot.constants.extend(duplicates);
        if reversed {
            snapshot.constants.reverse();
        }
        let report = emit(plan(snapshot, false));
        assert_notes(&report, &admitted);
        assert_metadata(&report, false);
        assert!(report.constant_not_compared.is_empty());
        assert!(constant_rows(&report, "EQUAL").is_empty());
        for (name, path, selected, alternative, unknown_path) in [
            ("TYPED", "value", "7", "13", "type"),
            ("VALUE", "type", "I32", "U32", "value"),
        ] {
            let rows = constant_rows(&report, name);
            assert_eq!(rows.len(), 1);
            let DefinitionComparison::Different { keys, unknown } = &rows[0].outcome else {
                panic!("{:?}", rows[0]);
            };
            assert_eq!(
                keys,
                &[DefinitionDifference {
                    path: path.into(),
                    selected: selected.into(),
                    alternative: alternative.into(),
                }]
            );
            assert_eq!(
                unknown,
                &[DefinitionUnknown {
                    path: unknown_path.into(),
                    reason: format!("conflicting original-member scalar {unknown_path}"),
                }]
            );
        }
        if let Some(baseline) = &baseline {
            assert_eq!(&report, baseline);
        } else {
            baseline = Some(report);
        }
    }
}

#[test]
fn scalar_constant_conflicting_identity_coalesces_to_unproved_in_both_orders() {
    let partitioned = plan(capture(false), false);
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("scalar.dll");
    let (snapshot, display_names, mut source_names) = partitioned
        .snapshot
        .into_partitioned_planning_snapshot(&options);
    let planned = snapshot
        .plan_partitioned(&options, &display_names, &source_names)
        .unwrap();
    let member = source_names
        .constant_evidence
        .keys()
        .find(|member| member.root.tu == "scalar-second.cpp" && member.name == "VALUE")
        .unwrap()
        .clone();
    let mut baseline = None;
    for reversed in [false, true] {
        let mut left = scalar_constant_index(&snapshot)
            .remove(&member)
            .unwrap()
            .unwrap();
        let mut right = scalar_constant_index(&snapshot)
            .remove(&member)
            .unwrap()
            .unwrap();
        // Private conflicting normalized identity, not a fabricated native scope.
        right.identity.as_mut().unwrap().scope =
            vec![(DefinitionTypeRole::Namespace, "Other".into())];
        if reversed {
            std::mem::swap(&mut left, &mut right);
        }
        left.coalesce(right);
        assert_eq!(left.ty, Ok(Scalar::I32));
        assert_eq!(left.value, Ok(ScalarObservationValue::Integral(13)));
        source_names
            .constant_evidence
            .insert(member.clone(), Some(left));
        let observations = observe_constants(&snapshot, &planned.constants, Some(&source_names));
        assert!(
            !observations
                .rows
                .iter()
                .any(|row| row.identity.name == "VALUE")
        );
        let unproved: Vec<_> = observations
            .not_compared
            .into_iter()
            .filter(|case| case.name == "VALUE")
            .collect();
        assert_eq!(unproved.len(), 1);
        assert!(!unproved[0].selected);
        assert_eq!(unproved[0].observation.input, "scalar-second.cpp");
        assert_eq!(
            unproved[0].reason,
            "conflicting original-member identity or provenance"
        );
        if let Some(baseline) = &baseline {
            assert_eq!(&unproved, baseline);
        } else {
            baseline = Some(unproved);
        }
    }
}

#[test]
fn scalar_constant_final_admission_exclusions_and_unsupported_guards() {
    let snapshot = capture(false);
    let excluded = BTreeSet::from(["VALUE".to_string(), "FIRST".to_string()]);
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("scalar.dll");
    options.excluded_constants = Some(&excluded);
    let report = plan(snapshot.clone(), false)
        .emit_with_report(&options)
        .unwrap();
    for name in ["VALUE", "FIRST", "PROJECTED_MEMBER", "ABSENT", "POINTER"] {
        assert!(constant_rows(&report, name).is_empty());
    }
    assert!(
        !report
            .partitions
            .values()
            .any(|text| text.contains("const VALUE:") || text.contains("const FIRST:"))
    );
    let mut unsupported = snapshot;
    let selected = unsupported
        .constants
        .iter_mut()
        .find(|constant| constant.root.tu == "scalar-first.cpp" && constant.name == "VALUE")
        .unwrap();
    selected.ty = TypeRef::Void;
    options.excluded_constants = None;
    let report = plan(unsupported, false).emit_with_report(&options).unwrap();
    assert!(constant_rows(&report, "VALUE").is_empty());
    assert!(
        !report
            .partitions
            .values()
            .any(|text| text.contains("const VALUE:"))
    );
}

#[test]
fn scalar_constant_pure_lossless_values_float_bits_and_own_tu_aliases() {
    assert_eq!(
        scalar_observation_value(&Value::Unsigned(u64::MAX)).unwrap(),
        ScalarObservationValue::Integral(i128::from(u64::MAX))
    );
    assert_eq!(
        scalar_observation_value(&Value::Signed(i64::MIN)).unwrap(),
        ScalarObservationValue::Integral(i128::from(i64::MIN))
    );
    assert_eq!(
        scalar_observation_value(&Value::Signed(7)),
        scalar_observation_value(&Value::Unsigned(7))
    );
    for bits in [0, 0x8000_0000, 7f32.to_bits()] {
        assert_eq!(
            scalar_observation_value(&Value::F32(bits)),
            Ok(ScalarObservationValue::F32(bits))
        );
    }
    assert!(scalar_observation_value(&Value::F32(f32::INFINITY.to_bits())).is_err());
    assert!(scalar_observation_value(&Value::F64(f64::NAN.to_bits())).is_err());
    assert_eq!(
        scalar_observation_value(&Value::F64(7f64.to_bits())),
        Ok(ScalarObservationValue::F64(7f64.to_bits()))
    );
    let location = Location {
        file: "alias.hpp".into(),
        offset: 0,
    };
    let alias = TypeRef::Named {
        name: "Alias".into(),
        declaration: location.clone(),
    };
    let scalar = TypeRef::Scalar(Scalar::I32);
    let aliases = HashMap::from([(("own", "Alias", &location), &scalar)]);
    assert_eq!(
        extract::macro_scalar_type(&alias, "own", &aliases, None),
        Some(Scalar::I32)
    );
    assert_eq!(
        extract::macro_scalar_type(&alias, "foreign", &aliases, None),
        None
    );
    let cyclic = HashMap::from([(("own", "Alias", &location), &alias)]);
    assert_eq!(
        extract::macro_scalar_type(&alias, "own", &cyclic, None),
        None
    );
}
