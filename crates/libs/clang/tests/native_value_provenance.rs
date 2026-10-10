use std::collections::{BTreeMap, BTreeSet};
use windows_clang::{
    Annotation, AnnotationTarget, CallingConvention, Fact, FactData, FactKind, Input, Location,
    Origin, Scalar, Snapshot, TypeRef, Value, ValueDeclaration, ValueDeclarationKind, extract,
};

const ARGS: &[&str] = &["-x", "c++", "--target=x86_64-pc-windows-msvc"];
const CALLBACK_SOURCE: &str = "\
struct CallbackOwner { int (*invoke)(int); };
typedef int FinalAlias;
const int Authored = 11;
";

#[test]
fn callback_origin_does_not_alias_a_native_value() {
    helpers::ensure_libclang();
    let snapshot = extract([Input::new("callback-value.cpp", CALLBACK_SOURCE)], ARGS).unwrap();
    let constant = snapshot
        .constants()
        .iter()
        .find(|constant| constant.name == "Authored")
        .unwrap();
    assert_eq!(constant.ty, TypeRef::Scalar(Scalar::I32));
    assert_eq!(constant.value, Value::Signed(11));
    let callback = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == "CallbackOwner_invoke")
        .unwrap();
    let FactData::Callback {
        convention,
        params,
        result,
    } = &callback.data
    else {
        panic!("callback is absent");
    };
    assert_eq!(*convention, CallingConvention::C);
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "arg0");
    assert_eq!(params[0].ty, TypeRef::Scalar(Scalar::I32));
    assert_eq!(*result, TypeRef::Scalar(Scalar::I32));
    assert_eq!(callback.spelling, snapshot.facts()[0].spelling);
    assert_eq!(callback.expansion, callback.spelling);
    assert!(
        callback.parent.is_none() && callback.definition && callback.main_file && callback.root
    );
    assert!(!callback.system);
    assert_ne!(callback.origin, constant.root);
    assert_eq!(
        snapshot.emit("Test").unwrap(),
        r#"#[win32]
mod Test {
    const Authored: i32 = 11;
    struct CallbackOwner {
        invoke: CallbackOwner_invoke,
    }
    extern "C" fn CallbackOwner_invoke(arg0: i32) -> i32;
    type FinalAlias = i32;
}
"#
    );
    check_origins(&snapshot);
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SourceKind {
    Fact(FactKind),
    Value(ValueDeclarationKind),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SourceKey {
    kind: SourceKind,
    name: String,
    spelling: Location,
    expansion: Location,
    definition: bool,
    root: bool,
    system: bool,
    parent: Option<Box<Self>>,
}

#[derive(Debug, Eq, PartialEq)]
enum BindingError {
    Missing,
    Ambiguous,
}

fn source_key(
    facts: &[Fact],
    values: &[ValueDeclaration],
    origin: &Origin,
) -> Result<SourceKey, BindingError> {
    let matching_facts: Vec<_> = facts.iter().filter(|fact| &fact.origin == origin).collect();
    let declarations: Vec<_> = values
        .iter()
        .filter(|declaration| &declaration.origin == origin)
        .collect();
    let (mut key, parent) = match (matching_facts.as_slice(), declarations.as_slice()) {
        ([fact], []) => (
            SourceKey {
                kind: SourceKind::Fact(fact.kind),
                name: fact.name.clone(),
                spelling: fact.spelling.clone(),
                expansion: fact.expansion.clone(),
                definition: fact.definition,
                root: fact.root,
                system: fact.system,
                parent: None,
            },
            fact.parent.as_ref(),
        ),
        ([], [declaration]) => (
            SourceKey {
                kind: SourceKind::Value(declaration.kind),
                name: declaration.name.clone(),
                spelling: declaration.spelling.clone(),
                expansion: declaration.expansion.clone(),
                definition: declaration.definition,
                root: declaration.root,
                system: declaration.system,
                parent: None,
            },
            declaration.parent.as_ref(),
        ),
        ([], []) => return Err(BindingError::Missing),
        _ => return Err(BindingError::Ambiguous),
    };
    key.parent = parent
        .map(|parent| source_key(facts, values, parent))
        .transpose()?
        .map(Box::new);
    Ok(key)
}

fn check_origins(snapshot: &Snapshot) {
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
    for constant in snapshot.constants() {
        for origin in [&constant.root, &constant.definition] {
            source_key(snapshot.facts(), snapshot.value_declarations(), origin).unwrap();
        }
    }
}

fn values(
    snapshot: &Snapshot,
) -> BTreeMap<(SourceKey, SourceKey, String), (TypeRef, Value, Vec<Annotation>)> {
    snapshot
        .constants()
        .iter()
        .map(|constant| {
            let root = source_key(
                snapshot.facts(),
                snapshot.value_declarations(),
                &constant.root,
            )
            .unwrap();
            let definition = source_key(
                snapshot.facts(),
                snapshot.value_declarations(),
                &constant.definition,
            )
            .unwrap();
            let annotations = snapshot
                .annotations()
                .get(&AnnotationTarget::Declaration(constant.root.clone()))
                .cloned()
                .unwrap_or_default();
            (
                (root, definition, constant.name.clone()),
                (constant.ty.clone(), constant.value.clone(), annotations),
            )
        })
        .collect()
}

#[test]
fn native_values_preserve_physical_sources_when_original_roots_are_folded() {
    helpers::ensure_libclang();
    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-value-provenance-{}",
        std::process::id()
    ));
    std::fs::create_dir(&scratch).unwrap();
    let mut originals = Vec::new();
    let mut wrapper = String::new();
    for (name, number) in [("First", 11), ("Second", 23)] {
        let source = format!(
            "#define {name}Object 1\n#undef {name}Object\n#define {name}Object {number}\n\
             enum {name}Flags {{ {name}Flag = 1 }};\n\
             const int {name}Integer = -{number};\n\
             constexpr double {name}Double = {number}.25;\n\
             __attribute__((annotate(\"win32metadata:associated_enum={name}Flags\")))\n\
             const unsigned int {name}Annotated = 1;\n\
             enum {{ {name}Anonymous = 7, {name}Other = 9 }};\n\
             namespace {name} {{ const int {name}Nested = 17; enum {{ {name}NestedEnum = 3 }}; }}\n"
        );
        let file = scratch.join(format!("{name}.cpp"));
        std::fs::write(&file, &source).unwrap();
        let file = file.to_string_lossy().replace('\\', "/");
        wrapper.push_str(&format!("#include \"{file}\"\n"));
        originals.push(Input::new(file, source));
    }
    let original = extract(originals.clone(), ARGS).unwrap();
    let wrapper_name = scratch
        .join("folded.cpp")
        .to_string_lossy()
        .replace('\\', "/");
    let folded = extract(
        [Input::new(&wrapper_name, &wrapper).with_roots(
            originals
                .iter()
                .flat_map(|input| input.roots.iter().cloned()),
        )],
        ARGS,
    )
    .unwrap();
    for snapshot in [&original, &folded] {
        check_origins(snapshot);
        for (name, number) in [("First", 11), ("Second", 23)] {
            for (suffix, ty, value) in [
                ("Integer", Scalar::I32, Value::Signed(-number)),
                (
                    "Double",
                    Scalar::F64,
                    Value::F64((number as f64 + 0.25).to_bits()),
                ),
                ("Annotated", Scalar::U32, Value::Unsigned(1)),
            ] {
                let constant = snapshot
                    .constants()
                    .iter()
                    .find(|constant| constant.name == format!("{name}{suffix}"))
                    .unwrap();
                assert_eq!(constant.ty, TypeRef::Scalar(ty));
                assert_eq!(constant.value, value);
                let declaration = snapshot
                    .value_declarations()
                    .iter()
                    .find(|declaration| declaration.origin == constant.root)
                    .unwrap();
                assert_eq!(declaration.kind, ValueDeclarationKind::Variable);
                assert_eq!(declaration.name, constant.name);
                assert_eq!(declaration.spelling, constant.spelling);
                assert_eq!(declaration.expansion, constant.spelling);
                assert_eq!(declaration.main_file, std::ptr::eq(snapshot, &original));
                assert!(declaration.definition && declaration.root && !declaration.system);
                let source = &originals
                    .iter()
                    .find(|input| input.name == declaration.spelling.file)
                    .unwrap()
                    .source;
                assert!(source[declaration.spelling.offset as usize..].starts_with(&constant.name));
                if suffix == "Annotated" {
                    assert_eq!(
                        snapshot
                            .annotations()
                            .get(&AnnotationTarget::Declaration(constant.root.clone())),
                        Some(&vec![Annotation::AssociatedEnum(format!("{name}Flags"))])
                    );
                }
            }
            let macro_value = snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == format!("{name}Object"))
                .unwrap();
            assert_ne!(macro_value.root, macro_value.definition);
            assert!(
                snapshot
                    .value_declarations()
                    .iter()
                    .all(|declaration| declaration.origin != macro_value.root
                        && declaration.origin != macro_value.definition)
            );
            let anonymous = snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == format!("{name}Anonymous"))
                .unwrap();
            let other = snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == format!("{name}Other"))
                .unwrap();
            assert_eq!(anonymous.root, other.root);
            let declaration = snapshot
                .value_declarations()
                .iter()
                .find(|declaration| declaration.origin == anonymous.root)
                .unwrap();
            assert_eq!(declaration.kind, ValueDeclarationKind::AnonymousEnum);
            assert!(
                snapshot
                    .facts()
                    .iter()
                    .all(|fact| fact.origin != declaration.origin)
            );
            let nested = snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == format!("{name}Nested"))
                .unwrap();
            let evidence = source_key(
                snapshot.facts(),
                snapshot.value_declarations(),
                &nested.root,
            )
            .unwrap();
            assert_eq!(evidence.parent.as_ref().unwrap().name, name);
            assert_eq!(
                evidence.parent.as_ref().unwrap().kind,
                SourceKind::Fact(FactKind::Namespace)
            );
        }
    }
    assert_eq!(values(&original), values(&folded));
    assert_eq!(original.emit("Test").unwrap(), folded.emit("Test").unwrap());
    let declaration = &original.value_declarations()[0];
    assert_eq!(
        source_key(&[], &[], &declaration.origin),
        Err(BindingError::Missing)
    );
    assert_eq!(
        source_key(
            &[],
            &[declaration.clone(), declaration.clone()],
            &declaration.origin
        ),
        Err(BindingError::Ambiguous)
    );
    let nonroot = extract([Input::new(&wrapper_name, wrapper)], ARGS).unwrap();
    assert!(nonroot.constants().is_empty());
    assert!(nonroot.value_declarations().is_empty());
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn recovered_midl_values_keep_their_native_enum_fact() {
    helpers::ensure_libclang();
    let snapshot = extract(
        [Input::new(
            "midl-value.cpp",
            "enum __MIDL_generated_values { MIDL_NONE = 0, MIDL_ONE = 1 };",
        )],
        ARGS,
    )
    .unwrap();
    assert!(snapshot.value_declarations().is_empty());
    assert_eq!(snapshot.constants().len(), 2);
    check_origins(&snapshot);
    for constant in snapshot.constants() {
        assert_eq!(constant.root, constant.definition);
        let source = source_key(snapshot.facts(), &[], &constant.root).unwrap();
        assert_eq!(source.kind, SourceKind::Fact(FactKind::Enum));
        assert_eq!(source.name, "__MIDL_generated_values");
    }
}
