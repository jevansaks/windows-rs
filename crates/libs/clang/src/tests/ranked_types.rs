use super::*;
use windows_metadata::HasAttributes;

mod bodies;

const TARGETS: [&str; 3] = [
    "i686-pc-windows-msvc",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
];

fn ranked_plan(target: &str, case: &str, reversed: bool) -> HeaderPartitionPlan {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections");
    let header = directory.join("header-authority-types.hpp");
    let header = header.to_str().unwrap();
    let other = directory.join("header-authority-other.hpp");
    let other = other.to_str().unwrap();
    let evidence_prefix = if case.starts_with("evidence-") {
        format!(
            "#define AUTHORITY_EVIDENCE\n{}{}",
            match case {
                "evidence-scalar" => "#define AUTHORITY_EVIDENCE_SCALAR\n",
                "evidence-forward" => "#define AUTHORITY_EVIDENCE_FORWARD\n",
                "evidence-nominal" => "#define AUTHORITY_EVIDENCE_NOMINAL\n",
                "evidence-inline" => "#define AUTHORITY_EVIDENCE_INLINE\n",
                "evidence-direct-callback" => {
                    "#define AUTHORITY_EVIDENCE_INLINE\n#define AUTHORITY_EVIDENCE_DIRECT_CALLBACK\n"
                }
                "evidence-direct-callback-different" => {
                    "#define AUTHORITY_EVIDENCE_INLINE\n#define AUTHORITY_EVIDENCE_DIRECT_CALLBACK\n\
                     #define AUTHORITY_EVIDENCE_CALLBACK_DIFFERENT\n"
                }
                "evidence-unknown" => "#define AUTHORITY_EVIDENCE_ALIAS_ALTERNATIVE\n",
                _ => "",
            },
            match case {
                "evidence-reference" => "#define AUTHORITY_EVIDENCE_REFERENCES\n",
                "evidence-reference-negative" => {
                    "#define AUTHORITY_EVIDENCE_REFERENCES\n#define AUTHORITY_EVIDENCE_NEGATIVE\n"
                }
                "evidence-pointer-negative" => "#define AUTHORITY_EVIDENCE_NEGATIVE\n",
                _ => "",
            }
        )
    } else {
        String::new()
    };
    let home_prefix = match case {
        "absent-definition" => "#define AUTHORITY_ABSENT\n",
        "forward-definition" => "#define AUTHORITY_FORWARD\n",
        "unrepresentable" => "#define AUTHORITY_UNREPRESENTABLE\n",
        "opaque-pointer" | "opaque-reference" | "opaque-value" | "opaque-array" => {
            "#define AUTHORITY_OPAQUE\n"
        }
        "scope" => "#define AUTHORITY_SCOPE\n",
        "scope-distinct" => "#define AUTHORITY_SCOPE\n#define AUTHORITY_SCOPE_OTHER\n",
        _ => "",
    };
    let offsets = if case == "offsets" {
        "#define AUTHORITY_OFFSETS\n"
    } else {
        ""
    };
    let alternative = if case == "interface" {
        "#define AUTHORITY_EXTRA\n"
    } else {
        "#define AUTHORITY_RECORD\n#define AUTHORITY_EXTRA\n"
    };
    let use_source = match case {
        "by-value" => "extern \"C\" void UseAuthority(AuthorityInterface value);\n",
        "reference" => "extern \"C\" void UseAuthority(AuthorityInterface& value);\n",
        "counted" => {
            "extern \"C\" void UseAuthority(\
             __attribute__((annotate(\"_In_reads_(2)\"))) AuthorityInterface* value);\n"
        }
        "opaque-pointer" => "extern \"C\" void UseAuthority(const AuthorityOpaque* value);\n",
        "opaque-reference" => "extern \"C\" void UseAuthority(const AuthorityOpaque& value);\n",
        "opaque-value" => "extern \"C\" void UseAuthority(AuthorityOpaque value);\n",
        "opaque-array" => {
            "struct AuthorityOpaqueArray { AuthorityOpaque values[2]; };\n\
             extern \"C\" void UseAuthority(AuthorityOpaqueArray* value);\n"
        }
        "scope" | "scope-distinct" => {
            "extern \"C\" void UseAuthority(ScopeOuter::ScopeInner* value);\n"
        }
        _ => "extern \"C\" void UseAuthority(AuthorityInterface* value);\n",
    };
    let home = Input::new(
        "ranked-type-home.cpp",
        format!("{evidence_prefix}{home_prefix}{offsets}#include {header:?}\n"),
    )
    .with_roots([header])
    .partitioned("Home");
    let client = Input::new(
        "ranked-type-client.cpp",
        format!(
            "{evidence_prefix}{}{offsets}{alternative}#define AUTHORITY_ALTERNATIVE\n\
             #define AUTHORITY_VALUE {}\n#include {header:?}\n\
             {use_source}{}",
            if case.starts_with("opaque-") {
                "#define AUTHORITY_OPAQUE\n"
            } else if case.starts_with("scope") {
                "#define AUTHORITY_SCOPE\n"
            } else {
                ""
            },
            if case.starts_with("evidence-") {
                "long"
            } else {
                "unsigned long long"
            },
            if case.starts_with("evidence-") {
                "extern \"C\" void UseEvidence(EvidenceRecord* value);\n"
            } else if case == "offsets" {
                "extern \"C\" void UseOffset(AuthorityOffset* value);\n"
            } else {
                ""
            }
        ),
    )
    .with_roots([header, "ranked-type-client.cpp"])
    .partitioned("Api");
    let other_input = Input::new("ranked-type-other.cpp", format!("#include {other:?}\n"))
        .with_roots([other])
        .partitioned("Other");
    let mut policy = HeaderPartitionPolicy::new()
        .with_partition_authority("Home", 0, "ranked-type-home.cpp")
        .with_partition_authority("Api", 1, "ranked-type-client.cpp")
        .with_partition_authority("Other", 2, "ranked-type-other.cpp");
    let home_owner = if matches!(case, "excluded" | "excluded-absent-home") {
        RootPartition::new("Home", "Test.Home").with_exclusion("AuthorityInterface")
    } else if case == "excluded-alias" {
        RootPartition::new("Home", "Test.Home").with_exclusion("AuthorityAlias")
    } else {
        RootPartition::new("Home", "Test.Home")
    };
    policy.add_traversed_header(header, home_owner);
    policy.add_traversed_header(header, RootPartition::new("Api", "Test.Alternative"));
    policy.add_traversed_header(
        "ranked-type-client.cpp",
        RootPartition::new("Api", "Test.Api"),
    );
    policy.add_traversed_header(other, RootPartition::new("Other", "Test.Other"));
    let mut inputs = vec![home, client, other_input];
    if matches!(case, "absent-home" | "excluded-absent-home") {
        inputs.remove(0);
    }
    if reversed {
        inputs.reverse();
    }
    let target = format!("--target={target}");
    let mut args = vec!["-x", "c++", "-std=c++17", &target];
    if case != "driver-default-config" {
        args.push("--no-default-config");
    }
    extract_partitioned(inputs, &args)
        .unwrap()
        .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
        .unwrap()
}

