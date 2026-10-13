use super::*;

fn annotation_plan(target: &str, flags: &[&str], reversed: bool) -> HeaderPartitionPlan {
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("ranked_types")
        .join("annotations.hpp");
    let header = header.to_str().unwrap();
    let enum_domain = if flags.contains(&"ENUM_UNKNOWN") {
        "#define ANNOTATION_ENUM_DOMAIN\n"
    } else {
        ""
    };
    let defines = flags
        .iter()
        .map(|flag| format!("#define ANNOTATION_{flag}\n"))
        .collect::<String>();
    let home_defines = flags
        .iter()
        .filter(|flag| matches!(**flag, "EQUAL_COUNT" | "COUNTS"))
        .map(|flag| format!("#define ANNOTATION_{flag}\n"))
        .collect::<String>();
    let mut inputs = vec![
        Input::new(
            "annotation-home.cpp",
            format!("{enum_domain}{home_defines}#include {header:?}\n"),
        )
        .with_roots([header])
        .partitioned("Home"),
        Input::new(
            "annotation-client.cpp",
            format!("#define ANNOTATION_ALTERNATIVE\n{enum_domain}{defines}#include {header:?}\n"),
        )
        .with_roots([header])
        .partitioned("Api"),
    ];
    if reversed {
        inputs.reverse();
    }
    let policy = HeaderPartitionPolicy::new()
        .with_partition_authority("Home", 0, "annotation-home.cpp")
        .with_partition_authority("Api", 1, "annotation-client.cpp")
        .with_traversed_header(header, RootPartition::new("Home", "Test.Home"))
        .with_traversed_header(header, RootPartition::new("Api", "Test.Alternative"));
    extract_partitioned(
        inputs,
        &[
            "-x",
            "c++",
            "-std=c++17",
            &format!("--target={target}"),
            "--no-default-config",
        ],
    )
    .unwrap()
    .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
    .unwrap()
}

fn method(plan: &HeaderPartitionPlan, input: &str) -> Method {
    let FactData::Interface { methods, .. } = &fact(plan, input, "AnnotationInterface").data else {
        panic!()
    };
    assert_eq!(methods.len(), 1);
    methods[0].clone()
}

fn method_annotations(plan: &HeaderPartitionPlan, input: &str) -> Vec<Annotation> {
    annotations_for(
        &plan.snapshot.source_annotations,
        &AnnotationTarget::Method {
            declaration: fact(plan, input, "AnnotationInterface").origin.clone(),
            index: 0,
        },
    )
    .to_vec()
}

fn annotation_report(plan: HeaderPartitionPlan) -> HeaderPartitionReport {
    let selected = method(&plan, "annotation-home.cpp");
    assert_eq!(selected.name, "Value");
    assert_eq!(selected.result, TypeRef::Scalar(Scalar::I32));
    assert_eq!(selected.params[0].name, "value");
    assert_eq!(
        selected.params[0].ty,
        TypeRef::Pointer {
            mutable: true,
            target: Box::new(TypeRef::Scalar(Scalar::I32)),
        }
    );
    assert!(selected.params[0].annotation.input);
    assert!(!selected.params[0].annotation.output);
    assert_eq!(selected.params[1].name, "count");
    assert_eq!(selected.params[1].ty, TypeRef::Scalar(Scalar::I32));
    assert_eq!(
        method_annotations(&plan, "annotation-home.cpp"),
        [
            Annotation::PreserveResult,
            Annotation::CanReturnErrorsAsSuccess
        ]
    );
    assert_eq!(
        fact(&plan, "annotation-home.cpp", "AnnotationEnum").data,
        FactData::Enum {
            repr: Scalar::U32,
            variants: vec![
                Variant {
                    name: "First".to_string(),
                    value: 5
                },
                Variant {
                    name: "Maximum".to_string(),
                    value: -1
                },
            ],
            fixed: true,
            scoped: true,
        }
    );
    let without_notes = output_without_notes(plan.clone());
    let report = report(plan).unwrap();
    assert!(report.omissions.is_empty(), "{report:?}");
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
    for (partition, text) in &report.partitions {
        assert_eq!(without_evidence_comments(text), without_notes[partition]);
    }
    for row in &report.definition_evidence {
        assert_eq!(row.selected.partition.as_deref(), Some("Home"));
        assert_eq!(row.selected.rank, Some(0));
        assert_eq!(row.alternative.partition.as_deref(), Some("Api"));
        assert_eq!(row.alternative.rank, Some(1));
        let count = report
            .partitions
            .values()
            .map(|text| {
                text.matches(&format!("// windows-clang: {}\n", row.message))
                    .count()
            })
            .sum::<usize>();
        assert_eq!(
            count,
            usize::from(matches!(
                row.outcome,
                DefinitionComparison::Different { .. }
            ))
        );
    }
    report
}

