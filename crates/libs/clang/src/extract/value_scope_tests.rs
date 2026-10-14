use super::*;
use com_pointer::{SemanticTypeRole, checked_semantic_scope};

const ARGS: [&str; 5] = [
    "-x",
    "c++",
    "-std=c++17",
    "--target=x86_64-pc-windows-msvc",
    "--no-default-config",
];

fn scope(
    snapshot: &Snapshot,
    declaration: &ValueDeclaration,
) -> Result<Vec<(SemanticTypeRole, String)>, String> {
    let facts = snapshot
        .facts
        .iter()
        .map(|fact| (&fact.origin, fact))
        .collect();
    checked_semantic_scope(
        snapshot,
        &facts,
        &declaration.origin,
        |fact| fact.name.clone(),
        || {
            format!(
                "native value `{}` at {}",
                declaration.name,
                origin(&declaration.origin)
            )
        },
    )
}

#[test]
fn scalar_semantic_scope_native_global_named_and_anonymous_enums() {
    helpers::ensure_libclang();
    let snapshot = extract(
        [Input::new(
            "value-scopes.hpp",
            "const int GLOBAL = 5;\n\
             namespace A { const int VALUE = 7; enum { FIRST = 11, SECOND = 13 }; }\n\
             namespace B { const int VALUE = 7; }\n\
             namespace { const int HIDDEN = 17; }\n\
             int MUTABLE = 19;\nextern const int UNEVALUABLE;\n",
        )],
        &ARGS,
    )
    .unwrap();
    assert_eq!(snapshot.value_declarations.len(), 5);
    let global = snapshot
        .value_declarations
        .iter()
        .find(|declaration| declaration.name == "GLOBAL")
        .unwrap();
    assert_eq!(snapshot.semantic_parents.get(&global.origin), Some(&None));
    assert_eq!(scope(&snapshot, global).unwrap(), vec![]);
    let values: Vec<_> = snapshot
        .value_declarations
        .iter()
        .filter(|declaration| declaration.name == "VALUE")
        .collect();
    assert_eq!(values.len(), 2);
    let first = scope(&snapshot, values[0]).unwrap();
    let second = scope(&snapshot, values[1]).unwrap();
    assert_eq!(first, vec![(SemanticTypeRole::Namespace, "A".into())]);
    assert_eq!(second, vec![(SemanticTypeRole::Namespace, "B".into())]);
    assert_ne!(first, second);
    let anonymous = snapshot
        .value_declarations
        .iter()
        .find(|declaration| declaration.kind == ValueDeclarationKind::AnonymousEnum)
        .unwrap();
    assert_eq!(scope(&snapshot, anonymous).unwrap(), first);
    let members: Vec<_> = snapshot
        .constants
        .iter()
        .filter(|constant| constant.root == anonymous.origin)
        .collect();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].definition, members[1].definition);
    assert_eq!(snapshot.semantic_parents.len(), 6);
    assert!(snapshot.value_declarations.iter().all(|declaration| {
        !snapshot
            .facts
            .iter()
            .any(|fact| fact.origin == declaration.origin)
    }));
    let hidden = snapshot
        .value_declarations
        .iter()
        .find(|declaration| declaration.name == "HIDDEN")
        .unwrap();
    assert!(!snapshot.semantic_parents.contains_key(&hidden.origin));
    let error = scope(&snapshot, hidden).unwrap_err();
    assert!(error.contains("no captured native semantic-parent relation"));
    assert!(error.contains(&origin(&hidden.origin)));
    assert!(
        !snapshot
            .constants
            .iter()
            .any(|constant| { matches!(constant.name.as_str(), "MUTABLE" | "UNEVALUABLE") })
    );
    let output = snapshot.emit("Test").unwrap();
    let mut without_value_relations = snapshot.clone();
    for declaration in &snapshot.value_declarations {
        without_value_relations
            .semantic_parents
            .remove(&declaration.origin);
    }
    assert_eq!(without_value_relations.emit("Test").unwrap(), output);
    assert_eq!(without_value_relations.constants, snapshot.constants);
    assert_eq!(without_value_relations.facts, snapshot.facts);
    assert_eq!(without_value_relations.annotations, snapshot.annotations);
    let namespace = snapshot
        .facts
        .iter()
        .find(|fact| fact.kind == FactKind::Namespace && fact.name == "A")
        .unwrap();
    let facts = snapshot
        .facts
        .iter()
        .map(|fact| (&fact.origin, fact))
        .collect();
    let diagnostic_constructions = Cell::new(0);
    let subject = || {
        diagnostic_constructions.set(diagnostic_constructions.get() + 1);
        format!("required type `{}`", namespace.name)
    };
    assert_eq!(
        checked_semantic_scope(
            &snapshot,
            &facts,
            &namespace.origin,
            |fact| fact.name.clone(),
            subject,
        )
        .unwrap(),
        vec![]
    );
    assert_eq!(diagnostic_constructions.get(), 0);
    without_value_relations
        .semantic_parents
        .remove(&namespace.origin);
    let missing = checked_semantic_scope(
        &without_value_relations,
        &facts,
        &namespace.origin,
        |fact| fact.name.clone(),
        subject,
    )
    .unwrap_err();
    assert_eq!(diagnostic_constructions.get(), 1);
    assert_eq!(
        missing,
        format!(
            "required type `A` has no captured native semantic-parent relation for {}",
            origin(&namespace.origin)
        )
    );
    eprintln!("type_scope_subject_constructions success=0 missing=1 error={missing}");
    eprintln!(
        "global={:?} A={first:?} B={second:?} enum_origin={:?} siblings={} hidden={error}",
        global.origin,
        anonymous.origin,
        members.len()
    );
}

