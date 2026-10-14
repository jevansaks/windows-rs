use super::*;

#[test]
fn native_ranked_shared_enum_losing_context_control() {
    helpers::ensure_libclang();
    for target in [
        "x86_64-pc-windows-msvc",
        "i686-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        println!("TARGET {target}");
        let mut baseline = None;
        for case in [
            "baseline",
            "equivalent",
            "losing-disagreement",
            "selected-disagreement",
        ] {
            for reversed in [false, true] {
                let losing_prefix = match case {
                    "equivalent" => "#define CONTROL_EQUIVALENT\n",
                    "losing-disagreement" => "#define CONTROL_DIFFERENT\n",
                    _ => "",
                };
                let selected_prefix = if case == "selected-disagreement" {
                    "#define CONTROL_DIFFERENT\n"
                } else {
                    ""
                };
                let plan =
                    shared_control_plan(target, selected_prefix, losing_prefix, "", reversed);
                let snapshot = &plan.snapshot;
                assert_eq!(snapshot.shared_enum_identities.len(), 8);
                let keys: BTreeSet<_> = snapshot
                    .shared_enum_identities
                    .values()
                    .map(|identity| {
                        (
                            &identity.definition.enum_usr,
                            &identity.definition.alias_usr,
                        )
                    })
                    .collect();
                assert_eq!(
                    keys.len(),
                    1,
                    "native USRs did not activate one group: {keys:?}"
                );
                let enumerations: Vec<_> = snapshot
                    .facts
                    .iter()
                    .filter(|fact| matches!(fact.data, FactData::Enum { .. }))
                    .collect();
                assert_eq!(enumerations.len(), 4);
                let files: BTreeSet<_> = enumerations
                    .iter()
                    .map(|fact| &fact.spelling.file)
                    .collect();
                assert_eq!(files.len(), 2);
                for fact in &enumerations {
                    let identity = &snapshot.shared_enum_identities[&fact.origin];
                    assert_eq!(identity.definition.data, fact.data);
                    println!(
                        "NATIVE case={case} reversed={reversed} fact={fact:?} identity={identity:?} \
                     owner={:?} suppressed={}",
                        snapshot.root_owners.get(&fact.origin),
                        snapshot.suppressed_type_origins.contains(&fact.origin),
                    );
                }
                let selected_b = enumerations
                    .iter()
                    .find(|fact| fact.origin.tu == "control-b.cpp")
                    .unwrap();
                assert!(selected_b.root);
                let losing = enumerations
                    .iter()
                    .find(|fact| fact.origin.tu == "control-losing.cpp")
                    .unwrap();
                assert!(!losing.root);
                assert!(!snapshot.root_owners.contains_key(&losing.origin));
                let selected_a = enumerations
                    .iter()
                    .find(|fact| fact.origin.tu == "control-a.cpp")
                    .unwrap();
                assert!(!selected_a.root);
                assert_eq!(snapshot.root_owners[&selected_a.origin].partition, "A");
                assert_eq!(
                    losing.data == selected_a.data,
                    case != "losing-disagreement"
                );
                let required = snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        fact.name == "CONTROL_REQUIRED" && fact.origin.tu == "control-use.cpp"
                    })
                    .unwrap();
                assert!(required.root);
                let FactData::Record { fields, .. } = &required.data else {
                    panic!("{required:?}")
                };
                assert_eq!(fields.len(), 1);
                let TypeRef::Named { declaration, .. } = &fields[0].ty else {
                    panic!("{required:?}")
                };
                assert_eq!(declaration.file, selected_b.spelling.file);
                println!("REQUIRED case={case} reversed={reversed} fact={required:?}");
                if case != "selected-disagreement" {
                    assert_eq!(selected_a.data, selected_b.data);
                } else {
                    assert_ne!(selected_a.data, selected_b.data);
                }
                let providers = PlanningSourceNames::shared_enum_providers(snapshot);
                assert_eq!(providers.len(), 4);
                assert!(!providers.contains_key(&losing.origin));
                assert!(
                    providers.keys().all(|origin| matches!(
                        origin.tu.as_str(),
                        "control-a.cpp" | "control-b.cpp"
                    ))
                );
                if case != "selected-disagreement" {
                    assert!(providers.values().all(|provider| {
                        provider
                            .as_ref()
                            .is_ok_and(|origin| *origin == selected_b.origin)
                    }));
                }
                println!("ADMISSION case={case} reversed={reversed} providers={providers:?}");
                let references = BTreeMap::new();
                let outcome = plan.emit_with_report(&EmitOptions::new("Test", &references));
                println!("STRICT case={case} reversed={reversed} outcome={outcome:#?}");
                match case {
                    "baseline" | "equivalent" | "losing-disagreement" => {
                        let report = outcome.unwrap();
                        assert!(report.omissions.is_empty());
                        assert!(
                            report
                                .partitions
                                .values()
                                .any(|rdl| rdl.contains("struct CONTROL_REQUIRED")
                                    && rdl.contains("Test::B::CONTROL_ENUM"))
                        );
                        assert_eq!(
                            report
                                .partitions
                                .values()
                                .filter(|rdl| rdl.contains("enum CONTROL_ENUM"))
                                .count(),
                            1
                        );
                        let body =
                            certified_shared_body(&report, case == "losing-disagreement", false);
                        if let Some(baseline) = &baseline {
                            assert_eq!(&body, baseline);
                        } else {
                            baseline = Some(body);
                        }
                        assert_control_metadata(&report, 1);
                    }
                    _ => {
                        assert!(providers.values().all(|provider| matches!(
                            provider,
                            Err(SharedEnumProviderFailure::ConflictingDefinitions)
                        )));
                        let error = outcome.unwrap_err();
                        assert!(
                            error
                                .to_string()
                                .contains("conflicting native shared-enum literal definitions"),
                            "{error}"
                        );
                    }
                }
            }
        }
    }
}

