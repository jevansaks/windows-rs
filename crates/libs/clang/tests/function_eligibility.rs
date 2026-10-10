use std::collections::{BTreeMap, BTreeSet};
use windows_clang::{
    Annotation, AnnotationTarget, CallingConvention, EmitOptions, FactData, FunctionExclusion,
    FunctionLinkage, Input, Scalar, TypeRef, Value, ValueDeclarationKind, extract,
};

#[test]
fn native_disposition_preserves_signatures_and_external_declarations() {
    helpers::ensure_libclang();
    let source = r#"
        #define API __stdcall
        #define IN __attribute__((annotate("win32metadata:in")))
        #define SUCCESS __attribute__((annotate("win32metadata:can_return_multiple_success_values")))
        extern "C" SUCCESS long API Imported(IN const unsigned* value, long count);
        extern "C" SUCCESS inline long API Inline(IN const unsigned* value, long count) { return count; }
        extern "C" SUCCESS long API Definition(IN const unsigned* value, long count) { return count; }
        extern "C" SUCCESS constexpr long API Constexpr(IN const unsigned* value, long count) { return count; }
        extern "C" { static SUCCESS long API Internal(IN const unsigned* value, long count); }
        extern "C" SUCCESS long API Redeclared(IN const unsigned* value, long count);
        extern "C" SUCCESS long API Redeclared(IN const unsigned* value, long count) { return count; }
    "#;
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let target_arg = format!("--target={target}");
        let args = ["-x", "c++", "-std=c++20", "-fms-extensions", &target_arg];
        let snapshot = extract([Input::new("eligibility.hpp", source)], &args).unwrap();
        assert_eq!(snapshot.unsupported().count(), 0, "{}", snapshot.dump());
        let imported = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "Imported")
            .unwrap();
        let FactData::Function {
            convention,
            params,
            result,
            ..
        } = &imported.data
        else {
            panic!("{}", snapshot.dump());
        };
        assert_eq!(*convention, CallingConvention::Platform);
        assert_eq!(*result, TypeRef::Scalar(Scalar::I32));
        assert_eq!(
            params[0].ty,
            TypeRef::Pointer {
                mutable: false,
                target: Box::new(TypeRef::Scalar(Scalar::U32))
            }
        );
        assert_eq!(params[1].ty, TypeRef::Scalar(Scalar::I32));
        assert!(params[0].annotation.input);
        for (name, reason) in [
            (
                "Inline",
                FunctionExclusion::Definition {
                    linkage: FunctionLinkage::External,
                },
            ),
            (
                "Definition",
                FunctionExclusion::Definition {
                    linkage: FunctionLinkage::External,
                },
            ),
            (
                "Constexpr",
                FunctionExclusion::Definition {
                    linkage: FunctionLinkage::External,
                },
            ),
            (
                "Internal",
                FunctionExclusion::NonExternalLinkage {
                    linkage: FunctionLinkage::Internal,
                },
            ),
        ] {
            let fact = snapshot
                .facts()
                .iter()
                .find(|fact| fact.name == name)
                .unwrap();
            let FactData::NonEmittableFunction {
                reason: actual,
                signature,
            } = &fact.data
            else {
                panic!("{name}: {:?}", fact.data);
            };
            assert_eq!(*actual, reason);
            assert_eq!(signature.params, *params);
            assert_eq!(signature.result, *result);
            assert_eq!(signature.convention, *convention);
            assert_eq!(
                snapshot
                    .annotations()
                    .get(&AnnotationTarget::Declaration(fact.origin.clone())),
                Some(&vec![Annotation::CanReturnMultipleSuccessValues])
            );
            assert_eq!(
                snapshot.annotations().get(&AnnotationTarget::Parameter {
                    declaration: fact.origin.clone(),
                    index: 0
                }),
                Some(&vec![Annotation::In])
            );
        }
        let redeclared: Vec<_> = snapshot
            .facts()
            .iter()
            .filter(|fact| fact.name == "Redeclared")
            .collect();
        assert_eq!(redeclared.len(), 2);
        assert!(!redeclared[0].definition);
        assert!(matches!(redeclared[0].data, FactData::Function { .. }));
        assert!(redeclared[1].definition);
        assert!(matches!(
            redeclared[1].data,
            FactData::NonEmittableFunction { .. }
        ));
        let references = BTreeMap::new();
        let mut options = EmitOptions::new("Test", &references);
        options.library = Some("test.dll");
        let rdl = snapshot.emit_with_options(&options).unwrap();
        assert_eq!(rdl.matches("fn ").count(), 2, "{rdl}");
        let declarations = source.split("extern \"C\" SUCCESS inline").next().unwrap();
        let unchanged = extract([Input::new("declarations.hpp", declarations)], &args).unwrap();
        let selected = BTreeSet::from(["Imported".to_string()]);
        options.functions = Some(&selected);
        assert_eq!(
            snapshot.emit_with_options(&options).unwrap(),
            unchanged.emit_with_options(&options).unwrap()
        );
        let selected = BTreeSet::from(["Definition".to_string()]);
        let mut options = EmitOptions::new("Test", &references);
        options.library = Some("test.dll");
        options.functions = Some(&selected);
        assert!(
            snapshot
                .emit_with_options(&options)
                .unwrap_err()
                .to_string()
                .contains("was not found")
        );
    }
}

