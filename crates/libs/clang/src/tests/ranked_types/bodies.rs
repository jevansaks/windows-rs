use super::*;

#[path = "annotations.rs"]
mod annotations;

fn body_plan(
    target: &str,
    case: &str,
    reversed: bool,
    provenance: &str,
    unused: Option<(&str, bool)>,
) -> HeaderPartitionPlan {
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("ranked_types")
        .join("bodies.hpp");
    let header = header.to_str().unwrap();
    let flags = match case {
        "signature" => "#define BODY_SIGNATURE\n",
        "record" => "#define BODY_SIGNATURE\n#define BODY_RECORD\n",
        "equivalent" => "#define BODY_EQUIVALENT\n",
        "order" => "#define BODY_METHOD_ORDER\n",
        "special" => "#define BODY_SPECIAL\n",
        "annotations" => "#define BODY_ANNOTATIONS\n",
        "enum-value" => "#define BODY_ENUM_VALUE\n",
        "enum-repr" => "#define BODY_ENUM_REPR\n",
        "enum-order" => "#define BODY_ENUM_ORDER\n",
        "enum-shape" => "#define BODY_ENUM_SHAPE\n",
        "both" => "#define BODY_SIGNATURE\n#define BODY_ENUM_VALUE\n",
        "iid" => "#define BODY_IID\n#define BODY_SIGNATURE\n",
        "flags" => "#define BODY_SIGNED_ENUM\n#define BODY_ENUM_VALUE\n",
        "shared" => "#define BODY_SHARED\n",
        "empty" => "#define BODY_EMPTY\n",
        "forward" => "#define BODY_FORWARD\n",
        "redeclared" => {
            "#define BODY_REDECLARATIONS\n#define BODY_SIGNATURE\n#define BODY_ENUM_VALUE\n"
        }
        "scope" => "#define BODY_OTHER_SCOPE\n#define BODY_SIGNATURE\n#define BODY_ENUM_VALUE\n",
        _ => "",
    };
    let home_flags = match case {
        "iid" => "#define BODY_IID\n",
        "flags" => "#define BODY_SIGNED_ENUM\n",
        "shared" => "#define BODY_SHARED\n",
        "empty" => "#define BODY_EMPTY\n",
        _ => "",
    };
    let home = Input::new(
        "body-home.cpp",
        format!("{home_flags}#include {header:?}\n"),
    )
    .with_roots([header])
    .partitioned("Home");
    let client = Input::new(
        "body-client.cpp",
        format!("#define BODY_ALTERNATIVE\n{flags}#include {header:?}\n"),
    )
    .with_roots([header])
    .partitioned("Api");
    let home_owner = if case == "flags" {
        RootPartition::new("Home", "Test.Home").with_flags("BodyEnum")
    } else {
        RootPartition::new("Home", "Test.Home")
    };
    let mut policy = HeaderPartitionPolicy::new()
        .with_partition_authority("Home", 0, "body-home.cpp")
        .with_partition_authority("Api", 1, "body-client.cpp")
        .with_traversed_header(header, home_owner);
    if provenance != "missing" {
        policy.add_traversed_header(header, RootPartition::new("Api", "Test.Alternative"));
    }
    if provenance == "ambiguous" {
        policy.set_partition_authority("OtherApi", 2, "body-client.cpp");
        policy.add_traversed_header(header, RootPartition::new("OtherApi", "Test.Other"));
    }
    if let Some((partition, home)) = unused {
        policy.set_partition_authority(
            partition,
            2,
            if home {
                "body-home.cpp"
            } else {
                "body-client.cpp"
            },
        );
    }
    let mut inputs = vec![home, client];
    if reversed {
        inputs.reverse();
    }
    let target = format!("--target={target}");
    let options = if case == "shared" {
        ExtractionOptions::new().with_module_free_headers()
    } else {
        ExtractionOptions::new()
    };
    extract_partitioned_with_options(
        inputs,
        &["-x", "c++", "-std=c++17", &target, "--no-default-config"],
        &options,
    )
    .unwrap()
    .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
    .unwrap()
}