fn shared_control_plan(
    target: &str,
    selected_prefix: &str,
    losing_prefix: &str,
    use_prefix: &str,
    reversed: bool,
) -> HeaderPartitionPlan {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("shared_enum_authority_control");
    let a = directory.join("a.h");
    let b = directory.join("b.h");
    let consumer = directory.join("consumer.h");
    let a = a.to_str().unwrap();
    let b = b.to_str().unwrap();
    let consumer = consumer.to_str().unwrap();
    let mut inputs = vec![
        Input::new("control-a.cpp", format!("#include {a:?}\n"))
            .with_roots([a])
            .partitioned("A"),
        Input::new(
            "control-b.cpp",
            format!("{selected_prefix}#include {b:?}\n"),
        )
        .with_roots([b])
        .partitioned("B"),
        Input::new(
            "control-use.cpp",
            format!("{use_prefix}#include {consumer:?}\n"),
        )
        .with_roots([a, consumer])
        .partitioned("Use"),
        Input::new(
            "control-losing.cpp",
            format!("{losing_prefix}#include {a:?}\n"),
        )
        .with_roots([a])
        .partitioned("Losing"),
    ];
    if reversed {
        inputs.reverse();
    }
    let policy = HeaderPartitionPolicy::new()
        .with_partition_authority("A", 0, "control-a.cpp")
        .with_partition_authority("B", 1, "control-b.cpp")
        .with_partition_authority("Use", 2, "control-use.cpp")
        .with_partition_authority("Losing", 3, "control-losing.cpp")
        .with_traversed_header(
            a,
            RootPartition::new("A", "Test.A").with_exclusion("CONTROL_ENUM"),
        )
        .with_traversed_header(b, RootPartition::new("Use", "Test.Use"))
        .with_traversed_header(a, RootPartition::new("Losing", "Test.Losing"))
        .with_traversed_header(b, RootPartition::new("B", "Test.B"))
        .with_traversed_header(consumer, RootPartition::new("Use", "Test.Use"));
    extract_ranked_header_partitions_with_options(
        inputs,
        &[
            "--no-default-config",
            "-x",
            "c++",
            "-std=c++17",
            &format!("--target={target}"),
            "-nostdinc",
        ],
        &policy,
        &NamespaceAuthorities::new(),
        &ExtractionOptions::new().with_module_free_headers(),
    )
    .unwrap()
}

