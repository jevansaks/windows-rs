use std::collections::{BTreeMap, BTreeSet};
use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition,
    Scalar, Snapshot, TypeRef, Value, extract, extract_partitioned,
};
use windows_metadata::{HasAttributes, ParamAttributes, Type, reader::Item};

const SOURCE: &str = r#"
    #define MAX_PATH 260
    #define BYTE_CAPACITY 520
    #define RAW_BYTES 36
    #define ODD_BYTES 3
    #define SAL(text) __attribute__((annotate(text)))
    typedef char CHAR;
    typedef unsigned short WCHAR;
    typedef CHAR* PSTR;
    typedef const CHAR* PCSTR;
    typedef WCHAR* PWSTR;
    typedef const WCHAR* PCWSTR;
    struct CallbackOwner { int (*invoke)(int); };
    constexpr int Helper(int value) {
        typedef unsigned LocalType;
        const int LocalConstant = 6;
        return value + LocalConstant;
    }
    constexpr int GlobalValue = Helper(5);
    const int GlobalPlain = 9;
    enum { AnonymousValue = 7 };
    extern "C" void Buffers(
        SAL("_Out_writes_(MAX_PATH)") PSTR narrow,
        SAL("_Out_writes_(MAX_PATH)") PWSTR wide,
        SAL("_In_reads_(MAX_PATH)") PCWSTR input,
        SAL("_Out_writes_(MAX_PATH)") unsigned* raw,
        SAL("_Out_writes_bytes_(BYTE_CAPACITY)") WCHAR* bytes,
        unsigned count,
        SAL("_Out_writes_(count)") unsigned* linked,
        SAL("_Out_writes_bytes_(count)") WCHAR* linked_bytes,
        SAL("_Out_writes_bytes_(RAW_BYTES)") void* opaque,
        SAL("_Out_writes_bytes_(ODD_BYTES)") WCHAR* odd);
    typedef void (*CountCallback)(SAL("_Out_writes_(MAX_PATH)") PWSTR value);
    struct __declspec(uuid("12345678-1234-1234-1234-123456789abc")) ICount {
        virtual void Write(SAL("_Out_writes_(MAX_PATH)") PWSTR value) = 0;
    };
"#;

fn emit(snapshot: &Snapshot, mode: usize, optional: bool) -> Result<String, windows_clang::Error> {
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    assert!(!options.mutable_string_aliases);
    options.mutable_string_aliases = optional;
    options.library = Some("test.dll");
    match mode {
        0 => snapshot.emit_with_options(&options),
        1 => snapshot
            .plan_header_partitions(
                &HeaderPartitionPolicy::new()
                    .with_traversed_header("counts.hpp", RootPartition::new("counts", "Test")),
                &NamespaceAuthorities::new(),
            )?
            .emit_with_options(&options)
            .map(|partitions| partitions.into_values().collect()),
        2 => snapshot
            .clone()
            .emit_partitioned_with_options(&options)
            .map(|partitions| partitions.into_values().collect()),
        _ => unreachable!(),
    }
}