fn assert_selected_body(plan: &HeaderPartitionPlan) {
    let interface = fact(plan, "body-home.cpp", "BodyInterface");
    let FactData::Interface {
        base,
        guid,
        methods,
    } = &interface.data
    else {
        panic!("{interface:?}")
    };
    assert_eq!(
        guid.as_deref(),
        Some("75000000-0000-0000-0000-000000000002")
    );
    let Some(TypeRef::Named { name, declaration }) = base else {
        panic!("{base:?}")
    };
    let base = fact(plan, "body-home.cpp", "BodyBase");
    assert_eq!(name, "BodyBase");
    assert_eq!(*declaration, base.spelling);
    assert_eq!(
        methods
            .iter()
            .map(|method| method.name.as_str())
            .collect::<Vec<_>>(),
        ["Value", "Overload", "Overload", "Property"]
    );
    assert_eq!(methods[0].result, TypeRef::Scalar(Scalar::I32));
    assert_eq!(methods[0].params[0].name, "value");
    assert!(methods[0].params[0].annotation.input);
    let alias = fact(plan, "body-home.cpp", "BodyConst");
    assert_eq!(
        methods[0].params[0].ty,
        TypeRef::Pointer {
            mutable: true,
            target: Box::new(TypeRef::Named {
                name: "BodyConst".to_string(),
                declaration: alias.spelling.clone()
            }),
        }
    );
    assert_eq!(methods[0].params[1].name, "count");
    assert_eq!(methods[0].params[1].ty, TypeRef::Scalar(Scalar::I32));
    assert_eq!(methods[1].params[0].ty, TypeRef::Scalar(Scalar::I32));
    assert_eq!(methods[2].params[0].ty, TypeRef::Scalar(Scalar::U32));
    assert!(!methods[3].special);
    let enumeration = fact(plan, "body-home.cpp", "BodyEnum");
    assert_eq!(
        enumeration.data,
        FactData::Enum {
            repr: Scalar::U32,
            variants: vec![
                Variant {
                    name: "First".to_string(),
                    value: 1
                },
                Variant {
                    name: "Last".to_string(),
                    value: -1
                },
            ],
            fixed: true,
            scoped: true,
        }
    );
}

fn body_report(plan: HeaderPartitionPlan) -> HeaderPartitionReport {
    assert_selected_body(&plan);
    let without_notes = output_without_notes(plan.clone());
    let report = report(plan).unwrap();
    assert!(report.omissions.is_empty(), "{report:?}");
    for (partition, text) in &report.partitions {
        assert_eq!(without_evidence_comments(text), without_notes[partition]);
    }
    let text = report.partitions.values().cloned().collect::<String>();
    for row in &report.definition_evidence {
        assert_eq!(row.selected.partition.as_deref(), Some("Home"));
        assert_eq!(row.selected.rank, Some(0));
        assert_eq!(row.selected.input, "body-home.cpp");
        assert_eq!(row.alternative.input, "body-client.cpp");
        let different = matches!(row.outcome, DefinitionComparison::Different { .. });
        assert_eq!(
            text.matches(&format!("// windows-clang: {}\n", row.message))
                .count(),
            usize::from(different)
        );
        match row.body_kind {
            DefinitionBodyKind::Interface => assert_eq!(
                row.selected_emission,
                Some(DefinitionEmission::Interface {
                    guid: Some(format!(
                        "75000000-0000-0000-0000-{:012}",
                        if row.identity.name == "BodyBase" {
                            1
                        } else {
                            2
                        }
                    )),
                    method_order: if row.identity.name == "BodyBase" {
                        vec![0]
                    } else {
                        vec![0, 2, 1, 3]
                    },
                }),
            ),
            DefinitionBodyKind::Enum => assert_eq!(
                row.selected_emission,
                Some(DefinitionEmission::Enum {
                    repr: Scalar::U32,
                    flags: false
                }),
            ),
            DefinitionBodyKind::Record => panic!("{row:?}"),
        }
    }
    report
}

fn counts(
    report: &HeaderPartitionReport,
) -> (&DefinitionDomainCoverage, &DefinitionDomainCoverage) {
    let DefinitionEvidenceCoverage::CapturedBodiesAndReferentConst {
        selected_records,
        alternative_definitions,
        declaration_only_observations,
        interfaces,
        enums,
    } = &report.definition_evidence_coverage
    else {
        panic!("{report:?}")
    };
    assert_eq!(
        (
            *selected_records,
            *alternative_definitions,
            *declaration_only_observations
        ),
        (0, 0, 0)
    );
    (interfaces, enums)
}

