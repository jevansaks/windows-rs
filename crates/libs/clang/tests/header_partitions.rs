use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use windows_clang::{
    EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities, PartitionConflictReason,
    PartitionItemKind, RdlPartition, RootPartition, Snapshot, extract, extract_partitioned,
};

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-header-partitions-{name}-{}",
        std::process::id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn aggregate_snapshot(scratch: &Path, headers: &[&Path]) -> Snapshot {
    let source = headers
        .iter()
        .map(|header| format!("#include \"{}\"\n", header.to_string_lossy()))
        .collect::<String>();
    extract(
        [Input::new("aggregate.cpp", source)
            .with_root_dirs([scratch.to_string_lossy().to_string()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap()
}

fn output<'a>(partitions: &'a BTreeMap<RdlPartition, String>, namespace: &str) -> &'a str {
    partitions
        .iter()
        .find(|(partition, _)| partition.namespace == namespace)
        .unwrap()
        .1
}

#[test]
fn traversed_headers_select_roots_and_inherit_dependency_owners() {
    helpers::ensure_libclang();

    let scratch = scratch("closure");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "typedef struct DEPENDENCY { int value; } DEPENDENCY;\n\
         typedef unsigned INCLUDED_ONLY;\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             typedef struct PUBLIC_TYPE {{ DEPENDENCY dependency; }} PUBLIC_TYPE;\n\
             typedef unsigned PUBLIC_ALIAS;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public"),
    );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let public_rdl = output(&partitions, "Example.Public");

    assert!(public_rdl.contains("struct PUBLIC_TYPE"), "{public_rdl}");
    assert!(
        public_rdl.contains("type PUBLIC_ALIAS = u32"),
        "{public_rdl}"
    );
    assert!(public_rdl.contains("struct DEPENDENCY"), "{public_rdl}");
    assert!(!public_rdl.contains("INCLUDED_ONLY"), "{public_rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn aggregate_facts_route_to_two_logical_namespaces() {
    helpers::ensure_libclang();

    let scratch = scratch("namespaces");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(&first, "typedef unsigned FIRST_VALUE;\n").unwrap();
    std::fs::write(&second, "typedef unsigned SECOND_VALUE;\n").unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first, &second]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        );
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_header_partitions_with_options(
            &policy,
            &EmitOptions::new("Example.Common", &references),
        )
        .unwrap();

    assert!(output(&partitions, "Example.First").contains("type FIRST_VALUE = u32"));
    assert!(output(&partitions, "Example.Second").contains("type SECOND_VALUE = u32"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn shared_header_candidates_use_exclusions_authority_and_owner_settings() {
    helpers::ensure_libclang();

    let scratch = scratch("shared");
    let shared = scratch.join("shared.h");
    std::fs::write(
        &shared,
        "typedef int FIRST_ONLY;\n\
         typedef int SECOND_ONLY;\n\
         typedef unsigned short FORCED;\n\
         extern \"C\" int RoutedFunction(void);\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&shared]);
    let first = RootPartition::new("first", "Example.First").with_exclusion("SECOND_ONLY");
    let second = RootPartition::new("second", "Example.Second")
        .with_exclusion("FIRST_ONLY")
        .with_remap("SECOND_ONLY", "SECOND_RENAMED")
        .with_u32_type("FORCED")
        .with_library("RoutedFunction", "routed.dll");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(shared.to_string_lossy(), first.clone())
        .with_traversed_header(shared.to_string_lossy(), second.clone());
    let reverse = HeaderPartitionPolicy::new()
        .with_traversed_header(shared.to_string_lossy(), second)
        .with_traversed_header(shared.to_string_lossy(), first);
    let authorities = NamespaceAuthorities::new()
        .with_exact("FORCED", "Example.Second")
        .with_exact("RoutedFunction", "Example.Second");
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let reverse_partitions = snapshot
        .plan_header_partitions(&reverse, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    assert_eq!(partitions, reverse_partitions);
    let first = output(&partitions, "Example.First");
    let second = output(&partitions, "Example.Second");

    assert!(first.contains("type FIRST_ONLY = i32"), "{first}");
    assert!(!first.contains("SECOND_ONLY"), "{first}");
    assert!(second.contains("type SECOND_RENAMED = i32"), "{second}");
    assert!(second.contains("type FORCED = u32"), "{second}");
    assert!(second.contains("#[library(\"routed.dll\")]"), "{second}");
    assert!(second.contains("fn RoutedFunction() -> i32"), "{second}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn audit_reports_all_dependency_owner_conflicts_and_authority_resolves_them() {
    helpers::ensure_libclang();

    let scratch = scratch("dependency-audit");
    let dependency = scratch.join("dependency.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         typedef struct DEP_FIRST { int value; } DEP_FIRST;\n\
         typedef struct DEP_SECOND { int value; } DEP_SECOND;\n",
    )
    .unwrap();
    std::fs::write(
        &first,
        format!(
            "#include \"{}\"\n\
             typedef struct FIRST_ROOT {{ DEP_FIRST first; DEP_SECOND second; }} FIRST_ROOT;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &second,
        format!(
            "#include \"{}\"\n\
             typedef struct SECOND_ROOT {{ DEP_FIRST first; DEP_SECOND second; }} SECOND_ROOT;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first, &second]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let audit = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&options)
        .unwrap();

    assert_eq!(audit.conflicts().len(), 2, "{audit}");
    assert_eq!(
        audit
            .conflicts()
            .iter()
            .map(|conflict| conflict.name.as_str())
            .collect::<Vec<_>>(),
        ["DEP_FIRST", "DEP_SECOND"]
    );
    assert!(audit.conflicts().iter().all(|conflict| {
        conflict.reason == PartitionConflictReason::AmbiguousOwners && conflict.owners.len() == 2
    }));

    let authorities = NamespaceAuthorities::new().with_wildcard("DEP_*", "Example.Dependencies");
    let plan = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let dependencies = output(&partitions, "Example.Dependencies");
    assert!(dependencies.contains("struct DEP_FIRST"), "{dependencies}");
    assert!(dependencies.contains("struct DEP_SECOND"), "{dependencies}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn audit_reports_every_unresolved_shared_header_owner() {
    helpers::ensure_libclang();

    let scratch = scratch("audit");
    let shared = scratch.join("shared.h");
    std::fs::write(
        &shared,
        "typedef unsigned FIRST_CONFLICT;\n\
         typedef unsigned SECOND_CONFLICT;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&shared]);
    let first = RootPartition::new("first", "Example.First");
    let second = RootPartition::new("second", "Example.Second");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(shared.to_string_lossy(), second.clone())
        .with_traversed_header(shared.to_string_lossy(), first.clone());
    let reverse = HeaderPartitionPolicy::new()
        .with_traversed_header(shared.to_string_lossy(), first)
        .with_traversed_header(shared.to_string_lossy(), second);
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let audit = plan.audit(&options).unwrap();
    let reverse_audit = snapshot
        .plan_header_partitions(&reverse, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&options)
        .unwrap();

    assert_eq!(audit, reverse_audit);
    assert_eq!(audit.conflicts().len(), 2, "{audit}");
    assert_eq!(
        audit
            .conflicts()
            .iter()
            .map(|conflict| conflict.name.as_str())
            .collect::<Vec<_>>(),
        ["FIRST_CONFLICT", "SECOND_CONFLICT"]
    );
    assert!(audit.conflicts().iter().all(|conflict| {
        conflict.kind == PartitionItemKind::Type
            && conflict.reason == PartitionConflictReason::AmbiguousRootCandidates
            && conflict.owners.len() == 2
    }));
    let error = plan.emit_with_options(&options).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("header partition planning found 2 conflict(s)"),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn macro_generated_declarations_use_expansion_provenance() {
    helpers::ensure_libclang();

    let scratch = scratch("macro");
    let macros = scratch.join("macros.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &macros,
        "#define DECLARE_FIXED typedef unsigned EXPANDED_TYPE\n\
         #define INCLUDED_CONSTANT 7\n\
         #define CONSTANT_VALUE 9\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             DECLARE_FIXED;\n\
             #define PUBLIC_CONSTANT CONSTANT_VALUE\n",
            macros.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let expanded = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == "EXPANDED_TYPE")
        .unwrap();
    assert!(
        expanded.spelling.file.ends_with("macros.h"),
        "{expanded:#?}"
    );
    assert!(
        expanded.expansion.file.ends_with("public.h"),
        "{expanded:#?}"
    );

    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public"),
    );
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_header_partitions_with_options(
            &policy,
            &EmitOptions::new("Example.Common", &references),
        )
        .unwrap();
    let public_rdl = output(&partitions, "Example.Public");

    assert!(
        public_rdl.contains("type EXPANDED_TYPE = u32"),
        "{public_rdl}"
    );
    assert!(
        public_rdl.contains("const PUBLIC_CONSTANT: i32 = 9"),
        "{public_rdl}"
    );
    assert!(!public_rdl.contains("INCLUDED_CONSTANT"), "{public_rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn legacy_partitioned_api_behavior_is_unchanged() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [Input::new("legacy.h", "typedef unsigned LEGACY_VALUE;\n")
            .partitioned("legacy-input")
            .with_root("legacy.h", "legacy", "Example.Legacy")],
        &["-x", "c++"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let partitions = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert!(output(&partitions, "Example.Legacy").contains("type LEGACY_VALUE = u32"));
}
