use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use windows_clang::{
    EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities, PartitionConflictReason,
    PartitionItemKind, RdlPartition, RootPartition, Snapshot, TypeReference, TypeReferenceKind,
    extract, extract_partitioned,
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

#[test]
fn excluded_dependency_reports_owner_diagnostic_without_panicking() {
    helpers::ensure_libclang();

    let scratch = scratch("excluded-dependency");
    let public = scratch.join("public.h");
    std::fs::write(
        &public,
        "typedef struct HIDDEN { int value; } HIDDEN;\n\
         typedef struct PUBLIC_TYPE { HIDDEN hidden; } PUBLIC_TYPE;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public").with_exclusion("HIDDEN"),
    );
    let references = BTreeMap::new();
    let error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();

    assert!(
        error.to_string().contains(
            "owner-excluded local type `HIDDEN` in partition `public` namespace \
             `Example.Public` is required without a retained public alias"
        ),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn excluded_empty_forward_dependency_reports_owner_diagnostic() {
    helpers::ensure_libclang();

    let scratch = scratch("empty-forward-dependency");
    let public = scratch.join("public.h");
    std::fs::write(
        &public,
        "struct EMPTY_RECORD;\n\
         typedef struct PUBLIC_TYPE { EMPTY_RECORD *empty; } PUBLIC_TYPE;\n\
         struct EMPTY_RECORD {};\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", "Example.Public").exclude_empty_records(),
    );
    let references = BTreeMap::new();
    let error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();

    assert!(
        error.to_string().contains(
            "owner-excluded local type `EMPTY_RECORD` in partition `public` namespace \
             `Example.Public` is required without a retained public alias"
        ),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn legacy_partitioned_dependencies_remain_unowned() {
    helpers::ensure_libclang();

    let scratch = scratch("legacy-unowned-dependency");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "typedef struct DEPENDENCY { int value; } DEPENDENCY;\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             typedef struct PUBLIC_TYPE {{ DEPENDENCY dependency; }} PUBLIC_TYPE;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = extract_partitioned(
        [Input::new(
            "legacy.cpp",
            format!("#include \"{}\"\n", public.to_string_lossy()),
        )
        .partitioned("legacy-input")
        .with_root(public.to_string_lossy(), "public", "Example.Public")],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let error = snapshot
        .emit_partitioned_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("selected type `DEPENDENCY` has no tagged root owner"),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn inherited_dependencies_compare_effective_partition_identity() {
    helpers::ensure_libclang();

    let scratch = scratch("effective-dependency-owner");
    let dependency = scratch.join("dependency.h");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(
        &dependency,
        "#pragma once\n\
         typedef struct DEPENDENCY { int value; } DEPENDENCY;\n",
    )
    .unwrap();
    std::fs::write(
        &first,
        format!(
            "#include \"{}\"\n\
             typedef struct FIRST_ROOT {{ DEPENDENCY value; }} FIRST_ROOT;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        &second,
        format!(
            "#include \"{}\"\n\
             typedef struct SECOND_ROOT {{ DEPENDENCY value; }} SECOND_ROOT;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&first, &second]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("shared", "Example.Shared").with_library("FirstApi", "first.dll"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("shared", "Example.Shared").with_library("SecondApi", "second.dll"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let shared = partitions.values().cloned().collect::<Vec<_>>().join("\n");
    assert_eq!(shared.matches("struct DEPENDENCY").count(), 1, "{shared}");
    assert!(shared.contains("struct FIRST_ROOT"), "{shared}");
    assert!(shared.contains("struct SECOND_ROOT"), "{shared}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn equivalent_roots_compare_full_owner_policy() {
    helpers::ensure_libclang();

    let scratch = scratch("full-root-policy");
    let first = scratch.join("first.h");
    let second = scratch.join("second.h");
    std::fs::write(&first, "typedef unsigned SHARED_VALUE;\n").unwrap();
    std::fs::write(&second, "typedef unsigned SHARED_VALUE;\n").unwrap();
    let snapshot = extract(
        [
            Input::new(
                "first.cpp",
                format!("#include \"{}\"\n", first.to_string_lossy()),
            ),
            Input::new(
                "second.cpp",
                format!("#include \"{}\"\n", second.to_string_lossy()),
            ),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first.to_string_lossy(),
            RootPartition::new("shared", "Example.Shared").with_library("FirstApi", "first.dll"),
        )
        .with_traversed_header(
            second.to_string_lossy(),
            RootPartition::new("shared", "Example.Shared").with_library("SecondApi", "second.dll"),
        );
    let references = BTreeMap::new();
    let audit = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert_eq!(audit.conflicts().len(), 1, "{audit}");
    assert_eq!(audit.conflicts()[0].name, "SHARED_VALUE");
    assert_eq!(
        audit.conflicts()[0].reason,
        PartitionConflictReason::AmbiguousOwners
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn route_audit_uses_source_names_after_owner_remaps() {
    helpers::ensure_libclang();

    let scratch = scratch("route-source-names");
    let shared = scratch.join("shared.h");
    std::fs::write(&shared, "typedef unsigned SOURCE_NAME;\n").unwrap();
    let include = format!("#include \"{}\"\n", shared.to_string_lossy());
    let snapshot = extract(
        [
            Input::new("first.cpp", &include)
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
            Input::new("second.cpp", &include)
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "first.cpp",
            "shared.h",
            RootPartition::new("first", "Example.First").with_remap("SOURCE_NAME", "REMAPPED"),
        )
        .with_traversed_header_for_input(
            "second.cpp",
            "shared.h",
            RootPartition::new("second", "Example.Second").with_remap("SOURCE_NAME", "REMAPPED"),
        );
    let references = BTreeMap::new();
    let audit = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert_eq!(audit.conflicts().len(), 1, "{audit}");
    assert_eq!(audit.conflicts()[0].name, "SOURCE_NAME");
    assert!(!audit.to_string().contains("REMAPPED"), "{audit}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn authority_routes_use_policy_partition_and_physical_header() {
    helpers::ensure_libclang();

    let scratch = scratch("authority-partition");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "typedef struct DEPENDENCY { int value; } DEPENDENCY;\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             typedef struct PUBLIC_TYPE {{ DEPENDENCY dependency; }} PUBLIC_TYPE;\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let partition = RootPartition::new("public", "Example.Public");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(public.to_string_lossy(), partition.clone())
        .with_authority_partition("authority");
    let default_policy =
        HeaderPartitionPolicy::new().with_traversed_header(public.to_string_lossy(), partition);
    let authorities = NamespaceAuthorities::new().with_exact("DEPENDENCY", "Example.Dependencies");
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);

    let partitions = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let authority = partitions
        .keys()
        .find(|partition| partition.namespace == "Example.Dependencies")
        .unwrap();
    assert_eq!(authority.partition, "authority");
    assert_eq!(
        authority.header,
        dependency.to_string_lossy().replace('\\', "/")
    );
    assert_ne!(authority.partition, "aggregate.cpp");

    let default_partitions = snapshot
        .plan_header_partitions(&default_policy, &authorities)
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let default_authority = default_partitions
        .keys()
        .find(|partition| partition.namespace == "Example.Dependencies")
        .unwrap();
    assert_eq!(default_authority.partition, "Example.Dependencies");
    assert_eq!(default_authority.header, authority.header);

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn relative_case_variant_headers_use_resolved_physical_paths() {
    helpers::ensure_libclang();

    let scratch = scratch("physical-paths");
    let include = scratch.join("Include");
    std::fs::create_dir_all(&include).unwrap();
    let public = include.join("PublicHeader.h");
    std::fs::write(
        &public,
        "typedef struct ORIGINAL { int value; } ORIGINAL;\n\
         typedef struct HOLDER { ORIGINAL value; } HOLDER;\n",
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&public]);
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        "include/publicheader.h",
        RootPartition::new("public", "Example.Public").with_remap("ORIGINAL", "RENAMED"),
    );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let (partition, public_rdl) = partitions.first_key_value().unwrap();

    assert_eq!(
        partition.header,
        public.to_string_lossy().replace('\\', "/")
    );
    assert!(public_rdl.contains("struct RENAMED"), "{public_rdl}");
    assert!(public_rdl.contains("value: RENAMED"), "{public_rdl}");
    assert!(!public_rdl.contains("ORIGINAL"), "{public_rdl}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn associated_constants_follow_annotated_declaration_owner() {
    helpers::ensure_libclang();

    let scratch = scratch("associated-constant");
    let provider = scratch.join("provider.h");
    let group = scratch.join("group.h");
    std::fs::write(
        &provider,
        "#define ERROR_TARGET 42UL\n\
         #define ERROR_ALIAS ERROR_TARGET\n\
         #define ERROR_NOISE 19\n",
    )
    .unwrap();
    std::fs::write(
        &group,
        format!(
            "#include \"{}\"\n\
             enum __attribute__((annotate(\
                 \"win32metadata:associated_constant=ERROR_ALIAS\"))) \
                 ERROR_KIND : unsigned long {{ ERROR_LOCAL = 1 }};\n",
            provider.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = aggregate_snapshot(&scratch, &[&group]);
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            group.to_string_lossy(),
            RootPartition::new("group", "Example.Group"),
        )
        .with_traversed_header(
            provider.to_string_lossy(),
            RootPartition::new("provider", "Example.Provider"),
        );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();
    let group_rdl = output(&partitions, "Example.Group");

    assert!(group_rdl.contains("enum ERROR_KIND"), "{group_rdl}");
    assert!(
        group_rdl.contains("const ERROR_ALIAS: u32 = 42"),
        "{group_rdl}"
    );
    assert!(!group_rdl.contains("ERROR_TARGET"), "{group_rdl}");
    assert!(!group_rdl.contains("ERROR_NOISE"), "{group_rdl}");
    assert!(
        !output(&partitions, "Example.Provider").contains("ERROR_ALIAS"),
        "{partitions:#?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn input_qualified_headers_preserve_psapi_compile_variants() {
    helpers::ensure_libclang();

    let scratch = scratch("input-qualified-psapi");
    let common = scratch.join("common.h");
    let header = scratch.join("PsApi.h");
    std::fs::write(&common, "typedef void *HANDLE;\ntypedef int BOOL;\n").unwrap();
    std::fs::write(
        &header,
        format!(
            "#include \"{}\"\n\
             extern \"C\" BOOL EmptyWorkingSet(HANDLE process);\n\
             extern \"C\" BOOL EnumProcesses(unsigned *processes, unsigned bytes, \
             unsigned *needed);\n",
            common.to_string_lossy()
        ),
    )
    .unwrap();
    let include = format!("#include \"{}\"\n", header.to_string_lossy());
    let snapshot = extract(
        [
            Input::new("psapi1.cpp", &include)
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
            Input::new(
                "psapi2.cpp",
                format!(
                    "#define EmptyWorkingSet K32EmptyWorkingSet\n\
                     #define EnumProcesses K32EnumProcesses\n\
                     {include}"
                ),
            )
            .with_root_dirs([scratch.to_string_lossy().to_string()]),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let v1 = RootPartition::new("psapi1", "Example.System.ProcessStatus")
        .with_library("EmptyWorkingSet", "PSAPI.dll")
        .with_library("EnumProcesses", "PSAPI.dll");
    let v2 = RootPartition::new("psapi2", "Example.System.ProcessStatus")
        .with_library("K32EmptyWorkingSet", "KERNEL32.dll")
        .with_library("K32EnumProcesses", "KERNEL32.dll");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input("PSAPI1.CPP", "psapi.h", v1)
        .with_traversed_header_for_input("psapi2.cpp", "PSAPI.H", v2);
    let references = BTreeMap::from([
        (
            "BOOL".to_string(),
            TypeReference::new("Example.Foundation", "BOOL", TypeReferenceKind::Type),
        ),
        (
            "HANDLE".to_string(),
            TypeReference::new("Example.Foundation", "HANDLE", TypeReferenceKind::Type),
        ),
    ]);
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let process_status = partitions.values().cloned().collect::<Vec<_>>().join("\n");

    assert_eq!(partitions.len(), 2, "{partitions:#?}");
    for name in [
        "EmptyWorkingSet",
        "K32EmptyWorkingSet",
        "EnumProcesses",
        "K32EnumProcesses",
    ] {
        assert_eq!(
            process_status.matches(&format!("fn {name}(")).count(),
            1,
            "{process_status}"
        );
    }
    assert!(
        process_status.contains("#[library(\"PSAPI.dll\")]"),
        "{process_status}"
    );
    assert!(
        process_status.contains("#[library(\"KERNEL32.dll\")]"),
        "{process_status}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn input_qualified_exclusions_filter_equivalent_alias_owners() {
    helpers::ensure_libclang();

    let scratch = scratch("input-qualified-exclusions");
    let header = scratch.join("PsApi.h");
    std::fs::write(
        &header,
        "#ifndef PSAPI_VERSION\n\
         #define PSAPI_VERSION 2\n\
         #endif\n\
         typedef struct _MODULEINFO { unsigned value; } MODULEINFO, *LPMODULEINFO;\n\
         typedef int (__stdcall *PENUM_PAGE_FILE_CALLBACK)(LPMODULEINFO value);\n\
         #if PSAPI_VERSION > 1\n\
         #define GetModuleInformation K32GetModuleInformation\n\
         #endif\n\
         extern \"C\" int GetModuleInformation(\
             LPMODULEINFO value, PENUM_PAGE_FILE_CALLBACK callback);\n",
    )
    .unwrap();
    let include = format!("#include \"{}\"\n", header.to_string_lossy());
    let snapshot = extract(
        [
            Input::new(
                "win32metadata-psapi-v1.cpp",
                format!("#define PSAPI_VERSION 1\n{include}"),
            )
            .with_root_dirs([scratch.to_string_lossy().to_string()]),
            Input::new(
                "win32metadata-psapi-v2.cpp",
                format!("#define PSAPI_VERSION 2\n{include}"),
            )
            .with_root_dirs([scratch.to_string_lossy().to_string()]),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "win32metadata-psapi-v1.cpp",
            "psapi.h",
            RootPartition::new("PsApi1", "Example.System.ProcessStatus")
                .with_library("GetModuleInformation", "PSAPI.dll"),
        )
        .with_traversed_header_for_input(
            "win32metadata-psapi-v2.cpp",
            "psapi.h",
            RootPartition::new("PsApi2", "Example.System.ProcessStatus")
                .with_exclusion("_MODULEINFO")
                .with_exclusion("PENUM_PAGE_FILE_CALLBACK")
                .with_library("K32GetModuleInformation", "KERNEL32.dll"),
        );
    let references = BTreeMap::new();
    let selected = BTreeSet::from([
        "GetModuleInformation".to_string(),
        "K32GetModuleInformation".to_string(),
    ]);
    let mut options = EmitOptions::new("Example.Common", &references);
    options.functions = Some(&selected);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();

    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let v1 = partitions
        .iter()
        .find(|(partition, _)| partition.partition == "PsApi1")
        .unwrap()
        .1;
    let v2 = partitions
        .iter()
        .find(|(partition, _)| partition.partition == "PsApi2")
        .unwrap()
        .1;

    assert!(v1.contains("struct MODULEINFO"), "{v1}");
    assert!(v1.contains("type LPMODULEINFO = *mut MODULEINFO"), "{v1}");
    assert!(v1.contains("extern fn PENUM_PAGE_FILE_CALLBACK"), "{v1}");
    assert!(!v2.contains("struct MODULEINFO"), "{v2}");
    assert!(!v2.contains("type LPMODULEINFO"), "{v2}");
    assert!(!v2.contains("extern fn PENUM_PAGE_FILE_CALLBACK"), "{v2}");
    assert!(v1.contains("fn GetModuleInformation("), "{v1}");
    assert!(v2.contains("fn K32GetModuleInformation("), "{v2}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn aggregate_and_satellite_inputs_match_case_variant_selectors() {
    helpers::ensure_libclang();

    let scratch = scratch("aggregate-satellite");
    let aggregate = scratch.join("Aggregate.h");
    let satellite = scratch.join("Satellite.h");
    std::fs::write(&aggregate, "typedef unsigned AGGREGATE_VALUE;\n").unwrap();
    std::fs::write(
        &satellite,
        "#ifndef SATELLITE_NAME\n\
         #define SATELLITE_NAME SATELLITE_VALUE\n\
         #endif\n\
         typedef unsigned SATELLITE_NAME;\n",
    )
    .unwrap();
    let snapshot = extract(
        [
            Input::new(
                "aggregate.cpp",
                format!(
                    "#include \"{}\"\n#include \"{}\"\n",
                    aggregate.to_string_lossy(),
                    satellite.to_string_lossy()
                ),
            ),
            Input::new(
                "satellite.cpp",
                format!(
                    "#define SATELLITE_NAME SATELLITE_SPECIAL\n#include \"{}\"\n",
                    satellite.to_string_lossy()
                ),
            ),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "AGGREGATE.CPP",
            "aggregate.h",
            RootPartition::new("aggregate", "Example.Aggregate"),
        )
        .with_traversed_header_for_input(
            "SATELLITE.CPP",
            "SATELLITE.H",
            RootPartition::new("satellite", "Example.Satellite"),
        );
    let references = BTreeMap::new();
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&EmitOptions::new("Example.Common", &references))
        .unwrap();

    assert!(output(&partitions, "Example.Aggregate").contains("type AGGREGATE_VALUE = u32"));
    assert!(output(&partitions, "Example.Satellite").contains("type SATELLITE_SPECIAL = u32"));
    assert!(
        !partitions
            .values()
            .any(|rdl| rdl.contains("SATELLITE_VALUE"))
    );

    std::fs::remove_dir_all(scratch).unwrap();
}
