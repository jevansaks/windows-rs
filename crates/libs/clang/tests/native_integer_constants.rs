use std::collections::BTreeMap;
use windows_clang::{
    Annotation, AnnotationTarget, EmitOptions, Input, Scalar, TypeRef, Value, extract,
};
use windows_metadata::{HasAttributes, Type, Value as MetadataValue, reader::Item};

const NAME: &str = "native_integer_constants.h";
const SOURCE: &str = include_str!("../../../tests/libs/clang/input/native_integer_constants.h");
const ARGS: &[&str] = &["-x", "c++", "--target=x86_64-pc-windows-msvc"];

#[test]
fn native_integer_declarations_reach_physical_constants() {
    helpers::ensure_libclang();
    let input = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .join("crates")
        .join("tests")
        .join("libs")
        .join("clang")
        .join("input")
        .join(NAME);
    let name = input.to_string_lossy().replace('\\', "/");
    let snapshot = extract([Input::new(&name, SOURCE)], ARGS).unwrap();
    let constants = snapshot.constants();
    let parent_process = constants
        .iter()
        .find(|constant| constant.name == "__forceconst__PROC_THREAD_ATTRIBUTE_PARENT_PROCESS")
        .unwrap_or_else(|| panic!("native integer VarDecl is absent: {constants:#?}"));
    assert_eq!(parent_process.ty, TypeRef::Scalar(Scalar::U32));
    assert_eq!(parent_process.value, Value::Unsigned(0x20000));

    let expected = [
        (
            "__forceconst__PROC_THREAD_ATTRIBUTE_PARENT_PROCESS",
            Scalar::U32,
            Value::Unsigned(0x20000),
            Type::U32,
            MetadataValue::U32(0x20000),
        ),
        (
            "HIGH_BIT",
            Scalar::U32,
            Value::Unsigned(0x80000000),
            Type::U32,
            MetadataValue::U32(0x80000000),
        ),
        (
            "WIDE_HIGH_BIT",
            Scalar::U64,
            Value::Unsigned(0x8000000000000000),
            Type::U64,
            MetadataValue::U64(0x8000000000000000),
        ),
        (
            "BYTE_VALUE",
            Scalar::U8,
            Value::Unsigned(255),
            Type::U8,
            MetadataValue::U8(255),
        ),
        (
            "WORD_VALUE",
            Scalar::U16,
            Value::Unsigned(65535),
            Type::U16,
            MetadataValue::U16(65535),
        ),
        (
            "SIGNED_BYTE",
            Scalar::I8,
            Value::Signed(-128),
            Type::I8,
            MetadataValue::I8(-128),
        ),
        (
            "SIGNED_WORD",
            Scalar::I16,
            Value::Signed(-32768),
            Type::I16,
            MetadataValue::I16(-32768),
        ),
        (
            "NEGATIVE",
            Scalar::I32,
            Value::Signed(-17),
            Type::I32,
            MetadataValue::I32(-17),
        ),
        (
            "SIGNED_WIDE",
            Scalar::I64,
            Value::Signed(i64::MIN),
            Type::I64,
            MetadataValue::I64(i64::MIN),
        ),
        (
            "NATIVE_EXPRESSION",
            Scalar::U32,
            Value::Unsigned(0x300011),
            Type::U32,
            MetadataValue::U32(0x300011),
        ),
        (
            "CONSTEXPR_VALUE",
            Scalar::U32,
            Value::Unsigned(0x300013),
            Type::U32,
            MetadataValue::U32(0x300013),
        ),
        (
            "ANNOTATED",
            Scalar::U32,
            Value::Unsigned(0x80000000),
            Type::U32,
            MetadataValue::U32(0x80000000),
        ),
        (
            "BOOL_TRUE",
            Scalar::Bool,
            Value::Unsigned(1),
            Type::U32,
            MetadataValue::U32(1),
        ),
        (
            "BOOL_FALSE",
            Scalar::Bool,
            Value::Unsigned(0),
            Type::U32,
            MetadataValue::U32(0),
        ),
        (
            "FLOAT_VALUE",
            Scalar::F32,
            Value::F32(1.25f32.to_bits()),
            Type::F32,
            MetadataValue::F32(1.25),
        ),
        (
            "DOUBLE_VALUE",
            Scalar::F64,
            Value::F64((-2.5f64).to_bits()),
            Type::F64,
            MetadataValue::F64(-2.5),
        ),
    ];
    for (name, scalar, value, _, _) in &expected {
        let constant = constants
            .iter()
            .find(|constant| constant.name == *name)
            .unwrap();
        assert_eq!(constant.ty, TypeRef::Scalar(*scalar), "{name}");
        assert_eq!(&constant.value, value, "{name}");
        assert_eq!(constant.root, constant.definition, "{name}");
        assert_eq!(
            constant.root.tu,
            input.to_string_lossy().replace('\\', "/"),
            "{name}"
        );
        assert_eq!(constant.spelling.file, constant.root.tu, "{name}");
        assert!(constant.spelling.offset < SOURCE.len() as u32, "{name}");
    }
    let annotated = constants
        .iter()
        .find(|constant| constant.name == "ANNOTATED")
        .unwrap();
    assert_eq!(
        snapshot
            .annotations()
            .get(&AnnotationTarget::Declaration(annotated.root.clone())),
        Some(&vec![Annotation::AssociatedEnum("NativeFlags".to_string())])
    );
    for name in [
        "NONCONSTANT",
        "MUTABLE",
        "DECLARATION_ONLY",
        "UNSUPPORTED_POINTER",
        "UNSELECTED",
    ] {
        assert!(
            constants.iter().all(|constant| constant.name != name),
            "{name}"
        );
    }
    let unselected = extract(
        [Input::new(&name, SOURCE).with_excluded_roots([&name])],
        ARGS,
    )
    .unwrap();
    assert!(
        unselected.constants().is_empty(),
        "{:#?}",
        unselected.constants()
    );

    let references = BTreeMap::new();
    let mut options = EmitOptions::new("NativeConstants", &references);
    options.library = Some("test.dll");
    let rdl = snapshot.emit_with_options(&options).unwrap();
    let output = std::env::temp_dir().join(format!(
        "windows-clang-native-integer-constants-{}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_text(include_str!("../../../../metadata/metadata.rdl"))
        .input_text(&rdl)
        .output(&output)
        .write()
        .unwrap_or_else(|error| panic!("{error}\n{rdl}"));
    let index = windows_metadata::reader::Index::read(&output).unwrap();
    for (name, _, _, ty, value) in expected {
        let Item::Const(field) = index.expect_item("NativeConstants", name) else {
            panic!("{name} was not emitted as a physical constant");
        };
        let constant = field.constant().unwrap();
        assert_eq!(field.ty(), ty, "{name}");
        assert_eq!(constant.ty(), ty, "{name}");
        assert_eq!(constant.value(), value, "{name}");
        if name == "ANNOTATED" {
            assert!(field.has_attribute("AssociatedEnumAttribute"));
        }
    }
    std::fs::remove_file(output).unwrap();
}
