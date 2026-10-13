use super::*;

fn extract(inputs: impl IntoIterator<Item = Input>, args: &[&str]) -> Result<Snapshot, Error> {
    let mut controlled = vec!["--no-default-config"];
    controlled.extend_from_slice(args);
    crate::extract(inputs, &controlled)
}

#[test]
fn native_evidence_shares_canonical_subgraphs_only_within_tu() {
    let snapshot = extract(
        [
            Input::new(
                "sharing.hpp",
                "struct Pod { int value; };\n\
                typedef Pod First; typedef Pod Second; typedef int Int;\n\
                struct Base { virtual Pod *Method(Pod *) = 0; };\n\
                struct Derived : Base { virtual Pod *Other(Pod *) = 0; };\n",
            ),
            Input::new("other-sharing.hpp", "typedef int OtherInt;"),
        ],
        &[
            "--no-default-config",
            "-x",
            "c++",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let origin = |name: &str| {
        &snapshot
            .facts
            .iter()
            .find(|fact| fact.name == name)
            .unwrap()
            .origin
    };
    let native = |name: &str| {
        snapshot.native_typedef_types[origin(name)]
            .as_ref()
            .unwrap()
    };
    assert!(Arc::ptr_eq(&native("First").0, &native("Second").0));
    assert_eq!(native("Int"), native("OtherInt"));
    assert!(!Arc::ptr_eq(&native("Int").0, &native("OtherInt").0));
    let base = &snapshot.native_com_contracts[origin("Base")];
    let derived = &snapshot.native_com_contracts[origin("Derived")];
    let result = base.slots[0].native_result.as_ref().unwrap();
    assert!(Arc::ptr_eq(
        &result.0,
        &derived.slots[0].native_result.as_ref().unwrap().0
    ));
    assert!(Arc::ptr_eq(
        &result.0,
        &base.slots[0].native_params[0].as_ref().unwrap().0
    ));
    let NativeTypeLayer::Indirect(target) = &result.layer else {
        panic!()
    };
    assert!(Arc::ptr_eq(&target.0, &native("First").0));
}

#[test]
fn contextual_interface_proof_lvalue_reference_native_identity() {
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("reference-contract.hpp");
    let header = header.to_str().unwrap();
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        for case in [
            "primitive",
            "long-long",
            "record",
            "record-attribute",
            "record-attribute-pointer",
            "record-attribute-value",
            "primitive-rank",
            "record-rank",
            "cv",
            "pointer",
            "native-kind",
            "source",
            "tu",
            "modifier",
            "address-space",
        ] {
            let left_kind = if case == "long-long" {
                "long long"
            } else {
                "long"
            };
            let right_kind = if matches!(case, "primitive-rank" | "record-rank") {
                "int"
            } else {
                left_kind
            };
            let right_cv = if case == "cv" { "" } else { "const" };
            let right_operator = if case == "pointer" { "*" } else { "&" };
            let attributed = case.starts_with("record-attribute");
            let prefix = if attributed {
                "#pragma clang attribute push (__attribute__((annotate(\"record-memory-control\"))), apply_to = record)\n"
            } else {
                ""
            };
            let reference_override = match case {
                "record-attribute-pointer" => "#define REF_OPERATOR *\n",
                "record-attribute-value" => "#define REF_OPERATOR\n",
                _ => "",
            };
            let suffix = if attributed {
                "#pragma clang attribute pop\n"
            } else {
                ""
            };
            let right_reference = if reference_override.is_empty() {
                format!("#define REF_OPERATOR {right_operator}\n")
            } else {
                reference_override.to_string()
            };
            let snapshot = extract(
                [
                    Input::new(
                        "reference-a.cpp",
                        format!(
                            "{prefix}{reference_override}#define FIELD_KIND {left_kind}\n\
                             #include {header:?}\n{suffix}"
                        ),
                    )
                    .with_roots([header]),
                    Input::new(
                        "reference-b.cpp",
                        format!(
                            "{prefix}#define FIELD_KIND {right_kind}\n#define REF_CV {right_cv}\n\
                             {right_reference}\
                             #include {header:?}\n{suffix}"
                        ),
                    )
                    .with_roots([header]),
                ],
                &[
                    "-x",
                    "c++",
                    "--no-default-config",
                    &format!("--target={target}"),
                    "-fms-extensions",
                ],
            )
            .unwrap();
            let alias_name =
                if attributed || matches!(case, "record" | "record-rank" | "source" | "tu") {
                    "PayloadReference"
                } else {
                    "PrimitiveReference"
                };
            let observation = |tu: &str| {
                let facts: Vec<_> = snapshot
                    .facts
                    .iter()
                    .filter(|fact| fact.origin.tu == tu && fact.name == alias_name)
                    .collect();
                assert_eq!(facts.len(), 1);
                facts[0]
            };
            let (left, right) = (
                observation("reference-a.cpp"),
                observation("reference-b.cpp"),
            );
            let (FactData::Typedef { target: lt }, FactData::Typedef { target: rt }) =
                (&left.data, &right.data)
            else {
                panic!()
            };
            let ln = snapshot.native_typedef_types[&left.origin]
                .as_ref()
                .unwrap();
            let mut rn = snapshot.native_typedef_types[&right.origin]
                .as_ref()
                .unwrap()
                .clone();
            if !matches!(case, "record-attribute-pointer" | "record-attribute-value") {
                assert_eq!(ln.kind, clang_sys::CXType_LValueReference);
            }
            if attributed {
                let record_type = match &ln.layer {
                    NativeTypeLayer::Indirect(target) => target,
                    NativeTypeLayer::Record(_) => ln,
                    _ => panic!(),
                };
                let NativeTypeLayer::Record(record) = &record_type.layer else {
                    panic!()
                };
                assert!(!record.aggregate_abi_proved);
            }
            if case == "record" {
                assert_eq!(ln.size, 16);
                assert_ne!(ln, &rn);
            }
            match case {
                "native-kind" => rn.kind = clang_sys::CXType_RValueReference,
                "modifier" => rn.spelling.push_str(" __unknown_modifier"),
                "address-space" => rn.address_space = 1,
                "source" | "tu" => {
                    let NativeTypeLayer::Indirect(child) = &mut rn.layer else {
                        panic!()
                    };
                    let NativeTypeLayer::Record(record) = &mut child.layer else {
                        panic!()
                    };
                    if case == "source" {
                        record.declaration.spelling.file = "other.hpp".into();
                    } else {
                        record.declaration.tu = "other.cpp".into();
                    }
                }
                _ => {}
            }
            let declarations = DeclarationIndex::new(&snapshot.facts);
            let by_declaration = snapshot
                .facts
                .iter()
                .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
                .collect();
            let source_names = PlanningSourceNames::new(&snapshot);
            let source_annotations = snapshot.source_annotation_signatures();
            let effective_annotations =
                Snapshot::relative_annotation_signatures(&snapshot.annotations);
            let context = crate::com_pointer::Context {
                snapshot: &snapshot,
                declarations: &declarations,
                facts_by_declaration: &by_declaration,
                source_names: Some(&source_names),
                excluded_types: None,
                source_annotations: &source_annotations,
                effective_annotations: &effective_annotations,
            };
            let result =
                context.canonical_types_equal(lt, &left.origin.tu, rt, &right.origin.tu, (ln, &rn));
            if case == "record-attribute-value" {
                let identical = context
                    .canonical_types_equal(lt, &left.origin.tu, lt, &left.origin.tu, (ln, ln))
                    .unwrap_err();
                assert!(identical.contains("value-ABI"), "{identical}");
            }
            if matches!(
                case,
                "primitive"
                    | "long-long"
                    | "record"
                    | "record-attribute"
                    | "record-attribute-pointer"
            ) {
                result.unwrap();
                println!("reference case={case} target={target} assertions=passed");
            } else {
                let error = result.unwrap_err();
                let expected = match case {
                    "primitive-rank" => "canonical native evidence differs",
                    "record-rank" => "field[0](value)",
                    "cv" | "pointer" | "native-kind" => "canonical native evidence differs",
                    "source" => {
                        "reference layout -> native nominal binding: projected terminal/source/name"
                    }
                    "tu" => "reference layout -> native nominal binding: declaration TU",
                    "modifier" | "address-space" => {
                        "unsupported canonical native kind/modifier/address-space"
                    }
                    "record-attribute-value" => "value-ABI",
                    _ => unreachable!(),
                };
                assert!(error.contains(expected), "{case}/{target}: {error}");
                println!(
                    "reference case={case} target={target} rejection={error} assertions=passed"
                );
            }
        }
    }
}

#[test]
fn contextual_interface_proof_typedef_provider_rejection_evidence() {
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("typedef-provider-contract.hpp");
    let header = header.to_str().unwrap();
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        for (qualifier, alias_qualifier) in [("", ""), ("", "const"), ("const", "")] {
            let original = extract(
                [
                    Input::new(
                        "typedef-provider-a.cpp",
                        format!(
                            "#define PROVIDER_CV {qualifier}\n\
                             #define PROVIDER_ALIAS_CV {alias_qualifier}\n#include {header:?}\n"
                        ),
                    )
                    .with_roots([header]),
                    Input::new(
                        "typedef-provider-b.cpp",
                        format!(
                            "#define PROVIDER_CV {qualifier}\n\
                             #define PROVIDER_ALIAS_CV {alias_qualifier}\n#include {header:?}\n"
                        ),
                    )
                    .with_roots([header]),
                ],
                &["-x", "c++", &format!("--target={target}")],
            )
            .unwrap();
            let cases: &[&str] = if !alias_qualifier.is_empty() {
                &["qualified-provider"]
            } else if qualifier.is_empty() {
                &[
                    "matching",
                    "missing",
                    "unsupported",
                    "native-source",
                    "suppression",
                    "context-exclusion",
                    "parent",
                    "duplicate",
                    "owner-exclusion",
                    "projection",
                    "namespace",
                ]
            } else {
                &["qualified-use"]
            };
            for &case in cases {
                let mut snapshot = original.clone();
                let provider = snapshot
                    .facts
                    .iter()
                    .find(|fact| {
                        fact.origin.tu == "typedef-provider-b.cpp" && fact.name == "PayloadAlias"
                    })
                    .unwrap()
                    .clone();
                let mut excluded = BTreeSet::new();
                match case {
                    "missing" => {
                        snapshot.native_typedef_types.remove(&provider.origin);
                    }
                    "unsupported" => {
                        snapshot
                            .native_typedef_types
                            .insert(provider.origin.clone(), None);
                    }
                    "native-source" => {
                        let native = snapshot
                            .native_typedef_types
                            .get_mut(&provider.origin)
                            .unwrap()
                            .as_mut()
                            .unwrap();
                        let NativeTypeLayer::Record(record) = &mut native.layer else {
                            panic!()
                        };
                        record.declaration.spelling.file = "other-provider.hpp".into();
                    }
                    "suppression" => {
                        snapshot
                            .suppressed_type_origins
                            .insert(provider.origin.clone());
                    }
                    "context-exclusion" => {
                        excluded.insert(provider.name.clone());
                    }
                    "parent" => {
                        snapshot
                            .facts
                            .iter_mut()
                            .find(|fact| fact.origin == provider.origin)
                            .unwrap()
                            .parent = Some(provider.origin.clone());
                    }
                    "duplicate" => {
                        let mut duplicate = provider.clone();
                        duplicate.origin.local += 1_000_000;
                        snapshot.native_typedef_types.insert(
                            duplicate.origin.clone(),
                            snapshot.native_typedef_types[&provider.origin].clone(),
                        );
                        snapshot.facts.push(duplicate);
                    }
                    "owner-exclusion" => {
                        let mut owner = root_owner(
                            &provider.origin.tu,
                            &provider.spelling.file,
                            &RootPartition::new("provider", "Example.Provider"),
                        );
                        owner.exclusions.insert(provider.name.clone());
                        snapshot.root_owners.insert(provider.origin.clone(), owner);
                    }
                    "projection" => {
                        snapshot
                            .projected_type_names
                            .insert(provider.origin.clone(), "OtherPayload".into());
                    }
                    "namespace" => {
                        snapshot
                            .fact_namespace_authorities
                            .insert(provider.origin.clone(), "Other.Provider".into());
                    }
                    _ => {}
                }
                let declarations = DeclarationIndex::new(&snapshot.facts);
                let by_declaration = snapshot
                    .facts
                    .iter()
                    .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
                    .collect();
                let source_names = PlanningSourceNames::new(&snapshot);
                let source_annotations = snapshot.source_annotation_signatures();
                let effective_annotations =
                    Snapshot::relative_annotation_signatures(&snapshot.annotations);
                let context = crate::com_pointer::Context {
                    snapshot: &snapshot,
                    declarations: &declarations,
                    facts_by_declaration: &by_declaration,
                    source_names: Some(&source_names),
                    excluded_types: Some(&excluded),
                    source_annotations: &source_annotations,
                    effective_annotations: &effective_annotations,
                };
                let interface = |tu: &str| {
                    snapshot
                        .facts
                        .iter()
                        .find(|fact| fact.origin.tu == tu && fact.name == "ProviderDerived")
                        .unwrap()
                };
                let left = interface("typedef-provider-a.cpp");
                let right = interface("typedef-provider-b.cpp");
                let contract = &snapshot.native_com_contracts[&right.origin];
                assert_eq!(contract.slots[0].name, "Observe");
                assert!(contract.slots[0].native_params[0].is_some());
                assert_eq!(
                    contract.slots[0]
                        .native_owner
                        .as_ref()
                        .unwrap()
                        .declaration
                        .parent
                        .name,
                    "ProviderBase"
                );
                let result = contextual_interface_matches(&context, left, right);
                if matches!(case, "matching" | "qualified-provider") {
                    result.unwrap();
                    println!("typedef provider case={case} target={target} assertions=passed");
                    continue;
                }
                let error = result.unwrap_err();
                assert!(
                    error.contains("method[0](Observe).params[0]: reference layout"),
                    "{case}/{target}: {error}"
                );
                let expected = match case {
                    "missing" => "native-binding=missing",
                    "unsupported" => "native-binding=unsupported",
                    "native-source" | "qualified-use" => "native-binding=mismatch",
                    "suppression" => "native-binding=matched suppressed=true excluded=false",
                    "context-exclusion" => "native-binding=matched suppressed=false excluded=true",
                    "parent" => "parent=Some(Origin",
                    "duplicate" => {
                        "native-binding=matched suppressed=false excluded=false declaration-count=2"
                    }
                    "owner-exclusion" => "unsupported source ownership or excluded provider",
                    "projection" | "namespace" => "canonical provider contract differs",
                    _ => unreachable!(),
                };
                assert!(error.contains(expected), "{case}/{target}: {error}");
                if !matches!(case, "owner-exclusion" | "projection" | "namespace") {
                    let rejected_tu = if matches!(case, "context-exclusion" | "qualified-use") {
                        "typedef-provider-a.cpp"
                    } else {
                        "typedef-provider-b.cpp"
                    };
                    let rejected = context
                        .exact(rejected_tu, &provider.name, &provider.spelling)
                        .unwrap();
                    assert!(
                        error.contains(&format!(
                            "ineligible canonical typedef provider {:?}: kind=Typedef",
                            rejected.origin
                        )),
                        "{error}"
                    );
                    assert!(error.contains("name=\"PayloadAlias\""), "{error}");
                    assert!(
                        error.contains(&format!("spelling={:?}", rejected.spelling)),
                        "{error}"
                    );
                    assert!(
                        error.contains(&format!(
                            "suppressed={} excluded={} declaration-count={}",
                            case == "suppression",
                            case == "context-exclusion",
                            if case == "duplicate" { 2 } else { 1 },
                        )),
                        "{error}"
                    );
                    assert!(error.contains("captured-native=("), "{error}");
                    assert!(error.contains("use-native=("), "{error}");
                    if case == "qualified-use" {
                        assert!(
                            error.contains("qualifiers=(false, false, false)"),
                            "{error}"
                        );
                        assert!(error.contains("qualifiers=(true, false, false)"), "{error}");
                    }
                }
                println!("typedef provider case={case} target={target} rejection={error}");
            }
        }
    }
}

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
        class_canonical_anchors: BTreeMap::new(),
        shared_enum_identities: BTreeMap::new(),
        native_com_contracts: BTreeMap::new(),
        native_typedef_types: BTreeMap::new(),
        unproved_com_records: BTreeSet::new(),
        observation_only_class_origins: BTreeSet::new(),
        pointer_callback_aliases: BTreeSet::new(),
        pointer_only_class_layouts: BTreeMap::new(),
        embeddable_class_layouts: BTreeSet::new(),
        clang_flag_enums: BTreeSet::new(),
        root_owners: BTreeMap::new(),
        constant_root_owners: BTreeMap::new(),
        root_partitions: BTreeMap::new(),
        header_authorities: BTreeMap::new(),
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
        ranked_header_partition_policy: false,
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

