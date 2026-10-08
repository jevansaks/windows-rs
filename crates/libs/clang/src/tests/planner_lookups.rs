use super::*;

fn snapshot(facts: Vec<Fact>) -> Snapshot {
    Snapshot {
        facts,
        constants: Vec::new(),
        included_files: Vec::new(),
        raw_function_link_names: BTreeMap::new(),
        canonical_function_origins: BTreeMap::new(),
        canonical_typedef_origins: BTreeMap::new(),
        function_link_name_index: FunctionLinkNameIndex::default(),
        declare_handles: Vec::new(),
        annotations: BTreeMap::new(),
        source_annotations: BTreeMap::new(),
        declaration_guids: BTreeMap::new(),
        pointer_callback_aliases: BTreeSet::new(),
        pointer_only_class_layouts: BTreeMap::new(),
        embeddable_class_layouts: BTreeSet::new(),
        clang_flag_enums: BTreeSet::new(),
        recovery_suppressed_origins: BTreeSet::new(),
        root_owners: BTreeMap::new(),
        constant_root_owners: BTreeMap::new(),
        root_partitions: BTreeMap::new(),
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
        header_authority_partition: None,
        timing_target: None,
    }
}

#[test]
fn function_link_name_resolver_uses_prebuilt_index() {
    let mut snapshot = snapshot(Vec::new());
    snapshot
        .function_link_name_index
        .insert("_IndexedFunction", "IndexedFunction");

    assert!(snapshot.facts().is_empty());
    assert!(snapshot.function_source_identities().next().is_none());
    assert!(snapshot.raw_function_link_names.is_empty());
    assert_eq!(
        snapshot
            .resolve_function_link_name("_IndexedFunction")
            .unwrap(),
        Some("IndexedFunction")
    );
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

fn function(local: u32, name: &str, link_name: &str, parameter: Scalar) -> Fact {
    test_fact(
        local,
        None,
        FactKind::Function,
        name,
        Location {
            file: "functions.h".to_string(),
            offset: local,
        },
        FactData::Function {
            link_name: link_name.to_string(),
            convention: CallingConvention::Platform,
            params: vec![Parameter {
                name: "value".to_string(),
                ty: TypeRef::Scalar(parameter),
                annotation: ParamAnnotation::default(),
            }],
            result: TypeRef::Void,
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
fn canonical_function_alias_requires_compatible_declarations() {
    let alias = function(1, "Alias", "Export", Scalar::I32);
    let export = function(2, "Export", "Export", Scalar::U32);
    let canonical = alias.origin.clone();
    let mut snapshot = snapshot(vec![alias, export]);
    for fact in &snapshot.facts {
        snapshot
            .canonical_function_origins
            .insert(fact.origin.clone(), canonical.clone());
    }

    assert!(
        snapshot
            .redundant_canonical_function_aliases(snapshot.facts.iter(), None)
            .is_empty()
    );
}

#[test]
fn canonical_function_alias_requires_matching_native_imports() {
    let alias = function(1, "Alias", "Export", Scalar::I32);
    let export = function(2, "Export", "Export", Scalar::I32);
    let canonical = alias.origin.clone();
    let mut snapshot = snapshot(vec![alias, export]);
    for fact in &snapshot.facts {
        snapshot
            .canonical_function_origins
            .insert(fact.origin.clone(), canonical.clone());
    }
    snapshot.annotations.insert(
        AnnotationTarget::Declaration(snapshot.facts[0].origin.clone()),
        vec![Annotation::ImportLibrary("alias.dll".to_string())],
    );
    snapshot.annotations.insert(
        AnnotationTarget::Declaration(snapshot.facts[1].origin.clone()),
        vec![Annotation::ImportLibrary("export.dll".to_string())],
    );

    assert!(
        snapshot
            .redundant_canonical_function_aliases(snapshot.facts.iter(), None)
            .is_empty()
    );
}

#[test]
fn canonical_function_alias_uses_unique_later_native_import() {
    let alias = function(1, "Alias", "Export", Scalar::I32);
    let export = function(2, "Export", "Export", Scalar::I32);
    let alias_origin = alias.origin.clone();
    let export_origin = export.origin.clone();
    let canonical = alias_origin.clone();
    let mut snapshot = snapshot(vec![alias, export]);
    snapshot
        .canonical_function_origins
        .insert(alias_origin.clone(), canonical.clone());
    snapshot
        .canonical_function_origins
        .insert(export_origin.clone(), canonical);
    snapshot.annotations.insert(
        AnnotationTarget::Declaration(export_origin),
        vec![Annotation::ImportLibrary("export.dll".to_string())],
    );

    assert_eq!(
        snapshot.redundant_canonical_function_aliases(snapshot.facts.iter(), None),
        BTreeSet::from([alias_origin])
    );
}

#[test]
fn canonical_function_alias_does_not_supply_library_to_export() {
    let alias = function(1, "Alias", "Export", Scalar::I32);
    let export = function(2, "Export", "Export", Scalar::I32);
    let canonical = alias.origin.clone();
    let mut snapshot = snapshot(vec![alias, export]);
    for fact in &snapshot.facts {
        snapshot
            .canonical_function_origins
            .insert(fact.origin.clone(), canonical.clone());
    }
    snapshot.annotations.insert(
        AnnotationTarget::Declaration(snapshot.facts[0].origin.clone()),
        vec![Annotation::ImportLibrary("alias.dll".to_string())],
    );

    assert!(
        snapshot
            .redundant_canonical_function_aliases(snapshot.facts.iter(), None)
            .is_empty()
    );
}

#[test]
fn canonical_function_alias_keeps_matching_pair_despite_third_library() {
    let matching_alias = function(1, "MatchingAlias", "Export", Scalar::I32);
    let export = function(2, "Export", "Export", Scalar::I32);
    let conflicting_alias = function(3, "ConflictingAlias", "Export", Scalar::I32);
    let matching_origin = matching_alias.origin.clone();
    let canonical = matching_origin.clone();
    let mut snapshot = snapshot(vec![matching_alias, export, conflicting_alias]);
    for fact in &snapshot.facts {
        snapshot
            .canonical_function_origins
            .insert(fact.origin.clone(), canonical.clone());
    }
    for (index, library) in [(0, "export.dll"), (1, "export.dll"), (2, "other.dll")] {
        snapshot.annotations.insert(
            AnnotationTarget::Declaration(snapshot.facts[index].origin.clone()),
            vec![Annotation::ImportLibrary(library.to_string())],
        );
    }

    assert_eq!(
        snapshot.redundant_canonical_function_aliases(snapshot.facts.iter(), None),
        BTreeSet::from([matching_origin])
    );
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
fn canonical_typedef_representative_prefers_rooted_fact() {
    let mut unrooted = alias(
        1,
        "LPDISPATCH",
        "a-included.h",
        1,
        TypeRef::Scalar(Scalar::U32),
    );
    unrooted.root = false;
    let mut rooted = alias(
        2,
        "LPDISPATCH",
        "z-traversed.h",
        1,
        TypeRef::Scalar(Scalar::U32),
    );
    rooted.root = true;

    assert!(canonical_typedef_fact_cmp(&rooted, &unrooted).is_lt());
}

#[test]
fn canonical_pointer_typedef_conflicting_annotations_are_not_coalesced() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [Input::new(
            "conflicting.cpp",
            "#define W32M(value) __attribute__((annotate(value)))\n\
             struct __declspec(uuid(\"12345678-1234-5678-90ab-cdef12345678\")) \
                 __declspec(novtable) IDispatch {\n\
                 virtual int Invoke() = 0;\n\
             };\n\
             typedef IDispatch *LPDISPATCH \
                 W32M(\"win32metadata:raii_free=CloseFirst\");\n\
             typedef IDispatch *LPDISPATCH \
                 W32M(\"win32metadata:raii_free=CloseSecond\");\n",
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let aliases = snapshot
        .facts
        .iter()
        .filter(|fact| fact.name == "LPDISPATCH")
        .collect::<Vec<_>>();
    assert_eq!(aliases.len(), 2);
    assert_eq!(
        snapshot.canonical_typedef_origins[&aliases[0].origin],
        snapshot.canonical_typedef_origins[&aliases[1].origin]
    );
    let annotations = snapshot.source_annotation_signatures();
    assert_ne!(
        annotations.get(&aliases[0].origin),
        annotations.get(&aliases[1].origin)
    );
    let canonical_typedefs = CanonicalTypedefIndex::new(
        &snapshot.facts,
        &snapshot.canonical_typedef_origins,
        &annotations,
        None,
    );
    assert!(
        aliases
            .iter()
            .all(|fact| canonical_typedefs.representative(&fact.origin).is_none())
    );
}

#[test]
fn canonical_pointer_typedef_identity_is_tu_scoped_and_type_exact() {
    helpers::ensure_libclang();

    let source = "#define W32M(value) __attribute__((annotate(value)))\n\
                  #define __RPC_unique_pointer W32M(\"_Maybenull_\")\n\
                  struct IDispatch;\n\
                  typedef struct IDispatch IDispatch;\n\
                  typedef /* [unique] */ __RPC_unique_pointer IDispatch *LPDISPATCH;\n\
                  struct IDispatch { virtual int Invoke() = 0; };\n\
                  typedef /* [unique] */ __RPC_unique_pointer IDispatch *LPDISPATCH;\n\
                  namespace First { typedef IDispatch *SAME_POINTER; }\n\
                  namespace Second { typedef IDispatch *SAME_POINTER; }\n\
                  namespace Mutable { typedef IDispatch *QUALIFIED_POINTER; }\n\
                  namespace Immutable { typedef const IDispatch *QUALIFIED_POINTER; }\n\
                  namespace DispatchTarget { typedef IDispatch *TARGET_POINTER; }\n\
                  namespace OtherTarget {\n\
                      struct IOther;\n\
                      typedef IOther *TARGET_POINTER;\n\
                  }\n\
                  namespace AnnotatedFirst {\n\
                      typedef IDispatch *ANNOTATED_POINTER \
                          W32M(\"win32metadata:raii_free=CloseFirst\");\n\
                  }\n\
                  namespace AnnotatedSecond {\n\
                      typedef IDispatch *ANNOTATED_POINTER \
                          W32M(\"win32metadata:raii_free=CloseSecond\");\n\
                  }\n";
    let snapshot = extract(
        [
            Input::new("first.cpp", source),
            Input::new("second.cpp", source),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();

    let aliases = snapshot
        .facts
        .iter()
        .filter(|fact| fact.name == "LPDISPATCH")
        .collect::<Vec<_>>();
    assert_eq!(aliases.len(), 4);
    let first = aliases
        .iter()
        .copied()
        .filter(|fact| fact.origin.tu == "first.cpp")
        .collect::<Vec<_>>();
    let second = aliases
        .iter()
        .copied()
        .filter(|fact| fact.origin.tu == "second.cpp")
        .collect::<Vec<_>>();
    assert_eq!(first.len(), 2);
    assert_eq!(second.len(), 2);
    assert_eq!(
        snapshot.canonical_typedef_origins[&first[0].origin],
        snapshot.canonical_typedef_origins[&first[1].origin]
    );
    assert_eq!(
        snapshot.canonical_typedef_origins[&second[0].origin],
        snapshot.canonical_typedef_origins[&second[1].origin]
    );
    assert_ne!(
        snapshot.canonical_typedef_origins[&first[0].origin],
        snapshot.canonical_typedef_origins[&second[0].origin]
    );
    let route_annotations = snapshot.source_annotation_signatures();
    let canonical_typedefs = CanonicalTypedefIndex::new(
        &snapshot.facts,
        &snapshot.canonical_typedef_origins,
        &route_annotations,
        None,
    );
    assert_eq!(
        canonical_typedefs.representative(&first[0].origin),
        canonical_typedefs.representative(&first[1].origin)
    );
    assert_eq!(
        canonical_typedefs.representative(&second[0].origin),
        canonical_typedefs.representative(&second[1].origin)
    );
    assert_ne!(
        canonical_typedefs.representative(&first[0].origin),
        canonical_typedefs.representative(&second[0].origin)
    );

    for name in [
        "SAME_POINTER",
        "QUALIFIED_POINTER",
        "TARGET_POINTER",
        "ANNOTATED_POINTER",
    ] {
        let facts = snapshot
            .facts
            .iter()
            .filter(|fact| fact.origin.tu == "first.cpp" && fact.name == name)
            .collect::<Vec<_>>();
        assert_eq!(facts.len(), 2, "{name}: {facts:#?}");
        assert_ne!(
            snapshot.canonical_typedef_origins[&facts[0].origin],
            snapshot.canonical_typedef_origins[&facts[1].origin],
            "{name}: {facts:#?}"
        );
        assert!(
            facts
                .iter()
                .all(|fact| canonical_typedefs.representative(&fact.origin).is_none()),
            "{name}: {facts:#?}"
        );
    }

    let facts = |name: &str| {
        snapshot
            .facts
            .iter()
            .filter(|fact| fact.origin.tu == "first.cpp" && fact.name == name)
            .collect::<Vec<_>>()
    };
    let same = facts("SAME_POINTER");
    assert_eq!(same[0].data, same[1].data);
    let qualified = facts("QUALIFIED_POINTER");
    assert_ne!(qualified[0].data, qualified[1].data);
    let target = facts("TARGET_POINTER");
    assert_ne!(target[0].data, target[1].data);
    let annotated = facts("ANNOTATED_POINTER");
    assert_eq!(annotated[0].data, annotated[1].data);
    assert_ne!(
        route_annotations.get(&annotated[0].origin),
        route_annotations.get(&annotated[1].origin)
    );
}

#[test]
fn canonical_pointer_typedef_owner_survives_normalized_target_variants() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-canonical-owner-unit-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let oaidl = scratch.join("oaidl.h");
    let oleauto = scratch.join("oleauto.h");
    let api = scratch.join("api.h");
    std::fs::write(
        &oaidl,
        "#pragma once\n\
         #define __RPC_unique_pointer __attribute__((annotate(\"_Maybenull_\")))\n\
         struct IUnknown { virtual int QueryInterface() = 0; };\n\
         struct IDispatch;\n\
         typedef struct IDispatch IDispatch;\n\
         typedef /* [unique] */ __RPC_unique_pointer IDispatch *LPDISPATCH;\n\
         struct __declspec(uuid(\"12345678-1234-5678-90ab-cdef12345678\")) \
             __declspec(novtable) IDispatch : public IUnknown {\n\
             virtual int Invoke(int value) = 0;\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &oleauto,
        "#pragma once\n\
         #include \"oaidl.h\"\n\
         typedef /* [unique] */ __RPC_unique_pointer IDispatch *LPDISPATCH;\n",
    )
    .unwrap();
    std::fs::write(
        &api,
        "#pragma once\n\
         #include \"oleauto.h\"\n\
         extern \"C\" LPDISPATCH UseDispatch(LPDISPATCH value);\n",
    )
    .unwrap();

    let extract_snapshot = |source: String| {
        let mut snapshot = extract(
            [Input::new("aggregate.cpp", source)
                .with_root_dirs([scratch.to_string_lossy().to_string()])],
            &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
        )
        .unwrap();
        let definition = snapshot
            .facts
            .iter()
            .find(|fact| {
                fact.name == "IDispatch"
                    && fact.definition
                    && matches!(fact.data, FactData::Interface { .. })
            })
            .unwrap()
            .spelling
            .clone();
        let aliases = snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "LPDISPATCH")
            .map(|fact| fact.origin.clone())
            .collect::<Vec<_>>();
        assert_eq!(aliases.len(), 2);
        assert_eq!(
            snapshot.canonical_typedef_origins[&aliases[0]],
            snapshot.canonical_typedef_origins[&aliases[1]]
        );
        let alias = snapshot
            .facts
            .iter_mut()
            .find(|fact| {
                fact.name == "LPDISPATCH"
                    && fact.spelling.file == oleauto.to_string_lossy().replace('\\', "/")
            })
            .unwrap();
        let FactData::Typedef {
            target:
                TypeRef::Pointer {
                    target: pointer_target,
                    ..
                },
        } = &mut alias.data
        else {
            panic!("LPDISPATCH was not extracted as a pointer typedef");
        };
        let TypeRef::Named { declaration, .. } = pointer_target.as_mut() else {
            panic!("LPDISPATCH did not retain the IDispatch declaration");
        };
        declaration.clone_from(&definition);
        snapshot
    };
    let include = |path: &std::path::Path| format!("#include \"{}\"\n", path.to_string_lossy());
    let forward = extract_snapshot(format!(
        "{}{}{}",
        include(&oaidl),
        include(&oleauto),
        include(&api)
    ));
    let reverse = extract_snapshot(format!(
        "{}{}{}",
        include(&api),
        include(&oleauto),
        include(&oaidl)
    ));

    let comole_partition = RootPartition::new("ComOle", "Windows.Win32.System.Ole");
    let api_partition =
        RootPartition::new("api", "Example.Api").with_library("UseDispatch", "api.dll");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(oaidl.to_string_lossy(), comole_partition.clone())
        .with_traversed_header(oleauto.to_string_lossy(), comole_partition.clone())
        .with_traversed_header(api.to_string_lossy(), api_partition.clone());
    let reverse_policy = HeaderPartitionPolicy::new()
        .with_traversed_header(api.to_string_lossy(), api_partition)
        .with_traversed_header(oleauto.to_string_lossy(), comole_partition.clone())
        .with_traversed_header(oaidl.to_string_lossy(), comole_partition);
    let authorities =
        NamespaceAuthorities::new().with_exact("IDispatch", "Windows.Win32.System.Com");
    let references = BTreeMap::new();
    let options = EmitOptions::new("Windows.Win32", &references);
    let emit = |snapshot: Snapshot, policy: &HeaderPartitionPolicy| {
        let plan = snapshot
            .plan_header_partitions(policy, &authorities)
            .unwrap();
        assert!(plan.audit(&options).unwrap().is_clean());
        plan.emit_with_options(&options).unwrap()
    };
    let partitions = emit(forward, &policy);
    assert_eq!(partitions, emit(reverse, &reverse_policy));
    assert_eq!(
        partitions
            .values()
            .map(|rdl| {
                rdl.matches("type LPDISPATCH = Windows::Win32::System::Com::IDispatch")
                    .count()
            })
            .sum::<usize>(),
        1,
        "{partitions:#?}"
    );
    assert!(
        partitions.values().any(|rdl| rdl.contains(
            "fn UseDispatch(value: Windows::Win32::System::Ole::LPDISPATCH) -> \
                 Windows::Win32::System::Ole::LPDISPATCH"
        )),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
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