#[test]
fn definition_only_inputs_never_import_and_unsupported_signatures_stay_visible() {
    helpers::ensure_libclang();
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let target_arg = format!("--target={target}");
        let args = ["-x", "c++", "-fms-extensions", &target_arg];
        let snapshot = extract(
            [Input::new(
                "definitions.hpp",
                r#"
            extern "C" inline int Inline(int value) { return value; }
            extern "C" int Definition(int value) { return value; }
            extern "C" { static int Internal(int value) { return value; } }
        "#,
            )],
            &args,
        )
        .unwrap();
        assert_eq!(snapshot.facts().len(), 3);
        assert!(
            snapshot
                .facts()
                .iter()
                .all(|fact| matches!(fact.data, FactData::NonEmittableFunction { .. }))
        );
        let references = BTreeMap::new();
        let mut options = EmitOptions::new("Test", &references);
        options.library = Some("not-imported.dll");
        let rdl = snapshot.emit_with_options(&options).unwrap();
        assert!(
            !rdl.contains("fn ") && !rdl.contains("not-imported.dll"),
            "{rdl}"
        );
        let selected = BTreeSet::from([
            "Inline".to_string(),
            "Definition".to_string(),
            "Internal".to_string(),
        ]);
        options.functions = Some(&selected);
        assert!(snapshot.emit_with_options(&options).is_err());
    }
    let snapshot = extract(
        [Input::new(
            "unsupported.hpp",
            r#"
        extern "C" __int128 UnsupportedResult();
        extern "C" void UnsupportedParameter(__int128 value);
        extern "C" void __vectorcall UnsupportedConvention(int value);
        extern "C" inline __int128 UnsupportedDefinition() { return 0; }
    "#,
        )],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    assert_eq!(snapshot.unsupported().count(), 4, "{}", snapshot.dump());
}

#[test]
fn constexpr_body_locals_are_not_metadata_but_global_evaluation_survives() {
    helpers::ensure_libclang();
    let source = r#"
                    struct CallbackOwner { int (*invoke)(int); };
                    typedef unsigned GlobalType;
                    constexpr int Helper(int value) {
                        typedef unsigned LocalType;
                        const int LocalConstant = 6;
                        return value + LocalConstant;
                    }
                    constexpr int GlobalValue = Helper(5);
                    const int GlobalPlain = 9;
                "#;
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let target_arg = format!("--target={target}");
        let snapshot = extract(
            [Input::new("constexpr.hpp", source)],
            &["-x", "c++", "-std=c++20", &target_arg],
        )
        .unwrap();
        assert_eq!(snapshot.unsupported().count(), 0, "{}", snapshot.dump());
        assert!(
            snapshot
                .facts()
                .iter()
                .any(|fact| fact.name == "GlobalType")
        );
        assert!(snapshot.facts().iter().any(|fact| {
            fact.name == "Helper"
                && fact.definition
                && matches!(fact.data, FactData::NonEmittableFunction { .. })
        }));
        let helper = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "Helper")
            .unwrap();
        let FactData::NonEmittableFunction { reason, signature } = &helper.data else {
            panic!("{:?}", helper.data);
        };
        assert_eq!(
            *reason,
            FunctionExclusion::Definition {
                linkage: FunctionLinkage::External
            }
        );
        assert_eq!(signature.result, TypeRef::Scalar(Scalar::I32));
        assert_eq!(signature.params.len(), 1);
        assert_eq!(signature.params[0].name, "value");
        assert_eq!(signature.params[0].ty, TypeRef::Scalar(Scalar::I32));
        assert!(!signature.variadic && !signature.noreturn);
        for name in ["LocalType", "LocalConstant"] {
            assert!(!snapshot.facts().iter().any(|fact| fact.name == name));
            assert!(
                !snapshot
                    .constants()
                    .iter()
                    .any(|constant| constant.name == name)
            );
            assert!(
                !snapshot
                    .value_declarations()
                    .iter()
                    .any(|declaration| declaration.name == name)
            );
        }
        let callback = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "CallbackOwner_invoke")
            .unwrap();
        assert!(matches!(callback.data, FactData::Callback { .. }));
        let origins: Vec<_> = snapshot
            .facts()
            .iter()
            .map(|fact| &fact.origin)
            .chain(
                snapshot
                    .value_declarations()
                    .iter()
                    .map(|declaration| &declaration.origin),
            )
            .collect();
        assert_eq!(
            origins.len(),
            origins.iter().copied().collect::<BTreeSet<_>>().len()
        );
        for origin in origins.iter().filter(|origin| **origin != &callback.origin) {
            assert_eq!(origin.tu, callback.origin.tu);
            assert!(origin.local < callback.origin.local);
        }
        for (name, value) in [("GlobalValue", 11), ("GlobalPlain", 9)] {
            let constant = snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == name)
                .unwrap();
            assert_eq!(constant.value, Value::Signed(value));
            assert_eq!(constant.ty, TypeRef::Scalar(Scalar::I32));
            assert_eq!(constant.root, constant.definition);
            let declarations: Vec<_> = snapshot
                .value_declarations()
                .iter()
                .filter(|declaration| declaration.origin == constant.root)
                .collect();
            let [declaration] = declarations.as_slice() else {
                panic!("{name}: {declarations:?}");
            };
            assert_eq!(declaration.kind, ValueDeclarationKind::Variable);
            assert_eq!(declaration.name, name);
            assert_eq!(declaration.origin.tu, "constexpr.hpp");
            assert_eq!(declaration.parent, None);
            assert_eq!(declaration.spelling, constant.spelling);
            assert_eq!(declaration.spelling.file, "constexpr.hpp");
            assert_eq!(declaration.expansion, declaration.spelling);
            assert!(
                declaration.definition
                    && declaration.main_file
                    && declaration.root
                    && !declaration.system
            );
            assert!(source[declaration.spelling.offset as usize..].starts_with(name));
            assert!(
                snapshot
                    .facts()
                    .iter()
                    .all(|fact| fact.origin != declaration.origin)
            );
        }
    }
}