#[test]
fn counts_are_independent_of_string_representation_on_every_emit_path() {
    helpers::ensure_libclang();
    for target in [
        "i686-pc-windows-msvc",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let target_arg = format!("--target={target}");
        let snapshot = extract_partitioned(
            [Input::new("counts.hpp", SOURCE)
                .partitioned("counts")
                .with_root("counts.hpp", "counts", "Test")],
            &["-x", "c++", "-std=c++20", "-fms-extensions", &target_arg],
        )
        .unwrap();
        let before = snapshot.clone();
        for mode in 0..3 {
            for optional in [false, true] {
                let rdl = emit(&snapshot, mode, optional).unwrap();
                let output = std::env::temp_dir().join(format!(
                    "windows-clang-count-transport-{}-{target}-{mode}-{optional}.winmd",
                    std::process::id()
                ));
                windows_rdl::reader()
                    .input_text(include_str!("../../../../metadata/metadata.rdl"))
                    .input_text(&rdl)
                    .output(&output)
                    .write()
                    .unwrap();
                let index = windows_metadata::reader::Index::read(&output).unwrap();
                std::fs::remove_file(output).unwrap();
                let Item::Fn(function) = index.expect_item("Test", "Buffers") else {
                    panic!("{rdl}");
                };
                let types = function.signature(&[]).types;
                assert_eq!(types[0], Type::value_named("Test", "PSTR"));
                assert_eq!(types[1], Type::value_named("Test", "PWSTR"));
                assert_eq!(
                    types[2],
                    Type::value_named("Test", if optional { "PWSTR" } else { "PCWSTR" })
                );
                assert_eq!(types[3], Type::PtrMut(Box::new(Type::U32), 1));
                assert_eq!(types[4], Type::PtrMut(Box::new(Type::U16), 1));
                assert_eq!(types[8], Type::PtrMut(Box::new(Type::Void), 1));
                assert_eq!(types[9], Type::PtrMut(Box::new(Type::U16), 1));
                let rows = function.params_by_sequence(10).unwrap();
                for (position, row) in rows.params().iter().enumerate() {
                    let row = row.unwrap();
                    if position < 5 {
                        assert_eq!(
                            row.find_attribute("NativeArrayInfoAttribute")
                                .unwrap()
                                .value(),
                            [("CountConst".to_string(), windows_metadata::Value::I32(260))]
                        );
                    }
                    if position >= 8 {
                        assert_eq!(
                            row.buffer_relationship(),
                            Some(windows_metadata::reader::BufferRelationship::BytesConst(
                                if position == 8 { 36 } else { 3 }
                            ))
                        );
                        assert!(!row.has_attribute("NativeArrayInfoAttribute"));
                        assert_eq!(
                            row.find_attribute("MemorySizeAttribute").unwrap().value(),
                            [(
                                "BytesConst".to_string(),
                                windows_metadata::Value::I32(if position == 8 { 36 } else { 3 })
                            )]
                        );
                    }
                    assert_eq!(
                        row.flags(),
                        if matches!(position, 2 | 5) {
                            ParamAttributes::In
                        } else {
                            ParamAttributes::Out
                        }
                    );
                    assert_eq!(
                        row.has_attribute("ConstAttribute"),
                        optional && position == 2
                    );
                }
                for (position, attribute, property) in [
                    (6, "NativeArrayInfoAttribute", "CountParamIndex"),
                    (7, "MemorySizeAttribute", "BytesParamIndex"),
                ] {
                    assert_eq!(
                        rows.params()[position]
                            .unwrap()
                            .find_attribute(attribute)
                            .unwrap()
                            .value(),
                        [(property.to_string(), windows_metadata::Value::I16(5))]
                    );
                }
                for (name, method) in [("CountCallback", "Invoke"), ("ICount", "Write")] {
                    let callable = index
                        .expect("Test", name)
                        .methods()
                        .find(|candidate| candidate.name() == method)
                        .unwrap();
                    let row = callable.params_by_sequence(1).unwrap().params()[0].unwrap();
                    assert_eq!(
                        row.find_attribute("NativeArrayInfoAttribute")
                            .unwrap()
                            .value(),
                        [("CountConst".to_string(), windows_metadata::Value::I32(260))]
                    );
                    assert_eq!(row.flags(), ParamAttributes::Out);
                }
                assert_eq!(snapshot, before);
            }
        }
        for name in ["LocalType", "LocalConstant"] {
            assert!(snapshot.facts().iter().all(|fact| fact.name != name));
            assert!(snapshot.constants().iter().all(|value| value.name != name));
            assert!(
                snapshot
                    .value_declarations()
                    .iter()
                    .all(|value| value.name != name)
            );
        }
        for (name, value) in [
            ("GlobalValue", 11),
            ("GlobalPlain", 9),
            ("AnonymousValue", 7),
        ] {
            let constant = snapshot
                .constants()
                .iter()
                .find(|value| value.name == name)
                .unwrap();
            assert_eq!(constant.value, Value::Signed(value));
            assert_eq!(constant.ty, TypeRef::Scalar(Scalar::I32));
            assert_eq!(
                snapshot
                    .value_declarations()
                    .iter()
                    .filter(|value| value.origin == constant.root)
                    .count(),
                1
            );
        }
        let helper = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "Helper")
            .unwrap();
        assert!(matches!(helper.data, FactData::NonEmittableFunction { .. }));
        let callback = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "CallbackOwner_invoke")
            .unwrap();
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
        assert!(
            origins
                .iter()
                .all(|origin| **origin == callback.origin || origin.local < callback.origin.local)
        );
    }
}

