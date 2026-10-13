use super::*;
use windows_metadata::HasAttributes;

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
        format!("{home_prefix}{offsets}#include {header:?}\n"),
    )
    .with_roots([header])
    .partitioned("Home");
    let client = Input::new(
        "ranked-type-client.cpp",
        format!(
            "{}{offsets}{alternative}#define AUTHORITY_ALTERNATIVE\n\
             #define AUTHORITY_VALUE unsigned long long\n#include {header:?}\n\
             {use_source}{}",
            if case.starts_with("opaque-") {
                "#define AUTHORITY_OPAQUE\n"
            } else if case.starts_with("scope") {
                "#define AUTHORITY_SCOPE\n"
            } else {
                ""
            },
            if case == "offsets" {
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