fn output(plan: HeaderPartitionPlan) -> Result<BTreeMap<RdlPartition, String>, Error> {
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("authority.dll");
    plan.emit_with_options(&options)
}

fn fact<'a>(plan: &'a HeaderPartitionPlan, tu: &str, name: &str) -> &'a Fact {
    plan.snapshot
        .facts
        .iter()
        .find(|fact| fact.origin.tu == tu && fact.name == name && fact.definition)
        .unwrap()
}

fn assert_selected_signature(plan: &HeaderPartitionPlan, case: &str) {
    let home = fact(plan, "ranked-type-home.cpp", "AuthorityInterface");
    let alternative = fact(plan, "ranked-type-client.cpp", "AuthorityInterface");
    assert_eq!(home.spelling, alternative.spelling);
    assert_eq!(home.expansion, alternative.expansion);
    assert_ne!(home.data, alternative.data);
    assert!(home.root);
    assert!(!alternative.root);
    assert_eq!(
        plan.snapshot.root_owners[&home.origin].namespace,
        "Test.Home"
    );
    assert!(!plan.snapshot.root_owners.contains_key(&alternative.origin));
    let FactData::Interface { base, methods, .. } = &home.data else {
        panic!("{case}: selected home is not an interface")
    };
    let Some(TypeRef::Named { name, declaration }) = base else {
        panic!("{case}: selected base is absent")
    };
    let base = fact(plan, "ranked-type-home.cpp", name);
    assert_eq!(*declaration, base.spelling);
    let FactData::Interface {
        methods: base_methods,
        ..
    } = &base.data
    else {
        panic!()
    };
    assert_eq!(base_methods[0].name, "QueryInterface");
    assert!(matches!(
        base_methods[0].params[0].ty,
        TypeRef::Reference { mutable: false, .. }
    ));
    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0].name, "Value");
    assert_eq!(methods[0].result, TypeRef::Scalar(Scalar::I32));
    assert_eq!(methods[0].params[0].ty, TypeRef::Scalar(Scalar::I32));
    assert!(matches!(
        methods[0].params[1].ty,
        TypeRef::Reference { mutable: false, .. }
    ));
    let payload = fact(plan, "ranked-type-home.cpp", "AuthorityPayload");
    let FactData::Record { fields, .. } = &payload.data else {
        panic!()
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].ty, TypeRef::Scalar(Scalar::I32));
}

fn assert_metadata(
    partitions: &BTreeMap<RdlPartition, String>,
    target: &str,
    case: &str,
    reversed: bool,
) {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-ranked-types-{}-{target}-{case}-{reversed}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_texts(partitions.values())
        .reference_default()
        .output(&path)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&path).unwrap();
    let windows_metadata::reader::Item::Type(interface) =
        index.expect_item("Test.Home", "AuthorityInterface")
    else {
        panic!("{partitions:?}")
    };
    let methods: Vec<_> = interface.methods().collect();
    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0].name(), "Value");
    let signature = methods[0].signature(&[]);
    assert_eq!(signature.return_type, windows_metadata::Type::I32);
    assert_eq!(signature.types[0], windows_metadata::Type::I32);
    assert!(!index.contains("Test.Alternative", "AuthorityInterface"));
    let windows_metadata::reader::Item::Type(other) =
        index.expect_item("Test.Other", "AuthorityOther")
    else {
        panic!()
    };
    assert_eq!(
        interface.find_attribute("GuidAttribute").unwrap().value(),
        other.find_attribute("GuidAttribute").unwrap().value()
    );
    assert_eq!(other.methods().next().unwrap().name(), "Other");
    let windows_metadata::reader::Item::Fn(function) =
        index.expect_item("Test.Api", "UseAuthority")
    else {
        panic!()
    };
    let interface = windows_metadata::Type::class_named("Test.Home", "AuthorityInterface");
    let expected = if case == "interface" {
        windows_metadata::Type::PtrMut(Box::new(interface), 1)
    } else {
        interface
    };
    assert_eq!(function.signature(&[]).types, [expected]);
    assert!(index.contains("Test.Home", "AuthorityAlias"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn ranked_selected_interface_signature_ignores_alternative_definition() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["interface", "record"] {
            let mut baseline = None;
            for reversed in [false, true] {
                let plan = ranked_plan(target, case, reversed);
                assert_selected_signature(&plan, case);
                let selected = output(plan).unwrap();
                assert_metadata(&selected, target, case, reversed);
                if let Some(baseline) = &baseline {
                    assert_eq!(&selected, baseline);
                } else {
                    baseline = Some(selected);
                }
            }
        }
    }
}

