use super::*;

fn source_inputs(
    home: &str,
    alternative: &str,
    reversed: bool,
) -> (Vec<PartitionedInput>, HeaderPartitionPolicy) {
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("ranked_types")
        .join("annotations.hpp");
    let header = header.to_str().unwrap();
    let mut inputs = vec![
        Input::new(
            "annotation-home.cpp",
            format!("{home}\n#include {header:?}\n"),
        )
        .with_roots([header])
        .partitioned("Home"),
        Input::new(
            "annotation-client.cpp",
            format!("#define ANNOTATION_ALTERNATIVE\n{alternative}\n#include {header:?}\n"),
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
    (inputs, policy)
}

fn source_plan(
    target: &str,
    home: &str,
    alternative: &str,
    reversed: bool,
) -> Result<HeaderPartitionPlan, Error> {
    let (inputs, policy) = source_inputs(home, alternative, reversed);
    extract_ranked_header_partitions_with_options(
        inputs,
        &[
            "-x",
            "c++",
            "-std=c++17",
            &format!("--target={target}"),
            "--no-default-config",
        ],
        &policy,
        &NamespaceAuthorities::new(),
        &ExtractionOptions::new().with_parallelism(if reversed { 2 } else { 1 }),
    )
}

#[test]
fn ranked_source_annotations_ignore_alternative_metadata_without_hiding_differences() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let baseline = stripped(&annotation_report(
            source_plan(target, "", "", false).unwrap(),
        ));
        for alternative in [
            "#define ANNOTATION_METHOD_UNKNOWN",
            "#define ANNOTATION_METHOD_UNKNOWN\n#define ANNOTATION_METHOD_CHANGE",
            "#define ANNOTATION_PARAMETER_UNKNOWN",
            "#define ANNOTATION_PARAMETER_CHANGE",
        ] {
            let mut first = None;
            for reversed in [false, true] {
                let plan = source_plan(target, "", alternative, reversed).unwrap();
                let report = annotation_report(plan);
                assert_eq!(stripped(&report), baseline);
                assert_annotation_metadata(&report, target);
                if alternative.contains("CHANGE") {
                    assert!(
                        report.definition_evidence.iter().any(|row| matches!(
                            row.outcome,
                            DefinitionComparison::Different { .. }
                        ))
                    );
                }
                if let Some(first) = &first {
                    assert_eq!(first, &report);
                } else {
                    first = Some(report);
                }
            }
        }
    }
}

#[test]
fn ranked_source_counts_cross_context_spelling_is_not_a_redeclaration_error() {
    helpers::ensure_libclang();
    let home = "#define ANNOTATION_COUNTS";
    for target in TARGETS {
        let baseline = stripped(&annotation_report(
            source_plan(target, home, home, false).unwrap(),
        ));
        for (count, change) in [("+02", false), ("3", false), ("invalid", true)] {
            let alternative = format!(
                "{home}\n#define ANNOTATION_COUNT_TEXT \"{count}\"\n{}",
                if change {
                    "#define ANNOTATION_METHOD_CHANGE\n"
                } else {
                    ""
                }
            );
            for reversed in [false, true] {
                let report =
                    annotation_report(source_plan(target, home, &alternative, reversed).unwrap());
                assert_eq!(stripped(&report), baseline);
                assert_annotation_metadata(&report, target);
                let path = std::env::temp_dir().join(format!(
                    "source-count-{}-{target}.winmd",
                    std::process::id()
                ));
                windows_rdl::reader()
                    .input_texts(report.partitions.values())
                    .reference_default()
                    .output(&path)
                    .write()
                    .unwrap();
                let index = windows_metadata::reader::Index::read(&path).unwrap();
                let method = index
                    .expect("Test.Home", "AnnotationInterface")
                    .methods()
                    .next()
                    .unwrap();
                let parameter = method
                    .params()
                    .find(|param| param.name() == "value")
                    .unwrap();
                assert!(
                    parameter
                        .find_attribute("NativeArrayInfoAttribute")
                        .unwrap()
                        .value()
                        .iter()
                        .any(|(name, value)| name == "CountConst"
                            && matches!(
                                value,
                                windows_metadata::Value::I32(2) | windows_metadata::Value::U32(2)
                            ))
                );
                std::fs::remove_file(path).unwrap();
                assert_eq!(
                    report
                        .definition_evidence
                        .iter()
                        .any(|row| matches!(row.outcome, DefinitionComparison::Different { .. })),
                    count == "3" || change,
                    "{report:?}"
                );
                let (inputs, _) = source_inputs(home, &alternative, reversed);
                let error = extract_partitioned(
                    inputs,
                    &["-x", "c++", "-std=c++17", &format!("--target={target}")],
                )
                .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("conflicting redeclaration annotation"),
                    "{error}"
                );
            }
        }
        let plan = source_plan(
            target,
            "#define ANNOTATION_COUNTS\n#define ANNOTATION_COUNT_TEXT \"invalid\"",
            home,
            false,
        )
        .unwrap();
        let error = report(plan).unwrap_err();
        assert!(error.to_string().contains("not an i32"), "{error}");
    }
}

