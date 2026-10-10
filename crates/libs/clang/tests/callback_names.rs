use std::collections::BTreeSet;
use std::path::PathBuf;
use windows_clang::{AnnotationTarget, Fact, FactData, Input, Snapshot, TypeRef, extract};

const ARGS: &[&str] = &["-x", "c++", "--target=x86_64-pc-windows-msvc"];

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-callback-names-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

fn callbacks(snapshot: &Snapshot) -> Vec<&Fact> {
    snapshot
        .facts()
        .iter()
        .filter(|fact| matches!(fact.data, FactData::Callback { .. }))
        .collect()
}

fn check_unique_origins(snapshot: &Snapshot) {
    let origins: Vec<_> = snapshot
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
        origins.iter().copied().collect::<BTreeSet<_>>().len()
    );
}

#[test]
fn repeated_physical_callback_names_are_independent_of_input_count_order_and_folding() {
    helpers::ensure_libclang();
    let root = scratch("repeated");
    let header = root.join("shared.h");
    std::fs::write(
        &header,
        "#pragma once\n\
         struct __attribute__((annotate(\"win32metadata:supported_os=windows6.1\")))\n\
         tagEXCEPINFO { int (__stdcall *pfnDeferredFillIn)(tagEXCEPINFO *); };\n\
         const int NativeValue = 11;\n",
    )
    .unwrap();
    let mut inputs = vec![];
    let mut wrapper = String::new();
    for name in ["first", "second", "third"] {
        let main = root.join(format!("{name}.cpp"));
        let source = "#include \"shared.h\"\n";
        std::fs::write(&main, source).unwrap();
        wrapper.push_str(&format!("#include \"{}\"\n", main.display()));
        inputs.push(
            Input::new(main.to_string_lossy(), source)
                .with_roots([header.to_string_lossy().into_owned()]),
        );
    }
    let single = extract([inputs[0].clone()], ARGS).unwrap();
    let split = extract(inputs.clone(), ARGS).unwrap();
    let mut permutations = vec![];
    for order in [
        vec![0, 1],
        vec![0, 2, 1],
        vec![1, 0, 2],
        vec![1, 2, 0],
        vec![2, 0, 1],
        vec![2, 1, 0],
    ] {
        permutations
            .push(extract(order.into_iter().map(|index| inputs[index].clone()), ARGS).unwrap());
    }
    let folded = extract(
        [
            Input::new(root.join("folded.cpp").to_string_lossy(), wrapper)
                .with_roots(inputs.iter().flat_map(|input| input.roots.iter().cloned())),
        ],
        ARGS,
    )
    .unwrap();
    assert_eq!(callbacks(&split).len(), 3);
    let expected = callbacks(&single)[0];
    for snapshot in [&single, &split, &folded].into_iter().chain(&permutations) {
        for callback in callbacks(snapshot) {
            assert_eq!(callback.name, "tagEXCEPINFO_pfnDeferredFillIn");
            assert_eq!(callback.name, expected.name);
            assert_eq!(callback.data, expected.data);
            assert_eq!(callback.spelling, expected.spelling);
            assert_eq!(callback.expansion, expected.expansion);
            assert_eq!(callback.parent, expected.parent);
            assert_eq!(callback.root, expected.root);
            assert_eq!(callback.main_file, expected.main_file);
            assert_eq!(callback.system, expected.system);
            assert_eq!(callback.definition, expected.definition);
            if callback.origin.tu == expected.origin.tu {
                assert_eq!(callback.origin, expected.origin);
            }
        }
        let owner = single
            .facts()
            .iter()
            .find(|fact| fact.name == "tagEXCEPINFO")
            .unwrap();
        for actual in snapshot
            .facts()
            .iter()
            .filter(|fact| fact.name == owner.name)
        {
            assert_eq!(actual.data, owner.data);
            assert_eq!(
                snapshot
                    .annotations()
                    .get(&AnnotationTarget::Declaration(actual.origin.clone())),
                single
                    .annotations()
                    .get(&AnnotationTarget::Declaration(owner.origin.clone()))
            );
        }
        check_unique_origins(snapshot);
        assert_eq!(snapshot.emit("Test").unwrap(), single.emit("Test").unwrap());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn distinct_sources_and_nested_typed_routes_do_not_share_collision_names() {
    helpers::ensure_libclang();
    let root = scratch("routes");
    let first = root.join("first.h");
    let second = root.join("second.h");
    std::fs::write(
        &first,
        "#pragma once\n\
         typedef int Collision_run;\n\
         struct _Collision { int (*run)(int); };\n\
         struct Paths {\n\
             struct { int (*run)(int); } nested;\n\
             int (*nested_run)(int);\n\
             int (**indirect)(int);\n\
             int (*array[2])(int);\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &second,
        "#pragma once\nstruct Collision { int (*run)(int); };\n",
    )
    .unwrap();
    let inputs: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|name| {
            Input::new(
                root.join(format!("{name}.cpp")).to_string_lossy(),
                format!("#include \"{name}.h\"\n"),
            )
            .with_roots([first.to_string_lossy(), second.to_string_lossy()])
        })
        .collect();
    let split = extract(inputs.clone(), ARGS).unwrap();
    let reversed = extract(inputs.iter().rev().cloned(), ARGS).unwrap();
    let folded = extract(
        [Input::new(
            root.join("folded.cpp").to_string_lossy(),
            "#include \"second.h\"\n#include \"first.h\"\n",
        )
        .with_roots([first.to_string_lossy(), second.to_string_lossy()])],
        ARGS,
    )
    .unwrap();
    let identity = |snapshot: &Snapshot| {
        callbacks(snapshot)
            .into_iter()
            .map(|fact| (fact.spelling.clone(), fact.name.clone(), fact.data.clone()))
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(callbacks(&split).len(), 6);
    assert_eq!(identity(&split), identity(&reversed));
    assert_eq!(identity(&split), identity(&folded));
    let colliding: Vec<_> = callbacks(&split)
        .into_iter()
        .filter(|fact| fact.name.starts_with("Collision_run"))
        .collect();
    assert_eq!(colliding.len(), 2);
    assert_ne!(colliding[0].spelling, colliding[1].spelling);
    assert_ne!(colliding[0].name, colliding[1].name);
    assert!(colliding.iter().all(|fact| fact.name != "Collision_run"));
    let nested: Vec<_> = callbacks(&split)
        .into_iter()
        .filter(|fact| fact.name.starts_with("Paths_nested_run"))
        .collect();
    assert_eq!(nested.len(), 2);
    assert_eq!(nested[0].spelling, nested[1].spelling);
    assert_eq!(nested[0].data, nested[1].data);
    assert_ne!(nested[0].name, nested[1].name);
    for snapshot in [&split, &reversed, &folded] {
        check_unique_origins(snapshot);
        assert_eq!(snapshot.emit("Test").unwrap(), folded.emit("Test").unwrap());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn conflicting_complete_callback_payloads_remain_visible_and_fail_emission() {
    helpers::ensure_libclang();
    let root = scratch("conflict");
    let header = root.join("shared.h");
    std::fs::write(&header, "struct Conflict { RESULT (*run)(int); };\n").unwrap();
    let inputs: Vec<_> = [("first", "int"), ("second", "double")]
        .into_iter()
        .map(|(name, result)| {
            Input::new(
                root.join(format!("{name}.cpp")).to_string_lossy(),
                format!("#define RESULT {result}\n#include \"shared.h\"\n"),
            )
            .with_roots([header.to_string_lossy()])
        })
        .collect();
    for inputs in [inputs.clone(), inputs.into_iter().rev().collect()] {
        let snapshot = extract(inputs, ARGS).unwrap();
        let observed = callbacks(&snapshot);
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].name, observed[1].name);
        assert_eq!(observed[0].spelling, observed[1].spelling);
        assert_ne!(observed[0].origin, observed[1].origin);
        let results: BTreeSet<_> = observed
            .iter()
            .map(|fact| {
                let FactData::Callback {
                    convention,
                    params,
                    result,
                } = &fact.data
                else {
                    unreachable!();
                };
                assert_eq!(*convention, windows_clang::CallingConvention::C);
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].ty, TypeRef::Scalar(windows_clang::Scalar::I32));
                result.clone()
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert_ne!(observed[0].data, observed[1].data);
        check_unique_origins(&snapshot);
        let error = snapshot.emit("Test").unwrap_err();
        assert!(error.to_string().contains("Conflict"), "{error}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