#[test]
fn ranked_selected_definition_can_have_different_source_offsets() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let plan = ranked_plan(target, "offsets", false);
        let home = fact(&plan, "ranked-type-home.cpp", "AuthorityOffset");
        let alternative = fact(&plan, "ranked-type-client.cpp", "AuthorityOffset");
        assert_eq!(home.spelling.file, alternative.spelling.file);
        assert_ne!(home.spelling.offset, alternative.spelling.offset);
        assert_ne!(home.expansion.offset, alternative.expansion.offset);
        assert_ne!(home.data, alternative.data);
        let partitions = output(plan).unwrap();
        let selected = partitions.values().cloned().collect::<String>();
        assert!(selected.contains("fn Home("), "{selected}");
        assert!(!selected.contains("fn Alternative("), "{selected}");
        assert!(selected.contains("fn UseOffset("), "{selected}");
    }
}

#[test]
fn ranked_absent_or_forward_home_definition_never_borrows_complete_alternative() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["absent-definition", "forward-definition"] {
            let plan = ranked_plan(target, case, false);
            assert!(matches!(
                fact(&plan, "ranked-type-client.cpp", "AuthorityInterface").data,
                FactData::Record { .. }
            ));
            let error = output(plan).unwrap_err().to_string();
            assert!(
                error.contains("RequiredAuthorityDefinitionAbsent"),
                "{target} {case}: {error}"
            );
            assert!(error.contains("ranked-type-home.cpp"), "{error}");
            assert!(error.contains("header-authority-types.hpp"), "{error}");
        }
    }
}

#[test]
fn ranked_unrepresentable_selected_signature_never_borrows_alternative() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let error = output(ranked_plan(target, "unrepresentable", false))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unsupported authoritative type")
                || error.contains("unproved callable evidence"),
            "{target}: {error}"
        );
        assert!(error.contains("AuthorityInterface"), "{error}");
    }
}

#[test]
fn ranked_excluded_selected_dependencies_never_fall_through() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["excluded", "excluded-alias", "excluded-absent-home"] {
            let error = output(ranked_plan(target, case, false))
                .unwrap_err()
                .to_string();
            assert!(error.contains("excluded"), "{target} {case}: {error}");
            assert!(error.contains("Home"), "{error}");
            assert!(!error.contains("RequiredAuthorityContextAbsent"), "{error}");
        }
    }
}

#[test]
fn ranked_required_original_home_context_remains_explicit() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let error = output(ranked_plan(target, "absent-home", false))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("RequiredAuthorityContextAbsent"),
            "{target}: {error}"
        );
        assert!(error.contains("ranked-type-home.cpp"), "{error}");
        assert!(error.contains("header-authority-types.hpp"), "{error}");
    }
}

#[test]
fn ranked_com_bridge_preserves_object_layout_and_counted_pointer_failures() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["by-value", "reference", "counted"] {
            let error = output(ranked_plan(target, case, false))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("requires object layout"),
                "{target} {case}: {error}"
            );
            assert!(error.contains("only COM pointer uses"), "{error}");
        }
    }
}

#[test]
fn ranked_selected_native_driver_guard_is_not_bypassed() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let error = output(ranked_plan(target, "driver-default-config", false))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("unproved callable evidence"),
            "{target}: {error}"
        );
        assert!(error.contains("result_present=false"), "{error}");
        assert!(error.contains("native_params=0/"), "{error}");
    }
}

#[test]
fn ranked_opaque_declarations_preserve_indirect_uses_without_alternative_body() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["opaque-pointer", "opaque-reference"] {
            let mut baseline = None;
            for reversed in [false, true] {
                let plan = ranked_plan(target, case, reversed);
                let opaque = plan
                    .snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        fact.origin.tu == "ranked-type-home.cpp"
                            && fact.name == "AuthorityOpaque"
                            && matches!(fact.data, FactData::Record { .. })
                    })
                    .unwrap();
                assert!(!opaque.definition);
                assert!(opaque.root);
                assert!(
                    matches!(&opaque.data, FactData::Record { fields, .. } if fields.is_empty())
                );
                let alternative = plan
                    .snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        fact.origin.tu == "ranked-type-client.cpp"
                            && fact.name == "AuthorityOpaque"
                            && fact.definition
                            && matches!(fact.data, FactData::Record { .. })
                    })
                    .unwrap();
                assert!(
                    matches!(&alternative.data, FactData::Record { fields, .. } if !fields.is_empty())
                );
                let selected = output(plan).unwrap();
                let home = selected
                    .iter()
                    .find(|(partition, _)| partition.namespace == "Test.Home")
                    .unwrap()
                    .1;
                assert!(home.contains("struct AuthorityOpaque {\n"), "{home}");
                assert!(!home.contains("alternative_field"), "{home}");
                let api = selected
                    .iter()
                    .find(|(partition, _)| partition.namespace == "Test.Api")
                    .unwrap()
                    .1;
                assert!(
                    api.contains("fn UseAuthority(value: *const Test::Home::AuthorityOpaque)"),
                    "{target} {case}: {api}"
                );
                if let Some(baseline) = &baseline {
                    assert_eq!(&selected, baseline);
                } else {
                    baseline = Some(selected);
                }
            }
        }
    }
}

#[test]
fn ranked_opaque_value_and_array_demands_never_borrow_alternative_layout() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["opaque-value", "opaque-array"] {
            let error = output(ranked_plan(target, case, false))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("incomplete record `AuthorityOpaque`"),
                "{target} {case}: {error}"
            );
            assert!(error.contains("object layout"), "{error}");
            assert!(error.contains("ranked-type-home.cpp"), "{error}");
            assert!(
                error.contains("no alternative context is eligible"),
                "{error}"
            );
            assert!(!error.contains("RequiredAuthorityContextAbsent"), "{error}");
        }
    }
}