#[test]
fn ranked_source_native_sal_counts_keep_the_selected_context() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let home = "#define ANNOTATION_NATIVE_COUNTS";
        let baseline = stripped(&annotation_report(
            source_plan(target, home, home, false).unwrap(),
        ));
        for reversed in [false, true] {
            let report = annotation_report(
                source_plan(
                    target,
                    home,
                    "#define ANNOTATION_NATIVE_COUNTS\n#define ANNOTATION_COUNT_TEXT \"3\"",
                    reversed,
                )
                .unwrap(),
            );
            assert_eq!(stripped(&report), baseline);
            assert!(report.definition_evidence.iter().any(|row| matches!(
                &row.outcome,
                DefinitionComparison::Different { keys, .. }
                    if keys.iter().any(|key| key.path.ends_with(".annotation.size.value"))
            )));
            assert_annotation_metadata(&report, target);
        }
    }
}

#[test]
fn ranked_source_annotation_boundary_requires_a_ranked_policy() {
    let error = extract_ranked_header_partitions(
        [],
        &[],
        &HeaderPartitionPolicy::new(),
        &NamespaceAuthorities::new(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires registered partition authorities"),
        "{error}"
    );
}

fn shared_plan(
    target: &str,
    home: &str,
    alternative: &str,
    reversed: bool,
) -> Result<HeaderPartitionPlan, Error> {
    shared_plan_with_remaps(target, home, alternative, reversed, None)
}

fn shared_plan_with_remaps(
    target: &str,
    home: &str,
    alternative: &str,
    reversed: bool,
    remaps: Option<(&str, &str)>,
) -> Result<HeaderPartitionPlan, Error> {
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("ranked_types")
        .join("source_annotations.hpp");
    let header = header.to_str().unwrap();
    let mut inputs = vec![
        Input::new("source-home.cpp", format!("{home}\n#include {header:?}\n"))
            .with_roots([header])
            .partitioned("Home"),
        Input::new(
            "source-client.cpp",
            format!(
                "{alternative}\n#include {header:?}\n\
                 extern \"C\" void UseSource(SourceRecord* value);\n"
            ),
        )
        .with_roots([header, "source-client.cpp"])
        .partitioned("Api"),
    ];
    if reversed {
        inputs.reverse();
    }
    let mut home_owner = RootPartition::new("Home", "Test.Home");
    let mut alternative_owner = RootPartition::new("Api", "Test.Alternative");
    if let Some((home, alternative)) = remaps {
        home_owner = home_owner.with_remap("SourceEnum", home);
        alternative_owner = alternative_owner.with_remap("SourceEnum", alternative);
    }
    let policy = HeaderPartitionPolicy::new()
        .with_partition_authority("Home", 0, "source-home.cpp")
        .with_partition_authority("Api", 1, "source-client.cpp")
        .with_traversed_header(header, home_owner)
        .with_traversed_header(header, alternative_owner)
        .with_traversed_header("source-client.cpp", RootPartition::new("Api", "Test.Api"));
    extract_ranked_header_partitions(
        inputs,
        &[
            "-x",
            "c++",
            "-std=c++17",
            &format!("--target={target}"),
            "--no-default-config",
        ],
        &policy,
        &NamespaceAuthorities::new(),
    )
}

fn assert_shared_metadata(report: &HeaderPartitionReport, target: &str) {
    let path = std::env::temp_dir().join(format!(
        "source-annotations-{}-{target}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_texts(report.partitions.values())
        .reference_default()
        .output(&path)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&path).unwrap();
    for name in ["SourceRecord", "SourceEnum"] {
        let ty = index.expect("Test.Home", name);
        assert!(ty.find_attribute("SupportedOSPlatformAttribute").is_none());
        assert!(!index.contains("Test.Alternative", name));
    }
    let record = index.expect("Test.Home", "SourceRecord");
    for field in record.fields() {
        assert_eq!(
            field.find_attribute("ConstAttribute").is_some(),
            field.name() == "first"
        );
    }
    let interface = index.expect("Test.Home", "SourceInterface");
    let methods = interface.methods().collect::<Vec<_>>();
    assert_eq!(methods.len(), 2);
    assert!(
        methods[0]
            .find_attribute("CanReturnErrorsAsSuccessAttribute")
            .is_some()
    );
    assert!(
        methods[1]
            .find_attribute("CanReturnErrorsAsSuccessAttribute")
            .is_none()
    );
    assert!(methods.iter().all(|method| {
        method
            .params()
            .all(|parameter| parameter.find_attribute("DoNotReleaseAttribute").is_none())
    }));
    let callback = index.expect("Test.Home", "SourceCallback");
    assert!(
        callback
            .methods()
            .all(|method| method.params().all(|parameter| {
                !parameter
                    .flags()
                    .contains(windows_metadata::ParamAttributes::Optional)
            }))
    );
    let apis = index.expect("Test.Home", "Apis");
    let constant = apis
        .fields()
        .find(|field| field.name() == "SourceConstant")
        .unwrap();
    assert!(constant.find_attribute("UnicodeAttribute").is_some());
    assert!(
        apis.methods()
            .any(|method| method.name() == "SourceFunction")
    );
    assert!(index.expect("Test.Api", "Apis").methods().any(|method| {
        method.name() == "UseSource"
            && method
                .signature(&[])
                .types
                .iter()
                .any(|ty| matches!(ty, windows_metadata::Type::PtrMut(_, 1)))
    }));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn ranked_source_annotation_shared_projection_uses_finalized_provider_slots() {
    helpers::ensure_libclang();
    let cases = [
        ("TYPE_EXTRA", "supported_os=Windows11"),
        ("FIELD_EXTRA", "const"),
        ("NESTED_EXTRA", "const"),
        ("VARIANT_EXTRA", "associated_enum=SourceEnum"),
        ("TYPEDEF_EXTRA", "supported_os=Windows11"),
        ("CALLBACK_EXTRA", "optional"),
        ("RESULT_EXTRA", "do_not_release"),
        ("PARAMETER_EXTRA", "optional"),
    ];
    for target in TARGETS {
        let baseline = report(shared_plan(target, "", "", false).unwrap()).unwrap();
        assert_shared_metadata(&baseline, target);
        let baseline_output = stripped(&baseline);
        for (slot, annotation) in cases {
            let alternative =
                format!("#define {slot} __attribute__((annotate(\"win32metadata:{annotation}\")))");
            let mut first = None;
            for reversed in [false, true] {
                let plan = shared_plan(target, "", &alternative, reversed).unwrap();
                let interface = plan
                    .snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        fact.name == "SourceInterface" && fact.origin.tu == "source-home.cpp"
                    })
                    .unwrap();
                for (index, annotation) in [
                    Annotation::PreserveResult,
                    Annotation::CanReturnErrorsAsSuccess,
                ]
                .into_iter()
                .enumerate()
                {
                    assert_eq!(
                        annotations_for(
                            &plan.snapshot.annotations,
                            &AnnotationTarget::Method {
                                declaration: interface.origin.clone(),
                                index,
                            }
                        ),
                        [annotation]
                    );
                }
                let without_notes = output_without_notes(plan.clone());
                let report = report(plan).unwrap();
                assert_eq!(stripped(&report), baseline_output, "{slot}: {report:?}");
                assert_eq!(stripped(&report), without_notes);
                assert_shared_metadata(&report, target);
                for row in &report.definition_evidence {
                    let text = report.partitions.values().cloned().collect::<String>();
                    assert_eq!(
                        text.matches(&format!("// windows-clang: {}\n", row.message))
                            .count(),
                        usize::from(matches!(
                            row.outcome,
                            DefinitionComparison::Different { .. }
                        ))
                    );
                }

                if let Some(first) = &first {
                    assert_eq!(first, &report);
                } else {
                    first = Some(report);
                }
            }
        }
    }
}

#[test]
fn ranked_source_annotation_contexts_keep_same_leaf_semantic_scopes_separate() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for reversed in [false, true] {
            let plan = shared_plan(
                target,
                "#define SOURCE_SCOPES",
                "#define SOURCE_SCOPES\n#define TYPE_EXTRA \
                 __attribute__((annotate(\"win32metadata:supported_os=Windows12\")))",
                reversed,
            )
            .unwrap();
            let facts: HashMap<_, _> = plan
                .snapshot
                .facts
                .iter()
                .map(|fact| (&fact.origin, fact))
                .collect();
            let scoped: Vec<_> = plan
                .snapshot
                .facts
                .iter()
                .filter(|fact| fact.name == "ScopedRecord" && fact.origin.tu == "source-home.cpp")
                .collect();
            assert_eq!(scoped.len(), 2);
            for fact in scoped {
                let parent = facts[fact.parent.as_ref().unwrap()];
                assert_eq!(
                    annotations_for(
                        &plan.snapshot.annotations,
                        &AnnotationTarget::Declaration(fact.origin.clone())
                    ),
                    [Annotation::SupportedOs(
                        if parent.name == "ScopeOne" {
                            "Windows10"
                        } else {
                            "Windows11"
                        }
                        .to_string()
                    )]
                );
            }
            let error = report(plan).unwrap_err();
            assert!(
                error.to_string().contains("ambiguous logical owners"),
                "{error}"
            );
        }
    }
}