fn differences(row: &DefinitionEvidence) -> (&[DefinitionDifference], &[DefinitionUnknown]) {
    let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
        panic!("{row:?}")
    };
    (keys, unknown)
}

fn assert_body_metadata(report: &HeaderPartitionReport, target: &str, case: &str, reversed: bool) {
    let path = std::env::temp_dir().join(format!(
        "body-evidence-{}-{target}-{case}-{reversed}.winmd",
        std::process::id(),
    ));
    windows_rdl::reader()
        .input_texts(report.partitions.values())
        .reference_default()
        .output(&path)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&path).unwrap();
    let interface = index.expect("Test.Home", "BodyInterface");
    let base = index.expect("Test.Home", "BodyBase");
    assert_eq!(
        interface.find_attribute("GuidAttribute").unwrap().value(),
        vec![
            (String::new(), windows_metadata::Value::U32(0x75000000)),
            (String::new(), windows_metadata::Value::U16(0)),
            (String::new(), windows_metadata::Value::U16(0)),
            (String::new(), windows_metadata::Value::U8(0)),
            (String::new(), windows_metadata::Value::U8(0)),
            (String::new(), windows_metadata::Value::U8(0)),
            (String::new(), windows_metadata::Value::U8(0)),
            (String::new(), windows_metadata::Value::U8(0)),
            (String::new(), windows_metadata::Value::U8(0)),
            (String::new(), windows_metadata::Value::U8(0)),
            (String::new(), windows_metadata::Value::U8(2)),
        ]
    );
    assert_ne!(
        interface.find_attribute("GuidAttribute").unwrap().value(),
        base.find_attribute("GuidAttribute").unwrap().value()
    );
    let methods: Vec<_> = interface.methods().collect();
    assert_eq!(
        methods
            .iter()
            .map(|method| method.name())
            .collect::<Vec<_>>(),
        ["Value", "Overload", "Overload", "Property"]
    );
    let signature = methods[0].signature(&[]);
    assert_eq!(signature.return_type, windows_metadata::Type::I32);
    assert_eq!(
        signature.types,
        [
            windows_metadata::Type::PtrMut(
                Box::new(windows_metadata::Type::value_named(
                    "Test.Home",
                    "BodyConst"
                )),
                1
            ),
            windows_metadata::Type::I32,
        ]
    );
    assert_eq!(
        methods[1].signature(&[]).types,
        [windows_metadata::Type::U32]
    );
    assert_eq!(
        methods[2].signature(&[]).types,
        [windows_metadata::Type::I32]
    );
    assert!(
        methods[0]
            .find_attribute("CanReturnErrorsAsSuccessAttribute")
            .is_some()
    );
    let enumeration = index.expect("Test.Home", "BodyEnum");
    assert_eq!(
        enumeration.underlying_type(),
        Some(windows_metadata::Type::U32)
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
            ("First", windows_metadata::Value::U32(1)),
            ("Last", windows_metadata::Value::U32(0xffffffff)),
        ]
    );
    assert!(!index.contains("Test.Alternative", "BodyInterface"));
    assert!(!index.contains("Test.Alternative", "BodyEnum"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn ranked_body_signatures_and_enum_values_report_without_changing_selected_metadata() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in [
            "signature",
            "record",
            "order",
            "special",
            "annotations",
            "enum-value",
            "enum-repr",
            "enum-order",
            "enum-shape",
            "both",
        ] {
            let mut baseline = None;
            for reversed in [false, true] {
                let report = body_report(body_plan(target, case, reversed, "known", None));
                let (interfaces, enums) = counts(&report);
                assert_eq!(
                    *interfaces,
                    DefinitionDomainCoverage {
                        selected_definitions: 2,
                        alternative_definitions: 2,
                        declaration_only_observations: 0,
                    }
                );
                assert_eq!(
                    *enums,
                    DefinitionDomainCoverage {
                        selected_definitions: 1,
                        alternative_definitions: 1,
                        declaration_only_observations: 0,
                    }
                );
                assert_eq!(
                    report.definition_evidence.len(),
                    if case == "both" { 2 } else { 1 },
                    "{target} {case}: {report:?}"
                );
                let row = evidence(
                    &report,
                    if case.starts_with("enum-") {
                        "BodyEnum"
                    } else {
                        "BodyInterface"
                    },
                );
                let (keys, unknown) = differences(row);
                assert_eq!(unknown.is_empty(), case != "record", "{row:?}");
                let path = match case {
                    "signature" | "both" => "declared.method[0].result.type",
                    "record" => "slots.slot[1].result",
                    "order" => "declared.method[1].params.param[0].type",
                    "special" => "declared.method[3].special",
                    "annotations" => "declared.method[0].annotations.can_return_errors_as_success",
                    "enum-value" => "variant[0].value",
                    "enum-repr" | "enum-shape" => "repr",
                    "enum-order" => "variant[0].name",
                    _ => unreachable!(),
                };
                assert!(keys.iter().any(|key| key.path == path), "{row:?}");
                assert_body_metadata(&report, target, case, reversed);
                if let Some(baseline) = &baseline {
                    assert_eq!(&report, baseline);
                } else {
                    baseline = Some(report);
                }
            }
        }
    }
}