#[test]
fn ranked_nested_and_out_of_line_declarations_share_native_semantic_scope() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let mut baseline = None;
        for reversed in [false, true] {
            let plan = ranked_plan(target, "scope", reversed);
            let home = fact(&plan, "ranked-type-home.cpp", "ScopeInner");
            let client = fact(&plan, "ranked-type-client.cpp", "ScopeInner");
            let home_outer = fact(&plan, "ranked-type-home.cpp", "ScopeOuter");
            let client_outer = fact(&plan, "ranked-type-client.cpp", "ScopeOuter");
            assert_eq!(home.parent.as_ref(), Some(&home_outer.origin));
            assert_eq!(client.parent, None);
            assert_eq!(
                plan.snapshot.semantic_parents[&home.origin].as_ref(),
                Some(&home_outer.origin)
            );
            assert_eq!(
                plan.snapshot.semantic_parents[&client.origin].as_ref(),
                Some(&client_outer.origin)
            );
            assert_eq!(home.spelling.file, client.spelling.file);
            assert_ne!(home.spelling.offset, client.spelling.offset);
            assert!(
                matches!(&home.data, FactData::Record { fields, .. } if fields.len() == 1
                        && fields[0].name == "value" && fields[0].ty == TypeRef::Scalar(Scalar::I32))
            );
            let selected = output(plan).unwrap();
            let text = selected.values().cloned().collect::<String>();
            assert!(text.contains("struct ScopeInner {\n"), "{text}");
            assert!(text.contains("value: i32"), "{text}");
            assert!(
                text.contains("fn UseAuthority(value: *mut Test::Home::ScopeInner)"),
                "{text}"
            );
            if let Some(baseline) = &baseline {
                assert_eq!(&selected, baseline);
            } else {
                baseline = Some(selected);
            }
        }
    }
}

#[test]
fn ranked_distinct_native_scopes_never_match_by_leaf_name() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for reversed in [false, true] {
            let plan = ranked_plan(target, "scope-distinct", reversed);
            let home = fact(&plan, "ranked-type-home.cpp", "ScopeInner");
            let home_outer = fact(&plan, "ranked-type-home.cpp", "ScopeOther");
            assert_eq!(
                plan.snapshot.semantic_parents[&home.origin].as_ref(),
                Some(&home_outer.origin)
            );
            assert!(fact(&plan, "ranked-type-client.cpp", "ScopeInner").definition);
            let error = output(plan).unwrap_err().to_string();
            assert!(
                error.contains("RequiredAuthorityDefinitionAbsent"),
                "{target} {reversed}: {error}"
            );
            assert!(error.contains("ScopeInner"), "{error}");
            assert!(error.contains("ranked-type-home.cpp"), "{error}");
            assert!(!error.contains("RequiredAuthorityContextAbsent"), "{error}");
        }
    }
}

#[test]
fn ranked_single_home_out_of_line_definition_precedes_opaque_forward() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let input = Input::new(
            "ranked-single-home.hpp",
            "struct Outer { struct Inner; };\n\
             struct Outer::Inner { int value; };\n\
             extern \"C\" void Use(Outer::Inner* value);\n",
        )
        .with_roots(["ranked-single-home.hpp"])
        .partitioned("Home");
        let target_arg = format!("--target={target}");
        let snapshot = extract_partitioned(
            [input],
            &[
                "-x",
                "c++",
                "-std=c++17",
                &target_arg,
                "--no-default-config",
            ],
        )
        .unwrap();
        let policy = HeaderPartitionPolicy::new()
            .with_partition_authority("Home", 0, "ranked-single-home.hpp")
            .with_traversed_header(
                "ranked-single-home.hpp",
                RootPartition::new("Home", "Test.Home"),
            );
        let plan = snapshot
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap();
        let declarations: Vec<_> = plan
            .snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "Inner")
            .collect();
        assert_eq!(declarations.len(), 2);
        let forward = declarations.iter().find(|fact| !fact.definition).unwrap();
        let complete = declarations.iter().find(|fact| fact.definition).unwrap();
        let outer = fact(&plan, "ranked-single-home.hpp", "Outer");
        assert_eq!(forward.parent.as_ref(), Some(&outer.origin));
        assert_eq!(complete.parent, None);
        for declaration in declarations {
            assert_eq!(
                plan.snapshot.semantic_parents[&declaration.origin].as_ref(),
                Some(&outer.origin)
            );
            assert_eq!(declaration.origin.tu, "ranked-single-home.hpp");
        }
        let selected = output(plan).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected.keys().next().unwrap().namespace, "Test.Home");
        let text = selected.values().next().unwrap();
        assert!(text.contains("struct Inner {\n"), "{target}: {text}");
        assert!(text.contains("value: i32"), "{target}: {text}");
        assert!(
            text.contains("fn Use(value: *mut Inner)"),
            "{target}: {text}"
        );
    }
}

#[test]
fn ranked_native_semantic_parent_evidence_is_required_in_its_original_tu() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for foreign in [false, true] {
            let mut plan = ranked_plan(target, "scope", false);
            let clients: Vec<_> = plan
                .snapshot
                .facts
                .iter()
                .filter(|fact| {
                    fact.origin.tu == "ranked-type-client.cpp"
                        && fact.name == "ScopeInner"
                        && fact.kind == FactKind::Struct
                })
                .map(|fact| fact.origin.clone())
                .collect();
            assert!(!clients.is_empty());
            if foreign {
                let home_outer = fact(&plan, "ranked-type-home.cpp", "ScopeOuter")
                    .origin
                    .clone();
                for client in clients {
                    plan.snapshot
                        .semantic_parents
                        .insert(client, Some(home_outer.clone()));
                }
            } else {
                for client in clients {
                    plan.snapshot.semantic_parents.remove(&client);
                }
            }
            let error = output(plan).unwrap_err().to_string();
            assert!(
                error.contains(if foreign {
                    "cross-TU native semantic scope"
                } else {
                    "no captured native semantic-parent relation"
                }),
                "{target} {foreign}: {error}"
            );
            assert!(
                !error.contains("RequiredAuthorityDefinitionAbsent"),
                "{error}"
            );
        }
    }
}

fn report(plan: HeaderPartitionPlan) -> Result<HeaderPartitionReport, Error> {
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("authority.dll");
    plan.emit_with_enum_diagnostics(&options)
}

fn evidence<'a>(report: &'a HeaderPartitionReport, name: &str) -> &'a DefinitionEvidence {
    report
        .definition_evidence
        .iter()
        .find(|row| row.identity.name == name)
        .unwrap()
}

