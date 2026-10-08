use std::collections::BTreeMap;
use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition,
    Value as ClangValue, extract,
};
use windows_metadata::{
    Type, Value,
    reader::{HasAttributes, Item},
};

const SOURCE: &str = r#"
typedef unsigned int UINT;
typedef wchar_t WCHAR;
typedef struct _GUID {
    unsigned long Data1;
    unsigned short Data2;
    unsigned short Data3;
    unsigned char Data4[8];
} GUID;
typedef GUID CLSID;

static const UINT DML_MINIMUM_BUFFER_TENSOR_ALIGNMENT = 16;
const long UIA_SummaryChangeId = 90000;
static const unsigned __int64 NATIVE_UNSIGNED_64 = 0xffffffffffffffffULL;
static const __int64 NATIVE_SIGNED_64 = -2;
static const char g_szWMTitle[] = "Title";
static const WCHAR g_wszWMTitle[] = L"Title";
extern const __declspec(selectany) CLSID CLSID_SoftwareBitmapNativeFactory = {
    0x84e65691, 0x8602, 0x4a84, { 0xbe, 0x46, 0x70, 0x8b, 0xe9, 0xcd, 0x4b, 0x74 }
};
extern const __declspec(selectany) CLSID CLSID_AudioFrameNativeFactory = {
    0x16a0a3b9, 0x9f65, 0x4102, { 0x93, 0x67, 0x2c, 0xda, 0x3a, 0x4f, 0x37, 0x2a }
};
extern const __declspec(selectany) CLSID CLSID_VideoFrameNativeFactory = {
    0xd194386a, 0x04e3, 0x4814, { 0x81, 0x00, 0xb2, 0xb0, 0xae, 0x6d, 0x78, 0xc7 }
};
"#;

