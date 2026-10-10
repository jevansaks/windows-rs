use std::collections::{BTreeMap, BTreeSet};
use windows_clang::{
    Annotation, AnnotationTarget, CanonicalRecord, CanonicalRecordKind, EmitOptions, FactData,
    FactKind, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition, TypeRef, extract,
    extract_partitioned,
};

#[test]
fn exact_coclass_identity_and_every_uuid_observation() {
    helpers::ensure_libclang();
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let target_arg = format!("--target={target}");
        let snapshot = extract(
            [Input::new(
                "coclass.hpp",
                r#"
            typedef class X X;
            class __declspec(uuid("11111111-2222-3333-4455-66778899aabb"))
                __attribute__((annotate("win32metadata:agile"))) X;
            class __declspec(uuid("11111111-2222-3333-4455-66778899aabb")) X;
            typedef unsigned Word;
            const Word Global = 7;
            struct Owner { int (__stdcall *Callback)(int value); };
        "#,
            )],
            &["-x", "c++", "-fms-extensions", &target_arg],
        )
        .unwrap();
        let classes: Vec<_> = snapshot
            .facts()
            .iter()
            .filter(|fact| fact.kind == FactKind::Class)
            .collect();
        assert_eq!(classes.len(), 3);
        assert!(classes.iter().all(|fact| !fact.definition));
        let canonical = &classes[0].origin;
        for fact in &classes {
            assert_eq!(
                snapshot.class_canonical_origins().get(&fact.origin),
                Some(&CanonicalRecord::Source(canonical.clone()))
            );
        }
        assert!(matches!(classes[0].data, FactData::Record { .. }));
        assert!(matches!(classes[1].data, FactData::Class { .. }));
        let alias = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "X" && fact.kind == FactKind::Typedef)
            .unwrap();
        let FactData::Typedef {
            target: TypeRef::Named { declaration, .. },
        } = &alias.data
        else {
            panic!();
        };
        assert_eq!(*declaration, classes[0].spelling);
        let aliases = snapshot.coclass_aliases().unwrap();
        let evidence = aliases.get(&alias.origin).unwrap();
        assert_eq!(
            evidence.canonical,
            CanonicalRecord::Source(canonical.clone())
        );
        assert_eq!(evidence.guids.len(), 2);
        assert_eq!(evidence.guids[0].0, classes[1].origin);
        assert_eq!(evidence.guids[1].0, classes[2].origin);
        assert_eq!(
            snapshot
                .annotations()
                .get(&AnnotationTarget::Declaration(classes[1].origin.clone())),
            Some(&vec![Annotation::Agile])
        );
        assert_eq!(snapshot, snapshot.clone());
        let origins: BTreeSet<_> = snapshot
            .facts()
            .iter()
            .map(|fact| &fact.origin)
            .chain(
                snapshot
                    .value_declarations()
                    .iter()
                    .map(|value| &value.origin),
            )
            .collect();
        assert_eq!(
            origins.len(),
            snapshot.facts().len() + snapshot.value_declarations().len()
        );
        let references = BTreeMap::new();
        let options = EmitOptions::new("Test", &references);
        let rdl = snapshot.emit_with_options(&options).unwrap();
        assert!(rdl.contains("const X: GUID"));
        assert!(!rdl.contains("struct X"));
        assert!(!rdl.contains("type X"));
        assert!(
            snapshot
                .emit_by_header_with_options(&options)
                .unwrap()
                .values()
                .any(|rdl| rdl.contains("const X: GUID"))
        );
        let policy = HeaderPartitionPolicy::new()
            .with_traversed_header("coclass.hpp", RootPartition::new("coclass", "Test"));
        let plan = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::default())
            .unwrap();
        let outputs = plan.emit_with_options(&options).unwrap();
        assert!(outputs.values().any(|rdl| rdl.contains("const X: GUID")));
        assert_eq!(snapshot.coclass_aliases().unwrap(), aliases);
    }
}

#[test]
fn ordinary_incomplete_and_by_value_types_are_not_guid_alias_obligations() {
    helpers::ensure_libclang();
    let args = [
        "-x",
        "c++",
        "-fms-extensions",
        "--target=x86_64-pc-windows-msvc",
    ];
    for source in [
        r#"typedef class Missing Missing; extern "C" void Use(Missing value);"#,
        r#"typedef class X X; class __declspec(uuid("11111111-2222-3333-4455-66778899aabb")) X;
                extern "C" void Use(X value);"#,
    ] {
        let snapshot = extract([Input::new("required.hpp", source)], &args).unwrap();
        assert!(snapshot.emit_with_library("Test", "test.dll").is_err());
    }
    let snapshot = extract_partitioned(
        [Input::new(
            "partition.hpp",
            r#"typedef class X X;
                class __declspec(uuid("11111111-2222-3333-4455-66778899aabb")) X;"#,
        )
        .partitioned("coclass")
        .with_root_partition("partition.hpp", RootPartition::new("coclass", "Test"))],
        &args,
    )
    .unwrap();
    let before = snapshot.coclass_aliases().unwrap();
    let references = BTreeMap::new();
    let options = EmitOptions::new("Test", &references);
    let outputs = snapshot
        .clone()
        .emit_partitioned_with_options(&options)
        .unwrap();
    assert!(outputs.values().any(|rdl| rdl.contains("const X: GUID")));
    assert_eq!(before, snapshot.coclass_aliases().unwrap());
}