fn diagnostic_interface(tu: &str, local: u32) -> Fact {
    test_fact_in_tu(
        tu,
        local,
        None,
        FactKind::Struct,
        "Example",
        Location {
            file: "common.hpp".to_string(),
            offset: 100,
        },
        FactData::Interface {
            guid: Some("00000000-0000-0000-c000-000000000046".to_string()),
            base: None,
            methods: vec![Method {
                name: "Update".to_string(),
                params: vec![Parameter {
                    name: "value".to_string(),
                    ty: TypeRef::Scalar(Scalar::I32),
                    annotation: ParamAnnotation::default(),
                }],
                result: TypeRef::Void,
                special: false,
            }],
        },
    )
}

#[test]
fn root_failure_diagnostics_preserve_non_type_error_contracts() {
    let snapshot = snapshot(vec![diagnostic_interface("owner", 1)]);
    let candidates = [&snapshot.facts[0]];
    for category in [
        "collision",
        "value",
        "constant",
        "function",
        "selected function",
    ] {
        let error = root_selection_error(
            &snapshot,
            "Example",
            category,
            Error("legacy failure".to_string()),
            &candidates,
            &[],
        );
        assert_eq!(error.error.to_string(), "legacy failure");
        assert_eq!(
            aggregate_root_selection_errors(vec![error]).to_string(),
            "legacy failure"
        );
    }
    let errors = ["first function failure", "second function failure"]
        .map(|message| {
            root_selection_error(
                &snapshot,
                "Example",
                "function",
                Error(message.to_string()),
                &candidates,
                &[],
            )
        })
        .into_iter()
        .collect();
    assert_eq!(
        aggregate_root_selection_errors(errors).to_string(),
        "ambiguous function roots (2):\n  first function failure\n  second function failure"
    );
}

#[test]
fn root_failure_diagnostics_single_initial_retains_uncoalesced_context() {
    let first = diagnostic_interface("owner", 1);
    let duplicate = diagnostic_interface("observed", 1);
    let mut missing = alias(2, "Example", "alias.hpp", 2, named(&first));
    missing.origin.tu = "consumer".to_string();
    let mut snapshot = snapshot(vec![first, duplicate, missing]);
    snapshot.root_owners.insert(
        snapshot.facts[0].origin.clone(),
        authority_root_owner("owner", "Test", "common.hpp"),
    );
    let error = snapshot
        .plan_partitioned(
            &EmitOptions::new("Test", &BTreeMap::new()),
            &BTreeMap::new(),
            &PlanningSourceNames::new(&snapshot),
        )
        .err()
        .unwrap()
        .to_string();
    for expected in [
        "phase=initial",
        "pre-coalescence",
        "tu: \"owner\"",
        "tu: \"observed\"",
        "tu: \"consumer\"",
        "owner=Some",
        "parent=None",
        "suppressed=false",
        "alias hop",
        "common.hpp:100",
        "missing exact bucket",
    ] {
        assert!(error.contains(expected), "{expected}: {error}");
    }
}

#[test]
fn root_failure_diagnostics_late_group_retains_terminal_parameter_path() {
    let owner = diagnostic_interface("owner", 1);
    let mut foreign = diagnostic_interface("consumer", 1);
    foreign.root = false;
    let FactData::Interface { methods, .. } = &mut foreign.data else {
        unreachable!()
    };
    methods[0].params[0].ty = TypeRef::Scalar(Scalar::F32);
    let mut bridge = alias(2, "Example", "alias.hpp", 2, named(&foreign));
    bridge.origin.tu = "consumer".to_string();
    bridge.root = false;
    let mut selected = function("consumer", 3, "Use", "use.hpp", "value", true);
    let FactData::Function { params, .. } = &mut selected.data else {
        unreachable!()
    };
    params[0].ty = TypeRef::Pointer {
        mutable: true,
        target: Box::new(named(&bridge)),
    };
    let snapshot = snapshot(vec![owner, foreign, bridge, selected]);
    let error = snapshot
        .plan_partitioned(
            &EmitOptions::new("Test", &BTreeMap::new()),
            &BTreeMap::new(),
            &PlanningSourceNames::new(&snapshot),
        )
        .err()
        .unwrap()
        .to_string();
    for expected in [
        "phase=post-closure",
        "tu: \"owner\"",
        "tu: \"consumer\"",
        "exact bucket",
        "terminal",
        "data.methods[0](Update).params[0](value).ty",
        "representation/source mismatch (not ABI proof)",
        "F32",
        "I32",
    ] {
        assert!(error.contains(expected), "{expected}: {error}");
    }
}

fn cross_tu_result_alias_snapshot(provider: &str, result: Option<Scalar>) -> Snapshot {
    let mut owner_result = alias(
        10,
        "Status",
        "status_a.hpp",
        16,
        TypeRef::Scalar(Scalar::I32),
    );
    owner_result.origin.tu = "owner".to_string();
    owner_result.root = false;
    let mut consumer_result = owner_result.clone();
    consumer_result.origin.tu = "consumer".to_string();
    consumer_result.spelling.file = provider.to_string();
    consumer_result.expansion = consumer_result.spelling.clone();
    consumer_result.data = FactData::Typedef {
        target: TypeRef::Scalar(result.unwrap_or(Scalar::I32)),
    };
    let mut owner = diagnostic_interface("owner", 1);
    let FactData::Interface { methods, .. } = &mut owner.data else {
        unreachable!()
    };
    methods[0].name = "Reset".to_string();
    methods[0].params.clear();
    methods[0].result = named(&owner_result);
    let mut consumer = owner.clone();
    consumer.origin.tu = "consumer".to_string();
    consumer.root = false;
    let FactData::Interface { methods, .. } = &mut consumer.data else {
        unreachable!()
    };
    methods[0].result = named(&consumer_result);
    let mut owned_alias = alias(2, "Example", "owned_alias.hpp", 32, named(&owner));
    owned_alias.origin.tu = "owner".to_string();
    let mut dependency_alias = alias(2, "Example", "common.hpp", 32, named(&consumer));
    dependency_alias.origin.tu = "consumer".to_string();
    dependency_alias.root = false;
    let mut use_alias = function("consumer", 3, "Use", "use.hpp", "value", true);
    let FactData::Function { params, .. } = &mut use_alias.data else {
        unreachable!()
    };
    params[0].ty = TypeRef::Pointer {
        mutable: true,
        target: Box::new(named(&dependency_alias)),
    };
    let mut facts = vec![
        owner,
        owned_alias,
        consumer,
        dependency_alias,
        use_alias,
        owner_result,
    ];
    if result.is_some() {
        facts.push(consumer_result);
    }
    let mut snapshot = snapshot(facts);
    // Authored compiler provenance for this private fixture, independent of projected Scalar.
    for (tu, kind) in [
        ("owner", clang_sys::CXType_Long),
        (
            "consumer",
            if result == Some(Scalar::F32) {
                clang_sys::CXType_Float
            } else {
                clang_sys::CXType_Long
            },
        ),
    ] {
        let native = NativeType(Arc::new(NativeTypeData {
            kind,
            is_const: false,
            is_volatile: false,
            is_restrict: false,
            size: 4,
            address_space: 0,
            spelling: if kind == clang_sys::CXType_Float {
                "float"
            } else {
                "long"
            }
            .to_string(),
            layer: NativeTypeLayer::Primitive,
        }));
        if let Some(provider) = snapshot
            .facts
            .iter()
            .find(|fact| fact.origin.tu == tu && fact.name == "Status")
        {
            snapshot
                .native_typedef_types
                .insert(provider.origin.clone(), Some(native.clone()));
        }
        let interface = snapshot
            .facts
            .iter()
            .find(|fact| fact.origin.tu == tu && matches!(fact.data, FactData::Interface { .. }))
            .unwrap();
        let FactData::Interface { guid, methods, .. } = &interface.data else {
            unreachable!()
        };
        snapshot.native_com_contracts.insert(
            interface.origin.clone(),
            Arc::new(NativeComContract {
                guid: guid.clone().unwrap(),
                has_fields: false,
                pointer_width: 64,
                bases: Vec::new(),
                slots: vec![NativeComMethod {
                    name: methods[0].name.clone(),
                    convention: clang_sys::CXCallingConv_C,
                    params: methods[0].params.clone(),
                    result: methods[0].result.clone(),
                    native_params: Vec::new(),
                    native_result: Some(native),
                    native_exception: Some(clang_sys::CXCursor_ExceptionSpecificationKind_None),
                    native_owner: None,
                }],
            }),
        );
    }
    for fact in &snapshot.facts[..2] {
        snapshot.root_owners.insert(
            fact.origin.clone(),
            authority_root_owner("owner", "Test", &fact.spelling.file),
        );
    }
    let mut index: HashMap<&str, Vec<&Fact>> = HashMap::new();
    for fact in &snapshot.facts {
        index.entry(&fact.name).or_default().push(fact);
    }
    assert!(
        choose_type_root("Example", &[&snapshot.facts[0], &snapshot.facts[1]], &index,).is_ok()
    );
    snapshot
}

#[test]
fn cross_tu_result_alias_same_provider_emits() {
    let snapshot = cross_tu_result_alias_snapshot("status_a.hpp", Some(Scalar::I32));
    let output = snapshot.emit_with_library("Test", "test.dll").unwrap();
    assert!(output.contains("fn Reset("), "{output}");
}

#[test]
fn cross_tu_result_alias_alternative_provider_emits() {
    let snapshot = cross_tu_result_alias_snapshot("status_b.hpp", Some(Scalar::I32));
    let output = snapshot
        .emit_with_library("Test", "test.dll")
        .unwrap_or_else(|error| panic!("equivalent result providers must emit:\n{error}"));
    assert!(output.contains("fn Reset("), "{output}");
}

#[test]
fn cross_tu_result_alias_different_and_unresolved_providers_fail() {
    for result in [Some(Scalar::F32), None] {
        let snapshot = cross_tu_result_alias_snapshot("status_b.hpp", result);
        assert!(snapshot.emit_with_library("Test", "test.dll").is_err());
    }
}

#[test]
fn cross_tu_result_alias_same_address_different_targets_fail() {
    let snapshot = cross_tu_result_alias_snapshot("status_a.hpp", Some(Scalar::F32));
    assert!(
        snapshot.emit_with_library("Test", "test.dll").is_err(),
        "matching nominal addresses must not hide different exact-TU typedef targets"
    );
}

#[test]
fn cross_tu_result_alias_same_address_missing_provider_fails() {
    let snapshot = cross_tu_result_alias_snapshot("status_a.hpp", None);
    assert!(
        snapshot.emit_with_library("Test", "test.dll").is_err(),
        "matching nominal addresses must not hide an unresolved exact-TU typedef"
    );
}