#[test]
fn ranked_body_alias_cv_annotation_order_and_unsigned_value_spelling_are_equivalent() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for reversed in [false, true] {
            let mut plan = body_plan(target, "equivalent", reversed, "known", None);
            let home = fact(&plan, "body-home.cpp", "BodyConst");
            let alternative = fact(&plan, "body-client.cpp", "BodyConst");
            assert_ne!(home.spelling.offset, alternative.spelling.offset);
            let enumeration = plan
                .snapshot
                .facts
                .iter_mut()
                .find(|fact| {
                    fact.origin.tu == "body-client.cpp"
                        && fact.name == "BodyEnum"
                        && fact.definition
                })
                .unwrap();
            let FactData::Enum { variants, .. } = &mut enumeration.data else {
                panic!()
            };
            variants[1].value = 0xffffffff;
            let report = body_report(plan);
            assert!(report.definition_evidence.is_empty(), "{report:?}");
            assert_body_metadata(&report, target, "equivalent", reversed);
        }
    }
}

#[test]
fn ranked_body_missing_contract_or_annotation_evidence_retains_independent_differences() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for missing_contract in [false, true] {
            let mut plan = body_plan(target, "signature", false, "known", None);
            let origin = fact(&plan, "body-client.cpp", "BodyInterface")
                .origin
                .clone();
            if missing_contract {
                assert!(plan.snapshot.native_com_contracts.remove(&origin).is_some());
            } else {
                let FactData::Interface { methods, .. } = &mut plan
                    .snapshot
                    .facts
                    .iter_mut()
                    .find(|fact| fact.origin == origin)
                    .unwrap()
                    .data
                else {
                    panic!()
                };
                methods[0].params[0].annotation.unsupported = Some("observer control".to_string());
            }
            let report = body_report(plan);
            assert_eq!(report.definition_evidence.len(), 1);
            let row = evidence(&report, "BodyInterface");
            let (keys, unknown) = differences(row);
            assert!(
                keys.iter()
                    .any(|key| key.path == "declared.method[0].result.type"),
                "{row:?}"
            );
            assert!(
                unknown.iter().any(|key| key.path
                    == if missing_contract {
                        "slots"
                    } else {
                        "declared.method[0].params.param[0].annotation.unsupported"
                    }),
                "{row:?}"
            );
            assert!(
                !keys
                    .iter()
                    .any(|key| key.path.starts_with(if missing_contract {
                        "slots."
                    } else {
                        "declared.method[0].params.param[0].annotation"
                    })),
                "{row:?}"
            );
        }
    }
    for target in TARGETS {
        let mut plan = body_plan(target, "enum-value", false, "known", None);
        let origin = fact(&plan, "body-client.cpp", "BodyEnum").origin.clone();
        plan.snapshot.source_annotations.insert(
            AnnotationTarget::Declaration(origin),
            vec![Annotation::AssociatedConstant(
                "uncompared control".to_string(),
            )],
        );
        let report = body_report(plan);
        assert_eq!(report.definition_evidence.len(), 1);
        let row = evidence(&report, "BodyEnum");
        let (keys, unknown) = differences(row);
        assert!(
            keys.iter().any(|key| key.path == "variant[0].value"),
            "{row:?}"
        );
        assert_eq!(unknown.len(), 1, "{row:?}");
        assert_eq!(unknown[0].path, "annotations.uncompared");
        assert!(!keys.iter().any(|key| key.path == "annotations"), "{row:?}");
    }
}