fn ranked_record_rows(report: &HeaderPartitionReport) -> Vec<&DefinitionEvidence> {
    let rows: Vec<_> = report
        .definition_evidence
        .iter()
        .filter(|row| row.body_kind == DefinitionBodyKind::Interface)
        .collect();
    assert_eq!(rows.len(), 1, "{report:?}");
    let row = rows[0];
    assert_eq!(row.identity.name, "AuthorityInterface");
    let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
        panic!("{row:?}")
    };
    assert!(
        keys.iter()
            .any(|key| key.path == "slots.count" && key.selected == "2" && key.alternative == "3"),
        "{row:?}"
    );
    assert!(unknown.iter().any(|key| key.path == "declared"), "{row:?}");
    let DefinitionEvidenceCoverage::CapturedBodiesAndReferentConst {
        interfaces, enums, ..
    } = &report.definition_evidence_coverage
    else {
        panic!("{report:?}")
    };
    assert_eq!(
        *interfaces,
        DefinitionDomainCoverage {
            selected_definitions: 3,
            alternative_definitions: 2,
            declaration_only_observations: 0,
        }
    );
    assert_eq!(*enums, DefinitionDomainCoverage::default());
    assert!(
        !report
            .definition_evidence
            .iter()
            .any(|row| row.body_kind == DefinitionBodyKind::Enum)
    );
    report
        .definition_evidence
        .iter()
        .filter(|row| row.body_kind == DefinitionBodyKind::Record)
        .collect()
}

fn without_evidence_comments(text: &str) -> String {
    text.lines()
        .filter(|line| {
            !["record", "interface", "enum"].iter().any(|kind| {
                line.trim_start()
                    .starts_with(&format!("// windows-clang: {kind} definition "))
            })
        })
        .map(|line| format!("{line}\n"))
        .collect()
}

fn output_without_notes(plan: HeaderPartitionPlan) -> BTreeMap<RdlPartition, String> {
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("authority.dll");
    let (mut snapshot, display_names, source_names) =
        plan.snapshot.into_partitioned_planning_snapshot(&options);
    snapshot.project_suppressed_declare_handles();
    let mut plan = snapshot
        .plan_partitioned(&options, &display_names, &source_names)
        .unwrap();
    let candidates = snapshot
        .partition_route_candidates(&plan, &source_names, &options)
        .unwrap();
    let routes = snapshot.resolve_partition_routes(candidates).unwrap();
    plan.definition_notes.clear();
    snapshot
        .format_partitioned_plan(plan, &options, &routes, &display_names, None)
        .unwrap()
}

fn assert_record_metadata(
    report: &HeaderPartitionReport,
    target: &str,
    case: &str,
    reversed: bool,
) {
    let path = std::env::temp_dir().join(format!(
        "record-evidence-{}-{target}-{case}-{reversed}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_texts(report.partitions.values())
        .reference_default()
        .output(&path)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&path).unwrap();
    assert!(index.contains("Test.Home", "EvidenceRecord"));
    std::fs::remove_file(path).unwrap();
}

fn assert_selected_alias_record(plan: &HeaderPartitionPlan, target: &str, references: bool) {
    let selected = fact(plan, "ranked-type-home.cpp", "EvidenceRecord");
    let FactData::Record {
        base,
        fields,
        size,
        align,
        packing,
        alignment,
        union,
    } = &selected.data
    else {
        panic!("{selected:?}")
    };
    let pointer_size = if target.starts_with("i686") { 4 } else { 8 };
    let count = if references { 5 } else { 3 };
    assert!(base.is_none());
    assert_eq!(fields.len(), count);
    assert_eq!(*size, pointer_size * count as i64);
    assert_eq!(*align, pointer_size);
    assert_eq!(*packing, None);
    assert_eq!(*alignment, None);
    assert!(!union);
    for (index, field) in fields.iter().enumerate() {
        assert_eq!(field.offset, index as i64 * pointer_size * 8);
        assert_eq!(field.size, pointer_size);
        assert_eq!(field.align, pointer_size);
        assert_eq!(field.bit_width, None);
        let target = match &field.ty {
            TypeRef::Pointer { target, .. } if index < 3 => target,
            TypeRef::Reference { target, .. } if index >= 3 => target,
            _ => panic!("{field:?}"),
        };
        let TypeRef::Named { name, .. } = target.as_ref() else {
            panic!("{field:?}")
        };
        assert_eq!(
            name,
            [
                "EvidenceA",
                "EvidenceCA",
                "EvidenceCP",
                "EvidenceCA",
                "EvidenceCP"
            ][index]
        );
    }
}

#[test]
fn ranked_record_difference_reports_causal_key_without_changing_selected_body() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let mut baseline = None;
        for reversed in [false, true] {
            let plan = ranked_plan(target, "evidence-scalar", reversed);
            assert_eq!(
                fact(&plan, "ranked-type-home.cpp", "EvidenceRecord").data,
                FactData::Record {
                    base: None,
                    fields: vec![Field {
                        name: "value".to_string(),
                        ty: TypeRef::Scalar(Scalar::I32),
                        offset: 0,
                        align: 4,
                        size: 4,
                        bit_width: None,
                    }],
                    size: 4,
                    align: 4,
                    packing: None,
                    alignment: None,
                    union: false,
                }
            );
            let references = BTreeMap::new();
            let mut options = EmitOptions::new("Test", &references);
            options.library = Some("authority.dll");
            assert!(!plan.audit(&options).unwrap().has_blocking_conflicts());
            let legacy = output_without_notes(plan.clone());
            let report = report(plan).unwrap();
            assert!(report.omissions.is_empty());
            assert_eq!(ranked_record_rows(&report).len(), 1);
            let row = evidence(&report, "EvidenceRecord");
            assert_eq!(row.selected.rank, Some(0));
            assert_eq!(row.selected.partition.as_deref(), Some("Home"));
            assert_eq!(row.selected.input, "ranked-type-home.cpp");
            assert_eq!(row.alternative.rank, Some(1));
            assert_eq!(row.alternative.partition.as_deref(), Some("Api"));
            assert_eq!(row.alternative.input, "ranked-type-client.cpp");
            assert_eq!(row.identity.role, DefinitionTypeRole::Record);
            let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
                panic!("{row:?}")
            };
            assert!(unknown.is_empty(), "{unknown:?}");
            assert!(
                keys.iter().any(|key| key.path == "field[0:value].type"
                    && key.selected == "scalar:I32"
                    && key.alternative == "scalar:U64"),
                "{keys:?}"
            );
            let text = report.partitions.values().cloned().collect::<String>();
            assert_eq!(
                text.matches(&format!("// windows-clang: {}\n", row.message))
                    .count(),
                1
            );
            assert!(text.contains("value: i32"), "{text}");
            for (partition, text) in &report.partitions {
                assert_eq!(without_evidence_comments(text), legacy[partition]);
            }
            assert_record_metadata(&report, target, "scalar", reversed);
            if let Some(baseline) = &baseline {
                assert_eq!(&report, baseline);
            } else {
                baseline = Some(report);
            }
        }
    }
}

