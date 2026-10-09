use std::collections::BTreeMap;
use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition,
    TypeRef, extract,
};
use windows_metadata::{Type, reader::Item};

#[test]
fn shared_typedef_identity_is_independent_of_translation_units() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("input")
        .join("typedef_identity");
    let first = root.join("first.h").to_string_lossy().to_string();
    let second = root.join("second.h").to_string_lossy().to_string();
    let source = format!("#include \"{first}\"\n#include \"{second}\"\n");
    let reversed_source = format!("#include \"{second}\"\n#include \"{first}\"\n");
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("test.dll");
    let mut baseline = None;
    let mut baseline_partitions = None;
    for (case, assignments) in [
        (
            "single",
            vec![("single.cpp", vec![first.clone(), second.clone()])],
        ),
        (
            "split",
            vec![
                ("first.cpp", vec![first.clone()]),
                ("second.cpp", vec![second.clone()]),
            ],
        ),
        (
            "split-reversed",
            vec![
                ("second.cpp", vec![second.clone()]),
                ("first.cpp", vec![first.clone()]),
            ],
        ),
        (
            "folded",
            vec![("folded.cpp", vec![first.clone(), second.clone()])],
        ),
        (
            "folded-reversed",
            vec![("folded.cpp", vec![second, first.clone()])],
        ),
    ] {
        let mut policy = HeaderPartitionPolicy::new();
        let mut inputs = Vec::new();
        for (name, headers) in assignments {
            for header in &headers {
                let (partition, namespace) = if header == &first {
                    ("first", "Example.First")
                } else {
                    ("second", "Example.Second")
                };
                policy.add_traversed_header_for_input(
                    name,
                    header.clone(),
                    RootPartition::new(partition, namespace),
                );
            }
            let source = if case == "folded-reversed" {
                &reversed_source
            } else {
                &source
            };
            inputs.push(Input::new(name, source.clone()).with_roots(headers));
        }
        let snapshot = extract(inputs, &["-x", "c++", "--target=x86_64-pc-windows-msvc"]).unwrap();
        for function in snapshot.facts().iter().filter(|fact| fact.name == "Second") {
            let FactData::Function { params, .. } = &function.data else {
                panic!()
            };
            let TypeRef::Named { name, declaration } = &params[0].ty else {
                panic!()
            };
            assert_eq!(name, "LPVOID");
            let alias = snapshot
                .facts()
                .iter()
                .find(|fact| {
                    fact.origin.tu == function.origin.tu
                        && fact.name == *name
                        && fact.spelling == *declaration
                })
                .unwrap();
            assert_eq!(
                alias.data,
                FactData::Typedef {
                    target: TypeRef::Pointer {
                        mutable: true,
                        target: Box::new(TypeRef::Void)
                    },
                }
            );
        }
        let partitions = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap()
            .emit_with_options(&options)
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "windows-clang-typedef-identity-{case}-{}.winmd",
            std::process::id()
        ));
        windows_rdl::reader()
            .input_texts(partitions.values())
            .reference_default()
            .output(&path)
            .write()
            .unwrap();
        let index = windows_metadata::reader::Index::read(&path).unwrap();
        let Item::Fn(function) = index.expect_item("Example.Second", "Second") else {
            panic!()
        };
        let signature = function.signature(&[]).types;
        let field = index
            .expect("Example.Second", "SECOND_RECORD")
            .fields()
            .next()
            .unwrap()
            .ty();
        let Item::Fn(raw) = index.expect_item("Example.Second", "Raw") else {
            panic!()
        };
        assert_eq!(
            raw.signature(&[]).types,
            [Type::PtrMut(Box::new(Type::Void), 1)]
        );
        std::fs::remove_file(path).unwrap();
        let observed = (signature, field);
        if let Some(baseline) = &baseline {
            assert_eq!(&observed, baseline, "{case}");
        } else {
            assert_eq!(observed.0, [Type::value_named("Example.Common", "LPVOID")]);
            assert_eq!(observed.1, Type::value_named("Example.Common", "PVOID"));
            baseline = Some(observed);
        }
        if let Some(baseline) = &baseline_partitions {
            assert_eq!(&partitions, baseline, "{case}");
        } else {
            baseline_partitions = Some(partitions);
        }
    }
}

#[test]
fn mixed_owned_and_unowned_typedef_observations_keep_input_scope() {
    helpers::ensure_libclang();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("input")
        .join("typedef_identity");
    let first = root.join("first.h").to_string_lossy().to_string();
    let second = root.join("second.h").to_string_lossy().to_string();
    let shared = root.join("shared.h").to_string_lossy().to_string();
    let source = format!("#include \"{first}\"\n#include \"{second}\"\n");
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "unowned.cpp",
            &first,
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header_for_input(
            "owned.cpp",
            &second,
            RootPartition::new("second", "Example.Second"),
        )
        .with_traversed_header_for_input(
            "owned.cpp",
            &shared,
            RootPartition::new("owner", "Example.Owner"),
        );
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("test.dll");
    for reverse in [false, true] {
        let mut inputs = vec![
            Input::new("unowned.cpp", source.clone()).with_roots([first.clone()]),
            Input::new("owned.cpp", source.clone()).with_roots([second.clone(), shared.clone()]),
        ];
        if reverse {
            inputs.reverse();
        }
        let snapshot = extract(inputs, &["-x", "c++", "--target=x86_64-pc-windows-msvc"]).unwrap();
        let partitions = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap()
            .emit_with_options(&options)
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "windows-clang-mixed-typedef-identity-{reverse}-{}.winmd",
            std::process::id()
        ));
        windows_rdl::reader()
            .input_texts(partitions.values())
            .reference_default()
            .output(&path)
            .write()
            .unwrap();
        let index = windows_metadata::reader::Index::read(&path).unwrap();
        let Item::Fn(first) = index.expect_item("Example.First", "First") else {
            panic!()
        };
        let Item::Fn(second) = index.expect_item("Example.Second", "Second") else {
            panic!()
        };
        let first_signature = first.signature(&[]).types;
        let second_signature = second.signature(&[]).types;
        let field = index
            .expect("Example.Second", "SECOND_RECORD")
            .fields()
            .next()
            .unwrap()
            .ty();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            first_signature,
            [Type::value_named("Example.Owner", "LPVOID")]
        );
        assert_eq!(second_signature, [Type::PtrMut(Box::new(Type::Void), 1)]);
        assert_eq!(field, Type::PtrMut(Box::new(Type::Void), 1));
    }
}