#[test]
fn ranked_body_finalized_iid_and_flag_settings_do_not_replace_native_keys() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["iid", "flags"] {
            let mut baseline = None;
            for reversed in [false, true] {
                let plan = body_plan(target, case, reversed, "known", None);
                let (name, expected) = if case == "iid" {
                    let selected = fact(&plan, "body-home.cpp", "BodyInterface");
                    assert!(matches!(
                        selected.data,
                        FactData::Interface { guid: None, .. }
                    ));
                    assert!(
                        plan.snapshot.native_com_contracts[&selected.origin]
                            .guid
                            .is_empty()
                    );
                    (
                        "BodyInterface",
                        DefinitionEmission::Interface {
                            guid: Some("75000000-0000-0000-0000-000000000002".to_string()),
                            method_order: vec![0, 2, 1, 3],
                        },
                    )
                } else {
                    for tu in ["body-home.cpp", "body-client.cpp"] {
                        assert!(matches!(
                            fact(&plan, tu, "BodyEnum").data,
                            FactData::Enum {
                                repr: Scalar::I32,
                                fixed: true,
                                scoped: true,
                                ..
                            }
                        ));
                    }
                    (
                        "BodyEnum",
                        DefinitionEmission::Enum {
                            repr: Scalar::U32,
                            flags: true,
                        },
                    )
                };
                let without_notes = output_without_notes(plan.clone());
                let report = report(plan).unwrap();
                assert_eq!(report.definition_evidence.len(), 1, "{report:?}");
                let row = evidence(&report, name);
                assert_eq!(row.selected_emission, Some(expected));
                let (keys, unknown) = differences(row);
                assert!(unknown.is_empty(), "{row:?}");
                assert!(
                    !keys
                        .iter()
                        .any(|key| key.path == "repr" || key.path.ends_with(".guid")),
                    "{row:?}"
                );
                for (partition, text) in &report.partitions {
                    assert_eq!(without_evidence_comments(text), without_notes[partition]);
                }
                assert_body_metadata(&report, target, case, reversed);
                if let Some(baseline) = &baseline {
                    assert_eq!(&report, baseline);
                } else {
                    baseline = Some(report);
                }
            }
        }
    }
}

#[test]
fn ranked_body_fixed_enum_forwards_count_contexts_and_definitions_not_variants() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["forward", "redeclared"] {
            let mut baseline = None;
            for reversed in [false, true] {
                let plan = body_plan(target, case, reversed, "known", None);
                for name in ["BodyInterface", "BodyEnum"] {
                    let declarations: Vec<_> = plan
                        .snapshot
                        .facts
                        .iter()
                        .filter(|fact| fact.origin.tu == "body-client.cpp" && fact.name == name)
                        .collect();
                    assert_eq!(
                        declarations.iter().filter(|fact| !fact.definition).count(),
                        2
                    );
                    assert_eq!(
                        declarations.iter().filter(|fact| fact.definition).count(),
                        usize::from(case == "redeclared")
                    );
                    if name == "BodyEnum" {
                        for fact in declarations.iter().filter(|fact| !fact.definition) {
                            assert!(
                                matches!(&fact.data, FactData::Enum { fixed: true, variants, .. }
                                if variants.is_empty()),
                                "{fact:?}"
                            );
                        }
                    }
                }
                let report = body_report(plan);
                let (interfaces, enums) = counts(&report);
                assert_eq!(
                    *interfaces,
                    DefinitionDomainCoverage {
                        selected_definitions: 2,
                        alternative_definitions: if case == "forward" { 1 } else { 2 },
                        declaration_only_observations: usize::from(case == "forward"),
                    }
                );
                assert_eq!(
                    *enums,
                    DefinitionDomainCoverage {
                        selected_definitions: 1,
                        alternative_definitions: usize::from(case == "redeclared"),
                        declaration_only_observations: usize::from(case == "forward"),
                    }
                );
                assert_eq!(
                    report.definition_evidence.len(),
                    if case == "forward" { 0 } else { 2 }
                );
                assert_body_metadata(&report, target, case, reversed);
                if let Some(baseline) = &baseline {
                    assert_eq!(&report, baseline);
                } else {
                    baseline = Some(report);
                }
            }
        }
    }
}

