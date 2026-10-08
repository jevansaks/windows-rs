use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use windows_clang::{
    Annotation, AnnotationTarget, EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities,
    RdlPartition, RootPartition, Snapshot, extract,
};

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-annotation-provenance-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn provider_source(cleanup: Option<&str>, function: &str) -> String {
    let annotation = cleanup.map_or_else(String::new, |cleanup| {
        format!("__attribute__((annotate(\"win32metadata:raii_free={cleanup}\"))) ")
    });
    format!(
        "#pragma once\n\
         typedef void* LPVOID;\n\
         typedef {annotation}LPVOID HINTERNET;\n\
         extern \"C\" void {function}(HINTERNET value);\n"
    )
}

fn aggregate_snapshot(root: &Path, headers: &[&Path]) -> Snapshot {
    let source: String = headers
        .iter()
        .map(|header| format!("#include \"{}\"\n", header.to_string_lossy()))
        .collect();
    extract(
        [
            Input::new("aggregate.cpp", source)
                .with_root_dirs([root.to_string_lossy().to_string()]),
        ],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap()
}

fn policy(
    first: &Path,
    first_namespace: &str,
    first_function: &str,
    second: Option<(&Path, &str, &str)>,
) -> HeaderPartitionPolicy {
    let mut policy = HeaderPartitionPolicy::new().with_traversed_header(
        first.to_string_lossy(),
        RootPartition::new("first", first_namespace).with_library(first_function, "first.dll"),
    );
    if let Some((second, namespace, function)) = second {
        policy = policy.with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("second", namespace).with_library(function, "second.dll"),
        );
    }
    policy
}

fn emit(snapshot: Snapshot, policy: &HeaderPartitionPolicy) -> BTreeMap<RdlPartition, String> {
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    plan.emit_with_options(&options).unwrap()
}

fn output<'a>(partitions: &'a BTreeMap<RdlPartition, String>, namespace: &str) -> &'a str {
    partitions
        .iter()
        .find(|(partition, _)| partition.namespace == namespace)
        .unwrap()
        .1
}