fn certified_shared_body(
    report: &HeaderPartitionReport,
    different_a: bool,
    different_b: bool,
) -> BTreeMap<RdlPartition, String> {
    assert!(report.omissions.is_empty());
    let DefinitionEvidenceCoverage::CapturedBodiesAndReferentConst { enums, .. } =
        &report.definition_evidence_coverage
    else {
        panic!("{report:?}")
    };
    assert_eq!(
        *enums,
        DefinitionDomainCoverage {
            selected_definitions: 2,
            alternative_definitions: 2,
            declaration_only_observations: 0,
        }
    );
    if !different_a && !different_b {
        assert!(report.definition_evidence.is_empty(), "{report:?}");
        assert!(
            report
                .partitions
                .values()
                .all(|text| !text.contains("// windows-clang:")),
            "{report:?}"
        );
        return report.partitions.clone();
    }
    let mut expected = Vec::new();
    if different_a {
        expected.push((
            "/a.h",
            "control-a.cpp",
            "A",
            0,
            "control-losing.cpp",
            "Losing",
            3,
        ));
    }
    if different_b {
        expected.push(("/b.h", "control-b.cpp", "B", 1, "control-use.cpp", "Use", 2));
    }
    assert_eq!(
        report.definition_evidence.len(),
        expected.len(),
        "{report:?}"
    );
    let mut certified_notes = Vec::new();
    let mut messages = BTreeSet::new();
    for (
        row,
        (
            header,
            selected_input,
            selected_partition,
            selected_rank,
            alternative_input,
            alternative_partition,
            alternative_rank,
        ),
    ) in report.definition_evidence.iter().zip(expected)
    {
        assert_eq!(row.body_kind, DefinitionBodyKind::Enum);
        assert_eq!(row.identity.name, "CONTROL_ENUM");
        assert_eq!(row.identity.role, DefinitionTypeRole::Enum);
        assert!(row.identity.scope.is_empty());
        assert!(row.identity.header.ends_with(header));
        assert_eq!(row.identity.authority_input, selected_input);
        assert_eq!(row.selected.input, selected_input);
        assert_eq!(row.selected.partition.as_deref(), Some(selected_partition));
        assert_eq!(row.selected.rank, Some(selected_rank));
        assert_eq!(
            row.selected.candidate_partitions,
            [(selected_partition.to_string(), selected_rank)]
        );
        assert_eq!(row.selected.location.file, row.identity.header);
        assert_eq!(
            row.selected.defining_locations,
            [row.selected.location.clone()]
        );
        assert_eq!(row.alternative.input, alternative_input);
        assert_eq!(
            row.alternative.partition.as_deref(),
            Some(alternative_partition)
        );
        assert_eq!(row.alternative.rank, Some(alternative_rank));
        assert_eq!(
            row.alternative.candidate_partitions,
            [(alternative_partition.to_string(), alternative_rank)]
        );
        assert_eq!(row.alternative.location.file, row.identity.header);
        assert_eq!(
            row.alternative.defining_locations,
            [row.alternative.location.clone()]
        );
        if header == "/a.h" {
            let provider = row.emitted_provider.as_ref().unwrap();
            assert!(provider.identity.header.ends_with("/b.h"));
            assert_eq!(provider.identity.authority_input, "control-b.cpp");
            assert_eq!(provider.identity.name, "CONTROL_ENUM");
            assert_eq!(provider.identity.role, DefinitionTypeRole::Enum);
            assert!(provider.identity.scope.is_empty());
            assert_eq!(provider.observation.input, "control-b.cpp");
            assert_eq!(provider.observation.partition.as_deref(), Some("B"));
            assert_eq!(provider.observation.rank, Some(1));
            assert_eq!(
                provider.observation.candidate_partitions,
                [("B".to_string(), 1)]
            );
            assert_eq!(provider.observation.location.file, provider.identity.header);
            assert_eq!(
                provider.observation.defining_locations,
                [provider.observation.location.clone()]
            );
            assert!(row.message.contains("emitted-provider="));
        } else {
            assert!(row.emitted_provider.is_none(), "{row:?}");
            assert!(!row.message.contains("emitted-provider="));
        }
        assert_eq!(
            row.selected_emission,
            Some(DefinitionEmission::Enum {
                repr: Scalar::I32,
                flags: false,
            })
        );
        let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
            panic!("{row:?}")
        };
        assert!(unknown.is_empty(), "{row:?}");
        assert_eq!(
            keys,
            &[DefinitionDifference {
                path: "variant[1].value".to_string(),
                selected: "1".to_string(),
                alternative: "2".to_string(),
            }]
        );
        assert!(row.message.contains(selected_input));
        assert!(row.message.contains(alternative_input));
        assert!(row.message.contains("control-b.cpp"));
        assert!(messages.insert(&row.message), "{row:?}");
        certified_notes.push(format!("        // windows-clang: {}\n", row.message));
    }
    let mut body = report.partitions.clone();
    let mut notes_count = 0;
    for (partition, text) in &mut body {
        if partition.partition == "B" {
            let block = certified_notes.join("");
            assert!(
                text.contains(&format!("{block}        #[repr(i32)]")),
                "{text}"
            );
        }
        for note in &certified_notes {
            let count = text.matches(note).count();
            let expected = usize::from(partition.partition == "B");
            assert_eq!(count, expected, "{text}");
            if expected == 1 {
                *text = text.replacen(note, "", 1);
            }
            notes_count += count;
        }
        assert!(!text.contains("// windows-clang:"), "{text}");
    }
    assert_eq!(notes_count, certified_notes.len());
    body
}