#[test]
fn typed_globals_emit_physically_across_windows_targets() {
    helpers::ensure_libclang();

    let scratch = scratch("physical");
    for (architecture, target) in [
        ("x86", "i686-pc-windows-msvc"),
        ("x64", "x86_64-pc-windows-msvc"),
        ("arm64", "aarch64-pc-windows-msvc"),
    ] {
        let snapshot = extract(
            [Input::new("typed-globals.h", SOURCE)],
            &[
                "-x",
                "c++",
                "-fms-extensions",
                &format!("--target={target}"),
            ],
        )
        .unwrap();
        assert_eq!(
            snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == "DML_MINIMUM_BUFFER_TENSOR_ALIGNMENT")
                .map(|constant| &constant.value),
            Some(&ClangValue::Unsigned(16)),
            "{architecture}: {:#?}",
            snapshot.constants()
        );
        assert_eq!(
            snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == "NATIVE_UNSIGNED_64")
                .map(|constant| &constant.value),
            Some(&ClangValue::Unsigned(u64::MAX)),
            "{architecture}: {:#?}",
            snapshot.constants()
        );
        assert_eq!(
            snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == "NATIVE_SIGNED_64")
                .map(|constant| &constant.value),
            Some(&ClangValue::Signed(-2)),
            "{architecture}: {:#?}",
            snapshot.constants()
        );
        assert_eq!(
            snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == "g_szWMTitle")
                .map(|constant| &constant.value),
            Some(&ClangValue::Utf8("Title".to_string())),
            "{architecture}: {:#?}",
            snapshot.constants()
        );
        assert_eq!(
            snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == "UIA_SummaryChangeId")
                .map(|constant| &constant.value),
            Some(&ClangValue::Signed(90000)),
            "{architecture}: {:#?}",
            snapshot.constants()
        );
        assert_eq!(
            snapshot
                .constants()
                .iter()
                .find(|constant| constant.name == "g_wszWMTitle")
                .map(|constant| &constant.value),
            Some(&ClangValue::Utf16("Title".to_string())),
            "{architecture}: {:#?}",
            snapshot.constants()
        );
        for (name, value) in [
            (
                "CLSID_SoftwareBitmapNativeFactory",
                "84e65691-8602-4a84-be46-708be9cd4b74",
            ),
            (
                "CLSID_AudioFrameNativeFactory",
                "16a0a3b9-9f65-4102-9367-2cda3a4f372a",
            ),
            (
                "CLSID_VideoFrameNativeFactory",
                "d194386a-04e3-4814-8100-b2b0ae6d78c7",
            ),
        ] {
            assert!(
                snapshot.facts().iter().any(|fact| {
                    fact.name == name
                        && matches!(&fact.data, FactData::Guid { value: actual } if actual == value)
                }),
                "{architecture}: {name}: {}",
                snapshot.dump()
            );
        }

        let rdl = snapshot.emit("Example.TypedGlobals").unwrap();
        assert!(
            rdl.contains("const DML_MINIMUM_BUFFER_TENSOR_ALIGNMENT: u32 = 16"),
            "{architecture}: {rdl}"
        );
        assert!(
            rdl.contains("const UIA_SummaryChangeId: i32 = 90000"),
            "{architecture}: {rdl}"
        );
        assert!(
            rdl.contains("const NATIVE_UNSIGNED_64: u64 = 18446744073709551615"),
            "{architecture}: {rdl}"
        );
        assert!(
            rdl.contains("const NATIVE_SIGNED_64: i64 = -2"),
            "{architecture}: {rdl}"
        );
        assert!(
            rdl.contains("#[encoding(\"ansi\")]\n        const g_szWMTitle: String = \"Title\""),
            "{architecture}: {rdl}"
        );
        assert!(
            rdl.contains("#[encoding(\"utf-16\")]\n        const g_wszWMTitle: String = \"Title\""),
            "{architecture}: {rdl}"
        );
        for (name, value) in [
            (
                "CLSID_SoftwareBitmapNativeFactory",
                "0x84e65691_8602_4a84_be46_708be9cd4b74",
            ),
            (
                "CLSID_AudioFrameNativeFactory",
                "0x16a0a3b9_9f65_4102_9367_2cda3a4f372a",
            ),
            (
                "CLSID_VideoFrameNativeFactory",
                "0xd194386a_04e3_4814_8100_b2b0ae6d78c7",
            ),
        ] {
            assert!(
                rdl.contains(&format!("const {name}: GUID = {value}")),
                "{architecture}: {name}: {rdl}"
            );
        }

        let output = scratch.join(format!("typed-globals-{architecture}.winmd"));
        windows_rdl::reader()
            .input_text(&rdl)
            .reference_default()
            .output(&output)
            .write()
            .unwrap();
        let index = windows_metadata::reader::Index::read(&output).unwrap();
        assert_constant(
            &index,
            "DML_MINIMUM_BUFFER_TENSOR_ALIGNMENT",
            Type::U32,
            Value::U32(16),
        );
        assert_constant(&index, "UIA_SummaryChangeId", Type::I32, Value::I32(90000));
        assert_constant(
            &index,
            "NATIVE_UNSIGNED_64",
            Type::U64,
            Value::U64(u64::MAX),
        );
        assert_constant(&index, "NATIVE_SIGNED_64", Type::I64, Value::I64(-2));
        assert_constant(
            &index,
            "g_szWMTitle",
            Type::String,
            Value::Utf16("Title".to_string()),
        );
        assert_constant(
            &index,
            "g_wszWMTitle",
            Type::String,
            Value::Utf16("Title".to_string()),
        );
        assert_encoding(&index, "g_szWMTitle", "ansi");
        assert_encoding(&index, "g_wszWMTitle", "utf-16");
        assert_guid(
            &index,
            "CLSID_SoftwareBitmapNativeFactory",
            [
                Value::U32(0x84e65691),
                Value::U16(0x8602),
                Value::U16(0x4a84),
                Value::U8(0xbe),
                Value::U8(0x46),
                Value::U8(0x70),
                Value::U8(0x8b),
                Value::U8(0xe9),
                Value::U8(0xcd),
                Value::U8(0x4b),
                Value::U8(0x74),
            ],
        );
        assert_guid(
            &index,
            "CLSID_AudioFrameNativeFactory",
            [
                Value::U32(0x16a0a3b9),
                Value::U16(0x9f65),
                Value::U16(0x4102),
                Value::U8(0x93),
                Value::U8(0x67),
                Value::U8(0x2c),
                Value::U8(0xda),
                Value::U8(0x3a),
                Value::U8(0x4f),
                Value::U8(0x37),
                Value::U8(0x2a),
            ],
        );
        assert_guid(
            &index,
            "CLSID_VideoFrameNativeFactory",
            [
                Value::U32(0xd194386a),
                Value::U16(0x04e3),
                Value::U16(0x4814),
                Value::U8(0x81),
                Value::U8(0x00),
                Value::U8(0xb2),
                Value::U8(0xb0),
                Value::U8(0xae),
                Value::U8(0x6d),
                Value::U8(0x78),
                Value::U8(0xc7),
            ],
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn typed_globals_keep_mutable_and_unsupported_initializers_private() {
    helpers::ensure_libclang();

    let source = format!(
        "{SOURCE}\n\
         extern const int EXTERNAL_VALUE;\n\
         static int MUTABLE_INTEGER = 7;\n\
         static const int DYNAMIC_INTEGER = MUTABLE_INTEGER;\n\
         static const int EXTERNAL_INTEGER = EXTERNAL_VALUE;\n\
         static const int* const ADDRESS_INTEGER = &MUTABLE_INTEGER;\n\
         struct Holder {{ static const int LOCAL_INTEGER = 1; }};\n\
         static WCHAR MUTABLE_STRING[] = L\"mutable\";\n\
         static const WCHAR* POINTER_STRING = L\"pointer\";\n\
         static const WCHAR* const CONST_POINTER_STRING = L\"pointer\";\n\
         #define WRAPPED_TEXT(value) L##value\n\
         static const WCHAR WRAPPED_STRING[] = WRAPPED_TEXT(\"wrapped\");\n\
         static const WCHAR COMPOSED_STRING[] = L\"left\" L\"right\";\n\
         GUID MUTABLE_GUID = {{ 1, 2, 3, {{ 4, 5, 6, 7, 8, 9, 10, 11 }} }};\n\
         extern const GUID EXTERNAL_GUID;\n\
         const GUID ALIAS_GUID = EXTERNAL_GUID;\n"
    );
    let snapshot = extract(
        [Input::new("unsupported-globals.h", source)],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let rdl = snapshot.emit("Example.Unsupported").unwrap();

    for name in [
        "MUTABLE_INTEGER",
        "DYNAMIC_INTEGER",
        "EXTERNAL_INTEGER",
        "ADDRESS_INTEGER",
        "LOCAL_INTEGER",
        "MUTABLE_STRING",
        "POINTER_STRING",
        "CONST_POINTER_STRING",
        "WRAPPED_STRING",
        "COMPOSED_STRING",
        "MUTABLE_GUID",
        "EXTERNAL_GUID",
        "ALIAS_GUID",
    ] {
        assert!(
            snapshot
                .constants()
                .iter()
                .all(|constant| constant.name != name),
            "{name}: {:#?}",
            snapshot.constants()
        );
        assert!(
            snapshot
                .facts()
                .iter()
                .all(|fact| { fact.name != name || !matches!(fact.data, FactData::Guid { .. }) }),
            "{name}: {}",
            snapshot.dump()
        );
        assert!(!rdl.contains(name), "{name}: {rdl}");
    }
}

#[test]
fn typed_global_roots_follow_explicit_header_traversal() {
    helpers::ensure_libclang();

    let scratch = scratch("header-policy");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "#pragma once\nstatic const unsigned int DEPENDENCY_ONLY = 9;\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        "#pragma once\n\
         #include \"dependency.h\"\n\
         static const unsigned int PUBLIC_VALUE = 7;\n",
    )
    .unwrap();
    let include = format!("-I{}", scratch.display());
    let snapshot = extract(
        [Input::new("aggregate.cpp", "#include \"public.h\"\n")
            .with_root_dirs([scratch.to_string_lossy().to_string()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc", &include],
    )
    .unwrap();
    assert!(
        snapshot
            .constants()
            .iter()
            .any(|constant| constant.name == "DEPENDENCY_ONLY")
    );
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
    let public = partitions
        .iter()
        .find_map(|(partition, rdl)| {
            (partition.namespace == "Example.Public").then_some(rdl.as_str())
        })
        .unwrap();

    assert!(public.contains("const PUBLIC_VALUE: u32 = 7"), "{public}");
    assert!(
        !partitions
            .values()
            .any(|rdl| rdl.contains("DEPENDENCY_ONLY"))
    );
    std::fs::remove_dir_all(scratch).unwrap();
}

fn scratch(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-typed-globals-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn assert_constant(index: &windows_metadata::reader::Index, name: &str, ty: Type, value: Value) {
    let Item::Const(constant) = index.expect_item("Example.TypedGlobals", name) else {
        panic!("{name} was not emitted as a constant");
    };
    assert_eq!(constant.ty(), ty, "{name}");
    assert_eq!(constant.constant().unwrap().value(), value, "{name}");
}

fn assert_guid(index: &windows_metadata::reader::Index, name: &str, value: [Value; 11]) {
    let Item::Const(guid) = index.expect_item("Example.TypedGlobals", name) else {
        panic!("{name} was not emitted as a GUID constant");
    };
    assert_eq!(
        guid.find_attribute("GuidAttribute").unwrap().value(),
        value
            .into_iter()
            .map(|value| (String::new(), value))
            .collect::<Vec<_>>(),
        "{name}"
    );
}

fn assert_encoding(index: &windows_metadata::reader::Index, name: &str, encoding: &str) {
    let Item::Const(constant) = index.expect_item("Example.TypedGlobals", name) else {
        panic!("{name} was not emitted as a string constant");
    };
    assert_eq!(
        constant
            .find_attribute("NativeEncodingAttribute")
            .unwrap()
            .value(),
        [(String::new(), Value::Utf8(encoding.to_string()))],
        "{name}"
    );
}