fn assert_annotation_metadata(report: &HeaderPartitionReport, target: &str) {
    let path = std::env::temp_dir().join(format!(
        "annotation-evidence-{}-{target}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_texts(report.partitions.values())
        .reference_default()
        .output(&path)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&path).unwrap();
    let interface = index.expect("Test.Home", "AnnotationInterface");
    let value = interface.methods().next().unwrap();
    let signature = value.signature(&[]);
    assert_eq!(signature.return_type, windows_metadata::Type::I32);
    assert_eq!(
        signature.types,
        [
            windows_metadata::Type::PtrMut(Box::new(windows_metadata::Type::I32), 1),
            windows_metadata::Type::I32,
        ]
    );
    assert!(
        value
            .find_attribute("CanReturnErrorsAsSuccessAttribute")
            .is_some()
    );
    let guid = interface.find_attribute("GuidAttribute").unwrap().value();
    assert_eq!(guid[0].1, windows_metadata::Value::U32(0x79000000));
    assert_eq!(guid[10].1, windows_metadata::Value::U8(2));
    let enumeration = index.expect("Test.Home", "AnnotationEnum");
    assert_eq!(
        enumeration.underlying_type(),
        Some(windows_metadata::Type::U32)
    );
    assert_eq!(
        enumeration
            .fields()
            .filter_map(|field| {
                field
                    .constant()
                    .map(|constant| (field.name(), constant.value()))
            })
            .collect::<Vec<_>>(),
        [
            ("First", windows_metadata::Value::U32(5)),
            ("Maximum", windows_metadata::Value::U32(0xffffffff))
        ]
    );
    assert!(!index.contains("Test.Alternative", "AnnotationInterface"));
    assert!(!index.contains("Test.Alternative", "AnnotationEnum"));
    std::fs::remove_file(path).unwrap();
}

fn stripped(report: &HeaderPartitionReport) -> BTreeMap<RdlPartition, String> {
    report
        .partitions
        .iter()
        .map(|(partition, text)| (partition.clone(), without_evidence_comments(text)))
        .collect()
}

#[test]
fn ranked_body_annotation_method_unknown_preserves_supported_difference() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let output = stripped(&annotation_report(annotation_plan(target, &[], false)));
        for unknown in [false, true] {
            let flags = if unknown {
                vec!["METHOD_CHANGE", "METHOD_UNKNOWN"]
            } else {
                vec!["METHOD_CHANGE"]
            };
            let mut baseline = None;
            for reversed in [false, true] {
                let plan = annotation_plan(target, &flags, reversed);
                let mut captured = method_annotations(&plan, "annotation-client.cpp");
                captured.sort();
                assert_eq!(
                    captured,
                    if unknown {
                        vec![
                            Annotation::PreserveResult,
                            Annotation::StaticLibrary("review.lib".to_string()),
                        ]
                    } else {
                        vec![Annotation::PreserveResult]
                    }
                );
                let report = annotation_report(plan);
                assert_eq!(report.definition_evidence.len(), 1);
                let row = evidence(&report, "AnnotationInterface");
                let (keys, unknown_keys) = differences(row);
                assert_eq!(keys.len(), 1, "{row:?}");
                assert_eq!(
                    keys[0].path,
                    "declared.method[0].annotations.can_return_errors_as_success"
                );
                assert_eq!(
                    unknown_keys
                        .iter()
                        .map(|key| key.path.as_str())
                        .collect::<Vec<_>>(),
                    if unknown {
                        vec!["declared.method[0].annotations.uncompared"]
                    } else {
                        vec![]
                    }
                );
                if !unknown {
                    assert_eq!(stripped(&report), output);
                    assert_annotation_metadata(&report, target);
                }
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
fn ranked_body_annotation_parameter_expression_preserves_declared_and_slot_direction() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let output = stripped(&annotation_report(annotation_plan(target, &[], false)));
        for expression in [false, true] {
            let flags = if expression {
                vec!["PARAMETER_CHANGE", "PARAMETER_EXPRESSION"]
            } else {
                vec!["PARAMETER_CHANGE"]
            };
            let mut baseline = None;
            for reversed in [false, true] {
                let plan = annotation_plan(target, &flags, reversed);
                let captured = method(&plan, "annotation-client.cpp").params[0]
                    .annotation
                    .clone();
                assert!(!captured.input);
                assert!(captured.output);
                assert_eq!(
                    captured.size,
                    expression.then(|| SalSize {
                        bytes: false,
                        value: SalSizeValue::Expression("count + 1".to_string())
                    })
                );
                let report = annotation_report(plan);
                assert_eq!(report.definition_evidence.len(), 1);
                let row = evidence(&report, "AnnotationInterface");
                let (keys, unknown) = differences(row);
                for prefix in ["declared.method[0]", "slots.slot[1]"] {
                    for (property, selected, alternative) in
                        [("input", "true", "false"), ("output", "false", "true")]
                    {
                        assert!(
                            keys.iter().any(|key| key.path
                                == format!("{prefix}.params.param[0].annotation.{property}")
                                && key.selected == selected
                                && key.alternative == alternative),
                            "{row:?}"
                        );
                    }
                }
                assert_eq!(
                    unknown
                        .iter()
                        .map(|key| key.path.as_str())
                        .collect::<Vec<_>>(),
                    if expression {
                        vec![
                            "declared.method[0].params.param[0].annotation.size.value",
                            "slots.slot[1].params.param[0].annotation.size.value",
                        ]
                    } else {
                        vec![]
                    }
                );
                assert!(
                    !keys.iter().any(|key| key.path.ends_with(".size.value")),
                    "{row:?}"
                );
                assert_eq!(stripped(&report), output);
                assert_annotation_metadata(&report, target);
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
fn ranked_body_annotation_equal_supported_with_unknown_is_report_only() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for flags in [
            vec!["METHOD_UNKNOWN"],
            vec!["PARAMETER_UNKNOWN"],
            vec!["EQUAL_COUNT"],
        ] {
            let output = stripped(&annotation_report(annotation_plan(
                target,
                if flags == ["EQUAL_COUNT"] {
                    &["EQUAL_COUNT"]
                } else {
                    &[]
                },
                false,
            )));
            let mut baseline = None;
            for reversed in [false, true] {
                let report = annotation_report(annotation_plan(target, &flags, reversed));
                assert_eq!(report.definition_evidence.len(), 1);
                let row = evidence(&report, "AnnotationInterface");
                let DefinitionComparison::Unknown(unknown) = &row.outcome else {
                    panic!("{row:?}")
                };
                let paths = unknown
                    .iter()
                    .map(|key| key.path.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(
                    paths,
                    match flags[0] {
                        "METHOD_UNKNOWN" => vec!["declared.method[0].annotations.uncompared"],
                        "PARAMETER_UNKNOWN" =>
                            vec!["declared.method[0].params.param[0].annotations.uncompared"],
                        "EQUAL_COUNT" => vec![
                            "declared.method[0].params.param[0].annotation.size.value",
                            "slots.slot[1].params.param[0].annotation.size.value"
                        ],
                        _ => unreachable!(),
                    }
                );
                if flags == ["EQUAL_COUNT"] {
                    assert_eq!(stripped(&report), output);
                    assert_annotation_metadata(&report, target);
                }
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
fn ranked_body_annotation_unsupported_parameter_diagnostic_preserves_captured_properties() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for different in [false, true] {
            let flags: &[&str] = if different {
                &["PARAMETER_CHANGE"]
            } else {
                &[]
            };
            let output = stripped(&annotation_report(annotation_plan(target, flags, false)));
            let mut baseline = None;
            for reversed in [false, true] {
                let mut plan = annotation_plan(target, flags, reversed);
                let origin = fact(&plan, "annotation-client.cpp", "AnnotationInterface")
                    .origin
                    .clone();
                // Capture rejects unsupported parameter representations before planning.
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
                Arc::make_mut(plan.snapshot.native_com_contracts.get_mut(&origin).unwrap()).slots
                    [1]
                .params[0]
                    .annotation
                    .unsupported = Some("observer control".to_string());
                let report = annotation_report(plan);
                assert_eq!(report.definition_evidence.len(), 1);
                let row = evidence(&report, "AnnotationInterface");
                let unknown = if different {
                    let (keys, unknown) = differences(row);
                    assert_eq!(keys.len(), 4, "{row:?}");
                    for prefix in ["declared.method[0]", "slots.slot[1]"] {
                        for property in ["input", "output"] {
                            assert!(
                                keys.iter().any(|key| key.path
                                    == format!("{prefix}.params.param[0].annotation.{property}")),
                                "{row:?}"
                            );
                        }
                    }
                    unknown
                } else {
                    let DefinitionComparison::Unknown(unknown) = &row.outcome else {
                        panic!("{row:?}")
                    };
                    unknown.as_slice()
                };
                assert_eq!(
                    unknown
                        .iter()
                        .map(|key| key.path.as_str())
                        .collect::<Vec<_>>(),
                    [
                        "declared.method[0].params.param[0].annotation.unsupported",
                        "slots.slot[1].params.param[0].annotation.unsupported"
                    ]
                );
                assert_eq!(stripped(&report), output);
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
fn ranked_body_annotation_invalid_count_is_local_and_numeric_order_is_equivalent() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let output = stripped(&annotation_report(annotation_plan(
            target,
            &["COUNTS"],
            false,
        )));
        for invalid in [false, true] {
            let flags = if invalid {
                vec!["COUNTS", "COUNT_INVALID"]
            } else {
                vec!["COUNTS", "EQUIVALENT"]
            };
            let mut baseline = None;
            for reversed in [false, true] {
                let mut plan = annotation_plan(target, &flags, reversed);
                let origin = fact(&plan, "annotation-client.cpp", "AnnotationInterface")
                    .origin
                    .clone();
                // Extraction rejects differing raw count spellings before the observer runs.
                let annotations = plan
                    .snapshot
                    .source_annotations
                    .get_mut(&AnnotationTarget::MethodParameter {
                        declaration: origin,
                        method: 0,
                        parameter: 0,
                    })
                    .unwrap();
                for annotation in annotations {
                    match annotation {
                        Annotation::ArrayCountConst(value) => {
                            *value = if invalid { "invalid" } else { "+02" }.to_string();
                        }
                        Annotation::MemorySizeConst(value) => *value = "+08".to_string(),
                        _ => {}
                    }
                }
                let report = annotation_report(plan);
                if invalid {
                    assert_eq!(report.definition_evidence.len(), 1);
                    let row = evidence(&report, "AnnotationInterface");
                    let (keys, unknown) = differences(row);
                    assert!(
                        keys.iter().any(|key| key.path
                            == "declared.method[0].params.param[0].annotations.optional"),
                        "{row:?}"
                    );
                    assert!(
                        keys.iter()
                            .any(|key| key.path
                                == "slots.slot[1].params.param[0].annotation.optional"),
                        "{row:?}"
                    );
                    assert_eq!(keys.len(), 3, "{row:?}");
                    assert!(
                        keys.iter().any(|key| key.path
                            == "declared.method[0].params.param[0].annotation.optional"),
                        "{row:?}"
                    );
                    assert_eq!(unknown.len(), 1, "{row:?}");
                    assert_eq!(
                        unknown[0].path,
                        "declared.method[0].params.param[0].annotations.array_count_const"
                    );
                    assert!(
                        !keys
                            .iter()
                            .any(|key| key.path.ends_with(".array_count_const")
                                || key.path.ends_with(".memory_size_const")),
                        "{row:?}"
                    );
                } else {
                    assert!(report.definition_evidence.is_empty(), "{report:?}");
                }
                if !invalid {
                    assert_eq!(stripped(&report), output);
                }
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
fn ranked_body_annotation_enum_unknown_preserves_supported_family_and_notes() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for different in [false, true] {
            let flags = if different {
                vec!["ENUM_UNKNOWN", "ENUM_CHANGE"]
            } else {
                vec!["ENUM_UNKNOWN"]
            };
            let mut baseline = None;
            for reversed in [false, true] {
                let report = annotation_report(annotation_plan(target, &flags, reversed));
                assert_eq!(report.definition_evidence.len(), 1, "{report:?}");
                let row = evidence(&report, "AnnotationEnum");
                let unknown = if different {
                    let (keys, unknown) = differences(row);
                    assert_eq!(keys.len(), 1, "{row:?}");
                    assert_eq!(keys[0].path, "annotations.supported_os");
                    unknown
                } else {
                    let DefinitionComparison::Unknown(unknown) = &row.outcome else {
                        panic!("{row:?}")
                    };
                    unknown.as_slice()
                };
                assert_eq!(
                    unknown
                        .iter()
                        .map(|key| key.path.as_str())
                        .collect::<Vec<_>>(),
                    [
                        "annotations.uncompared",
                        "variant[0].annotations.uncompared"
                    ]
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