#[test]
fn ranked_record_alias_const_is_normalized_only_at_its_own_indirection_level() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["evidence-pointer", "evidence-reference"] {
            let mut baseline = None;
            for reversed in [false, true] {
                let plan = ranked_plan(target, case, reversed);
                assert_selected_alias_record(&plan, target, case == "evidence-reference");
                let report = report(plan).unwrap();
                assert!(
                    ranked_record_rows(&report).is_empty(),
                    "{target} {case}: {report:?}"
                );
                let text = report.partitions.values().cloned().collect::<String>();
                assert!(
                    !text.contains("// windows-clang: record definition "),
                    "{text}"
                );
                assert!(text.contains("struct EvidenceRecord"), "{text}");
                assert_record_metadata(&report, target, case, reversed);
                if case == "evidence-reference" {
                    assert!(text.contains("qualified_ref:"), "{text}");
                    assert!(text.contains("pointer_const_ref:"), "{text}");
                }
                if let Some(baseline) = &baseline {
                    assert_eq!(&report, baseline);
                } else {
                    baseline = Some(report);
                }
            }
        }
        for case in ["evidence-pointer-negative", "evidence-reference-negative"] {
            let report = report(ranked_plan(target, case, false)).unwrap();
            let row = evidence(&report, "EvidenceRecord");
            let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
                panic!("{row:?}")
            };
            assert!(unknown.is_empty(), "{unknown:?}");
            assert!(
                keys.iter()
                    .any(|key| key.path == "field[2:pointer_const].type.mutable"
                        && key.selected == "false"
                        && key.alternative == "true"),
                "{keys:?}"
            );
            assert!(
                keys.iter().any(
                    |key| key.path == "field[2:pointer_const].type.target.mutable"
                        && key.selected == "true"
                        && key.alternative == "false"
                ),
                "{keys:?}"
            );
            if case == "evidence-reference-negative" {
                assert!(
                    keys.iter()
                        .any(|key| key.path == "field[4:pointer_const_ref].type.mutable"
                            && key.selected == "false"
                            && key.alternative == "true"),
                    "{keys:?}"
                );
                assert!(
                    keys.iter().any(|key| key.path
                        == "field[4:pointer_const_ref].type.target.mutable"
                        && key.selected == "true"
                        && key.alternative == "false"),
                    "{keys:?}"
                );
            }
        }
    }
}

#[test]
fn ranked_record_missing_alternative_alias_evidence_is_unknown_not_borrowed() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for remove_fact in [false, true] {
            let mut plan = ranked_plan(target, "evidence-unknown", false);
            let alias = fact(&plan, "ranked-type-client.cpp", "EvidenceCA")
                .origin
                .clone();
            if remove_fact {
                plan.snapshot.facts.retain(|fact| fact.origin != alias);
            } else {
                *plan.snapshot.native_typedef_types.get_mut(&alias).unwrap() = None;
            }
            let report = report(plan).unwrap();
            assert_eq!(ranked_record_rows(&report).len(), 1, "{report:?}");
            let row = evidence(&report, "EvidenceRecord");
            let DefinitionComparison::Unknown(keys) = &row.outcome else {
                panic!("{row:?}")
            };
            assert!(
                keys.iter()
                    .any(|key| key.path == "field[1:qualified].type.target"
                        && key.reason.contains(if remove_fact {
                            "own-TU declaration"
                        } else {
                            "own-TU canonical alias const evidence"
                        })),
                "{keys:?}"
            );
            let text = report.partitions.values().cloned().collect::<String>();
            assert!(
                !text.contains("// windows-clang: record definition "),
                "{text}"
            );
            assert!(text.contains("struct EvidenceRecord"), "{text}");
        }
    }
}

#[test]
fn ranked_record_forward_only_alternative_is_accounted_without_body_evidence() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let mut baseline = None;
        for reversed in [false, true] {
            let report = report(ranked_plan(target, "evidence-forward", reversed)).unwrap();
            assert!(ranked_record_rows(&report).is_empty(), "{report:?}");
            let DefinitionEvidenceCoverage::CapturedBodiesAndReferentConst {
                selected_records,
                declaration_only_observations,
                ..
            } = report.definition_evidence_coverage
            else {
                panic!("{report:?}")
            };
            assert!(selected_records >= 1);
            assert_eq!(declaration_only_observations, 1);
            let text = report.partitions.values().cloned().collect::<String>();
            assert!(text.contains("struct EvidenceRecord"), "{text}");
            assert!(
                !text.contains("// windows-clang: record definition "),
                "{text}"
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
fn ranked_record_nominal_referents_report_their_own_body_only() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let report = report(ranked_plan(target, "evidence-nominal", false)).unwrap();
        assert_eq!(ranked_record_rows(&report).len(), 1, "{report:?}");
        let row = evidence(&report, "EvidenceNominal");
        let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
            panic!("{row:?}")
        };
        assert!(unknown.is_empty(), "{unknown:?}");
        assert!(keys.iter().any(|key| key.path == "field[0:payload].type"));
    }
}