#[test]
fn native_ranked_shared_enum_reporting_provenance_and_unknown() {
    helpers::ensure_libclang();
    let references = BTreeMap::new();
    let options = EmitOptions::new("Test", &references);
    let mut baseline_body = None;
    for (different, different_b) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut baseline_report = None;
        for reversed in [false, true] {
            let plan = shared_control_plan(
                "x86_64-pc-windows-msvc",
                "",
                if different {
                    "#define CONTROL_DIFFERENT\n"
                } else {
                    ""
                },
                if different_b {
                    "#define CONTROL_DIFFERENT\n"
                } else {
                    ""
                },
                reversed,
            );
            let report = plan.clone().emit_with_report(&options).unwrap();
            let body = certified_shared_body(&report, different, different_b);
            if let Some(baseline_body) = &baseline_body {
                assert_eq!(&body, baseline_body);
            } else {
                baseline_body = Some(body);
            }
            assert_control_metadata(&report, 1);
            if let Some(baseline_report) = &baseline_report {
                assert_eq!(&report, baseline_report);
            } else {
                baseline_report = Some(report.clone());
            }
            println!(
                "REPORTING different_a={different} different_b={different_b} reversed={reversed} report={report:?}"
            );
            if different_b {
                continue;
            }
            let mut unknown_plan = plan;
            let origin = unknown_plan
                .snapshot
                .facts
                .iter()
                .find(|fact| fact.origin.tu == "control-losing.cpp" && fact.kind == FactKind::Enum)
                .unwrap()
                .origin
                .clone();
            unknown_plan.snapshot.source_annotations.insert(
                AnnotationTarget::Declaration(origin),
                vec![Annotation::AssociatedConstant(
                    "uncompared control".to_string(),
                )],
            );
            let unknown_report = unknown_plan.emit_with_report(&options).unwrap();
            assert_eq!(unknown_report.definition_evidence.len(), 1);
            let row = &unknown_report.definition_evidence[0];
            let unknown = match &row.outcome {
                DefinitionComparison::Different { keys, unknown } => {
                    assert!(different);
                    assert_eq!(
                        keys,
                        match &report.definition_evidence[0].outcome {
                            DefinitionComparison::Different { keys, .. } => keys,
                            _ => unreachable!(),
                        }
                    );
                    unknown
                }
                DefinitionComparison::Unknown(unknown) => {
                    assert!(!different);
                    unknown
                }
            };
            assert_eq!(unknown.len(), 1, "{row:?}");
            assert_eq!(unknown[0].path, "annotations.uncompared");
            assert_eq!(
                row.emitted_provider.as_ref().unwrap().observation.input,
                "control-b.cpp"
            );
            let note = format!("        // windows-clang: {}\n", row.message);
            let mut unknown_body = unknown_report.partitions.clone();
            for (partition, text) in &mut unknown_body {
                let expected = usize::from(different && partition.partition == "B");
                assert_eq!(text.matches(&note).count(), expected, "{text}");
                if expected == 1 {
                    *text = text.replacen(&note, "", 1);
                }
                assert!(!text.contains("// windows-clang:"), "{text}");
            }
            assert_eq!(&unknown_body, baseline_body.as_ref().unwrap());
            println!(
                "REPORTING_UNKNOWN different={different} reversed={reversed} coverage={:?} row={:?} unknown={row:?}",
                report.definition_evidence_coverage, report.definition_evidence
            );
        }
    }
}

