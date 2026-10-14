use super::*;
use windows_metadata::HasAttributes;

pub(super) fn observe(
    tu: CXTranslationUnit,
    cursor: CXCursor,
    origin: &Origin,
    seen: &HashMap<u32, Vec<(CXCursor, Origin)>>,
) {
    if !origin.tu.starts_with("linkage-probe-") {
        return;
    }
    let mut current = cursor;
    let mut visited = Vec::new();
    for depth in 0..16 {
        let parent = unsafe { clang_getCursorSemanticParent(current) };
        let kind = unsafe { clang_getCursorKind(parent) };
        let null = unsafe { clang_Cursor_isNull(parent) } != 0;
        let same_tu = unsafe { clang_Cursor_getTranslationUnit(parent) } == tu;
        let matches: Vec<_> = seen
            .get(&unsafe { clang_hashCursor(parent) })
            .into_iter()
            .flatten()
            .filter(|(candidate, _)| unsafe { clang_equalCursors(parent, *candidate) } != 0)
            .map(|(_, origin)| origin)
            .collect();
        eprintln!(
            "LINKAGE_NATIVE origin={origin:?} name={:?} depth={depth} kind={kind} \
             null={null} cursor_same_tu={} parent_same_tu={same_tu} matches={matches:?}",
            cx_string(unsafe { clang_getCursorSpelling(cursor) }),
            unsafe { clang_Cursor_getTranslationUnit(current) } == tu,
        );
        if null
            || kind == CXCursor_TranslationUnit
            || visited
                .iter()
                .any(|previous| unsafe { clang_equalCursors(*previous, parent) } != 0)
        {
            break;
        }
        visited.push(parent);
        current = parent;
    }
}

fn plan(reversed: bool, demand: bool, collision: bool) -> HeaderPartitionPlan {
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("extract")
        .join("linkage_scope_tests.hpp");
    let header = header.to_str().unwrap();
    let source = |alternative| {
        format!(
            "{}{}{}#include {header:?}\n",
            if alternative {
                "#define LINKAGE_ALTERNATIVE\n"
            } else {
                ""
            },
            if demand {
                "#define LINKAGE_DEMAND\n"
            } else {
                ""
            },
            if collision {
                "#define LINKAGE_COLLISION\n"
            } else {
                ""
            },
        )
    };
    let mut inputs = vec![
        Input::new("linkage-probe-home.cpp", source(false))
            .with_roots([header])
            .partitioned("Home"),
        Input::new("linkage-probe-alternative.cpp", source(true))
            .with_roots([header])
            .partitioned("Alternative"),
    ];
    if reversed {
        inputs.reverse();
    }
    let policy = HeaderPartitionPolicy::new()
        .with_partition_authority("Home", 0, "linkage-probe-home.cpp")
        .with_partition_authority("Alternative", 1, "linkage-probe-alternative.cpp")
        .with_traversed_header(
            header,
            RootPartition::new("Home", "Test.Home").with_exclusion("Excluded"),
        )
        .with_traversed_header(header, RootPartition::new("Alternative", "Test.Other"));
    extract_ranked_header_partitions(
        inputs,
        &[
            "--no-default-config",
            "-x",
            "c++",
            "-std=c++17",
            "--target=x86_64-pc-windows-msvc",
            "-nostdinc",
        ],
        &policy,
        &NamespaceAuthorities::new(),
    )
    .unwrap()
}

fn assert_scope(plan: &HeaderPartitionPlan, collision: bool) {
    for tu in ["linkage-probe-home.cpp", "linkage-probe-alternative.cpp"] {
        let facts: Vec<_> = plan
            .snapshot()
            .facts()
            .iter()
            .filter(|fact| fact.origin.tu == tu)
            .collect();
        for name in [
            "Plain",
            "Wrapped",
            "Nested",
            "PlainInterface",
            "WrappedInterface",
        ] {
            let declarations: Vec<_> = facts.iter().filter(|fact| fact.name == name).collect();
            assert!(!declarations.is_empty(), "{tu} {name}");
            for fact in declarations {
                assert_eq!(
                    plan.snapshot.semantic_parents.get(&fact.origin),
                    Some(&None)
                );
            }
        }
        let mut named_parents = Vec::new();
        for (name, namespace) in if collision {
            [("Named", "A"), ("Named", "B")]
        } else {
            [("NamedA", "A"), ("NamedB", "B")]
        } {
            let parent = facts.iter().find(|fact| fact.name == namespace).unwrap();
            let children: Vec<_> = facts
                .iter()
                .filter(|fact| fact.name == name)
                .filter(|fact| {
                    plan.snapshot.semantic_parents.get(&fact.origin)
                        == Some(&Some(parent.origin.clone()))
                })
                .collect();
            assert_eq!(children.len(), 1, "{tu} {namespace}::{name}");
            named_parents.push(&parent.origin);
            assert_eq!(plan.snapshot.semantic_parents[&parent.origin], None);
        }
        assert_ne!(named_parents[0], named_parents[1]);
        let outer = facts.iter().find(|fact| fact.name == "Outer").unwrap();
        let a = facts.iter().find(|fact| fact.name == "A").unwrap();
        assert_eq!(
            plan.snapshot.semantic_parents[&outer.origin],
            Some(a.origin.clone())
        );
        let inner: Vec<_> = facts.iter().filter(|fact| fact.name == "Inner").collect();
        assert_eq!(inner.len(), 2);
        assert_eq!(inner.iter().filter(|fact| fact.definition).count(), 1);
        for fact in inner {
            assert_eq!(
                plan.snapshot.semantic_parents[&fact.origin],
                Some(outer.origin.clone())
            );
        }
    }
}