fn raii_free(snapshot: &Snapshot, header: &Path) -> BTreeSet<String> {
    let header = header.to_string_lossy().replace('\\', "/");
    snapshot
        .facts()
        .iter()
        .filter(|fact| fact.name == "HINTERNET" && fact.spelling.file == header)
        .flat_map(|fact| {
            snapshot
                .annotations()
                .get(&AnnotationTarget::Declaration(fact.origin.clone()))
                .into_iter()
                .flatten()
        })
        .filter_map(|annotation| match annotation {
            Annotation::RaiiFree(cleanup) => Some(cleanup.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn same_tu_typedef_annotations_follow_exact_header_origins_in_both_orders() {
    helpers::ensure_libclang();

    let root = scratch("same-tu");
    let first = root.join("first.h");
    let second = root.join("second.h");
    std::fs::write(&first, provider_source(Some("CloseFirst"), "UseFirst")).unwrap();
    std::fs::write(&second, provider_source(Some("CloseSecond"), "UseSecond")).unwrap();

    let forward = aggregate_snapshot(&root, &[&first, &second]);
    assert_eq!(
        raii_free(&forward, &first),
        ["CloseFirst".to_string()].into()
    );
    assert_eq!(
        raii_free(&forward, &second),
        ["CloseSecond".to_string()].into()
    );
    let reverse = aggregate_snapshot(&root, &[&second, &first]);
    assert_eq!(
        raii_free(&reverse, &first),
        ["CloseFirst".to_string()].into()
    );
    assert_eq!(
        raii_free(&reverse, &second),
        ["CloseSecond".to_string()].into()
    );

    let policy = policy(
        &first,
        "Example.First",
        "UseFirst",
        Some((&second, "Example.Second", "UseSecond")),
    );
    let forward = emit(forward, &policy);
    let reverse = emit(reverse, &policy);
    assert_eq!(forward, reverse);
    let first_rdl = output(&forward, "Example.First");
    assert!(
        first_rdl.contains("#[raii_free(\"CloseFirst\")]"),
        "{first_rdl}"
    );
    assert!(!first_rdl.contains("CloseSecond"), "{first_rdl}");
    let second_rdl = output(&forward, "Example.Second");
    assert!(
        second_rdl.contains("#[raii_free(\"CloseSecond\")]"),
        "{second_rdl}"
    );
    assert!(!second_rdl.contains("CloseFirst"), "{second_rdl}");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn single_and_repeated_provider_includes_keep_one_annotation() {
    helpers::ensure_libclang();

    let root = scratch("repeated");
    let first = root.join("first.h");
    std::fs::write(&first, provider_source(Some("CloseFirst"), "UseFirst")).unwrap();
    let single = aggregate_snapshot(&root, &[&first]);
    let repeated = aggregate_snapshot(&root, &[&first, &first]);
    assert_eq!(
        raii_free(&single, &first),
        ["CloseFirst".to_string()].into()
    );
    assert_eq!(
        raii_free(&repeated, &first),
        ["CloseFirst".to_string()].into()
    );

    let policy = policy(&first, "Example.First", "UseFirst", None);
    let single = emit(single, &policy);
    let repeated = emit(repeated, &policy);
    assert_eq!(single, repeated);
    assert_eq!(
        output(&single, "Example.First")
            .matches("#[raii_free(\"CloseFirst\")]")
            .count(),
        1
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn separate_translation_units_keep_provider_annotations_independent() {
    helpers::ensure_libclang();

    let root = scratch("separate-tu");
    let first = root.join("first.h");
    let second = root.join("second.h");
    std::fs::write(&first, provider_source(Some("CloseFirst"), "UseFirst")).unwrap();
    std::fs::write(&second, provider_source(Some("CloseSecond"), "UseSecond")).unwrap();
    let roots = [root.to_string_lossy().to_string()];
    let snapshot = extract(
        [
            Input::new(
                "first.cpp",
                format!("#include \"{}\"\n", first.to_string_lossy()),
            )
            .with_root_dirs(roots.clone()),
            Input::new(
                "second.cpp",
                format!("#include \"{}\"\n", second.to_string_lossy()),
            )
            .with_root_dirs(roots),
        ],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    assert_eq!(
        raii_free(&snapshot, &first),
        ["CloseFirst".to_string()].into()
    );
    assert_eq!(
        raii_free(&snapshot, &second),
        ["CloseSecond".to_string()].into()
    );

    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "first.cpp",
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First").with_library("UseFirst", "first.dll"),
        )
        .with_traversed_header_for_input(
            "second.cpp",
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second").with_library("UseSecond", "second.dll"),
        );
    let partitions = emit(snapshot, &policy);
    assert!(output(&partitions, "Example.First").contains("#[raii_free(\"CloseFirst\")]"));
    assert!(output(&partitions, "Example.Second").contains("#[raii_free(\"CloseSecond\")]"));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unannotated_redeclaration_does_not_inherit_provider_annotation() {
    helpers::ensure_libclang();

    let root = scratch("unannotated");
    let first = root.join("first.h");
    let second = root.join("second.h");
    std::fs::write(&first, provider_source(Some("CloseFirst"), "UseFirst")).unwrap();
    std::fs::write(&second, provider_source(None, "UseSecond")).unwrap();
    let snapshot = aggregate_snapshot(&root, &[&first, &second]);
    assert_eq!(
        raii_free(&snapshot, &first),
        ["CloseFirst".to_string()].into()
    );
    assert!(raii_free(&snapshot, &second).is_empty());

    let policy = policy(
        &first,
        "Example.First",
        "UseFirst",
        Some((&second, "Example.Second", "UseSecond")),
    );
    let partitions = emit(snapshot, &policy);
    assert!(output(&partitions, "Example.First").contains("#[raii_free(\"CloseFirst\")]"));
    let second_rdl = output(&partitions, "Example.Second");
    assert!(second_rdl.contains("type HINTERNET"), "{second_rdl}");
    assert!(!second_rdl.contains("raii_free"), "{second_rdl}");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn conflicting_typedef_annotations_without_distinct_routes_are_diagnostic() {
    helpers::ensure_libclang();

    let root = scratch("conflict");
    let first = root.join("first.h");
    let second = root.join("second.h");
    std::fs::write(&first, provider_source(Some("CloseFirst"), "UseFirst")).unwrap();
    std::fs::write(&second, provider_source(Some("CloseSecond"), "UseSecond")).unwrap();
    let snapshot = aggregate_snapshot(&root, &[&first, &second]);
    let owner = RootPartition::new("shared", "Example.Shared");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            owner.clone().with_library("UseFirst", "first.dll"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            owner.with_library("UseSecond", "second.dll"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let audit = plan.audit(&options).unwrap();
    assert!(!audit.is_clean());
    let audit = audit.to_string();
    assert!(audit.contains("HINTERNET"), "{audit}");
    assert!(audit.contains("ambiguous logical owners"), "{audit}");

    std::fs::remove_dir_all(root).unwrap();
}