#[test]
fn ranked_source_annotations_retain_home_only_metadata_in_winmd() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for reversed in [false, true] {
            let plan = shared_plan(
                target,
                "#define FIELD_EXTRA __attribute__((annotate(\"win32metadata:const\")))\n\
                 #define NESTED_EXTRA __attribute__((annotate(\"win32metadata:const\")))\n\
                 #define CALLBACK_EXTRA __attribute__((annotate(\"win32metadata:optional\")))\n\
                 #define PARAMETER_EXTRA __attribute__((annotate(\"win32metadata:optional\")))",
                "",
                reversed,
            )
            .unwrap();
            let report = report(plan).unwrap();
            let text = report.partitions.values().cloned().collect::<String>();
            assert!(text.contains("nested: i32"), "{text}");
            assert_eq!(text.matches("#[native_const]").count(), 3, "{text}");
            let path = std::env::temp_dir().join(format!(
                "source-home-retention-{}-{target}.winmd",
                std::process::id()
            ));
            windows_rdl::reader()
                .input_texts(report.partitions.values())
                .reference_default()
                .output(&path)
                .write()
                .unwrap();
            let index = windows_metadata::reader::Index::read(&path).unwrap();
            let record = index.expect("Test.Home", "SourceRecord");
            assert!(
                record
                    .fields()
                    .filter(|field| { matches!(field.name(), "first" | "second") })
                    .all(|field| field.find_attribute("ConstAttribute").is_some())
            );
            for name in ["SourceCallback", "SourceInterface"] {
                let ty = index.expect("Test.Home", name);
                assert!(
                    ty.methods()
                        .flat_map(|method| method.params())
                        .any(|parameter| {
                            parameter.name() == "value"
                                && parameter
                                    .flags()
                                    .contains(windows_metadata::ParamAttributes::Optional)
                        })
                );
            }
            std::fs::remove_file(path).unwrap();
        }
    }
}