#[test]
fn same_names_do_not_join_and_missing_uuid_is_not_a_coclass() {
    helpers::ensure_libclang();
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let target_arg = format!("--target={target}");
        let snapshot = extract([Input::new("scope.hpp", r#"
            namespace A { typedef class X X; class __declspec(uuid("11111111-2222-3333-4455-66778899aabb")) X; }
            namespace B { typedef class X X; class __declspec(uuid("aaaaaaaa-bbbb-cccc-ddee-ff0011223344")) X; }
            typedef class Missing Missing;
            class Missing;
            class Value { public: int payload; };
            typedef Value ValueAlias;
        "#)], &["-x", "c++", "-fms-extensions", &target_arg]).unwrap();
        let aliases = snapshot.coclass_aliases().unwrap();
        assert_eq!(aliases.len(), 2);
        let evidence: Vec<_> = aliases.values().collect();
        assert_ne!(evidence[0].canonical, evidence[1].canonical);
        assert_ne!(evidence[0].guids[0].1, evidence[1].guids[0].1);
        for name in ["Missing", "ValueAlias"] {
            let alias = snapshot
                .facts()
                .iter()
                .find(|fact| fact.kind == FactKind::Typedef && fact.name == name)
                .unwrap();
            assert!(!aliases.contains_key(&alias.origin));
        }
    }
}

#[test]
fn conflicting_uuid_is_a_native_error() {
    helpers::ensure_libclang();
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let target_arg = format!("--target={target}");
        let error = extract(
            [Input::new(
                "conflict.hpp",
                r#"
            typedef class X X;
            class __declspec(uuid("11111111-2222-3333-4455-66778899aabb")) X;
            class __declspec(uuid("aaaaaaaa-bbbb-cccc-ddee-ff0011223344")) X;
        "#,
            )],
            &["-x", "c++", "-fms-extensions", &target_arg],
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("uuid does not match previous declaration")
        );
    }
}

#[test]
fn mixed_record_keywords_preserve_native_facts_and_alias_disposition() {
    helpers::ensure_libclang();
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let target_arg = format!("--target={target}");
        let args = ["-x", "c++", "-fms-extensions", &target_arg];
        for (first, later, first_kind, later_kind) in [
            ("struct", "class", FactKind::Struct, FactKind::Class),
            ("class", "struct", FactKind::Class, FactKind::Struct),
        ] {
            let source = format!("typedef {first} X X;\n{later} X {{ public: int value; }};");
            let snapshot = extract([Input::new("allocation.hpp", source)], &args).unwrap();
            let records: Vec<_> = snapshot
                .facts()
                .iter()
                .filter(|fact| matches!(fact.kind, FactKind::Class | FactKind::Struct))
                .collect();
            assert_eq!(records.len(), 2);
            assert_eq!(records[0].kind, first_kind);
            assert_eq!(records[1].kind, later_kind);
            assert!(!records[0].definition);
            assert!(records[1].definition);
            assert_eq!(
                snapshot.class_canonical_origins().get(&records[1].origin),
                Some(&CanonicalRecord::Source(records[0].origin.clone()))
            );
            assert!(snapshot.coclass_aliases().unwrap().is_empty());
            assert!(snapshot.emit("Test").unwrap().contains("struct X"));

            let source = format!(
                "typedef {first} X X;\n{later} __declspec(uuid(\"11111111-2222-3333-4455-66778899aabb\")) X;\n{first} __declspec(uuid(\"11111111-2222-3333-4455-66778899aabb\")) X;"
            );
            let snapshot = extract([Input::new("guid.hpp", source.clone())], &args).unwrap();
            let records: Vec<_> = snapshot
                .facts()
                .iter()
                .filter(|fact| matches!(fact.kind, FactKind::Class | FactKind::Struct))
                .collect();
            assert_eq!(records.len(), 3);
            assert_eq!(records[0].kind, first_kind);
            assert_eq!(records[1].kind, later_kind);
            assert!(records.iter().all(|fact| !fact.definition));
            assert!(matches!(records[0].data, FactData::Record { .. }));
            assert!(matches!(records[1].data, FactData::Class { .. }));
            let aliases = snapshot.coclass_aliases().unwrap();
            assert_eq!(aliases.len(), 1);
            let evidence = aliases.values().next().unwrap();
            assert_eq!(
                evidence.canonical,
                CanonicalRecord::Source(records[0].origin.clone())
            );
            assert_eq!(evidence.guids.len(), 2);
            assert_eq!(evidence.guids[0].0, records[1].origin);
            assert_eq!(evidence.guids[1].0, records[2].origin);
            assert!(snapshot.emit("Test").unwrap().contains("const X: GUID"));
            let required = extract(
                [Input::new(
                    "required.hpp",
                    format!("{source}\nextern \"C\" void Use(X value);"),
                )],
                &args,
            )
            .unwrap();
            assert!(required.emit_with_library("Test", "test.dll").is_err());
        }

        let snapshot = extract([Input::new("unrelated.hpp", r#"
                namespace A { typedef struct X X; class X; }
                namespace B { typedef class X X; struct __declspec(uuid("11111111-2222-3333-4455-66778899aabb")) X; }
            "#)], &args).unwrap();
        let aliases = snapshot.coclass_aliases().unwrap();
        assert_eq!(aliases.len(), 1);
        let alias = snapshot
            .facts()
            .iter()
            .find(|fact| aliases.contains_key(&fact.origin))
            .unwrap();
        assert_eq!(
            alias.spelling.offset,
            snapshot
                .facts()
                .iter()
                .filter(|fact| fact.kind == FactKind::Typedef)
                .map(|fact| fact.spelling.offset)
                .max()
                .unwrap()
        );
    }
}

#[test]
fn compiler_created_record_identity_preserves_physical_declarations() {
    helpers::ensure_libclang();
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let target_arg = format!("--target={target}");
        let args = ["-x", "c++", "-fms-extensions", &target_arg];
        for (source, definitions) in [
            ("struct _GUID { unsigned int Data1; };", vec![true]),
            (
                "struct _GUID; struct _GUID { unsigned int Data1; };",
                vec![false, true],
            ),
            (
                "struct _GUID { unsigned int Data1; }; struct _GUID;",
                vec![true, false],
            ),
        ] {
            let snapshot = extract([Input::new("builtin.hpp", source)], &args).unwrap();
            let records: Vec<_> = snapshot
                .facts()
                .iter()
                .filter(|fact| fact.kind == FactKind::Struct)
                .collect();
            assert_eq!(records.len(), definitions.len());
            let identity = CanonicalRecord::CompilerGenerated {
                anchor: records[0].origin.clone(),
                kind: CanonicalRecordKind::Struct,
            };
            for (record, definition) in records.iter().zip(definitions) {
                assert_eq!(record.definition, definition);
                assert_eq!(record.kind, FactKind::Struct);
                assert_eq!(record.spelling.file, "builtin.hpp");
                assert_eq!(
                    snapshot.class_canonical_origins().get(&record.origin),
                    Some(&identity)
                );
                if definition {
                    let FactData::Record { fields, .. } = &record.data else {
                        panic!();
                    };
                    assert_eq!(fields.len(), 1);
                    assert_eq!(fields[0].name, "Data1");
                    assert_eq!(fields[0].ty, TypeRef::Scalar(windows_clang::Scalar::U32));
                }
            }
            assert!(snapshot.coclass_aliases().unwrap().is_empty());
            assert!(snapshot.emit("Test").unwrap().contains("struct _GUID"));
        }
        let snapshot = extract(
            [Input::new(
                "groups.hpp",
                r#"
                    struct _GUID; struct _GUID { unsigned int Data1; };
                    namespace A { struct _GUID; struct _GUID { int value; }; }
                    namespace B { struct _GUID; struct _GUID { double value; }; }
                "#,
            )],
            &args,
        )
        .unwrap();
        let records: Vec<_> = snapshot
            .facts()
            .iter()
            .filter(|fact| fact.kind == FactKind::Struct)
            .collect();
        assert_eq!(records.len(), 6);
        let identities: Vec<_> = records
            .iter()
            .map(|fact| &snapshot.class_canonical_origins()[&fact.origin])
            .collect();
        assert_eq!(identities[0], identities[1]);
        assert_eq!(identities[2], identities[3]);
        assert_eq!(identities[4], identities[5]);
        assert_ne!(identities[0], identities[2]);
        assert_ne!(identities[0], identities[4]);
        assert_ne!(identities[2], identities[4]);
        assert!(matches!(
            identities[0],
            CanonicalRecord::CompilerGenerated { .. }
        ));
        assert_eq!(
            identities[2],
            &CanonicalRecord::Source(records[2].origin.clone())
        );
        assert_eq!(
            identities[4],
            &CanonicalRecord::Source(records[4].origin.clone())
        );
    }
}