#[test]
fn selected_invalid_counts_fail_in_both_modes() {
    helpers::ensure_libclang();
    for expression in ["-1", "NEGATIVE", "TOO_LARGE"] {
        let source = format!(
            "#define NEGATIVE -1\n#define TOO_LARGE 2147483648ULL\n\
             extern \"C\" void Bad(unsigned count, \
             __attribute__((annotate(\"_Out_writes_({expression})\"))) unsigned* value);"
        );
        let snapshot = extract(
            [Input::new("counts.hpp", source)],
            &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
        )
        .unwrap();
        for mode in 0..2 {
            for optional in [false, true] {
                let error = emit(&snapshot, mode, optional).unwrap_err().to_string();
                assert!(error.contains(expression), "{error}");
                assert!(error.contains("Bad") && error.contains("value"), "{error}");
            }
        }
    }
}

#[test]
fn unsupported_raw_counts_retain_apis_and_report_selected_source_diagnostics() {
    helpers::ensure_libclang();
    for expression in ["UNKNOWN_COUNT", "count + 1"] {
        let source = format!(
            "extern \"C\" void Bad(unsigned count, \
                         __attribute__((annotate(\"_Out_writes_({expression})\"))) unsigned* value);\n\
                         extern \"C\" void Known(unsigned* value);\n\
                         inline void Helper(__attribute__((annotate(\"_Out_writes_(UNKNOWN_HELPER)\"))) \
                         unsigned* value) {{}}\n"
        );
        let snapshot = extract_partitioned(
            [Input::new("counts.hpp", source)
                .partitioned("counts")
                .with_root("counts.hpp", "counts", "Test")],
            &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
        )
        .unwrap();
        let before = snapshot.clone();
        let references = BTreeMap::new();
        for optional in [false, true] {
            let mut options = EmitOptions::new("Test", &references);
            options.mutable_string_aliases = optional;
            options.library = Some("test.dll");
            let diagnostics = snapshot.sal_count_diagnostics(&options).unwrap();
            assert_eq!(diagnostics.len(), 1);
            let diagnostic = &diagnostics[0];
            assert_eq!(diagnostic.source.file, "counts.hpp");
            assert_eq!(diagnostic.declaration.tu, "counts.hpp");
            assert_eq!(diagnostic.parameter, "value");
            assert!(diagnostic.reason.contains(expression), "{diagnostic:?}");
            let function = snapshot
                .facts()
                .iter()
                .find(|fact| fact.name == "Bad")
                .unwrap();
            assert_eq!(diagnostic.declaration, function.origin);
            let FactData::Function { params, .. } = &function.data else {
                panic!()
            };
            assert_eq!(
                params[1].annotation.size.as_ref().unwrap().value,
                windows_clang::SalSizeValue::Expression(expression.to_string())
            );
            let header = snapshot
                .plan_header_partitions(
                    &HeaderPartitionPolicy::new()
                        .with_traversed_header("counts.hpp", RootPartition::new("counts", "Test")),
                    &NamespaceAuthorities::new(),
                )
                .unwrap();
            assert_eq!(header.sal_count_diagnostics(&options).unwrap(), diagnostics);
            for mode in 0..3 {
                let rdl = emit(&snapshot, mode, optional).unwrap();
                assert!(rdl.contains("fn Bad(count: u32, value: *mut u32)"), "{rdl}");
                assert!(
                    !rdl.contains("len_const") && !rdl.contains("size_const"),
                    "{rdl}"
                );
            }
            let selected = BTreeSet::from(["Known".to_string()]);
            options.functions = Some(&selected);
            assert!(snapshot.sal_count_diagnostics(&options).unwrap().is_empty());
            assert_eq!(snapshot, before);
        }
    }
}