#[test]
fn ranked_source_associated_enum_remaps_follow_the_authoritative_provider() {
    helpers::ensure_libclang();
    let source = "#define FIELD_EXTRA \
        __attribute__((annotate(\"win32metadata:associated_enum=SourceEnum\")))";
    for target in TARGETS {
        for reversed in [false, true] {
            let plan = shared_plan_with_remaps(
                target,
                source,
                source,
                reversed,
                Some(("ChosenEnum", "AlternativeEnum")),
            )
            .unwrap();
            let report = report(plan).unwrap();
            let text = report.partitions.values().cloned().collect::<String>();
            assert!(text.contains("enum ChosenEnum"), "{text}");
            assert!(text.contains("associated_enum(\"ChosenEnum\")"), "{text}");
            assert!(!text.contains("AlternativeEnum"), "{text}");
            let path = std::env::temp_dir().join(format!(
                "source-associated-{}-{target}.winmd",
                std::process::id()
            ));
            let error = windows_rdl::reader()
                .input_texts(report.partitions.values())
                .reference_default()
                .output(&path)
                .write()
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("pseudo-attribute type not found"),
                "{error}"
            );
            assert!(!path.try_exists().unwrap());
        }
    }
}

#[test]
fn ranked_source_annotations_preserve_same_context_inheritance_and_errors() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for (source, diagnostic) in [
            (
                "struct __attribute__((annotate(\"win32metadata:supported_os=Windows10\"))) \
                 OwnRecord; struct OwnRecord { int first; };",
                None,
            ),
            (
                "struct __attribute__((annotate(\"win32metadata:struct_size_field=first\"))) \
                 __attribute__((annotate(\"win32metadata:struct_size_field=second\"))) \
                 OwnRecord { int first; int second; };",
                Some("conflicting redeclaration annotation"),
            ),
            (
                "struct __attribute__((annotate(\"win32metadata:struct_size_field=first\"))) \
                 OwnRecord; struct \
                 __attribute__((annotate(\"win32metadata:struct_size_field=second\"))) \
                 OwnRecord { int first; int second; };",
                Some("conflicting redeclaration annotation"),
            ),
            (
                "struct __attribute__((annotate(\"win32metadata:unknown_key\"))) \
                 OwnRecord { int first; };",
                Some("unknown win32metadata annotation"),
            ),
            (
                "struct __attribute__((annotate(\"win32metadata:const\"))) \
                 OwnRecord { int first; };",
                Some("not valid on this declaration"),
            ),
        ] {
            let policy = HeaderPartitionPolicy::new()
                .with_partition_authority("Home", 0, "own.cpp")
                .with_traversed_header("own.cpp", RootPartition::new("Home", "Test.Home"));
            let result = extract_ranked_header_partitions(
                [Input::new("own.cpp", source).partitioned("Home")],
                &[
                    "-x",
                    "c++",
                    "-std=c++17",
                    &format!("--target={target}"),
                    "--no-default-config",
                ],
                &policy,
                &NamespaceAuthorities::new(),
            );
            if let Some(diagnostic) = diagnostic {
                let error = result.unwrap_err();
                assert!(error.to_string().contains(diagnostic), "{error}");
            } else {
                let output = report(result.unwrap()).unwrap();
                assert!(
                    output
                        .partitions
                        .values()
                        .any(|text| text.contains("#[supported_os(\"Windows10\")]")),
                    "{output:?}"
                );
            }
        }
    }
}
