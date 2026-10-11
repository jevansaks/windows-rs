use super::*;

#[test]
fn field_bearing_com_pointer_contract_preserves_native_state() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("com_pointer");
    let use_source = r#"#define SAL(text) __attribute__((annotate(text)))
typedef NativeTag* PTag;
extern "C" void UseTag(NativeTag* value, NativeTag** result);
extern "C" void UseAlias(PTag value);
extern "C" void UseArray(SAL("_In_reads_(2)") NativeTag** values);
extern "C" void UseOut(SAL("_COM_Outptr_") NativeTag** value);
"#;
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let args = ["-x", "c++", "-std=c++17", &format!("--target={target}")];
        let reference = Input::new(
            root.join("reference.cpp").to_str().unwrap(),
            format!("#include \"provider.hpp\"\n{use_source}"),
        )
        .with_roots([
            root.join("reference.cpp").to_str().unwrap(),
            root.join("provider.hpp").to_str().unwrap(),
        ])
        .partitioned("reference");
        let mut reference_policy = HeaderPartitionPolicy::new();
        for header in ["base.hpp", "argument.hpp", "provider.hpp"] {
            reference_policy.add_traversed_header_for_input(
                reference.input.name.clone(),
                root.join(header).to_str().unwrap(),
                RootPartition::new("Tag", "Test.Tag"),
            );
        }
        reference_policy.add_traversed_header_for_input(
            reference.input.name.clone(),
            reference.input.name.clone(),
            RootPartition::new("Api", "Test.Api"),
        );
        let references = BTreeMap::new();
        let mut options = EmitOptions::new("Test.Common", &references);
        options.library = Some("native.dll");
        let reference_output = extract_partitioned([reference], &args)
            .unwrap()
            .into_header_partition_plan(&reference_policy, &NamespaceAuthorities::new())
            .unwrap()
            .emit_with_options(&options)
            .unwrap();
        let reference_winmd = std::env::temp_dir().join(format!(
            "windows-clang-com-reference-{}-{target}.winmd",
            std::process::id()
        ));
        windows_rdl::reader()
            .input_texts(reference_output.values())
            .reference_default()
            .output(&reference_winmd)
            .write()
            .unwrap();
        let reference_index = windows_metadata::reader::Index::read(&reference_winmd).unwrap();
        let windows_metadata::reader::Item::Fn(reference_function) =
            reference_index.expect_item("Test.Api", "UseTag")
        else {
            panic!("{reference_output:?}");
        };
        let reference_signature = reference_function.signature(&[]);
        let reference_peer = reference_index
            .expect("Test.Tag", "NativeTag")
            .methods()
            .find(|method| method.name() == "GetPeer")
            .unwrap()
            .signature(&[]);
        let mut baseline = None;
        for reversed in [false, true] {
            let provider = Input::new(
                root.join("provider.cpp").to_str().unwrap(),
                "#include \"provider.hpp\"\n",
            )
            .with_roots([root.join("provider.hpp").to_str().unwrap()])
            .partitioned("provider");
            let client = Input::new(
                root.join("client.cpp").to_str().unwrap(),
                format!("#include \"helper.hpp\"\n{use_source}"),
            )
            .with_roots([
                root.join("client.cpp").to_str().unwrap(),
                root.join("helper.hpp").to_str().unwrap(),
            ])
            .partitioned("client");
            let mut policy = HeaderPartitionPolicy::new();
            for header in ["base.hpp", "argument.hpp", "provider.hpp", "helper.hpp"] {
                policy.add_traversed_header_for_input(
                    provider.input.name.clone(),
                    root.join(header).to_str().unwrap(),
                    RootPartition::new("Tag", "Test.Tag"),
                );
            }
            policy.add_traversed_header_for_input(
                client.input.name.clone(),
                client.input.name.clone(),
                RootPartition::new("Api", "Test.Api"),
            );
            let mut inputs = vec![provider, client];
            if reversed {
                inputs.reverse();
            }
            let snapshot = extract_partitioned(inputs, &args).unwrap();
            let record = snapshot
                .facts
                .iter()
                .find(|fact| {
                    fact.name == "NativeTag"
                        && fact.definition
                        && matches!(fact.data, FactData::Record { .. })
                })
                .unwrap();
            assert_eq!(record.kind, FactKind::Struct);
            let FactData::Record {
                base,
                fields,
                size,
                align,
                ..
            } = &record.data
            else {
                unreachable!()
            };
            assert!(base.is_some());
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0].name, "Base");
            assert_eq!(fields[1].name, "helper_state");
            assert_eq!(
                fields[1].offset,
                if target.starts_with("i686") { 32 } else { 64 }
            );
            assert_eq!(
                (*size, *align),
                if target.starts_with("i686") {
                    (12, 4)
                } else {
                    (16, 8)
                }
            );
            assert_eq!(
                snapshot.fact_uuid(record),
                Some("6b44d453-343e-4246-826d-208c460655c5")
            );
            let forward = snapshot
                .facts
                .iter()
                .find(|fact| {
                    fact.name == record.name
                        && fact.origin.tu == record.origin.tu
                        && !fact.definition
                        && fact.kind == FactKind::Struct
                })
                .unwrap();
            assert_eq!(
                snapshot.class_canonical_origins[&forward.origin],
                snapshot.class_canonical_origins[&record.origin]
            );
            let contract = &snapshot.native_com_contracts[&record.origin];
            assert_eq!(contract.slots.len(), 3);
            assert_eq!(contract.slots[0].name, "BaseCall");
            assert_eq!(contract.slots[1].name, "Call");
            assert_eq!(contract.slots[2].name, "GetPeer");
            assert!(matches!(
                &contract.slots[0].params[0].ty,
                TypeRef::Reference { mutable: false, target }
                    if matches!(target.as_ref(), TypeRef::Named { name, .. } if name == "NativeId")
            ));
            assert!(!contract.slots[0].params[1].annotation.com_out_ptr);
            let provider_fact = snapshot
                .facts
                .iter()
                .find(|fact| {
                    fact.name == "NativeTag"
                        && fact.definition
                        && matches!(fact.data, FactData::Interface { .. })
                })
                .unwrap();
            assert!(
                snapshot.native_com_contracts[&provider_fact.origin].slots[0].params[1]
                    .annotation
                    .com_out_ptr
            );
            assert_eq!(
                snapshot.native_com_contracts[&forward.origin],
                snapshot.native_com_contracts[&record.origin]
            );
            let alias = snapshot
                .facts
                .iter()
                .find(|fact| {
                    fact.origin.tu == record.origin.tu
                        && fact.name == record.name
                        && fact.kind == FactKind::Typedef
                })
                .unwrap();
            assert!(matches!(
                &alias.data,
                FactData::Typedef { target: TypeRef::Named { name, declaration } }
                    if name == &record.name && declaration == &record.spelling
            ));
            let mut native_record = record.clone();
            native_record.root = false;
            let plan = snapshot
                .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                .unwrap();
            assert!(
                !plan
                    .snapshot
                    .root_owners
                    .contains_key(&native_record.origin)
            );
            assert_eq!(
                plan.snapshot
                    .facts
                    .iter()
                    .find(|fact| fact.origin == native_record.origin),
                Some(&native_record)
            );
            let strict = plan.clone().emit_with_options(&options).unwrap();
            let report = plan.emit_with_enum_diagnostics(&options).unwrap();
            assert_eq!(strict, report.partitions);
            assert!(report.omissions.is_empty());
            let output = strict.values().cloned().collect::<String>();
            assert_eq!(output.matches("interface NativeTag").count(), 1, "{output}");
            assert!(!output.contains("helper_state"), "{output}");
            assert!(!output.contains("helper_value"), "{output}");
            assert!(!output.contains("helper_result"), "{output}");
            assert!(output.contains("Test::Tag::NativeTag"), "{output}");
            let winmd = std::env::temp_dir().join(format!(
                "windows-clang-com-pointer-{}-{target}-{reversed}.winmd",
                std::process::id()
            ));
            windows_rdl::reader()
                .input_texts(strict.values())
                .reference_default()
                .output(&winmd)
                .write()
                .unwrap();
            let index = windows_metadata::reader::Index::read(&winmd).unwrap();
            assert_eq!(index.expect("Test.Tag", "NativeTag").methods().count(), 2);
            assert_eq!(index.expect("Test.Tag", "NativeBase").methods().count(), 1);
            index.expect("Test.Api", "Apis");
            let windows_metadata::reader::Item::Fn(function) =
                index.expect_item("Test.Api", "UseTag")
            else {
                panic!("{output}");
            };
            let signature = function.signature(&[]);
            assert_eq!(signature.flags, reference_signature.flags);
            assert_eq!(signature.return_type, reference_signature.return_type);
            assert_eq!(signature.types, reference_signature.types);
            assert_eq!(
                signature.types,
                [
                    windows_metadata::Type::class_named("Test.Tag", "NativeTag"),
                    windows_metadata::Type::PtrMut(
                        Box::new(windows_metadata::Type::class_named("Test.Tag", "NativeTag")),
                        1,
                    ),
                ]
            );
            let peer = index
                .expect("Test.Tag", "NativeTag")
                .methods()
                .find(|method| method.name() == "GetPeer")
                .unwrap()
                .signature(&[]);
            assert_eq!(peer.flags, reference_peer.flags);
            assert_eq!(peer.return_type, reference_peer.return_type);
            assert_eq!(peer.types, reference_peer.types);
            for name in ["UseAlias", "UseArray", "UseOut"] {
                let windows_metadata::reader::Item::Fn(candidate) =
                    index.expect_item("Test.Api", name)
                else {
                    panic!("{output}");
                };
                let windows_metadata::reader::Item::Fn(reference) =
                    reference_index.expect_item("Test.Api", name)
                else {
                    panic!("{reference_output:?}");
                };
                let candidate_signature = candidate.signature(&[]);
                let reference_signature = reference.signature(&[]);
                assert_eq!(
                    candidate_signature.flags, reference_signature.flags,
                    "{name}"
                );
                assert_eq!(
                    candidate_signature.return_type, reference_signature.return_type,
                    "{name}"
                );
                assert_eq!(
                    candidate_signature.types, reference_signature.types,
                    "{name}"
                );
                assert_eq!(
                    candidate
                        .params()
                        .map(|param| param.flags())
                        .collect::<Vec<_>>(),
                    reference
                        .params()
                        .map(|param| param.flags())
                        .collect::<Vec<_>>(),
                    "{name}"
                );
            }
            std::fs::remove_file(&winmd).unwrap();
            if let Some(baseline) = &baseline {
                assert_eq!(&strict, baseline);
            } else {
                baseline = Some(strict);
            }
            println!("native_com positive target={target} reversed={reversed} assertions=passed");
        }
        std::fs::remove_file(&reference_winmd).unwrap();
    }
}