#[test]
fn ranked_body_distinct_semantic_scopes_never_pair_by_leaf_name_or_iid() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for reversed in [false, true] {
            let report = body_report(body_plan(target, "scope", reversed, "known", None));
            assert!(report.definition_evidence.is_empty(), "{report:?}");
            let (interfaces, enums) = counts(&report);
            assert_eq!(
                *interfaces,
                DefinitionDomainCoverage {
                    selected_definitions: 2,
                    alternative_definitions: 1,
                    declaration_only_observations: 0,
                }
            );
            assert_eq!(
                *enums,
                DefinitionDomainCoverage {
                    selected_definitions: 1,
                    alternative_definitions: 0,
                    declaration_only_observations: 0,
                }
            );
            assert_body_metadata(&report, target, "scope", reversed);
        }
    }
}

#[test]
fn ranked_body_enum_domain_excludes_shared_aliases_and_distinguishes_empty_definitions() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["shared", "empty"] {
            for reversed in [false, true] {
                let plan = body_plan(target, case, reversed, "known", None);
                if case == "shared" {
                    assert_eq!(plan.snapshot.shared_enum_identities.len(), 4);
                } else {
                    for tu in ["body-home.cpp", "body-client.cpp"] {
                        let native = plan
                            .snapshot
                            .facts
                            .iter()
                            .find(|fact| fact.origin.tu == tu && fact.name == "BodyEmpty")
                            .unwrap();
                        assert_eq!(native.definition, tu == "body-home.cpp");
                        assert!(
                            matches!(&native.data, FactData::Enum { fixed: true, scoped: true, variants, .. }
                            if variants.is_empty()),
                            "{native:?}"
                        );
                    }
                }
                let report = body_report(plan);
                assert!(report.definition_evidence.is_empty(), "{report:?}");
                let (_, enums) = counts(&report);
                assert_eq!(
                    *enums,
                    DefinitionDomainCoverage {
                        selected_definitions: if case == "shared" { 1 } else { 2 },
                        alternative_definitions: 1,
                        declaration_only_observations: usize::from(case == "empty"),
                    }
                );
                assert_body_metadata(&report, target, case, reversed);
            }
        }
    }
}

#[test]
fn ranked_body_provenance_uses_traversing_owners_and_retains_unknown_candidates() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let baseline = body_report(body_plan(target, "both", false, "known", None));
        for unused in ["AUnused", "ZUnused"] {
            for home in [false, true] {
                for reversed in [false, true] {
                    let report = body_report(body_plan(
                        target,
                        "both",
                        reversed,
                        "known",
                        Some((unused, home)),
                    ));
                    assert_eq!(report, baseline);
                }
            }
        }
        for provenance in ["ambiguous", "missing"] {
            for case in ["both", "equivalent"] {
                let mut baseline = None;
                for reversed in [false, true] {
                    let report = body_report(body_plan(target, case, reversed, provenance, None));
                    assert_eq!(report.definition_evidence.len(), 3);
                    for row in &report.definition_evidence {
                        assert_eq!(row.alternative.partition, None);
                        assert_eq!(row.alternative.rank, None);
                        assert_eq!(
                            row.alternative.candidate_partitions,
                            if provenance == "ambiguous" {
                                vec![("Api".to_string(), 1), ("OtherApi".to_string(), 2)]
                            } else {
                                Vec::new()
                            }
                        );
                        let unknown = match &row.outcome {
                            DefinitionComparison::Different { keys, unknown } => {
                                assert_eq!(case, "both");
                                assert!(!keys.is_empty());
                                unknown
                            }
                            DefinitionComparison::Unknown(unknown) => unknown,
                        };
                        assert!(
                            unknown
                                .iter()
                                .any(|key| key.path == "alternative.provenance"),
                            "{row:?}"
                        );
                    }
                    assert_eq!(
                        report
                            .definition_evidence
                            .iter()
                            .filter(|row| matches!(
                                row.outcome,
                                DefinitionComparison::Different { .. }
                            ))
                            .count(),
                        if case == "both" { 2 } else { 0 }
                    );
                    if let Some(baseline) = &baseline {
                        assert_eq!(&report, baseline);
                    } else {
                        baseline = Some(report);
                    }
                }
            }
        }
    }
}