#[test]
fn ranked_record_inline_and_function_pointer_shapes_are_observed_in_context() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let report = report(ranked_plan(target, "evidence-inline", false)).unwrap();
        let row = evidence(&report, "EvidenceRecord");
        let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
            panic!("{row:?}")
        };
        assert!(unknown.is_empty(), "{unknown:?}");
        assert!(
            keys.iter()
                .any(|key| key.path == "field[0:value].type.field[0:nested].type"
                    && key.selected == "scalar:I32"
                    && key.alternative == "scalar:U32"),
            "{keys:?}"
        );
        assert!(
            !keys.iter().any(|key| key.path.contains("callback")),
            "{keys:?}"
        );
    }
}

#[test]
fn ranked_record_direct_callback_guard_precedes_observation_and_raw_shape_is_compared() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for different in [false, true] {
            let plan = ranked_plan(
                target,
                if different {
                    "evidence-direct-callback-different"
                } else {
                    "evidence-direct-callback"
                },
                false,
            );
            let selected = fact(&plan, "ranked-type-home.cpp", "EvidenceRecord");
            let FactData::Record { fields, .. } = &selected.data else {
                panic!("{selected:?}")
            };
            let TypeRef::Named { name, .. } = &fields[1].ty else {
                panic!("{fields:?}")
            };
            assert_eq!(name, "EvidenceRecord_callback");
            let callback = fact(&plan, "ranked-type-home.cpp", name);
            assert!(callback.root);
            assert!(matches!(callback.data, FactData::Callback { .. }));
            assert!(
                !plan
                    .snapshot
                    .semantic_parents
                    .contains_key(&callback.origin)
            );
            let snapshot = plan.snapshot.clone();
            OBSERVATION_CALLS.set(0);
            let error = report(plan).unwrap_err().to_string();
            assert!(error.contains("EvidenceRecord_callback"), "{error}");
            assert!(
                error.contains("no captured native semantic-parent relation"),
                "{error}"
            );
            assert!(error.contains("selection phase=initial"), "{error}");
            assert_eq!(
                OBSERVATION_CALLS.get(),
                0,
                "observation entered before selection failure"
            );
            let comparison = compare_extracted_callbacks(
                &snapshot,
                "EvidenceRecord_callback",
                "ranked-type-home.cpp",
                "ranked-type-client.cpp",
            );
            if different {
                let Some(DefinitionComparison::Different { keys, unknown }) = comparison else {
                    panic!("{comparison:?}")
                };
                assert!(unknown.is_empty(), "{unknown:?}");
                assert_eq!(keys.len(), 1, "{keys:?}");
                assert_eq!(keys[0].path, "param[0]");
                assert_eq!(keys[0].selected, "scalar:I32");
                assert_eq!(keys[0].alternative, "scalar:U32");
            } else {
                assert_eq!(comparison, None);
            }
        }
    }
}

fn observer_plan(
    target: &str,
    case: &str,
    reversed: bool,
    unused: Option<(&str, bool)>,
) -> HeaderPartitionPlan {
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("header-authority-observer.hpp");
    let header = header.to_str().unwrap();
    let home = Input::new("observer-home.cpp", format!("#include {header:?}\n"))
        .with_roots([header])
        .partitioned("Home");
    let flags = match case {
        "forward-defined" => "#define AUTHORITY_OBSERVER_FORWARD_DEFINED\n",
        "forward-only" => "#define AUTHORITY_OBSERVER_FORWARD_ONLY\n",
        "ambiguous-equal" | "missing-equal" => "#define AUTHORITY_OBSERVER_EQUAL\n",
        _ => "",
    };
    let client = Input::new(
                "observer-client.cpp",
                format!(
                    "#define AUTHORITY_OBSERVER_CLIENT\n\
                     {flags}#include {header:?}\nextern \"C\" void UseObserver(ObserverRecord* value);\n"
                ),
            )
            .with_roots([header, "observer-client.cpp"])
            .partitioned("Api");
    let mut policy = HeaderPartitionPolicy::new()
        .with_partition_authority("Home", 0, "observer-home.cpp")
        .with_partition_authority("Api", 1, "observer-client.cpp")
        .with_traversed_header(header, RootPartition::new("Home", "Test.Home"))
        .with_traversed_header("observer-client.cpp", RootPartition::new("Api", "Test.Api"));
    if let Some((partition, home_input)) = unused {
        policy.set_partition_authority(
            partition,
            2,
            if home_input {
                "observer-home.cpp"
            } else {
                "observer-client.cpp"
            },
        );
    }
    if !matches!(case, "missing" | "missing-equal") {
        policy.add_traversed_header(header, RootPartition::new("Api", "Test.Alternative"));
    }
    if matches!(case, "ambiguous" | "ambiguous-equal") {
        policy.set_partition_authority("AOtherApi", 2, "observer-client.cpp");
        policy.add_traversed_header(header, RootPartition::new("AOtherApi", "Test.Other"));
    }
    let mut inputs = vec![home, client];
    if reversed {
        inputs.reverse();
    }
    let target = format!("--target={target}");
    extract_partitioned(
        inputs,
        &["-x", "c++", "-std=c++17", &target, "--no-default-config"],
    )
    .unwrap()
    .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
    .unwrap()
}

fn assert_observer_selected_body(plan: &HeaderPartitionPlan) {
    assert_eq!(
        fact(plan, "observer-home.cpp", "ObserverRecord").data,
        FactData::Record {
            base: None,
            fields: vec![Field {
                name: "value".to_string(),
                ty: TypeRef::Scalar(Scalar::I32),
                offset: 0,
                align: 4,
                size: 4,
                bit_width: None,
            }],
            size: 4,
            align: 4,
            packing: None,
            alignment: None,
            union: false,
        }
    );
}