fn assert_control_metadata(report: &HeaderPartitionReport, value: i32) {
    let path = std::env::temp_dir().join(format!(
        "shared-enum-authority-{}-{:?}.winmd",
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
    let enumeration = index.expect("Test.B", "CONTROL_ENUM");
    assert_eq!(
        enumeration.underlying_type(),
        Some(windows_metadata::Type::I32)
    );
    let variants: Vec<_> = enumeration
        .fields()
        .filter_map(|field| {
            field
                .constant()
                .map(|constant| (field.name(), constant.value()))
        })
        .collect();
    assert_eq!(
        variants,
        [
            ("CONTROL_NONE", windows_metadata::Value::I32(0)),
            ("CONTROL_VALUE", windows_metadata::Value::I32(value)),
        ]
    );
    assert!(!index.contains("Test.A", "CONTROL_ENUM"));
    let record = index.expect("Test.Use", "CONTROL_REQUIRED");
    let fields: Vec<_> = record.fields().collect();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name(), "value");
    assert_eq!(
        fields[0].ty(),
        windows_metadata::Type::ValueName(windows_metadata::TypeName::named(
            "Test.B",
            "CONTROL_ENUM"
        ))
    );
    println!("READER enum=Test.B.CONTROL_ENUM value={value} required_field=Test.B.CONTROL_ENUM");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn native_ranked_shared_enum_authority_boundaries() {
    helpers::ensure_libclang();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("shared_enum_authority_control");
    let a = directory.join("a.h");
    let b = directory.join("b.h");
    let consumer = directory.join("consumer.h");
    let a = a.to_str().unwrap();
    let b = b.to_str().unwrap();
    let consumer = consumer.to_str().unwrap();
    for case in [
        "unproved-a",
        "unproved-b",
        "unproved-required",
        "proved-excluded-required",
        "absent-required",
        "absent-unrelated",
        "owners",
        "missing-provider",
        "unranked-disagreement",
    ] {
        for reversed in [false, true] {
            let absent = case.starts_with("absent-");
            let unranked = case == "unranked-disagreement";
            let mut inputs = vec![
                Input::new(
                    "boundary-b.cpp",
                    format!(
                        "{}#include {b:?}\n",
                        if case == "unproved-b" {
                            "#define CONTROL_UNPROVED\n"
                        } else {
                            ""
                        }
                    ),
                )
                .with_roots([b])
                .partitioned("B"),
                Input::new(
                    "boundary-use.cpp",
                    format!(
                        "{}#include {consumer:?}\n",
                        if matches!(
                            case,
                            "absent-required" | "unproved-required" | "proved-excluded-required"
                        ) {
                            "#define CONTROL_USE_A\n"
                        } else {
                            ""
                        }
                    ),
                )
                .with_roots([a, b, consumer])
                .partitioned("Use"),
                Input::new(
                    "boundary-losing.cpp",
                    format!(
                        "{}#include {a:?}\n",
                        if unranked {
                            "#define CONTROL_DIFFERENT\n"
                        } else {
                            ""
                        }
                    ),
                )
                .with_roots([a])
                .partitioned("Losing"),
            ];
            if !absent {
                inputs.push(
                    Input::new(
                        "boundary-a.cpp",
                        format!(
                            "{}#include {a:?}\n",
                            if matches!(case, "unproved-a" | "unproved-required") {
                                "#define CONTROL_UNPROVED\n"
                            } else {
                                ""
                            }
                        ),
                    )
                    .with_roots([a])
                    .partitioned("A"),
                );
            }
            if reversed {
                inputs.reverse();
            }
            let a_owner = if case == "owners" || case == "absent-required" {
                RootPartition::new("A", "Test.A")
            } else {
                RootPartition::new("A", "Test.A").with_exclusion("CONTROL_ENUM")
            };
            let b_owner = if case == "missing-provider" {
                RootPartition::new("B", "Test.B").with_exclusion("CONTROL_ENUM")
            } else {
                RootPartition::new("B", "Test.B")
            };
            let mut policy = HeaderPartitionPolicy::new();
            if !unranked {
                policy = policy
                    .with_partition_authority("A", 0, "boundary-a.cpp")
                    .with_partition_authority("B", 1, "boundary-b.cpp")
                    .with_partition_authority("Use", 2, "boundary-use.cpp")
                    .with_partition_authority("Losing", 3, "boundary-losing.cpp")
                    .with_traversed_header(a, a_owner)
                    .with_traversed_header(b, b_owner)
                    .with_traversed_header(a, RootPartition::new("Losing", "Test.Losing"))
                    .with_traversed_header(a, RootPartition::new("Use", "Test.Use"))
                    .with_traversed_header(b, RootPartition::new("Use", "Test.Use"))
                    .with_traversed_header(consumer, RootPartition::new("Use", "Test.Use"));
            } else {
                policy.add_traversed_header_for_input("boundary-a.cpp", a, a_owner);
                policy.add_traversed_header_for_input("boundary-b.cpp", b, b_owner);
                policy.add_traversed_header_for_input(
                    "boundary-use.cpp",
                    consumer,
                    RootPartition::new("Use", "Test.Use"),
                );
            }
            let plan = extract_partitioned_with_options(
                inputs,
                &[
                    "--no-default-config",
                    "-x",
                    "c++",
                    "-std=c++17",
                    "--target=x86_64-pc-windows-msvc",
                    "-nostdinc",
                ],
                &ExtractionOptions::new().with_module_free_headers(),
            )
            .unwrap()
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap();
            let snapshot = &plan.snapshot;
            let selected = snapshot
                .facts
                .iter()
                .find(|fact| {
                    matches!(fact.data, FactData::Enum { .. })
                        && fact.origin.tu
                            == if matches!(
                                case,
                                "unproved-a" | "unproved-required" | "proved-excluded-required"
                            ) {
                                "boundary-a.cpp"
                            } else {
                                "boundary-b.cpp"
                            }
                })
                .unwrap();
            let providers = PlanningSourceNames::shared_enum_providers(snapshot);
            if case == "proved-excluded-required" {
                let provider = snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        fact.origin.tu == "boundary-b.cpp"
                            && matches!(fact.data, FactData::Enum { .. })
                    })
                    .unwrap();
                let alias = snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        fact.origin.tu == selected.origin.tu
                            && fact.kind == FactKind::Typedef
                            && fact.name == "CONTROL_ENUM"
                    })
                    .unwrap();
                assert!(
                    snapshot
                        .shared_enum_identities
                        .contains_key(&selected.origin)
                );
                assert!(snapshot.shared_enum_identities.contains_key(&alias.origin));
                assert_eq!(providers[&selected.origin], Ok(provider.origin.clone()));
                assert_eq!(providers[&alias.origin], Ok(provider.origin.clone()));
                assert!(!selected.root);
                assert!(
                    snapshot.root_owners[&selected.origin]
                        .exclusions
                        .contains("CONTROL_ENUM")
                );
                let required = snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        fact.origin.tu == "boundary-use.cpp" && fact.name == "CONTROL_REQUIRED"
                    })
                    .unwrap();
                let FactData::Record { fields, .. } = &required.data else {
                    panic!("{required:?}")
                };
                let TypeRef::Named { declaration, .. } = &fields[0].ty else {
                    panic!("{required:?}")
                };
                let requested_alias = snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        fact.origin.tu == required.origin.tu
                            && fact.kind == FactKind::Typedef
                            && fact.spelling == *declaration
                    })
                    .unwrap();
                assert_eq!(requested_alias.spelling.file, selected.spelling.file);
                assert!(
                    snapshot
                        .shared_enum_identities
                        .contains_key(&requested_alias.origin)
                );
                println!(
                    "PROVED_EXCLUDED selected={selected:?} alias={alias:?} \
                    provider={provider:?} required={required:?} requested_alias={requested_alias:?}"
                );
            }
            if case.starts_with("unproved-") {
                assert!(
                    !snapshot
                        .shared_enum_identities
                        .contains_key(&selected.origin)
                );
                let alternative = snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        matches!(fact.data, FactData::Enum { .. })
                            && fact.spelling.file == selected.spelling.file
                            && fact.origin.tu != selected.origin.tu
                    })
                    .unwrap();
                assert!(
                    snapshot
                        .shared_enum_identities
                        .contains_key(&alternative.origin)
                );
                assert!(providers.is_empty(), "{providers:?}");
            }
            if absent {
                assert!(providers.is_empty(), "{providers:?}");
            }
            if case == "owners" {
                assert_eq!(providers.len(), 4);
                assert!(providers.values().all(|provider| matches!(
                    provider,
                    Err(SharedEnumProviderFailure::ConflictingOwners)
                )));
            }
            if case == "missing-provider" {
                assert_eq!(providers.len(), 4);
                assert!(providers.values().all(|provider| matches!(
                    provider,
                    Err(SharedEnumProviderFailure::MissingProvider)
                )));
            }
            if unranked {
                assert_eq!(providers.len(), 8);
                assert!(providers.values().all(|provider| matches!(
                    provider,
                    Err(SharedEnumProviderFailure::ConflictingDefinitions)
                )));
            }
            println!(
                "BOUNDARY case={case} reversed={reversed} native_selected={selected:?} \
                                    native_identities={:?} providers={providers:?}",
                snapshot.shared_enum_identities
            );
            let references = BTreeMap::new();
            let outcome = plan.emit_with_report(&EmitOptions::new("Test", &references));
            println!("BOUNDARY_STRICT case={case} reversed={reversed} outcome={outcome:#?}");
            match case {
                "unproved-a" | "unproved-b" | "absent-unrelated" => {
                    let report = outcome.unwrap();
                    assert!(report.omissions.is_empty());
                    assert_control_metadata(&report, if case == "unproved-b" { 3 } else { 1 });
                }
                "absent-required" => assert!(
                    outcome
                        .unwrap_err()
                        .to_string()
                        .contains("RequiredAuthorityContextAbsent")
                ),
                "unproved-required" => assert!(
                    outcome
                        .unwrap_err()
                        .to_string()
                        .contains("is excluded by authoritative partition")
                ),
                "proved-excluded-required" => {
                    let error = outcome.unwrap_err().to_string();
                    assert!(
                        error.contains("is excluded by authoritative partition"),
                        "{error}"
                    );
                    assert!(
                        error.contains("original input \"boundary-a.cpp\""),
                        "{error}"
                    );
                }
                "unranked-disagreement" => assert!(
                    outcome
                        .unwrap_err()
                        .to_string()
                        .contains("conflicting native shared-enum literal definitions")
                ),
                _ => assert!(outcome.is_err(), "{outcome:?}"),
            }
        }
    }
}