#[test]
fn scalar_semantic_scope_native_out_of_line_matches_in_class_context() {
    helpers::ensure_libclang();
    let snapshot = extract(
        [
            Input::new(
                "in-class.hpp",
                "namespace A { struct C { static const int VALUE = 7; }; }\n",
            ),
            Input::new(
                "out-of-line.hpp",
                "\n\nnamespace A { struct C { static const int VALUE; }; }\n\
                 const int A::C::VALUE = 7;\n",
            ),
        ],
        &ARGS,
    )
    .unwrap();
    let values: Vec<_> = snapshot
        .value_declarations
        .iter()
        .filter(|declaration| declaration.name == "VALUE")
        .collect();
    assert_eq!(values.len(), 2, "{:?}", snapshot.value_declarations);
    assert_eq!(values[0].origin.tu, "in-class.hpp");
    assert_eq!(values[1].origin.tu, "out-of-line.hpp");
    assert!(values[0].parent.is_some());
    assert_eq!(values[1].parent, None);
    assert_ne!(values[0].spelling.offset, values[1].spelling.offset);
    let expected = vec![
        (SemanticTypeRole::Namespace, "A".into()),
        (SemanticTypeRole::Record, "C".into()),
    ];
    for declaration in &values {
        assert_eq!(scope(&snapshot, declaration).unwrap(), expected);
        let parent = snapshot.semantic_parents[&declaration.origin]
            .as_ref()
            .unwrap();
        assert_eq!(parent.tu, declaration.origin.tu);
        assert!(snapshot.facts.iter().any(|fact| {
            fact.origin == *parent && fact.name == "C" && fact.kind == FactKind::Struct
        }));
        eprintln!("captured={declaration:?} semantic_parent={parent:?} checked_scope={expected:?}");
    }
    assert_eq!(snapshot.constants.len(), 2);
    assert!(snapshot.constants.iter().all(|constant| {
        constant.value == Value::Signed(7) && constant.ty == TypeRef::Scalar(Scalar::I32)
    }));
}

#[test]
fn scalar_semantic_scope_defensive_missing_cyclic_cross_tu_and_unsupported() {
    helpers::ensure_libclang();
    let original = extract(
        [Input::new(
            "value-defensive.hpp",
            "namespace A { const int VALUE = 7; }\n",
        )],
        &ARGS,
    )
    .unwrap();
    let declaration = &original.value_declarations[0];
    let parent = original.semantic_parents[&declaration.origin]
        .as_ref()
        .unwrap();
    let output = original.emit("Test").unwrap();
    for case in [
        "missing-declaration",
        "missing-ancestor",
        "missing-fact",
        "cycle",
        "cross-tu",
        "unsupported",
    ] {
        let mut snapshot = original.clone();
        match case {
            "missing-declaration" => {
                snapshot.semantic_parents.remove(&declaration.origin);
            }
            "missing-ancestor" => {
                snapshot.semantic_parents.remove(parent);
            }
            "missing-fact" => snapshot.facts.retain(|fact| fact.origin != *parent),
            "cycle" => {
                snapshot
                    .semantic_parents
                    .insert(parent.clone(), Some(parent.clone()));
            }
            "cross-tu" => {
                snapshot.semantic_parents.insert(
                    declaration.origin.clone(),
                    Some(Origin {
                        tu: "foreign.hpp".into(),
                        local: parent.local,
                    }),
                );
            }
            "unsupported" => {
                snapshot
                    .facts
                    .iter_mut()
                    .find(|fact| fact.origin == *parent)
                    .unwrap()
                    .kind = FactKind::Function;
            }
            _ => unreachable!(),
        }
        let error = scope(&snapshot, declaration).unwrap_err();
        assert!(error.contains("native value `VALUE`"));
        assert!(error.contains(&origin(&declaration.origin)));
        assert!(error.contains(match case {
            "missing-declaration" | "missing-ancestor" => {
                "no captured native semantic-parent relation"
            }
            "missing-fact" => "no captured native semantic parent fact",
            "cycle" | "cross-tu" => "cyclic or cross-TU native semantic scope",
            "unsupported" => "unsupported native semantic parent Function",
            _ => unreachable!(),
        }));
        if case != "unsupported" {
            assert_eq!(snapshot.emit("Test").unwrap(), output, "{case}");
        }
        eprintln!("private_fixture={case} unproved={error}");
    }
}