#[test]
fn linkage_scope_ranked_native_causal_control() {
    helpers::ensure_libclang();
    let mut baseline = None;
    for reversed in [false, true] {
        let plan = plan(reversed, false, false);
        assert_scope(&plan, false);
        for fact in plan.snapshot().facts() {
            eprintln!(
                "LINKAGE_MAP origin={:?} name={} lexical={:?} semantic={:?}",
                fact.origin,
                fact.name,
                fact.parent,
                plan.snapshot.semantic_parents.get(&fact.origin),
            );
        }
        let references = BTreeMap::new();
        let options = EmitOptions::new("Test.Common", &references);
        let report = plan.emit_with_report(&options).unwrap();
        let text = report.partitions.values().cloned().collect::<String>();
        assert!(text.contains("struct WrappedAlias {"), "{text}");
        assert!(text.contains("struct NestedAlias {"), "{text}");
        assert!(text.contains("interface WrappedInterface"), "{text}");
        assert!(!text.contains("struct Excluded"), "{text}");
        for name in [
            "Plain",
            "Wrapped",
            "Nested",
            "PlainInterface",
            "WrappedInterface",
        ] {
            let rows: Vec<_> = report
                .definition_evidence
                .iter()
                .filter(|row| row.identity.name == name)
                .collect();
            assert_eq!(rows.len(), 1, "{report:?}");
            assert!(matches!(
                rows[0].outcome,
                DefinitionComparison::Different { .. }
            ));
            assert_eq!(rows[0].selected.input, "linkage-probe-home.cpp");
            assert_eq!(rows[0].selected.partition.as_deref(), Some("Home"));
            assert_eq!(rows[0].selected.rank, Some(0));
            assert_eq!(rows[0].alternative.input, "linkage-probe-alternative.cpp");
            assert_eq!(rows[0].alternative.rank, Some(1));
            assert_eq!(text.matches(&rows[0].message).count(), 1);
        }
        eprintln!("LINKAGE_RDL reversed={reversed} text={text:?}");
        let directory = std::env::temp_dir().join(format!(
            "windows-clang-linkage-{}-{reversed}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let file = directory.join("linkage.winmd");
        windows_rdl::reader()
            .input_texts(report.partitions.values())
            .reference_default()
            .output(&file)
            .write()
            .unwrap();
        let metadata = windows_metadata::reader::Index::read(&file).unwrap();
        for name in ["PlainAlias", "WrappedAlias", "NestedAlias"] {
            let record = metadata.expect("Test.Home", name);
            let fields: Vec<_> = record.fields().collect();
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].name(), "value");
            assert_eq!(fields[0].ty(), windows_metadata::Type::I32);
        }
        for (name, expected) in [
            ("NamedA", windows_metadata::Type::I32),
            ("NamedB", windows_metadata::Type::U32),
            ("Inner", windows_metadata::Type::I32),
        ] {
            let record = metadata.expect("Test.Home", name);
            let fields: Vec<_> = record.fields().collect();
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].ty(), expected);
        }
        for (name, guid) in [("PlainInterface", 1), ("WrappedInterface", 2)] {
            let interface = metadata.expect("Test.Home", name);
            let methods: Vec<_> = interface.methods().collect();
            assert_eq!(methods.len(), 1);
            assert_eq!(methods[0].name(), "Value");
            let signature = methods[0].signature(&[]);
            assert_eq!(signature.return_type, windows_metadata::Type::I32);
            assert_eq!(signature.types, [windows_metadata::Type::I32]);
            let uuid = interface.find_attribute("GuidAttribute").unwrap().value();
            assert_eq!(uuid[0].1, windows_metadata::Value::U32(0xa3000000));
            assert_eq!(uuid.last().unwrap().1, windows_metadata::Value::U8(guid));
        }
        eprintln!(
            "LINKAGE_WINMD reversed={reversed} bytes={:?}",
            std::fs::read(&file).unwrap()
        );
        drop(metadata);
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(directory).unwrap();
        if let Some(baseline) = &baseline {
            assert_eq!(&report, baseline);
        } else {
            baseline = Some(report);
        }
    }
}

#[test]
fn linkage_scope_distinct_namespaces_keep_same_leaf_collision() {
    helpers::ensure_libclang();
    let references = BTreeMap::new();
    for reversed in [false, true] {
        let plan = plan(reversed, false, true);
        assert_scope(&plan, true);
        let error = plan
            .emit_with_report(&EmitOptions::new("Test.Common", &references))
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("ambiguous type root `Named`:"), "{error}");
        assert!(
            !error.contains("no captured native semantic-parent relation"),
            "{error}"
        );
        eprintln!("LINKAGE_COLLISION reversed={reversed} error={error}");
    }
}

#[test]
fn linkage_scope_excluded_dependency_remains_rejected() {
    helpers::ensure_libclang();
    let references = BTreeMap::new();
    for reversed in [false, true] {
        let error = plan(reversed, true, false)
            .emit_with_report(&EmitOptions::new("Test.Common", &references))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("reason: required `Excluded` is excluded by authoritative partition \"Home\", original input \"linkage-probe-home.cpp\""),
            "{error}"
        );
        assert!(
            error.starts_with("header partition dependency closure found 1 blocker(s)"),
            "{error}"
        );
        assert!(
            error.contains("type `Demand` in translation unit `linkage-probe-home.cpp`"),
            "{error}"
        );
        assert!(
            error.contains("no alternative context is eligible"),
            "{error}"
        );
        assert!(
            !error.contains("no captured native semantic-parent relation"),
            "{error}"
        );
        assert!(!error.contains("ambiguous type root"), "{error}");
        eprintln!("LINKAGE_EXCLUDED_DEPENDENCY reversed={reversed} error={error}");
    }
}