#[test]
fn native_ranked_shared_enum_unregistered_observation_control() {
    helpers::ensure_libclang();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("shared_enum_authority_control");
    let a = directory.join("a.h");
    let b = directory.join("b.h");
    let consumer = directory.join("consumer.h");
    let a = a.to_str().unwrap();
    let b = b.to_str().unwrap();
    let consumer = consumer.to_str().unwrap();
    let mut baseline = None;
    let mut compatible_failures = Vec::new();
    for case in [
        "baseline",
        "unrelated-disagreement",
        "requested-compatible",
        "requested-incompatible",
        "requested-unproved",
    ] {
        let requested_a = case.starts_with("requested-");
        let different = case == "unrelated-disagreement";
        for reversed in [false, true] {
            let mut inputs = vec![
                Input::new("unregistered-b.cpp", format!("#include {b:?}\n"))
                    .with_roots([b])
                    .partitioned("B"),
                Input::new(
                    "unregistered-use.cpp",
                    format!(
                        "{}{}#include {consumer:?}\n",
                        if requested_a {
                            "#define CONTROL_USE_A\n"
                        } else {
                            ""
                        },
                        match case {
                            "requested-incompatible" => "#define CONTROL_DIFFERENT\n",
                            "requested-unproved" => "#define CONTROL_UNPROVED\n",
                            _ => "",
                        }
                    ),
                )
                .with_roots([b, consumer])
                .partitioned("Use"),
                Input::new(
                    "unregistered-losing.cpp",
                    format!(
                        "{}#include {a:?}\n",
                        if different {
                            "#define CONTROL_DIFFERENT\n"
                        } else {
                            ""
                        }
                    ),
                )
                .with_roots([a])
                .partitioned("Losing"),
            ];
            if reversed {
                inputs.reverse();
            }
            let policy = HeaderPartitionPolicy::new()
                .with_partition_authority("B", 0, "unregistered-b.cpp")
                .with_partition_authority("Use", 1, "unregistered-use.cpp")
                .with_partition_authority("Losing", 2, "unregistered-losing.cpp")
                .with_traversed_header(b, RootPartition::new("B", "Test.B"))
                .with_traversed_header(b, RootPartition::new("Use", "Test.Use"))
                .with_traversed_header(consumer, RootPartition::new("Use", "Test.Use"));
            let plan = extract_partitioned_with_options(
                inputs,
                &[
                    "--no-default-config",
                    "-x",
                    "c++",
                    "-std=c++17",
                    "--target=x86_64-pc-windows-msvc",
                    "-nostdinc",
                ],
                &ExtractionOptions::new().with_module_free_headers(),
            )
            .unwrap()
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap();
            let snapshot = &plan.snapshot;
            assert_eq!(
                snapshot.shared_enum_identities.len(),
                if case == "requested-unproved" { 4 } else { 6 }
            );
            let keys: BTreeSet<_> = snapshot
                .shared_enum_identities
                .values()
                .map(|identity| {
                    (
                        &identity.definition.enum_usr,
                        &identity.definition.alias_usr,
                    )
                })
                .collect();
            assert_eq!(keys.len(), 1, "{keys:?}");
            let enumerations: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| matches!(fact.data, FactData::Enum { .. }))
                .collect();
            assert_eq!(enumerations.len(), 3);
            let files: BTreeSet<_> = enumerations
                .iter()
                .map(|fact| &fact.spelling.file)
                .collect();
            assert_eq!(files.len(), 2);
            let selected = enumerations
                .iter()
                .find(|fact| fact.origin.tu == "unregistered-b.cpp")
                .unwrap();
            let losing = enumerations
                .iter()
                .find(|fact| fact.origin.tu == "unregistered-losing.cpp")
                .unwrap();
            assert!(selected.root);
            assert_eq!(
                fact_header_authority(&snapshot.header_authorities, selected)
                    .unwrap()
                    .owner
                    .input,
                "unregistered-b.cpp"
            );
            assert!(!losing.root);
            assert!(!snapshot.root_owners.contains_key(&losing.origin));
            assert!(fact_header_authority(&snapshot.header_authorities, losing).is_none());
            assert_eq!(selected.data == losing.data, !different);
            let required = snapshot
                .facts
                .iter()
                .find(|fact| fact.name == "CONTROL_REQUIRED")
                .unwrap();
            let FactData::Record { fields, .. } = &required.data else {
                panic!("{required:?}")
            };
            let TypeRef::Named { declaration, .. } = &fields[0].ty else {
                panic!("{required:?}")
            };
            assert!(required.root);
            assert_eq!(
                declaration.file,
                if requested_a {
                    losing.spelling.file.clone()
                } else {
                    selected.spelling.file.clone()
                }
            );
            let alias = snapshot
                .facts
                .iter()
                .find(|fact| {
                    fact.kind == FactKind::Typedef
                        && fact.origin.tu == required.origin.tu
                        && fact.spelling == *declaration
                })
                .unwrap();
            let FactData::Typedef {
                target: TypeRef::Named { declaration, .. },
            } = &alias.data
            else {
                panic!("{alias:?}")
            };
            let requested_enum = enumerations
                .iter()
                .find(|fact| fact.origin.tu == required.origin.tu && fact.spelling == *declaration)
                .unwrap();
            let providers = PlanningSourceNames::shared_enum_providers(snapshot);
            assert_eq!(
                providers.len(),
                if requested_a && case != "requested-unproved" {
                    6
                } else {
                    4
                }
            );
            assert!(providers.contains_key(&losing.origin));
            assert_eq!(providers[&selected.origin], Ok(selected.origin.clone()));
            if requested_a {
                assert!(
                    fact_header_authority(&snapshot.header_authorities, requested_enum).is_none()
                );
                assert!(!snapshot.root_owners.contains_key(&requested_enum.origin));
                match case {
                    "requested-compatible" => {
                        assert_eq!(
                            providers[&requested_enum.origin],
                            Ok(selected.origin.clone())
                        );
                        assert_eq!(providers[&alias.origin], Ok(selected.origin.clone()));
                    }
                    "requested-incompatible" => {
                        assert_eq!(
                            providers[&requested_enum.origin],
                            Err(SharedEnumProviderFailure::ConflictingDefinitions)
                        );
                        assert_eq!(
                            providers[&alias.origin],
                            Err(SharedEnumProviderFailure::ConflictingDefinitions)
                        );
                        assert_eq!(providers[&losing.origin], Ok(selected.origin.clone()));
                    }
                    "requested-unproved" => {
                        assert_eq!(requested_enum.data, selected.data);
                        assert!(
                            !snapshot
                                .shared_enum_identities
                                .contains_key(&requested_enum.origin)
                        );
                        assert!(!snapshot.shared_enum_identities.contains_key(&alias.origin));
                        assert!(snapshot.shared_enum_identities.contains_key(&losing.origin));
                        assert!(!providers.contains_key(&requested_enum.origin));
                        assert!(!providers.contains_key(&alias.origin));
                    }
                    _ => unreachable!(),
                }
            } else if different {
                assert_eq!(
                    providers[&losing.origin],
                    Err(SharedEnumProviderFailure::ConflictingDefinitions)
                );
            }
            println!(
                "UNREGISTERED_NATIVE case={case} reversed={reversed} \
                facts={enumerations:#?} identities={:#?} required={required:#?} \
                requested_alias={alias:#?} requested_enum={requested_enum:#?} providers={providers:#?}",
                snapshot.shared_enum_identities
            );
            let references = BTreeMap::new();
            let outcome = plan.emit_with_report(&EmitOptions::new("Test", &references));
            println!(
                "UNREGISTERED_STRICT case={case} reversed={reversed} \
                outcome={outcome:#?}"
            );
            match case {
                "requested-incompatible" => {
                    assert!(
                        outcome
                            .unwrap_err()
                            .to_string()
                            .contains("conflicting native shared-enum literal definitions")
                    );
                }
                "requested-unproved" => {
                    let error = outcome.unwrap_err().to_string();
                    assert!(error.contains("ambiguous type root"), "{error}");
                    assert!(error.contains("selection phase=post-closure"), "{error}");
                    assert!(!error.contains("RequiredAuthorityContextAbsent"), "{error}");
                }
                "requested-compatible" if outcome.is_err() => {
                    compatible_failures.push((reversed, outcome.unwrap_err()));
                }
                _ => {
                    let report = outcome.unwrap();
                    assert!(report.omissions.is_empty());
                    assert_control_metadata(&report, 1);
                    if let Some(baseline) = &baseline {
                        assert_eq!(&report.partitions, baseline);
                    } else {
                        baseline = Some(report.partitions);
                    }
                }
            }
        }
    }
    assert!(
        compatible_failures.is_empty(),
        "native admission succeeded but strict requested use remains blocked: {compatible_failures:#?}"
    );
}