#[test]
fn cross_tu_result_alias_native_scalar_rank_is_not_retained() {
    let snapshot = extract(
        [Input::new(
            "native-rank.hpp",
            "typedef int NativeInt;\ntypedef long NativeLong;\n",
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    for name in ["NativeInt", "NativeLong"] {
        let fact = snapshot
            .facts
            .iter()
            .find(|fact| fact.name == name)
            .unwrap();
        assert_eq!(
            fact.data,
            FactData::Typedef {
                target: TypeRef::Scalar(Scalar::I32)
            }
        );
        eprintln!("{name}: {:?}", fact.data);
    }
}

#[test]
fn contextual_interface_proof_rejects_ineligible_providers() {
    for case in [
        "source-annotation",
        "annotation",
        "projection",
        "namespace",
        "ambiguous",
        "cycle",
    ] {
        let mut snapshot = cross_tu_result_alias_snapshot("status_b.hpp", Some(Scalar::I32));
        let provider = snapshot
            .facts
            .iter()
            .find(|fact| fact.origin.tu == "consumer" && fact.name == "Status")
            .unwrap()
            .clone();
        match case {
            "source-annotation" => {
                snapshot.source_annotations.insert(
                    AnnotationTarget::Declaration(provider.origin.clone()),
                    vec![Annotation::Const],
                );
            }
            "annotation" => {
                snapshot.annotations.insert(
                    AnnotationTarget::Declaration(provider.origin.clone()),
                    vec![Annotation::Const],
                );
            }
            "projection" => {
                snapshot
                    .projected_type_names
                    .insert(provider.origin.clone(), "OtherStatus".to_string());
            }
            "namespace" => {
                snapshot
                    .fact_namespace_authorities
                    .insert(provider.origin.clone(), "Other.Namespace".to_string());
            }
            "ambiguous" => {
                let mut other = provider.clone();
                other.origin.local += 1;
                snapshot.facts.push(other);
            }
            "cycle" => {
                snapshot
                    .facts
                    .iter_mut()
                    .find(|fact| fact.origin == provider.origin)
                    .unwrap()
                    .data = FactData::Typedef {
                    target: named(&provider),
                };
            }
            _ => unreachable!(),
        }
        assert!(
            snapshot.emit_with_library("Test", "test.dll").is_err(),
            "{case}"
        );
    }
}

fn portable_native_provider_snapshot(int_provider: bool) -> Snapshot {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("nominal_providers");
    let input = |file: &str, roots: &[&str], prefix: &str| {
        let path = root.join(file);
        Input::new(
            path.to_str().unwrap(),
            format!("{prefix}{}", std::fs::read_to_string(&path).unwrap()),
        )
        .with_roots(
            roots
                .iter()
                .map(|name| root.join(name).to_str().unwrap().to_string()),
        )
    };
    extract(
        [
            input(
                "owner.cpp",
                &["base.hpp", "interface.hpp", "owned-alias.hpp"],
                "",
            ),
            input(
                "different-provider.cpp",
                &["consumer.hpp"],
                if int_provider {
                    "#define INT_PROVIDER\n"
                } else {
                    ""
                },
            ),
        ],
        &[
            "-x",
            "c++",
            "-std=c++17",
            "--target=x86_64-pc-windows-msvc",
            "-fms-extensions",
        ],
    )
    .unwrap()
}

#[test]
fn contextual_interface_proof_portable_native_long_providers() {
    let output = portable_native_provider_snapshot(false)
        .emit_with_library("PortableRepro", "portable.dll")
        .unwrap();
    for name in ["NativeContract", "Reset", "Use"] {
        assert!(output.contains(name), "{output}");
    }
}

#[test]
fn contextual_interface_proof_portable_native_int_long_rejected() {
    let error = portable_native_provider_snapshot(true)
        .emit_with_library("PortableRepro", "portable.dll")
        .unwrap_err()
        .to_string();
    for expected in [
        "ambiguous type root `NativeContract`",
        "contextual interface proof",
        "method[0](Reset).result",
        "canonical native evidence differs",
        "provider",
        "kind: 17",
        "kind: 18",
    ] {
        assert!(error.contains(expected), "{expected}: {error}");
    }
    assert!(
        !error.contains("ambiguous type root `NativeBase`"),
        "{error}"
    );
}

#[test]
fn contextual_interface_proof_matching_provider_policies() {
    for case in [
        "namespace",
        "source",
        "effective",
        "projection",
        "owned",
        "header-owned",
    ] {
        let mut snapshot = cross_tu_result_alias_snapshot("status_b.hpp", Some(Scalar::I32));
        let providers: Vec<_> = snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "Status")
            .cloned()
            .collect();
        for provider in &providers {
            match case {
                "namespace" => {
                    snapshot
                        .fact_namespace_authorities
                        .insert(provider.origin.clone(), "Foundation".to_string());
                }
                "source" => {
                    snapshot.source_annotations.insert(
                        AnnotationTarget::Declaration(provider.origin.clone()),
                        vec![Annotation::Const, Annotation::PreserveResult],
                    );
                }
                "effective" => {
                    snapshot.annotations.insert(
                        AnnotationTarget::Declaration(provider.origin.clone()),
                        vec![Annotation::Const, Annotation::PreserveResult],
                    );
                }
                "projection" => {
                    snapshot
                        .projected_type_names
                        .insert(provider.origin.clone(), "Status".to_string());
                }
                "owned" | "header-owned" => {
                    let owner = authority_root_owner(
                        &provider.origin.tu,
                        "Foundation",
                        &provider.spelling.file,
                    );
                    if case == "header-owned" && provider.origin.tu == "consumer" {
                        snapshot.root_partitions.insert(
                            (provider.origin.tu.clone(), provider.spelling.file.clone()),
                            owner,
                        );
                    } else {
                        snapshot.root_owners.insert(provider.origin.clone(), owner);
                    }
                }
                _ => unreachable!(),
            }
        }
        let declarations = DeclarationIndex::new(&snapshot.facts);
        let by_declaration = snapshot
            .facts
            .iter()
            .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
            .collect();
        let source_annotations = snapshot.source_annotation_signatures();
        let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
        let context = crate::com_pointer::Context {
            snapshot: &snapshot,
            declarations: &declarations,
            facts_by_declaration: &by_declaration,
            source_names: None,
            excluded_types: None,
            source_annotations: &source_annotations,
            effective_annotations: &effective_annotations,
        };
        contextual_interface_matches(&context, &snapshot.facts[2], &snapshot.facts[0])
            .unwrap_or_else(|error| panic!("{case}: {error}"));
    }
}

#[test]
fn contextual_interface_proof_conflicting_provider_policies() {
    for case in [
        "namespace",
        "source",
        "effective",
        "premerge",
        "projection",
        "owner-namespace",
        "owner-remap",
        "one-owner",
        "suppressed",
        "excluded",
    ] {
        let mut snapshot = cross_tu_result_alias_snapshot("status_b.hpp", Some(Scalar::I32));
        let providers: Vec<_> = snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "Status")
            .cloned()
            .collect();
        for provider in &providers {
            let consumer = provider.origin.tu == "consumer";
            match case {
                "namespace" => {
                    snapshot.fact_namespace_authorities.insert(
                        provider.origin.clone(),
                        if consumer { "Other" } else { "Foundation" }.to_string(),
                    );
                }
                "source" | "premerge" => {
                    snapshot.source_annotations.insert(
                        AnnotationTarget::Declaration(provider.origin.clone()),
                        vec![if consumer {
                            Annotation::PreserveResult
                        } else {
                            Annotation::Const
                        }],
                    );
                    if case == "premerge" {
                        snapshot.annotations.insert(
                            AnnotationTarget::Declaration(provider.origin.clone()),
                            vec![Annotation::Const, Annotation::PreserveResult],
                        );
                    }
                }
                "effective" => {
                    snapshot.annotations.insert(
                        AnnotationTarget::Declaration(provider.origin.clone()),
                        vec![if consumer {
                            Annotation::PreserveResult
                        } else {
                            Annotation::Const
                        }],
                    );
                }
                "projection" => {
                    snapshot.projected_type_names.insert(
                        provider.origin.clone(),
                        if consumer { "OtherStatus" } else { "Status" }.to_string(),
                    );
                }
                "owner-namespace" | "owner-remap" | "one-owner" | "excluded" => {
                    if case != "one-owner" || !consumer {
                        let mut owner = authority_root_owner(
                            &provider.origin.tu,
                            if case == "owner-namespace" && consumer {
                                "Other"
                            } else {
                                "Foundation"
                            },
                            &provider.spelling.file,
                        );
                        if case == "owner-remap" && consumer {
                            owner
                                .remaps
                                .insert("Status".to_string(), "OtherStatus".to_string());
                        }
                        if case == "excluded" && consumer {
                            owner.exclusions.insert("Status".to_string());
                        }
                        snapshot.root_owners.insert(provider.origin.clone(), owner);
                    }
                }
                "suppressed" if consumer => {
                    snapshot
                        .suppressed_type_origins
                        .insert(provider.origin.clone());
                }
                "suppressed" => {}
                _ => unreachable!(),
            }
        }
        let error = snapshot
            .emit_with_library("Test", "test.dll")
            .unwrap_err()
            .to_string();
        assert!(error.contains("method[0](Reset).result"), "{case}: {error}");
        assert!(error.contains("provider"), "{case}: {error}");
    }
}

#[test]
fn contextual_interface_proof_portable_namespace_authority() {
    let snapshot = portable_native_provider_snapshot(false);
    let sources: Vec<_> = snapshot
        .facts
        .iter()
        .filter(|fact| fact.name == "Status")
        .map(|fact| fact.spelling.clone())
        .collect();
    assert_eq!(sources.len(), 2);
    assert_ne!(sources[0], sources[1]);
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("PortableRepro", &references);
    options.library = Some("portable.dll");
    let mut policy = HeaderPartitionPolicy::new();
    for fact in snapshot.facts.iter().filter(|fact| fact.root) {
        policy.add_traversed_header_for_input(
            fact.origin.tu.clone(),
            fact.spelling.file.clone(),
            RootPartition::new("Portable", "PortableRepro"),
        );
    }
    let output = snapshot
        .into_header_partition_plan(
            &policy,
            &NamespaceAuthorities::new().with_exact("Status", "Foundation"),
        )
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let text = output.values().cloned().collect::<Vec<_>>().join("\n");
    for name in ["NativeContract", "Reset", "Use"] {
        assert!(text.contains(name), "{text}");
    }
}

#[test]
fn contextual_interface_proof_portable_scoped_metadata_emission() {
    let mut snapshot = portable_native_provider_snapshot(false);
    let other = extract(
        [Input::new(
            "other-scope.hpp",
            "typedef short ProjectedStatus;",
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    snapshot.facts.extend(other.facts);
    snapshot
        .native_typedef_types
        .extend(other.native_typedef_types);
    let providers: Vec<_> = snapshot
        .facts
        .iter()
        .filter(|fact| fact.name == "Status")
        .cloned()
        .collect();
    assert_eq!(providers.len(), 2);
    assert_ne!(providers[0].spelling, providers[1].spelling);
    for interface in snapshot.facts.iter().filter(|fact| {
        fact.name == "NativeContract" && matches!(fact.data, FactData::Interface { .. })
    }) {
        let target = AnnotationTarget::Method {
            declaration: interface.origin.clone(),
            index: 0,
        };
        snapshot
            .source_annotations
            .insert(target.clone(), vec![Annotation::PreserveResult]);
        snapshot
            .annotations
            .insert(target, vec![Annotation::PreserveResult]);
    }
    let mut policy = HeaderPartitionPolicy::new();
    for fact in snapshot
        .facts
        .iter()
        .filter(|fact| fact.root || fact.name == "Status")
    {
        let partition = if fact.name == "Status" {
            RootPartition::new("Status", "Foundation").with_remap("Status", "ProjectedStatus")
        } else if fact.origin.tu == "other-scope.hpp" {
            RootPartition::new("Other", "Other")
        } else {
            RootPartition::new("Portable", "PortableRepro")
        };
        policy.add_traversed_header_for_input(
            fact.origin.tu.clone(),
            fact.spelling.file.clone(),
            partition,
        );
    }
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("PortableRepro", &references);
    options.library = Some("portable.dll");
    let plan = snapshot
        .into_header_partition_plan(
            &policy,
            &NamespaceAuthorities::new().with_exact("Status", "Foundation"),
        )
        .unwrap();
    let (prepared, _, names) = plan
        .snapshot
        .clone()
        .into_partitioned_planning_snapshot(&options);
    for provider in &providers {
        let current = prepared
            .facts
            .iter()
            .find(|fact| fact.origin == provider.origin)
            .unwrap();
        assert_eq!(names.fact_name(current), "Status");
        assert!(current.name.starts_with("__partition_"), "{}", current.name);
        assert_eq!(current.spelling, provider.spelling);
    }
    let output = plan.emit_with_options(&options).unwrap();
    let text = output.values().cloned().collect::<Vec<_>>().join("\n");
    for expected in [
        "NativeContract",
        "fn Reset(",
        "#[preserve_sig]",
        "ProjectedStatus",
        "Use",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(
        output
            .keys()
            .any(|partition| partition.namespace == "Foundation")
    );
    assert!(
        output
            .keys()
            .any(|partition| partition.namespace == "Other")
    );
}

#[test]
fn contextual_interface_proof_portable_header_owner_binding_rejected() {
    for mismatch in ["input", "source"] {
        let mut snapshot = portable_native_provider_snapshot(false);
        let providers: Vec<_> = snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "Status")
            .cloned()
            .collect();
        for (index, provider) in providers.iter().enumerate() {
            let mut owner =
                authority_root_owner(&provider.origin.tu, "Foundation", &provider.spelling.file);
            if index == 1 {
                if mismatch == "input" {
                    owner.input = providers[0].origin.tu.clone();
                } else {
                    owner.root = providers[0].spelling.file.clone();
                }
            }
            snapshot.root_partitions.insert(
                (provider.origin.tu.clone(), provider.spelling.file.clone()),
                owner,
            );
        }
        let error = snapshot
            .emit_with_library("PortableRepro", "portable.dll")
            .unwrap_err()
            .to_string();
        for expected in ["method[0](Reset).result", "source ownership", "provider"] {
            assert!(error.contains(expected), "{mismatch}: {error}");
        }
        assert!(
            !error.contains("ambiguous type root `NativeBase`"),
            "{error}"
        );
    }
}

#[test]
fn contextual_interface_proof_scoped_authored_provider_policies() {
    let mut accepted_conflicts = Vec::new();
    for case in ["remap", "u32", "flags", "pointer-level", "exclusion"] {
        for conflicting in [false, true] {
            if case == "exclusion" && !conflicting {
                continue;
            }
            let mut snapshot = cross_tu_result_alias_snapshot("status_b.hpp", Some(Scalar::I32));
            let names = PlanningSourceNames::new(&snapshot);
            for provider in snapshot
                .facts
                .iter_mut()
                .filter(|fact| fact.name == "Status")
            {
                let mut owner = authority_root_owner(
                    &provider.origin.tu,
                    "Foundation",
                    &provider.spelling.file,
                );
                let contributes = !conflicting || provider.origin.tu == "consumer";
                if contributes {
                    match case {
                        "remap" => {
                            owner
                                .remaps
                                .insert("Status".to_string(), "ProjectedStatus".to_string());
                        }
                        "u32" => {
                            owner.u32_types.insert("Status".to_string());
                        }
                        "flags" => {
                            owner.flags.insert("Status".to_string());
                        }
                        "pointer-level" => {
                            owner
                                .preserved_auto_function_pointer_levels
                                .insert("Status".to_string());
                        }
                        "exclusion" => {
                            owner.exclusions.insert("Status".to_string());
                        }
                        _ => unreachable!(),
                    }
                }
                snapshot.root_owners.insert(provider.origin.clone(), owner);
                provider.name = format!("__partition_{}_Status", provider.origin.local);
            }
            let result = scoped_provider_interface_proof(&snapshot, &names);
            if conflicting {
                match result {
                    Ok(()) => accepted_conflicts.push(case),
                    Err(error) => {
                        assert!(error.contains("method[0](Reset).result"), "{case}: {error}");
                        assert!(error.contains("provider"), "{case}: {error}");
                    }
                }
            } else {
                result.unwrap_or_else(|error| panic!("{case}: {error}"));
            }
            println!("authored-provider-key case={case} conflicting={conflicting}");
        }
    }
    assert!(accepted_conflicts.is_empty(), "{accepted_conflicts:?}");
}

fn scoped_provider_interface_proof(
    snapshot: &Snapshot,
    names: &PlanningSourceNames,
) -> Result<(), String> {
    let declarations = DeclarationIndex::new(&snapshot.facts);
    let by_declaration = snapshot
        .facts
        .iter()
        .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
        .collect();
    let source_annotations = snapshot.source_annotation_signatures();
    let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
    let context = crate::com_pointer::Context {
        snapshot,
        declarations: &declarations,
        facts_by_declaration: &by_declaration,
        source_names: Some(names),
        excluded_types: None,
        source_annotations: &source_annotations,
        effective_annotations: &effective_annotations,
    };
    contextual_interface_matches(&context, &snapshot.facts[2], &snapshot.facts[0])
}

#[test]
fn contextual_interface_proof_provider_owner_input_binding() {
    let mut accepted_conflicts = Vec::new();
    for header_owned in [false, true] {
        for conflicting in [false, true] {
            let mut snapshot = cross_tu_result_alias_snapshot("status_b.hpp", Some(Scalar::I32));
            let providers: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| fact.name == "Status")
                .cloned()
                .collect();
            for provider in providers {
                let identity = format!("input-{}", provider.origin.tu);
                snapshot
                    .partition_inputs
                    .insert(provider.origin.tu.clone(), identity.clone());
                for (origin, owner) in &mut snapshot.root_owners {
                    if origin.tu == provider.origin.tu {
                        owner.input.clone_from(&identity);
                    }
                }
                let mut owner =
                    authority_root_owner(&identity, "Foundation", &provider.spelling.file);
                if conflicting && provider.origin.tu == "consumer" {
                    owner.input = "unrelated-input".to_string();
                }
                if header_owned {
                    snapshot.root_partitions.insert(
                        (provider.origin.tu.clone(), provider.spelling.file.clone()),
                        owner,
                    );
                } else {
                    snapshot.root_owners.insert(provider.origin.clone(), owner);
                }
            }
            let names = PlanningSourceNames::new(&snapshot);
            let result = scoped_provider_interface_proof(&snapshot, &names);
            if conflicting {
                match result {
                    Ok(()) => accepted_conflicts.push(header_owned),
                    Err(error) => {
                        assert!(error.contains("method[0](Reset).result"), "{error}");
                        assert!(error.contains("source ownership"), "{error}");
                    }
                }
            } else {
                result.unwrap();
            }
        }
    }
    assert!(accepted_conflicts.is_empty(), "{accepted_conflicts:?}");
}

#[test]
fn contextual_interface_proof_inherited_exception_evidence() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("inherited-contract.hpp");
    let path = path.to_str().unwrap();
    for (left, right, supported) in [
        ("noexcept", "noexcept", true),
        ("noexcept", "", false),
        ("noexcept(true)", "noexcept(true)", false),
    ] {
        let snapshot = extract([
            Input::new("inherited-a.cpp", format!(
                "#define BASE_VALUE long\n#define OWN_EXCEPTION {left}\n#include {path:?}\n"
            )).with_roots([path]),
            Input::new("inherited-b.cpp", format!(
                "#define BASE_VALUE long\n#define OWN_EXCEPTION {right}\n#include {path:?}\n"
            )).with_roots([path]),
        ], &["-x", "c++", "--target=x86_64-pc-windows-msvc", "-fms-extensions"]).unwrap();
        let result = inherited_interface_proof(&snapshot);
        if supported {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert!(error.contains("exception specification"), "{error}");
        }
    }
}

#[test]
fn contextual_interface_proof_inherited_owner_context() {
    for case in [
        "matching",
        "dependency",
        "input",
        "source",
        "namespace",
        "policy",
    ] {
        let mut snapshot = inherited_interface_snapshot(false);
        let bases: Vec<_> = snapshot
            .facts
            .iter()
            .filter(|fact| {
                fact.name == "GenericBase" && matches!(fact.data, FactData::Interface { .. })
            })
            .cloned()
            .collect();
        for (index, base) in bases.iter().enumerate() {
            if case == "dependency" && index == 1 {
                continue;
            }
            let mut owner = authority_root_owner(&base.origin.tu, "BaseSpace", &base.spelling.file);
            if case != "dependency" {
                snapshot.root_partitions.insert(
                    (base.origin.tu.clone(), base.spelling.file.clone()),
                    owner.clone(),
                );
            }
            if index == 1 {
                match case {
                    "input" => owner.input = bases[0].origin.tu.clone(),
                    "source" => owner.root = "other.hpp".to_string(),
                    "namespace" => owner.namespace = "Other".to_string(),
                    "policy" => {
                        owner
                            .remaps
                            .insert("GenericBase".to_string(), "Other".to_string());
                    }
                    _ => {}
                }
                if case != "matching" {
                    snapshot.root_owners.insert(base.origin.clone(), owner);
                }
            } else {
                snapshot.root_owners.insert(base.origin.clone(), owner);
            }
        }
        let result = inherited_interface_proof(&snapshot);
        if matches!(case, "matching" | "dependency") {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert!(error.contains("base `GenericBase`"), "{case}: {error}");
            assert!(error.contains("owner"), "{case}: {error}");
            println!("inherited-owner case={case} rejection={error}");
        }
        println!("inherited-owner case={case} assertions=passed");
    }
}

#[test]
fn contextual_interface_proof_inherited_native_rank() {
    let snapshot = inherited_interface_snapshot(true);
    let bases: Vec<_> = snapshot
        .facts
        .iter()
        .filter(|fact| {
            fact.name == "GenericBase" && matches!(fact.data, FactData::Interface { .. })
        })
        .collect();
    assert_eq!(bases[0].spelling, bases[1].spelling);
    assert_eq!(bases[0].data, bases[1].data);
    let error = inherited_interface_proof(&snapshot).unwrap_err();
    assert!(error.contains("base `GenericBase`"), "{error}");
    assert!(
        error.contains("canonical native evidence differs"),
        "{error}"
    );
}

#[test]
fn contextual_interface_proof_inherited_order_and_context() {
    let mut accepted = Vec::new();
    for case in [
        "order",
        "convention",
        "source",
        "effective",
        "premerge",
        "missing",
        "ambiguous",
        "cycle",
    ] {
        let mut snapshot = inherited_interface_snapshot(false);
        let base = snapshot
            .facts
            .iter()
            .find(|fact| {
                fact.origin.tu == "inherited-b.cpp"
                    && fact.name == "GenericBase"
                    && matches!(fact.data, FactData::Interface { .. })
            })
            .unwrap()
            .clone();
        let derived = snapshot
            .facts
            .iter()
            .find(|fact| {
                fact.origin.tu == "inherited-b.cpp"
                    && fact.name == "GenericDerived"
                    && matches!(fact.data, FactData::Interface { .. })
            })
            .unwrap()
            .clone();
        match case {
            "order" => Arc::make_mut(
                snapshot
                    .native_com_contracts
                    .get_mut(&derived.origin)
                    .unwrap(),
            )
            .slots
            .swap(0, 1),
            "convention" => {
                Arc::make_mut(
                    snapshot
                        .native_com_contracts
                        .get_mut(&derived.origin)
                        .unwrap(),
                )
                .slots[0]
                    .convention = clang_sys::CXCallingConv_X86FastCall;
            }
            "source" | "premerge" => {
                snapshot.source_annotations.insert(
                    AnnotationTarget::Method {
                        declaration: base.origin.clone(),
                        index: 0,
                    },
                    vec![Annotation::PreserveResult],
                );
                if case == "premerge" {
                    for fact in snapshot.facts.iter().filter(|fact| {
                        fact.name == "GenericBase"
                            && matches!(fact.data, FactData::Interface { .. })
                    }) {
                        snapshot.annotations.insert(
                            AnnotationTarget::Method {
                                declaration: fact.origin.clone(),
                                index: 0,
                            },
                            vec![Annotation::PreserveResult],
                        );
                    }
                }
            }
            "effective" => {
                snapshot.annotations.insert(
                    AnnotationTarget::Method {
                        declaration: base.origin.clone(),
                        index: 0,
                    },
                    vec![Annotation::PreserveResult],
                );
            }
            "missing" => snapshot.facts.retain(|fact| fact.origin != base.origin),
            "ambiguous" => {
                let mut other = base.clone();
                other.origin.local += 10000;
                snapshot.facts.push(other);
            }
            "cycle" => {
                let FactData::Interface { base: target, .. } = &mut snapshot
                    .facts
                    .iter_mut()
                    .find(|fact| fact.origin == base.origin)
                    .unwrap()
                    .data
                else {
                    unreachable!()
                };
                *target = Some(named(&derived));
            }
            _ => unreachable!(),
        }
        match inherited_interface_proof(&snapshot) {
            Ok(()) => accepted.push(case),
            Err(error) => println!("inherited case={case} rejected={error}"),
        }
    }
    assert!(accepted.is_empty(), "{accepted:?}");
}

#[test]
fn contextual_interface_proof_opaque_interface_pointer_provider() {
    helpers::ensure_libclang();
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("interface-pointer-contract.hpp");
    let header = header.to_str().unwrap();
    let original = extract(
        [
            Input::new("pointer-provider-a.cpp", format!("#include {header:?}\n"))
                .with_roots([header]),
            Input::new("pointer-provider-b.cpp", format!("#include {header:?}\n"))
                .with_roots([header]),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    for case in [
        "matching",
        "provider-namespace",
        "provider-source",
        "missing-contract",
        "missing-parameter",
        "missing-owner",
        "param-owner",
        "base-cycle",
        "erased-provider",
        "exception",
        "noninterface",
        "byvalue",
    ] {
        let mut snapshot = original.clone();
        let provider = snapshot
            .facts
            .iter()
            .find(|fact| {
                fact.origin.tu == "pointer-provider-b.cpp" && fact.name == "NativeProvider"
            })
            .unwrap()
            .origin
            .clone();
        match case {
            "provider-namespace" => {
                snapshot
                    .fact_namespace_authorities
                    .insert(provider.clone(), "Wrong.Provider".into());
            }
            "provider-source" => {
                let contract =
                    Arc::make_mut(snapshot.native_com_contracts.get_mut(&provider).unwrap());
                let NativeTypeLayer::Indirect(target) =
                    &mut contract.slots[0].native_params[0].as_mut().unwrap().layer
                else {
                    panic!()
                };
                let NativeTypeLayer::Interface(declaration) = &mut target.layer else {
                    panic!()
                };
                declaration.spelling.offset += 1;
            }
            "missing-contract" => {
                snapshot.native_com_contracts.remove(&provider);
            }
            "base-cycle" => {
                for fact in snapshot
                    .facts
                    .iter_mut()
                    .filter(|fact| fact.name == "NativeProvider")
                {
                    let target = named(fact);
                    let FactData::Interface { base, .. } = &mut fact.data else {
                        panic!()
                    };
                    *base = Some(target);
                }
            }
            "missing-parameter" | "missing-owner" | "param-owner" | "exception" => {
                let contract =
                    Arc::make_mut(snapshot.native_com_contracts.get_mut(&provider).unwrap());
                match case {
                    "missing-parameter" => contract.slots[0].native_params[0] = None,
                    "missing-owner" => contract.slots[0].native_owner = None,
                    "param-owner" => {
                        let witness =
                            Arc::make_mut(contract.slots[0].native_owner.as_mut().unwrap());
                        witness.params[0].parent.spelling.as_mut().unwrap().offset += 1;
                    }
                    "exception" => contract.slots[0].native_exception = None,
                    _ => unreachable!(),
                }
            }
            _ => {}
        }
        let declarations = DeclarationIndex::new(&snapshot.facts);
        let by_declaration = snapshot
            .facts
            .iter()
            .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
            .collect();
        let source_annotations = snapshot.source_annotation_signatures();
        let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
        let context = crate::com_pointer::Context {
            snapshot: &snapshot,
            declarations: &declarations,
            facts_by_declaration: &by_declaration,
            source_names: None,
            excluded_types: None,
            source_annotations: &source_annotations,
            effective_annotations: &effective_annotations,
        };
        let fact = |tu: &str, name: &str| {
            snapshot
                .facts
                .iter()
                .find(|fact| fact.origin.tu == tu && fact.name == name)
                .unwrap()
        };
        if case == "noninterface" {
            assert!(
                snapshot.native_typedef_types
                    [&fact("pointer-provider-a.cpp", "NonInterfacePointer").origin]
                    .is_none()
            );
            continue;
        }
        if case == "byvalue" {
            let left = fact("pointer-provider-a.cpp", "NativeProviderValue");
            let FactData::Typedef { target } = &left.data else {
                panic!()
            };
            let native = snapshot.native_typedef_types[&left.origin]
                .as_ref()
                .unwrap();
            let error = context
                .canonical_types_equal(
                    target,
                    &left.origin.tu,
                    target,
                    &left.origin.tu,
                    (native, native),
                )
                .unwrap_err();
            assert!(error.contains("unsupported canonical native"), "{error}");
            continue;
        }
        if case == "erased-provider" {
            let left = fact("pointer-provider-a.cpp", "NativeProvider");
            let native = snapshot.native_com_contracts[&left.origin].slots[0].native_params[0]
                .as_ref()
                .unwrap();
            let erased = TypeRef::Pointer {
                mutable: true,
                target: Box::new(TypeRef::Object),
            };
            let error = context
                .canonical_types_equal(
                    &erased,
                    &left.origin.tu,
                    &erased,
                    &left.origin.tu,
                    (native, native),
                )
                .unwrap_err();
            assert!(error.contains("exact named interface provider"), "{error}");
            continue;
        }
        let result = contextual_interface_matches(
            &context,
            fact("pointer-provider-a.cpp", "NativeConsumer"),
            fact("pointer-provider-b.cpp", "NativeConsumer"),
        );
        if case == "matching" {
            result.unwrap();
            let provider = &snapshot.native_com_contracts
                [&fact("pointer-provider-a.cpp", "NativeProvider").origin];
            let consumer = &snapshot.native_com_contracts
                [&fact("pointer-provider-a.cpp", "NativeConsumer").origin];
            assert!(Arc::ptr_eq(
                provider.slots[0].native_owner.as_ref().unwrap(),
                consumer.slots[0].native_owner.as_ref().unwrap(),
            ));
        } else {
            let error = result.unwrap_err();
            let expected = match case {
                "provider-namespace" => "namespace contract differs",
                "provider-source" => "native nominal binding",
                "missing-contract" => "missing native interface contract",
                "missing-parameter" => "missing canonical native evidence",
                "missing-owner" => "missing native callable owner witness",
                "param-owner" => "native ParmDecl witness",
                "base-cycle" => "cyclic exact interface bases",
                "exception" => "missing native exception specification",
                _ => unreachable!(),
            };
            assert!(error.contains(expected), "{case}: {error}");
        }
    }
}

#[test]
fn contextual_interface_proof_inherited_matching_emits() {
    let snapshot = inherited_interface_snapshot(false);
    inherited_interface_proof(&snapshot).unwrap();
    let output = snapshot
        .emit_with_library("Inherited", "inherited.dll")
        .unwrap();
    for name in ["GenericBase", "GenericDerived", "First", "Second", "Own"] {
        assert!(output.contains(name), "{output}");
    }
}

fn inherited_interface_snapshot(different: bool) -> Snapshot {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("inherited-contract.hpp");
    let path = path.to_str().unwrap();
    extract(
        [
            Input::new(
                "inherited-a.cpp",
                format!("#define BASE_VALUE long\n#include {path:?}\n"),
            )
            .with_roots([path]),
            Input::new(
                "inherited-b.cpp",
                format!(
                    "#define BASE_VALUE {}\n#include {path:?}\n",
                    if different { "int" } else { "long" }
                ),
            )
            .with_roots([path]),
        ],
        &[
            "-x",
            "c++",
            "--target=x86_64-pc-windows-msvc",
            "-fms-extensions",
        ],
    )
    .unwrap()
}

fn inherited_interface_proof(snapshot: &Snapshot) -> Result<(), String> {
    let declarations = DeclarationIndex::new(&snapshot.facts);
    let by_declaration = snapshot
        .facts
        .iter()
        .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
        .collect();
    let source_annotations = snapshot.source_annotation_signatures();
    let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
    let context = crate::com_pointer::Context {
        snapshot,
        declarations: &declarations,
        facts_by_declaration: &by_declaration,
        source_names: None,
        excluded_types: None,
        source_annotations: &source_annotations,
        effective_annotations: &effective_annotations,
    };
    let derived: Vec<_> = snapshot
        .facts
        .iter()
        .filter(|fact| {
            fact.name == "GenericDerived" && matches!(fact.data, FactData::Interface { .. })
        })
        .collect();
    contextual_interface_matches(&context, derived[0], derived[1])
}

#[test]
fn contextual_interface_proof_native_pointer_layers() {
    let snapshot = extract(
        [Input::new(
            "native-pointer-proof.hpp",
            "typedef long L; typedef long Other; typedef int I;\n\
         typedef L* LP; typedef Other* OP; typedef I* IP;\n\
         typedef const void* CV; typedef void* V; typedef volatile void* VV;\n\
         typedef void** Nested; typedef void* __restrict Restricted;\n\
         typedef void* __ptr32 P32; typedef void* __ptr64 P64;\n\
         typedef int __attribute__((address_space(1)))* AS;\n",
        )],
        &[
            "-x",
            "c++",
            "--target=x86_64-pc-windows-msvc",
            "-fms-extensions",
        ],
    )
    .unwrap();
    let declarations = DeclarationIndex::new(&snapshot.facts);
    let by_declaration = snapshot
        .facts
        .iter()
        .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
        .collect();
    let source_annotations = snapshot.source_annotation_signatures();
    let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
    let context = crate::com_pointer::Context {
        snapshot: &snapshot,
        declarations: &declarations,
        facts_by_declaration: &by_declaration,
        source_names: None,
        excluded_types: None,
        source_annotations: &source_annotations,
        effective_annotations: &effective_annotations,
    };
    let fact = |name: &str| {
        snapshot
            .facts
            .iter()
            .find(|fact| fact.name == name)
            .unwrap()
    };
    let native = |name: &str| {
        snapshot.native_typedef_types[&fact(name).origin]
            .as_ref()
            .unwrap()
    };
    let compare = |a: &str, b: &str| {
        let (FactData::Typedef { target: at }, FactData::Typedef { target: bt }) =
            (&fact(a).data, &fact(b).data)
        else {
            panic!()
        };
        context.canonical_types_equal(
            at,
            &fact(a).origin.tu,
            bt,
            &fact(b).origin.tu,
            (native(a), native(b)),
        )
    };
    for (a, b) in [
        ("LP", "OP"),
        ("V", "V"),
        ("CV", "CV"),
        ("Nested", "Nested"),
        ("Restricted", "Restricted"),
    ] {
        compare(a, b).unwrap();
    }
    for (a, b) in [("LP", "IP"), ("CV", "V"), ("VV", "V"), ("Restricted", "V")] {
        assert!(compare(a, b).is_err(), "{a}/{b}");
    }
    for name in ["P32", "P64", "AS"] {
        assert!(
            snapshot.native_typedef_types[&fact(name).origin].is_none(),
            "{name}"
        );
    }
    assert!(matches!(fact("P32").data, FactData::Unsupported { .. }));
    assert!(matches!(fact("AS").data, FactData::Unsupported { .. }));
    assert_eq!(native("V").size, 8);
    assert_eq!(native("V").address_space, 0);
    let mut address_space = native("V").clone();
    let NativeTypeLayer::Indirect(target) = &mut address_space.layer else {
        panic!()
    };
    target.address_space = 1;
    let FactData::Typedef { target: projected } = &fact("V").data else {
        panic!()
    };
    assert!(
        context
            .canonical_types_equal(
                projected,
                &fact("V").origin.tu,
                projected,
                &fact("V").origin.tu,
                (&address_space, &address_space)
            )
            .is_err()
    );
}

#[test]
fn native_evidence_retains_primitive_rank_and_provider_context() {
    use clang_sys::*;
    let snapshot = extract(
        [
            Input::new(
                "native-kinds.hpp",
                "typedef int I;\ntypedef long L;\ntypedef unsigned int U;\n\
                 typedef unsigned long UL;\ntypedef double D;\ntypedef long double LD;\n",
            ),
            Input::new("native-other-provider.hpp", "typedef long OtherLong;\n"),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let fact = |name: &str| {
        snapshot
            .facts
            .iter()
            .find(|fact| fact.name == name)
            .unwrap()
    };
    let native = |name: &str| {
        snapshot.native_typedef_types[&fact(name).origin]
            .as_ref()
            .unwrap()
    };
    for (name, kind, scalar) in [
        ("I", CXType_Int, Scalar::I32),
        ("L", CXType_Long, Scalar::I32),
        ("U", CXType_UInt, Scalar::U32),
        ("UL", CXType_ULong, Scalar::U32),
        ("D", CXType_Double, Scalar::F64),
        ("LD", CXType_LongDouble, Scalar::F64),
    ] {
        assert_eq!(native(name).kind, kind);
        assert_eq!(native(name).layer, NativeTypeLayer::Primitive);
        assert_eq!(
            fact(name).data,
            FactData::Typedef {
                target: TypeRef::Scalar(scalar)
            }
        );
    }
    for (left, right) in [("I", "L"), ("U", "UL"), ("D", "LD")] {
        assert_ne!(native(left), native(right));
    }
    assert_eq!(native("L"), native("OtherLong"));
    assert_ne!(fact("L").spelling, fact("OtherLong").spelling);
    assert_ne!(fact("L").origin.tu, fact("OtherLong").origin.tu);
    let mut without = snapshot.clone();
    without.native_typedef_types.clear();
    assert_eq!(snapshot, without);
    std::thread::spawn(move || {
        assert!(!snapshot.native_typedef_types.is_empty());
    })
    .join()
    .unwrap();
}

#[test]
fn native_evidence_retains_layers_and_marks_nominal_incomplete_absent() {
    use clang_sys::*;
    let snapshot = extract(
        [Input::new(
            "native-layers.hpp",
            "typedef const volatile int CV;\ntypedef CV* Pointer;\n\
             typedef int* const ConstPointer;\ntypedef int* __restrict Restricted;\n\
             typedef volatile long& LRef;\ntypedef const long&& RRef;\n\
             typedef const int Array[3];\n\
             typedef int (*Callback)(const volatile long*, unsigned long);\n\
             struct R { int value; };\ntypedef R RecordAlias;\n\
             enum E { Value };\ntypedef E EnumAlias;\ntypedef int Incomplete[];\n",
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let evidence = |name: &str| {
        let fact = snapshot
            .facts
            .iter()
            .find(|fact| fact.name == name)
            .unwrap();
        &snapshot.native_typedef_types[&fact.origin]
    };
    let native = |name: &str| evidence(name).as_ref().unwrap();
    assert!(native("CV").is_const && native("CV").is_volatile);
    let NativeTypeLayer::Indirect(target) = &native("Pointer").layer else {
        panic!()
    };
    assert_eq!(target.as_ref(), native("CV"));
    assert_eq!(native("Pointer").kind, CXType_Pointer);
    assert!(native("ConstPointer").is_const);
    assert!(native("Restricted").is_restrict);
    for (name, kind, constant, volatile) in [
        ("LRef", CXType_LValueReference, false, true),
        ("RRef", CXType_RValueReference, true, false),
    ] {
        assert_eq!(native(name).kind, kind);
        let NativeTypeLayer::Indirect(target) = &native(name).layer else {
            panic!()
        };
        assert_eq!(target.kind, CXType_Long);
        assert_eq!((target.is_const, target.is_volatile), (constant, volatile));
    }
    let NativeTypeLayer::Array { target, len } = &native("Array").layer else {
        panic!()
    };
    assert_eq!(*len, 3);
    assert_eq!(target.kind, CXType_Int);
    assert!(native("Array").is_const);
    let NativeTypeLayer::Indirect(function) = &native("Callback").layer else {
        panic!()
    };
    let NativeTypeLayer::Function {
        convention,
        params,
        result,
        variadic,
        ..
    } = &function.layer
    else {
        panic!()
    };
    assert_eq!(*convention, CXCallingConv_C);
    assert!(!variadic);
    assert_eq!(result.kind, CXType_Int);
    assert_eq!(params[1].kind, CXType_ULong);
    let NativeTypeLayer::Indirect(target) = &params[0].layer else {
        panic!()
    };
    assert_eq!(target.kind, CXType_Long);
    assert!(target.is_const && target.is_volatile);
    let NativeTypeLayer::Record(record) = &native("RecordAlias").layer else {
        panic!()
    };
    assert_eq!(record.declaration.name, "R");
    assert_eq!(record.declaration.kind, CXCursor_StructDecl);
    assert_eq!(record.fields.len(), 1);
    assert_eq!(record.fields[0].declaration.name, "value");
    assert_eq!(record.fields[0].ty.kind, CXType_Int);
    let NativeTypeLayer::Enum(enumeration) = &native("EnumAlias").layer else {
        panic!()
    };
    assert_eq!(enumeration.declaration.name, "E");
    assert_eq!(enumeration.underlying.kind, CXType_Int);
    assert!(!enumeration.scoped);
    assert_eq!(enumeration.variants[0].declaration.name, "Value");
    assert_eq!(enumeration.variants[0].signed_value, 0);
    assert_eq!(evidence("Incomplete"), &None);
}

#[test]
fn native_evidence_nominal_declarations_members_and_absence() {
    use clang_sys::*;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("nominal-evidence.hpp");
    let path = path.to_str().unwrap();
    for (right, value) in [("long", 2), ("int", 3)] {
        let snapshot = extract(
            [
                Input::new("nominal-a.cpp", format!(
                    "#define MEMBER_TYPE long\n#define ENUM_VALUE 2\n#include {path:?}\n"
                )).with_roots([path]),
                Input::new("nominal-b.cpp", format!(
                    "#define MEMBER_TYPE {right}\n#define ENUM_VALUE {value}\n#include {path:?}\n"
                )).with_roots([path]),
            ],
            &["-x", "c++", "--target=x86_64-pc-windows-msvc", "-fms-extensions"],
        ).unwrap();
        let fact = |tu: &str, name: &str| {
            let observations: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| fact.origin.tu == tu && fact.name == name)
                .collect();
            assert_eq!(observations.len(), 1, "{tu}/{name}");
            let fact = observations[0];
            let parents: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|parent| Some(&parent.origin) == fact.parent.as_ref())
                .collect();
            assert_eq!(parents.len(), 1, "{tu}/{name} namespace parent");
            let parent = parents[0];
            assert_eq!(parent.origin.tu, tu);
            assert_eq!(parent.kind, FactKind::Namespace);
            assert_eq!(parent.name, "Generic");
            assert_eq!(parent.spelling.file, fact.spelling.file);
            fact
        };
        let native = |tu: &str, name: &str| {
            snapshot.native_typedef_types[&fact(tu, name).origin]
                .as_ref()
                .unwrap()
        };
        for name in ["RecordAlias", "UnionAlias", "EnumAlias", "ScopedAlias"] {
            let a = native("nominal-a.cpp", name);
            let b = native("nominal-b.cpp", name);
            let declaration = |ty: &NativeType| match &ty.layer {
                NativeTypeLayer::Record(record) => record.declaration.clone(),
                NativeTypeLayer::Enum(enumeration) => enumeration.declaration.clone(),
                _ => panic!(),
            };
            let (ad, bd) = (declaration(a), declaration(b));
            assert_eq!(ad.spelling, bd.spelling);
            assert_eq!(ad.expansion, bd.expansion);
            assert_eq!(ad.parent, bd.parent);
            assert_eq!(ad.parent.kind, CXCursor_Namespace);
            assert_eq!(ad.parent.name, "Generic");
            assert_eq!(ad.tu, "nominal-a.cpp");
            assert_eq!(bd.tu, "nominal-b.cpp");
            assert_eq!(ad.spelling.file, fact("nominal-a.cpp", name).spelling.file);
            assert_eq!(
                fact("nominal-a.cpp", name).data,
                fact("nominal-b.cpp", name).data
            );
        }
        let NativeTypeLayer::Record(a) = &native("nominal-a.cpp", "RecordAlias").layer else {
            panic!()
        };
        let NativeTypeLayer::Record(b) = &native("nominal-b.cpp", "RecordAlias").layer else {
            panic!()
        };
        assert_eq!(a.declaration.kind, CXCursor_StructDecl);
        assert_eq!(a.align, b.align);
        assert_eq!(
            a.fields.iter().map(|f| f.ordinal).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(a.fields[0].offset, b.fields[0].offset);
        assert_eq!(a.fields[0].ty.kind, CXType_Long);
        assert_eq!(
            b.fields[0].ty.kind,
            if right == "long" {
                CXType_Long
            } else {
                CXType_Int
            }
        );
        assert_eq!(
            a.fields[0].declaration.parent.spelling.as_ref(),
            Some(&a.declaration.spelling)
        );
        assert_eq!(a.fields[1].bit_width, Some(3));
        let NativeTypeLayer::Array { target, len } = &a.fields[2].ty.layer else {
            panic!()
        };
        assert_eq!(*len, 2);
        assert_eq!(target.kind, CXType_Long);
        assert!(a.fields[2].ty.is_const && a.fields[2].ty.is_volatile);
        println!(
            "nominal profile={right}/{value} array_cv={}/{} element_cv={}/{}",
            a.fields[2].ty.is_const,
            a.fields[2].ty.is_volatile,
            target.is_const,
            target.is_volatile
        );
        let NativeTypeLayer::Record(parts) = &native("nominal-a.cpp", "UnionAlias").layer else {
            panic!()
        };
        assert_eq!(parts.declaration.kind, CXCursor_UnionDecl);
        assert_eq!(parts.fields.len(), 2);
        assert_eq!(parts.fields[1].offset, 0);
        let NativeTypeLayer::Record(anonymous) = &parts.fields[1].ty.layer else {
            panic!()
        };
        assert!(anonymous.declaration.anonymous);
        assert_eq!(
            anonymous
                .fields
                .iter()
                .map(|f| f.declaration.name.as_str())
                .collect::<Vec<_>>(),
            ["low", "high"]
        );
        assert_eq!(anonymous.fields[0].ty.kind, CXType_Int);
        assert_eq!(anonymous.fields[1].ty.kind, CXType_Long);
        let NativeTypeLayer::Enum(a) = &native("nominal-a.cpp", "EnumAlias").layer else {
            panic!()
        };
        let NativeTypeLayer::Enum(b) = &native("nominal-b.cpp", "EnumAlias").layer else {
            panic!()
        };
        assert_eq!(a.underlying.kind, CXType_Long);
        assert_eq!(
            b.underlying.kind,
            if right == "long" {
                CXType_Long
            } else {
                CXType_Int
            }
        );
        assert!(!a.scoped);
        assert_eq!(
            a.variants
                .iter()
                .map(|v| (v.declaration.name.as_str(), v.signed_value))
                .collect::<Vec<_>>(),
            [("First", -1), ("Second", 2)]
        );
        assert_eq!(a.variants[1].unsigned_value, 2);
        assert_eq!(b.variants[1].signed_value, value);
        let NativeTypeLayer::Enum(scoped) = &native("nominal-a.cpp", "ScopedAlias").layer else {
            panic!()
        };
        assert!(scoped.scoped);
        for name in ["IncompleteAlias", "RecursiveAlias", "NonPodAlias"] {
            assert_eq!(
                snapshot.native_typedef_types[&fact("nominal-a.cpp", name).origin],
                None,
                "{name}"
            );
        }
        let attributed_fact = fact("nominal-a.cpp", "AttributedAlias");
        let attributed = snapshot.native_typedef_types[&attributed_fact.origin]
            .as_ref()
            .unwrap();
        let NativeTypeLayer::Record(record) = &attributed.layer else {
            panic!()
        };
        assert!(!record.aggregate_abi_proved);
        assert_eq!(record.declaration.tu, attributed_fact.origin.tu);
        assert_eq!(record.fields.len(), 1);
        let FactData::Typedef { target } = &attributed_fact.data else {
            panic!()
        };
        let declarations = DeclarationIndex::new(&snapshot.facts);
        let by_declaration = snapshot
            .facts
            .iter()
            .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
            .collect();
        let source_annotations = snapshot.source_annotation_signatures();
        let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
        let context = crate::com_pointer::Context {
            snapshot: &snapshot,
            declarations: &declarations,
            facts_by_declaration: &by_declaration,
            source_names: None,
            excluded_types: None,
            source_annotations: &source_annotations,
            effective_annotations: &effective_annotations,
        };
        assert!(
            context
                .canonical_types_equal(
                    target,
                    &attributed_fact.origin.tu,
                    target,
                    &attributed_fact.origin.tu,
                    (attributed, attributed)
                )
                .unwrap_err()
                .contains("value-ABI")
        );
        for tu in ["nominal-a.cpp", "nominal-b.cpp"] {
            let slot = &snapshot.native_com_contracts[&fact(tu, "Interface").origin].slots[0];
            assert_eq!(
                slot.native_result.as_ref().unwrap(),
                native(tu, "RecordAlias")
            );
            assert_eq!(
                slot.native_params[0].as_ref().unwrap(),
                native(tu, "UnionAlias")
            );
            assert_eq!(
                slot.native_params[1].as_ref().unwrap(),
                native(tu, "EnumAlias")
            );
            let mut without = slot.clone();
            without.native_result = None;
            without.native_params.clear();
            assert_eq!(slot, &without);
        }
        let mut without = snapshot.clone();
        without.native_typedef_types.clear();
        assert_eq!(snapshot, without);
        println!(
            "nominal profile={right}/{value} branches=source-parent,record-rank,bitfield,array,\
             anonymous-union,enum-rank-values-scoped,unsupported-none,nominal-com,legacy-eq \
             assertions=passed"
        );
    }
}

#[test]
fn contextual_interface_proof_nominal_native_emission() {
    fn nominal_contract_snapshot(field: &str, enumeration: &str) -> Snapshot {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("tests")
            .join("producer_corrections")
            .join("nominal-contract.hpp");
        let path = path.to_str().unwrap();
        extract(
        [
            Input::new("nominal-proof-a.cpp", format!(
                "#define FIELD_KIND long\n#define ENUM_KIND long\n#include {path:?}\n"
            )).with_roots([path]),
            Input::new("nominal-proof-b.cpp", format!(
                "#define FIELD_KIND {field}\n#define ENUM_KIND {enumeration}\n#include {path:?}\n"
            )).with_roots([path]),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc", "-fms-extensions"],
    ).unwrap()
    }

    let snapshot = nominal_contract_snapshot("long", "long");
    let output = snapshot.emit_with_library("NominalProof", "nominal.dll");
    if let Err(error) = &output {
        println!("nominal expected-valid rejection={error}");
    }
    let output = output.unwrap();
    for expected in [
        "#[win32]\nmod NominalProof {",
        "#[repr(i32)]\n    enum EnumAlias {\n        First = 1,\n        Second = 2,\n    }",
        "type Leaf = i32;",
        "struct PlainAlias {\n        value: Leaf,\n    }",
        "union PartsAlias {\n        whole: u64,\n        Anonymous: struct {\n            low: i32,\n            high: Leaf,\n        },\n    }",
        "struct ScopedAlias {\n        value: i32,\n    }",
        "#[guid(0xa7a60184_8028_413a_bde9_4b724d5945cd)]",
        "interface NominalContract {\n        fn Transform(&self, parts: PartsAlias, kind: EnumAlias, scoped: ScopedAlias) -> PlainAlias;\n    }",
    ] {
        assert!(output.contains(expected), "{expected}: {output}");
    }
    println!("nominal emitted RDL:\n{output}");
    println!(
        "nominal emission branches=record,anonymous-union,enum,namespace-record assertions=passed"
    );
}

#[test]
fn native_evidence_com_capture_preserves_matching_and_nominal_addresses() {
    use clang_sys::*;
    for target in ["x86_64-pc-windows-msvc", "i686-pc-windows-msvc"] {
        let snapshot = extract(
            [
                Input::new(
                    "native-com-a.hpp",
                    "typedef long Result;\nstruct __declspec(uuid(\"11111111-1111-1111-1111-111111111111\")) \
                     A { virtual Result __stdcall Reset(const volatile int* value) = 0; };\n",
                ),
                Input::new(
                    "native-com-b.hpp",
                    "typedef long Result;\nstruct __declspec(uuid(\"22222222-2222-2222-2222-222222222222\")) \
                     B { virtual Result __stdcall Reset(const volatile int* value) = 0; };\n",
                ),
            ],
            &["-x", "c++", &format!("--target={target}")],
        )
        .unwrap();
        let slot = |name: &str| {
            let fact = snapshot
                .facts
                .iter()
                .find(|fact| fact.name == name)
                .unwrap();
            &snapshot.native_com_contracts[&fact.origin].slots[0]
        };
        let (a, b) = (slot("A"), slot("B"));
        assert_eq!(a.native_result, b.native_result);
        assert_eq!(a.native_result.as_ref().unwrap().kind, CXType_Long);
        assert_ne!(a.result, b.result);
        assert_eq!(a.native_params, b.native_params);
        let param = a.native_params[0].as_ref().unwrap();
        let NativeTypeLayer::Indirect(pointee) = &param.layer else {
            panic!()
        };
        assert!(pointee.is_const && pointee.is_volatile);
        assert_eq!(pointee.kind, CXType_Int);
        if target.starts_with("i686") {
            assert_eq!(a.convention, CXCallingConv_X86StdCall);
        }
        let mut without = a.clone();
        without.native_params.clear();
        without.native_result = None;
        assert_eq!(a, &without);
    }
}

#[test]
fn contextual_interface_proof_nominal_declaration_binding_rejects_corruption() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("nominal-contract.hpp");
    let path = path.to_str().unwrap();
    for case in ["tu", "source", "kind", "parent", "missing", "ambiguous"] {
        let mut snapshot = extract(
            [
                Input::new(
                    "nominal-proof-a.cpp",
                    format!("#define FIELD_KIND long\n#define ENUM_KIND long\n#include {path:?}\n"),
                )
                .with_roots([path]),
                Input::new(
                    "nominal-proof-b.cpp",
                    format!("#define FIELD_KIND long\n#define ENUM_KIND long\n#include {path:?}\n"),
                )
                .with_roots([path]),
            ],
            &[
                "-x",
                "c++",
                "--target=x86_64-pc-windows-msvc",
                "-fms-extensions",
            ],
        )
        .unwrap();
        let interface = snapshot
            .facts
            .iter()
            .find(|fact| fact.origin.tu == "nominal-proof-b.cpp" && fact.name == "NominalContract")
            .unwrap()
            .origin
            .clone();
        if matches!(case, "missing" | "ambiguous") {
            let position = snapshot
                .facts
                .iter()
                .position(|fact| fact.origin.tu == "nominal-proof-b.cpp" && fact.name == "Plain")
                .unwrap();
            if case == "missing" {
                snapshot.facts.remove(position);
            } else {
                let mut duplicate = snapshot.facts[position].clone();
                duplicate.origin.local += 10000;
                snapshot.facts.push(duplicate);
            }
        } else {
            let contract =
                Arc::make_mut(snapshot.native_com_contracts.get_mut(&interface).unwrap());
            let NativeTypeLayer::Record(record) =
                &mut contract.slots[0].native_result.as_mut().unwrap().layer
            else {
                panic!()
            };
            match case {
                "tu" => record.declaration.tu = "other.cpp".into(),
                "source" => record.declaration.spelling.offset += 1,
                "kind" => record.declaration.kind = clang_sys::CXCursor_UnionDecl,
                "parent" => record.declaration.parent.name = "other.cpp".into(),
                _ => unreachable!(),
            }
        }
        let declarations = DeclarationIndex::new(&snapshot.facts);
        let by_declaration = snapshot
            .facts
            .iter()
            .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
            .collect();
        let source_annotations = snapshot.source_annotation_signatures();
        let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
        let context = crate::com_pointer::Context {
            snapshot: &snapshot,
            declarations: &declarations,
            facts_by_declaration: &by_declaration,
            source_names: None,
            excluded_types: None,
            source_annotations: &source_annotations,
            effective_annotations: &effective_annotations,
        };
        let interfaces: Vec<_> = snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "NominalContract")
            .collect();
        let error =
            contextual_interface_matches(&context, interfaces[0], interfaces[1]).unwrap_err();
        assert!(
            error.contains("method[0](Transform).result"),
            "{case}: {error}"
        );
        assert!(error.contains("native nominal binding"), "{case}: {error}");
        println!("nominal binding case={case} rejection={error}");
    }
}

#[test]
fn root_failure_diagnostics_alias_failures_reuse_exact_resolver() {
    let terminal = diagnostic_interface("tu", 1);
    let mut bridge = alias(2, "Example", "alias.hpp", 2, named(&terminal));
    bridge.root = false;
    for case in ["missing", "cycle", "conflict", "non-tag"] {
        let mut facts = vec![terminal.clone(), bridge.clone()];
        match case {
            "missing" => {
                facts.remove(0);
            }
            "cycle" => {
                facts[1].data = FactData::Typedef {
                    target: named(&facts[1]),
                };
            }
            "conflict" => {
                let mut conflict = terminal.clone();
                conflict.origin.local = 4;
                let FactData::Interface { methods, .. } = &mut conflict.data else {
                    unreachable!()
                };
                methods[0].result = TypeRef::Scalar(Scalar::I32);
                facts.push(conflict);
            }
            "non-tag" => {
                facts[0].kind = FactKind::Typedef;
                facts[0].data = FactData::Typedef {
                    target: TypeRef::Scalar(Scalar::I32),
                };
            }
            _ => unreachable!(),
        }
        let index = facts
            .iter()
            .fold(HashMap::<&str, Vec<&Fact>>::new(), |mut index, fact| {
                index.entry(&fact.name).or_default().push(fact);
                index
            });
        let alias = facts.iter().find(|fact| fact.origin.local == 2).unwrap();
        let ordinary = typedef_declaration(alias, &index).map(|fact| &fact.origin);
        let snapshot = snapshot(facts.clone());
        let mut trace = String::new();
        let observed =
            typedef_declaration_with_diagnostics(alias, &index, Some((&snapshot, &mut trace)))
                .map(|fact| &fact.origin);
        assert_eq!(ordinary, observed);
        let selected = vec![alias];
        for phase in ["associated", "constant-collision"] {
            let error = aggregate_root_selection_errors(vec![type_root_selection_error(
                &snapshot,
                "Example",
                phase,
                Error("ambiguous type root `Example`".to_string()),
                &selected,
                &selected,
                &index,
            )])
            .to_string();
            assert!(error.contains(&format!("phase={phase}")), "{error}");
            let expected = match case {
                "missing" => "missing exact bucket",
                "cycle" => "cycle at",
                "conflict" => "conflicting bucket",
                "non-tag" => "non-tag terminal",
                _ => unreachable!(),
            };
            assert!(error.contains(expected), "{error}");
            if case == "conflict" {
                assert!(error.contains("data.methods[0](Update).result"), "{error}");
                assert!(error.contains("local: 4"), "{error}");
            }
        }
    }
}

#[test]
fn root_failure_diagnostics_unselected_target_retains_full_context() {
    let provider = diagnostic_interface("provider", 1);
    let mut target = diagnostic_interface("consumer", 9);
    target.root = false;
    target.parent = Some(Origin {
        tu: "consumer".to_string(),
        local: 99,
    });
    let FactData::Interface { methods, .. } = &mut target.data else {
        unreachable!()
    };
    methods[0].params[0].ty = TypeRef::Scalar(Scalar::F32);
    let mut bridge = alias(2, "Example", "alias.hpp", 2, named(&target));
    bridge.origin.tu = "consumer".to_string();
    let target_origin = target.origin.clone();
    let mut snapshot = snapshot(vec![provider, target, bridge]);
    snapshot.root_owners.insert(
        target_origin.clone(),
        authority_root_owner("TargetOwner", "Target.Namespace", "common.hpp"),
    );
    snapshot.root_partitions.insert(
        ("consumer".to_string(), "common.hpp".to_string()),
        authority_root_owner("SourceOwner", "Source.Namespace", "common.hpp"),
    );
    snapshot
        .fact_namespace_authorities
        .insert(target_origin.clone(), "Terminal.Authority".to_string());
    snapshot
        .suppressed_type_origins
        .insert(target_origin.clone());
    let selected = [&snapshot.facts[0], &snapshot.facts[2]];
    assert!(selected.iter().all(|fact| fact.origin != target_origin));
    let index =
        snapshot
            .facts
            .iter()
            .fold(HashMap::<&str, Vec<&Fact>>::new(), |mut index, fact| {
                index.entry(&fact.name).or_default().push(fact);
                index
            });
    let failure = choose_type_root("Example", &selected, &index)
        .err()
        .unwrap();
    let error = aggregate_root_selection_errors(vec![type_root_selection_error(
        &snapshot,
        "Example",
        "post-closure",
        failure,
        &selected,
        &selected,
        &index,
    )])
    .to_string();
    for marker in ["exact bucket", "    terminal"] {
        let row = error.lines().find(|row| row.contains(marker)).unwrap();
        for expected in [
            "tu: \"consumer\", local: 9",
            "spelling=common.hpp:100",
            "expansion=common.hpp:100",
            "parent=Some(Origin { tu: \"consumer\", local: 99 })",
            "root=false",
            "suppressed=true",
            "namespace=Some(\"Terminal.Authority\")",
            "owner=Some(RootOwner { input: \"TargetOwner\"",
            "source_owner=Some(RootOwner { input: \"SourceOwner\"",
        ] {
            assert!(row.contains(expected), "{marker} missing {expected}: {row}");
        }
    }
}

#[test]
fn root_failure_diagnostics_preserve_success_and_all_interface_paths() {
    let terminal = diagnostic_interface("tu", 1);
    let bridge = alias(2, "Example", "alias.hpp", 2, named(&terminal));
    let facts = [terminal.clone(), bridge];
    let index = HashMap::from([("Example", facts.iter().collect::<Vec<_>>())]);
    let roots = facts.iter().collect::<Vec<_>>();
    assert_eq!(
        choose_type_root("Example", &roots, &index).unwrap().origin,
        terminal.origin
    );
    for path in ["base", "result", "parameter"] {
        let mut changed = terminal.clone();
        let FactData::Interface { base, methods, .. } = &mut changed.data else {
            unreachable!()
        };
        match path {
            "base" => *base = Some(TypeRef::Object),
            "result" => methods[0].result = TypeRef::Scalar(Scalar::I32),
            "parameter" => methods[0].params[0].annotation.optional = true,
            _ => unreachable!(),
        }
        let mismatch = source_declaration_mismatch(&terminal, &changed);
        let expected = match path {
            "base" => "data.base",
            "result" => "data.methods[0](Update).result",
            "parameter" => "data.methods[0](Update).params[0](value).annotation",
            _ => unreachable!(),
        };
        assert!(mismatch.contains(expected), "{mismatch}");
    }
}

#[test]
fn contextual_interface_proof_nominal_binding_contexts_reach_boundary() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("nominal-contract.hpp");
    let path = path.to_str().unwrap();
    let snapshot = extract(
        [
            Input::new(
                "nominal-proof-a.cpp",
                format!("#define FIELD_KIND long\n#define ENUM_KIND long\n#include {path:?}\n"),
            )
            .with_roots([path]),
            Input::new(
                "nominal-proof-b.cpp",
                format!("#define FIELD_KIND long\n#define ENUM_KIND long\n#include {path:?}\n"),
            )
            .with_roots([path]),
        ],
        &[
            "-x",
            "c++",
            "--target=x86_64-pc-windows-msvc",
            "-fms-extensions",
        ],
    )
    .unwrap();
    let declarations = DeclarationIndex::new(&snapshot.facts);
    let by_declaration = snapshot
        .facts
        .iter()
        .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
        .collect();
    let source_annotations = snapshot.source_annotation_signatures();
    let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
    let context = crate::com_pointer::Context {
        snapshot: &snapshot,
        declarations: &declarations,
        facts_by_declaration: &by_declaration,
        source_names: None,
        excluded_types: None,
        source_annotations: &source_annotations,
        effective_annotations: &effective_annotations,
    };
    for (name, scope) in [
        ("PlainAlias", "global-record"),
        ("PartsAlias", "global-union"),
        ("EnumAlias", "global-enum"),
        ("ScopedAlias", "namespace-record"),
    ] {
        let observation = |tu: &str| {
            let facts: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| fact.origin.tu == tu && fact.name == name)
                .collect();
            assert_eq!(facts.len(), 1, "{name}/{tu}");
            facts[0]
        };
        let (left, right) = (
            observation("nominal-proof-a.cpp"),
            observation("nominal-proof-b.cpp"),
        );
        let (FactData::Typedef { target: lt }, FactData::Typedef { target: rt }) =
            (&left.data, &right.data)
        else {
            panic!()
        };
        let ln = snapshot.native_typedef_types[&left.origin]
            .as_ref()
            .unwrap();
        let rn = snapshot.native_typedef_types[&right.origin]
            .as_ref()
            .unwrap();
        let result =
            context.canonical_types_equal(lt, &left.origin.tu, rt, &right.origin.tu, (ln, rn));
        if name == "EnumAlias" {
            result.unwrap_or_else(|error| panic!("{name}/{scope}: {error}"));
            println!("nominal binding positive={scope} alias={name} enum-proof=passed");
        } else {
            result.unwrap_or_else(|error| panic!("{name}/{scope}: {error}"));
            println!("nominal binding positive={scope} alias={name} record-proof=passed");
        }
    }
}

fn record_only_native_snapshot(field: &str) -> Snapshot {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("record-only-contract.hpp");
    let path = path.to_str().unwrap();
    extract(
        [
            Input::new(
                "record-a.cpp",
                format!("#define FIELD_KIND long\n#define ENUM_KIND long\n#include {path:?}\n"),
            )
            .with_roots([path]),
            Input::new(
                "record-b.cpp",
                format!("#define FIELD_KIND {field}\n#define ENUM_KIND long\n#include {path:?}\n"),
            )
            .with_roots([path]),
        ],
        &[
            "-x",
            "c++",
            "--target=x86_64-pc-windows-msvc",
            "-fms-extensions",
        ],
    )
    .unwrap()
}

#[test]
fn contextual_interface_proof_record_only_native_emission() {
    let snapshot = record_only_native_snapshot("long");
    assert!(
        snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "NominalContract")
            .all(|fact| !fact.root)
    );
    let output = snapshot
        .emit_with_library("RecordProof", "record.dll")
        .unwrap();
    for expected in [
        "#[win32]\nmod RecordProof {",
        "type Leaf = i32;",
        "struct PlainAlias {\n        value: Leaf,\n    }",
        "union PartsAlias {\n        whole: u64,\n        Anonymous: struct {\n            low: i32,\n            high: Leaf,\n        },\n    }",
        "struct ScopedAlias {\n        value: i32,\n    }",
        "#[guid(0xb2ed46df_42e3_46d9_bc8b_a57d473bc5dd)]",
        "interface RecordOnlyContract {\n        fn Transform(&self, parts: PartsAlias, scoped: ScopedAlias) -> PlainAlias;\n    }",
    ] {
        assert!(output.contains(expected), "{expected}: {output}");
    }
    println!("record-only emitted RDL:\n{output}");
}

#[test]
fn contextual_interface_proof_record_only_native_falsifiers() {
    for case in [
        "matching",
        "field-rank",
        "terminal",
        "missing",
        "ambiguous",
        "cycle",
        "source",
        "effective",
        "premerge",
        "owner",
        "projection",
        "suppression",
    ] {
        let mut snapshot =
            record_only_native_snapshot(if case == "field-rank" { "int" } else { "long" });
        let names = PlanningSourceNames::new(&snapshot);
        let plain = snapshot
            .facts
            .iter()
            .find(|fact| fact.origin.tu == "record-b.cpp" && fact.name == "Plain")
            .unwrap()
            .clone();
        let alias = snapshot
            .facts
            .iter()
            .position(|fact| fact.origin.tu == "record-b.cpp" && fact.name == "PlainAlias")
            .unwrap();
        match case {
            "terminal" => {
                let target = snapshot
                    .facts
                    .iter()
                    .find(|fact| fact.origin.tu == "record-b.cpp" && fact.name == "ScopedRecord")
                    .unwrap();
                snapshot.facts[alias].data = FactData::Typedef {
                    target: named(target),
                };
            }
            "missing" => snapshot.facts.retain(|fact| fact.origin != plain.origin),
            "ambiguous" => {
                let mut duplicate = plain.clone();
                duplicate.origin.local += 10000;
                snapshot.facts.push(duplicate);
            }
            "cycle" => {
                snapshot.facts[alias].data = FactData::Typedef {
                    target: named(&snapshot.facts[alias]),
                }
            }
            "source" | "premerge" => {
                snapshot.source_annotations.insert(
                    AnnotationTarget::Declaration(plain.origin.clone()),
                    vec![Annotation::Const],
                );
                if case == "premerge" {
                    for fact in snapshot.facts.iter().filter(|fact| fact.name == "Plain") {
                        snapshot.annotations.insert(
                            AnnotationTarget::Declaration(fact.origin.clone()),
                            vec![Annotation::Const],
                        );
                    }
                }
            }
            "effective" => {
                snapshot.annotations.insert(
                    AnnotationTarget::Declaration(plain.origin.clone()),
                    vec![Annotation::Const],
                );
            }
            "owner" => {
                let mut owner =
                    authority_root_owner(&plain.origin.tu, "RecordProof", &plain.spelling.file);
                owner.remaps.insert("Plain".into(), "OtherPlain".into());
                snapshot.root_owners.insert(plain.origin.clone(), owner);
            }
            "projection" => {
                snapshot
                    .projected_type_names
                    .insert(plain.origin.clone(), "OtherPlain".into());
            }
            "suppression" => {
                snapshot
                    .suppressed_type_origins
                    .insert(plain.origin.clone());
            }
            _ => {}
        }
        let declarations = DeclarationIndex::new(&snapshot.facts);
        let by_declaration = snapshot
            .facts
            .iter()
            .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
            .collect();
        let source_annotations = snapshot.source_annotation_signatures();
        let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
        let context = crate::com_pointer::Context {
            snapshot: &snapshot,
            declarations: &declarations,
            facts_by_declaration: &by_declaration,
            source_names: Some(&names),
            excluded_types: None,
            source_annotations: &source_annotations,
            effective_annotations: &effective_annotations,
        };
        let interfaces: Vec<_> = snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "RecordOnlyContract")
            .collect();
        let result = contextual_interface_matches(&context, interfaces[0], interfaces[1]);
        if case == "matching" {
            result.unwrap();
            println!("record-only case={case} assertions=passed");
        } else {
            let error = result.unwrap_err();
            assert!(
                error.contains("method[0](Transform).result"),
                "{case}: {error}"
            );
            if case == "field-rank" {
                assert!(
                    error.contains("field[0](value)")
                        && error.contains("canonical native evidence differs"),
                    "{error}"
                );
            }
            println!("record-only case={case} rejection={error} assertions=passed");
        }
    }
}

#[test]
fn native_evidence_record_only_anonymous_binding_predicates() {
    let snapshot = record_only_native_snapshot("long");
    for tu in ["record-a.cpp", "record-b.cpp"] {
        let observation = |name: &str| {
            let facts: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| fact.origin.tu == tu && fact.name == name)
                .collect();
            assert_eq!(facts.len(), 1, "{tu}/{name}");
            facts[0]
        };
        let (alias, fact) = (observation("PartsAlias"), observation("SharedParts"));
        let native = snapshot.native_typedef_types[&alias.origin]
            .as_ref()
            .unwrap();
        let NativeTypeLayer::Record(record) = &native.layer else {
            panic!("{tu}: missing captured union")
        };
        let FactData::Record { fields, union, .. } = &fact.data else {
            panic!("{tu}: missing projected union")
        };
        let (member, projected) = (&record.fields[1], &fields[1]);
        let NativeTypeLayer::Record(nested) = &member.ty.layer else {
            panic!("{tu}: missing captured anonymous aggregate")
        };
        let TypeRef::InlineRecord(inline) = &projected.ty else {
            panic!("{tu}: missing projected inline aggregate")
        };
        println!(
            "anonymous audit tu={tu} union-declaration={:?}",
            record.declaration
        );
        println!(
            "anonymous audit tu={tu} field[1]-declaration={:?}",
            member.declaration
        );
        println!(
            "anonymous audit tu={tu} nested-declaration={:?}",
            nested.declaration
        );
        println!("anonymous audit tu={tu} projected-field={projected:?}");
        println!(
            "anonymous audit tu={tu} native-children={:?}",
            nested.fields
        );
        println!(
            "anonymous predicates tu={tu} native_name_empty={} nested_equals_field={} \
             projected_name_equals_Anonymous={}",
            member.declaration.name.is_empty(),
            nested.declaration == member.declaration,
            projected.name == "Anonymous"
        );
        println!(
            "anonymous units tu={tu} native_offset_bits={} projected_offset_bits={} \
             native_size_bytes={} projected_size_bytes={} native_align_bytes={} \
             projected_align_bytes={} ordinal={}",
            member.offset,
            projected.offset,
            member.ty.size,
            projected.size,
            member.align,
            projected.align,
            member.ordinal
        );
        assert!(*union);
        assert_eq!(record.declaration.tu, tu);
        assert_eq!(record.declaration.spelling, fact.spelling);
        assert_eq!(record.declaration.expansion, fact.expansion);
        assert_eq!(member.declaration.tu, tu);
        assert_eq!(member.ordinal, 1);
        assert!(member.declaration.anonymous);
        assert_eq!(member.declaration.kind, clang_sys::CXCursor_StructDecl);
        assert_eq!(member.declaration.parent.kind, record.declaration.kind);
        assert_eq!(member.declaration.parent.name, record.declaration.name);
        assert_eq!(
            member.declaration.parent.spelling.as_ref(),
            Some(&record.declaration.spelling)
        );
        assert_eq!(
            member.declaration.parent.expansion.as_ref(),
            Some(&record.declaration.expansion)
        );
        assert_eq!(
            member.declaration.spelling.file,
            record.declaration.spelling.file
        );
        assert_eq!(nested.declaration.tu, tu);
        assert_eq!(
            (
                member.offset,
                member.align,
                member.ty.size,
                member.bit_width
            ),
            (
                projected.offset,
                projected.align,
                projected.size,
                projected.bit_width
            )
        );
        assert_eq!((nested.align, member.ty.size), (inline.align, inline.size));
        assert_eq!(nested.fields.len(), inline.fields.len());
        for (native, projected) in nested.fields.iter().zip(&inline.fields) {
            assert_eq!(
                (
                    native.offset,
                    native.align,
                    native.ty.size,
                    native.bit_width
                ),
                (
                    projected.offset,
                    projected.align,
                    projected.size,
                    projected.bit_width
                )
            );
        }
        println!("anonymous audit tu={tu} source-parent-ordinal-layout assertions=passed");
    }
}

#[test]
fn contextual_interface_proof_anonymous_member_binding_falsifiers() {
    let snapshot = record_only_native_snapshot("long");
    let observation = |tu: &str| {
        let facts: Vec<_> = snapshot
            .facts
            .iter()
            .filter(|fact| fact.origin.tu == tu && fact.name == "PartsAlias")
            .collect();
        assert_eq!(facts.len(), 1, "{tu}");
        facts[0]
    };
    let (left, right) = (observation("record-a.cpp"), observation("record-b.cpp"));
    let (FactData::Typedef { target: lt }, FactData::Typedef { target: rt }) =
        (&left.data, &right.data)
    else {
        panic!()
    };
    let ln = snapshot.native_typedef_types[&left.origin]
        .as_ref()
        .unwrap();
    let rn = snapshot.native_typedef_types[&right.origin]
        .as_ref()
        .unwrap();
    let declarations = DeclarationIndex::new(&snapshot.facts);
    let by_declaration = snapshot
        .facts
        .iter()
        .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
        .collect();
    let source_names = PlanningSourceNames::new(&snapshot);
    let source_annotations = snapshot.source_annotation_signatures();
    let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
    let context = crate::com_pointer::Context {
        snapshot: &snapshot,
        declarations: &declarations,
        facts_by_declaration: &by_declaration,
        source_names: Some(&source_names),
        excluded_types: None,
        source_annotations: &source_annotations,
        effective_annotations: &effective_annotations,
    };
    for case in ["matching", "source", "parent", "ordinal", "nested", "name"] {
        let mut native = rn.clone();
        let NativeTypeLayer::Record(record) = &mut native.layer else {
            panic!()
        };
        let member = &mut record.fields[1];
        match case {
            "source" => member.declaration.spelling.file = "other.hpp".into(),
            "parent" => member.declaration.parent.spelling.as_mut().unwrap().offset += 1,
            "ordinal" => member.ordinal += 1,
            "nested" => {
                let NativeTypeLayer::Record(nested) = &mut member.ty.layer else {
                    panic!()
                };
                nested.declaration.spelling.offset += 1;
            }
            "name" => {
                member.declaration.name.push_str(" mismatch");
                let NativeTypeLayer::Record(nested) = &mut member.ty.layer else {
                    panic!()
                };
                nested.declaration.name.push_str(" mismatch");
            }
            _ => {}
        }
        let result =
            context.canonical_types_equal(lt, &left.origin.tu, rt, &right.origin.tu, (ln, &native));
        if case == "matching" {
            result.unwrap();
            println!("anonymous binding case={case} assertions=passed");
        } else {
            let error = result.unwrap_err();
            assert!(error.contains("field[1]"), "{case}: {error}");
            let expected = match case {
                "source" | "parent" | "ordinal" => "native field source/parent/ordinal/layout",
                "nested" => "anonymous native member declaration binding",
                "name" => "native declaration differs",
                _ => unreachable!(),
            };
            assert!(error.contains(expected), "{case}: {error}");
            println!("anonymous binding case={case} rejection={error} assertions=passed");
        }
    }
}

#[test]
fn contextual_interface_proof_enum_native_context() {
    fn observed(kind: &str) -> Snapshot {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("tests")
            .join("producer_corrections")
            .join("nominal-contract.hpp");
        let path = path.to_str().unwrap();
        extract(
            [
                Input::new(
                    "enum-a.cpp",
                    format!("#define FIELD_KIND long\n#define ENUM_KIND long\n#include {path:?}\n"),
                )
                .with_roots([path]),
                Input::new(
                    "enum-b.cpp",
                    format!(
                        "#define FIELD_KIND long\n#define ENUM_KIND {kind}\n#include {path:?}\n"
                    ),
                )
                .with_roots([path]),
            ],
            &[
                "-x",
                "c++",
                "--target=x86_64-pc-windows-msvc",
                "-fms-extensions",
            ],
        )
        .unwrap()
    }
    let baseline = observed("long");
    for case in [
        "matching",
        "rank",
        "name",
        "value",
        "count",
        "scoped",
        "tu",
        "source",
        "parent",
        "variant-tu",
        "variant-source",
        "variant-parent",
        "missing",
        "duplicate",
        "cycle",
        "source-annotation",
        "effective",
        "premerge",
        "owner",
        "projection",
        "namespace",
        "suppression",
    ] {
        let mut snapshot = if case == "rank" {
            observed("int")
        } else {
            baseline.clone()
        };
        let names = PlanningSourceNames::new(&snapshot);
        let alias = |tu: &str| {
            snapshot
                .facts
                .iter()
                .find(|fact| fact.origin.tu == tu && fact.name == "EnumAlias")
                .unwrap()
                .clone()
        };
        let (a, b) = (alias("enum-a.cpp"), alias("enum-b.cpp"));
        let (FactData::Typedef { target: at }, FactData::Typedef { target: bt }) =
            (&a.data, &b.data)
        else {
            panic!()
        };
        let left = snapshot.native_typedef_types[&a.origin]
            .as_ref()
            .unwrap()
            .clone();
        let mut right = snapshot.native_typedef_types[&b.origin]
            .as_ref()
            .unwrap()
            .clone();
        let position = snapshot
            .facts
            .iter()
            .position(|fact| fact.origin.tu == "enum-b.cpp" && fact.name == "PlainEnum")
            .unwrap();
        let origin = snapshot.facts[position].origin.clone();
        let NativeTypeLayer::Enum(native) = &mut right.layer else {
            panic!()
        };
        match case {
            "name" | "value" | "count" | "scoped" => {
                let FactData::Enum {
                    variants, scoped, ..
                } = &mut snapshot.facts[position].data
                else {
                    panic!()
                };
                match case {
                    "name" => {
                        native.variants[0].declaration.name = "Other".into();
                        variants[0].name = "Other".into();
                    }
                    "value" => {
                        native.variants[0].signed_value = 3;
                        native.variants[0].unsigned_value = 3;
                        variants[0].value = 3;
                    }
                    "count" => {
                        native.variants.pop();
                        variants.pop();
                    }
                    "scoped" => {
                        native.scoped = true;
                        *scoped = true;
                    }
                    _ => unreachable!(),
                }
            }
            "tu" => native.declaration.tu = "other.cpp".into(),
            "source" => native.declaration.spelling.offset += 1,
            "parent" => native.declaration.parent.name = "other.cpp".into(),
            "variant-tu" => native.variants[0].declaration.tu = "other.cpp".into(),
            "variant-source" => native.variants[0].declaration.spelling.file = "other.hpp".into(),
            "variant-parent" => {
                native.variants[0]
                    .declaration
                    .parent
                    .spelling
                    .as_mut()
                    .unwrap()
                    .offset += 1;
            }
            "missing" => {
                snapshot.facts.remove(position);
            }
            "duplicate" => {
                let mut duplicate = snapshot.facts[position].clone();
                duplicate.origin.local += 10000;
                snapshot.facts.push(duplicate);
            }
            "cycle" => {
                let i = snapshot
                    .facts
                    .iter()
                    .position(|fact| fact.origin == b.origin)
                    .unwrap();
                snapshot.facts[i].data = FactData::Typedef { target: named(&b) };
            }
            "source-annotation" | "premerge" => {
                snapshot.source_annotations.insert(
                    AnnotationTarget::Declaration(origin.clone()),
                    vec![Annotation::Const],
                );
                if case == "premerge" {
                    for fact in snapshot
                        .facts
                        .iter()
                        .filter(|fact| fact.name == "PlainEnum")
                    {
                        snapshot.annotations.insert(
                            AnnotationTarget::Declaration(fact.origin.clone()),
                            vec![Annotation::Const],
                        );
                    }
                }
            }
            "effective" => {
                snapshot.annotations.insert(
                    AnnotationTarget::Declaration(origin.clone()),
                    vec![Annotation::Const],
                );
            }
            "owner" => {
                let mut owner = authority_root_owner(
                    "enum-b.cpp",
                    "Other",
                    &snapshot.facts[position].spelling.file,
                );
                owner.remaps.insert("PlainEnum".into(), "OtherEnum".into());
                snapshot.root_owners.insert(origin.clone(), owner);
            }
            "projection" => {
                snapshot
                    .projected_type_names
                    .insert(origin.clone(), "OtherEnum".into());
            }
            "namespace" => {
                snapshot
                    .fact_namespace_authorities
                    .insert(origin.clone(), "Other".into());
            }
            "suppression" => {
                snapshot.suppressed_type_origins.insert(origin.clone());
            }
            _ => {}
        }
        let declarations = DeclarationIndex::new(&snapshot.facts);
        let by_declaration = snapshot
            .facts
            .iter()
            .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
            .collect();
        let source_annotations = snapshot.source_annotation_signatures();
        let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
        let context = crate::com_pointer::Context {
            snapshot: &snapshot,
            declarations: &declarations,
            facts_by_declaration: &by_declaration,
            source_names: Some(&names),
            excluded_types: None,
            source_annotations: &source_annotations,
            effective_annotations: &effective_annotations,
        };
        let result = if case == "rank" {
            let interfaces: Vec<_> = snapshot
                .facts
                .iter()
                .filter(|fact| fact.name == "NominalContract")
                .collect();
            contextual_interface_matches(&context, interfaces[0], interfaces[1])
        } else if case == "cycle" {
            context.canonical_types_equal(
                at,
                &a.origin.tu,
                &named(&b),
                &b.origin.tu,
                (&left, &right),
            )
        } else {
            context.canonical_types_equal(at, &a.origin.tu, bt, &b.origin.tu, (&left, &right))
        };
        if case == "matching" {
            result.unwrap();
            println!("enum native case={case} assertions=passed");
        } else {
            let error = result.unwrap_err();
            let expected = match case {
                "rank" => "method[0](Transform).params[1]",
                "name" | "value" => "enum variant[0]",
                "count" | "scoped" => "enum scoped/ordered variant count",
                "tu" | "source" | "parent" | "missing" | "duplicate" => "native nominal binding",
                "variant-tu" | "variant-source" | "variant-parent" => "enum variant[0]",
                "cycle" => "typedef cycle",
                "suppression" => "suppressed or excluded",
                _ => "canonical enum provider contract",
            };
            assert!(error.contains(expected), "{case}: {error}");
            if case == "rank" {
                assert!(
                    error.contains("enum underlying native evidence differs"),
                    "{error}"
                );
            }
            println!("enum native case={case} rejection={error} assertions=passed");
        }
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
            allow_enum_omissions: false,
            omission_namespace: "",
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