#[test]
fn scalar_semantic_scope_optional_native_registry_ambiguity_preserves_type_error() {
    helpers::ensure_libclang();
    let _library = Library::new().unwrap();
    let index = Index::new().unwrap();
    let input = Input::new(
        "value-registry.hpp",
        "namespace A { const int VALUE = 7; struct Item {}; }\n",
    );
    let tu = TranslationUnit::parse(&index, &input, &ARGS).unwrap();
    let namespace = cursor_children(unsafe { clang_getTranslationUnitCursor(tu.0) })
        .into_iter()
        .find(|cursor| unsafe { clang_getCursorKind(*cursor) } == CXCursor_Namespace)
        .unwrap();
    let children = cursor_children(namespace);
    let value = *children
        .iter()
        .find(|cursor| unsafe { clang_getCursorKind(**cursor) } == CXCursor_VarDecl)
        .unwrap();
    let record = *children
        .iter()
        .find(|cursor| unsafe { clang_getCursorKind(**cursor) } == CXCursor_StructDecl)
        .unwrap();
    let origin = |local| Origin {
        tu: input.name.clone(),
        local,
    };
    let pending = [(value, origin(1))];
    let hash = unsafe { clang_hashCursor(namespace) };
    let mut seen: HashMap<u32, Vec<(CXCursor, Origin)>> = HashMap::new();
    seen.entry(hash)
        .or_default()
        .extend([(namespace, origin(0)), (record, origin(2))]);
    seen.entry(unsafe { clang_hashCursor(record) })
        .or_default()
        .push((record, origin(2)));
    let mut output = BTreeMap::new();
    capture_value_semantic_parents(tu.0, &pending, &seen, &mut output);
    assert_eq!(output.get(&origin(1)), Some(&Some(origin(0))));
    let mut conflicting_binding = BTreeMap::from([(origin(2), None)]);
    assert_eq!(
        capture_semantic_parents(tu.0, &seen, &mut conflicting_binding)
            .unwrap_err()
            .to_string(),
        "conflicting native semantic parent binding"
    );
    seen.get_mut(&hash).unwrap().push((namespace, origin(3)));
    let mut ambiguous = BTreeMap::new();
    capture_value_semantic_parents(tu.0, &pending, &seen, &mut ambiguous);
    assert!(!ambiguous.contains_key(&origin(1)));
    let error = capture_semantic_parents(tu.0, &seen, &mut BTreeMap::new()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "conflicting exact native semantic parent origins"
    );
    for case in ["missing", "cross-tu-origin", "foreign-native-tu"] {
        let mut registry = seen.clone();
        let mut expected_tu = tu.0;
        let foreign = TranslationUnit::parse(
            &index,
            &Input::new("foreign.hpp", "const int FOREIGN = 1;"),
            &ARGS,
        )
        .unwrap();
        match case {
            "missing" => registry.clear(),
            "cross-tu-origin" => {
                registry.insert(
                    hash,
                    vec![(
                        namespace,
                        Origin {
                            tu: "foreign.hpp".into(),
                            local: 0,
                        },
                    )],
                );
            }
            "foreign-native-tu" => expected_tu = foreign.0,
            _ => unreachable!(),
        }
        let mut unproved = BTreeMap::new();
        capture_value_semantic_parents(expected_tu, &pending, &registry, &mut unproved);
        assert!(!unproved.contains_key(&origin(1)), "{case}");
        eprintln!("private_registry={case} value_relation=absent");
    }
    eprintln!("private_registry=ambiguous value_relation=absent strict_type_error={error}");
}