#[test]
fn ranked_record_unused_shared_input_registration_cannot_replace_provenance() {
    helpers::ensure_libclang();
    for target in TARGETS {
        let baseline = report(observer_plan(target, "defined", false, None)).unwrap();
        for partition in ["AUnused", "ZUnused"] {
            for home_input in [false, true] {
                for reversed in [false, true] {
                    let plan =
                        observer_plan(target, "defined", reversed, Some((partition, home_input)));
                    assert_observer_selected_body(&plan);
                    let legacy = output_without_notes(plan.clone());
                    let report = report(plan).unwrap();
                    assert_eq!(report.definition_evidence.len(), 1, "{report:?}");
                    let row = evidence(&report, "ObserverRecord");
                    assert_eq!(row.selected.partition.as_deref(), Some("Home"));
                    assert_eq!(row.selected.rank, Some(0));
                    assert_eq!(row.selected.input, "observer-home.cpp");
                    assert_eq!(row.alternative.partition.as_deref(), Some("Api"));
                    assert_eq!(row.alternative.rank, Some(1));
                    assert_eq!(row.alternative.input, "observer-client.cpp");
                    let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
                        panic!("{row:?}")
                    };
                    assert!(unknown.is_empty());
                    assert!(keys.iter().any(|key| key.path == "field[0:value].type"
                        && key.selected == "scalar:I32"
                        && key.alternative == "scalar:U64"));
                    let text = report.partitions.values().cloned().collect::<String>();
                    assert_eq!(
                        text.matches(&format!("// windows-clang: {}\n", row.message))
                            .count(),
                        1
                    );
                    assert!(!row.message.contains("rank=2"));
                    assert!(text.contains("value: i32"), "{text}");
                    for (partition, text) in &report.partitions {
                        assert_eq!(without_evidence_comments(text), legacy[partition]);
                    }
                    assert_eq!(report, baseline);
                }
            }
        }
    }
}

#[test]
fn ranked_record_forward_redeclarations_count_contexts_not_facts() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["forward-defined", "forward-only"] {
            let mut baseline = None;
            for reversed in [false, true] {
                let plan = observer_plan(target, case, reversed, None);
                assert_observer_selected_body(&plan);
                let declarations: Vec<_> = plan
                    .snapshot
                    .facts
                    .iter()
                    .filter(|fact| {
                        fact.origin.tu == "observer-client.cpp" && fact.name == "ObserverRecord"
                    })
                    .collect();
                assert_eq!(
                    declarations.iter().filter(|fact| !fact.definition).count(),
                    2
                );
                assert_eq!(
                    declarations.iter().filter(|fact| fact.definition).count(),
                    usize::from(case == "forward-defined")
                );
                let report = report(plan).unwrap();
                let DefinitionEvidenceCoverage::CapturedBodiesAndReferentConst {
                    alternative_definitions,
                    declaration_only_observations,
                    ..
                } = report.definition_evidence_coverage
                else {
                    panic!("{report:?}")
                };
                assert_eq!(
                    alternative_definitions,
                    usize::from(case == "forward-defined")
                );
                assert_eq!(
                    declaration_only_observations,
                    usize::from(case == "forward-only")
                );
                assert_eq!(
                    report.definition_evidence.len(),
                    usize::from(case == "forward-defined")
                );
                let text = report.partitions.values().cloned().collect::<String>();
                assert!(text.contains("value: i32"), "{text}");
                assert_eq!(
                    text.matches("// windows-clang: record definition ").count(),
                    usize::from(case == "forward-defined")
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

#[test]
fn ranked_record_ambiguous_provenance_retains_candidates_and_independent_body_evidence() {
    helpers::ensure_libclang();
    for target in TARGETS {
        for case in ["ambiguous", "ambiguous-equal", "missing", "missing-equal"] {
            let mut baseline = None;
            for reversed in [false, true] {
                let plan = observer_plan(target, case, reversed, None);
                assert_observer_selected_body(&plan);
                let legacy = output_without_notes(plan.clone());
                let report = report(plan).unwrap();
                assert_eq!(report.definition_evidence.len(), 1, "{report:?}");
                let row = evidence(&report, "ObserverRecord");
                assert_eq!(row.selected.partition.as_deref(), Some("Home"));
                assert_eq!(row.selected.rank, Some(0));
                assert_eq!(
                    row.selected.candidate_partitions,
                    vec![("Home".to_string(), 0)]
                );
                assert_eq!(row.alternative.partition, None);
                assert_eq!(row.alternative.rank, None);
                assert_eq!(row.alternative.input, "observer-client.cpp");
                let ambiguous = case.starts_with("ambiguous");
                assert_eq!(
                    row.alternative.candidate_partitions,
                    if ambiguous {
                        vec![("Api".to_string(), 1), ("AOtherApi".to_string(), 2)]
                    } else {
                        Vec::new()
                    }
                );
                let different = !case.ends_with("equal");
                let unknown = if different {
                    let DefinitionComparison::Different { keys, unknown } = &row.outcome else {
                        panic!("{row:?}")
                    };
                    assert!(keys.iter().any(|key| key.path == "field[0:value].type"
                        && key.selected == "scalar:I32"
                        && key.alternative == "scalar:U64"));
                    unknown
                } else {
                    let DefinitionComparison::Unknown(unknown) = &row.outcome else {
                        panic!("{row:?}")
                    };
                    unknown
                };
                assert_eq!(unknown.len(), 1, "{unknown:?}");
                assert_eq!(unknown[0].path, "alternative.provenance");
                assert!(unknown[0].reason.contains(if ambiguous {
                    "ambiguous traversing owners"
                } else {
                    "no traversing owner"
                }));
                assert!(
                    row.message
                        .contains("alternative=\"observer-client.cpp\" rank=Unknown")
                );
                assert!(row.message.contains(&format!(
                    "alternative-owners={:?}",
                    row.alternative.candidate_partitions
                )));
                let text = report.partitions.values().cloned().collect::<String>();
                assert!(text.contains("value: i32"), "{text}");
                assert_eq!(
                    text.matches(&format!("// windows-clang: {}\n", row.message))
                        .count(),
                    usize::from(different)
                );
                for (partition, text) in &report.partitions {
                    assert_eq!(without_evidence_comments(text), legacy[partition]);
                }
                assert!(matches!(
                    report.definition_evidence_coverage,
                    DefinitionEvidenceCoverage::CapturedBodiesAndReferentConst {
                        alternative_definitions: 1,
                        declaration_only_observations: 0,
                        ..
                    }
                ));
                if let Some(baseline) = &baseline {
                    assert_eq!(&report, baseline);
                } else {
                    baseline = Some(report);
                }
            }
        }
    }
}