#[test]
fn field_bearing_com_pointer_contract_rejects_conflicts_and_layout_uses() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("tests")
        .join("producer_corrections")
        .join("com_pointer");
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        for case in [
            "argument-context",
            "result",
            "slot-order",
            "guid",
            "base",
            "multiple-base",
            "virtual-base",
            "destructor",
            "conversion",
            "cdecl",
            "missing",
            "emit-excluded",
            "ambiguous-provider",
            "value",
            "mixed",
            "pointer-array",
            "object-array",
            "counted-objects",
            "byte-buffer",
            "counted-alias",
            "reference",
        ] {
            if case == "cdecl" && !target.starts_with("i686") {
                continue;
            }
            for reversed in [false, true] {
                let provider_prefix = if case == "cdecl" {
                    "#define COM_CDECL\n"
                } else {
                    ""
                };
                let mut client_prefix = match case {
                    "argument-context" => "#define COM_ARGUMENT_DOUBLE\n",
                    "result" => "#define COM_WRONG_RESULT\n",
                    "slot-order" => "#define COM_REORDER\n",
                    "guid" => "#define COM_WRONG_GUID\n",
                    "base" => "#define COM_WRONG_BASE\n",
                    "multiple-base" => "#define COM_MULTIPLE_BASE\n",
                    "virtual-base" => "#define COM_VIRTUAL_BASE\n",
                    "destructor" => "#define COM_VIRTUAL_DESTRUCTOR\n",
                    "conversion" => "#define COM_VIRTUAL_CONVERSION\n",
                    "cdecl" => "#define COM_CDECL\n",
                    _ => "",
                }
                .to_string();
                let layout_case = matches!(
                    case,
                    "value"
                        | "mixed"
                        | "pointer-array"
                        | "object-array"
                        | "counted-objects"
                        | "byte-buffer"
                        | "counted-alias"
                        | "reference"
                );
                if layout_case {
                    client_prefix.push_str("#define COM_CONCRETE\n");
                }
                let use_source = match case {
                    "value" => "extern \"C\" void UseTag(NativeTag value);",
                    "mixed" => "extern \"C\" void UseTag(NativeTag* valid, NativeTag value);",
                    "pointer-array" => "extern \"C\" void UseTag(NativeTag (*values)[2]);",
                    "object-array" => "struct Objects { NativeTag values[2]; };",
                    "counted-objects" => {
                        "extern \"C\" void UseTag(SAL(\"_In_reads_(2)\") NativeTag* values);"
                    }
                    "byte-buffer" => {
                        "extern \"C\" void UseTag(SAL(\"_In_reads_bytes_(24)\") NativeTag* values);"
                    }
                    "counted-alias" => {
                        "typedef NativeTag* PTag; extern \"C\" void UseTag(SAL(\"_In_reads_(2)\") PTag values);"
                    }
                    "reference" => "extern \"C\" void UseTag(NativeTag& value);",
                    _ => "extern \"C\" void UseTag(NativeTag* value);",
                };
                let provider = Input::new(
                    root.join("provider.cpp").to_str().unwrap(),
                    format!("{provider_prefix}#include \"provider.hpp\"\n"),
                )
                .with_roots([root.join("provider.hpp").to_str().unwrap()])
                .partitioned("provider");
                let client = Input::new(
                                root.join("client.cpp").to_str().unwrap(),
                                format!("{client_prefix}#include \"helper.hpp\"\n#define SAL(text) __attribute__((annotate(text)))\n{use_source}\n"),
                            ).with_roots([
                                root.join("client.cpp").to_str().unwrap(),
                                root.join("helper.hpp").to_str().unwrap(),
                            ]).partitioned("client");
                let mut policy = HeaderPartitionPolicy::new();
                let tag_owner = if case == "missing" {
                    RootPartition::new("Tag", "Test.Tag").with_exclusion("NativeTag")
                } else {
                    RootPartition::new("Tag", "Test.Tag")
                };
                for header in ["base.hpp", "argument.hpp", "provider.hpp", "helper.hpp"] {
                    policy.add_traversed_header_for_input(
                        provider.input.name.clone(),
                        root.join(header).to_str().unwrap(),
                        tag_owner.clone(),
                    );
                }
                policy.add_traversed_header_for_input(
                    client.input.name.clone(),
                    client.input.name.clone(),
                    RootPartition::new("Api", "Test.Api"),
                );
                let mut inputs = vec![provider, client];
                if case == "ambiguous-provider" {
                    let second = Input::new(
                        root.join("second.cpp").to_str().unwrap(),
                        "#include \"provider_copy.hpp\"\n",
                    )
                    .with_roots([root.join("provider_copy.hpp").to_str().unwrap()])
                    .partitioned("second");
                    for header in ["base.hpp", "argument.hpp", "provider_copy.hpp"] {
                        policy.add_traversed_header_for_input(
                            second.input.name.clone(),
                            root.join(header).to_str().unwrap(),
                            tag_owner.clone(),
                        );
                    }
                    inputs.push(second);
                }
                if reversed {
                    inputs.reverse();
                }
                let target_arg = format!("--target={target}");
                let snapshot =
                    extract_partitioned(inputs, &["-x", "c++", "-std=c++17", &target_arg])
                        .unwrap_or_else(|error| panic!("{target} {case}: {error}"));
                if case == "argument-context" {
                    let calls = snapshot
                        .facts
                        .iter()
                        .filter_map(|fact| {
                            let contract = snapshot.native_com_contracts.get(&fact.origin)?;
                            (fact.definition && fact.name == "NativeTag").then(|| {
                                contract
                                    .slots
                                    .iter()
                                    .find(|slot| slot.name == "Call")
                                    .unwrap_or_else(|| {
                                        panic!("{target} {case}: {fact:?}, {contract:?}")
                                    })
                            })
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(calls.len(), 2);
                    assert_eq!(calls[0].params[0].ty, calls[1].params[0].ty);
                    let arguments = snapshot
                        .facts
                        .iter()
                        .filter(|fact| fact.definition && fact.name == "Argument")
                        .collect::<Vec<_>>();
                    assert_eq!(arguments.len(), 2);
                    assert_eq!(arguments[0].spelling, arguments[1].spelling);
                    assert_ne!(arguments[0].data, arguments[1].data);
                }
                if layout_case {
                    let helper = snapshot
                        .facts
                        .iter()
                        .find(|fact| {
                            fact.definition
                                && fact.name == "NativeTag"
                                && matches!(fact.data, FactData::Record { .. })
                        })
                        .unwrap();
                    assert_eq!(helper.kind, FactKind::Struct);
                    assert!(
                        snapshot.native_com_contracts.contains_key(&helper.origin),
                        "{target} {case}"
                    );
                }
                if matches!(case, "destructor" | "conversion") {
                    let helper = snapshot
                        .facts
                        .iter()
                        .find(|fact| {
                            fact.name == "NativeTag"
                                && fact.definition
                                && matches!(fact.data, FactData::Record { .. })
                        })
                        .unwrap();
                    assert_eq!(helper.kind, FactKind::Struct);
                    assert_eq!(
                        snapshot.fact_uuid(helper),
                        Some("6b44d453-343e-4246-826d-208c460655c5")
                    );
                    assert!(snapshot.unproved_com_records.contains(&helper.origin));
                }
                let plan = snapshot
                    .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                    .unwrap();
                let references = BTreeMap::from([(
                    "NativeTag".to_string(),
                    TypeReference::new("External", "NativeTag", TypeReferenceKind::Interface),
                )]);
                let excluded = BTreeSet::from(["NativeTag".to_string()]);
                let mut options = EmitOptions::new("Test.Common", &references);
                options.library = Some("native.dll");
                if case == "emit-excluded" {
                    options.excluded_types = Some(&excluded);
                }
                let strict = plan.clone().emit_with_options(&options).unwrap_err();
                let diagnostic = plan.emit_with_enum_diagnostics(&options).unwrap_err();
                assert_eq!(strict.0, diagnostic.0, "{target} {case}");
                if layout_case {
                    assert!(
                        strict.0.contains("requires object layout"),
                        "{target} {case}: {strict}"
                    );
                } else if case == "argument-context" {
                    assert!(
                        strict
                            .0
                            .contains("conflicting native COM pointer contracts"),
                        "{strict}"
                    );
                } else {
                    assert!(strict.0.contains("NativeTag"), "{target} {case}: {strict}");
                }
                println!(
                    "native_com case={case} target={target} reversed={reversed} assertions=passed"
                );
            }
        }
    }
}
