use std::collections::BTreeMap;
use windows_clang::{
    EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition, extract,
};
use windows_metadata::{HasAttributes, ParamAttributes, Type, reader::Item};

const SOURCE: &str = include_str!("../../../tests/libs/clang/input/string_projection.h");

fn emit(
    source: &str,
    mutable_string_aliases: bool,
    partitioned: bool,
) -> Result<String, windows_clang::Error> {
    helpers::ensure_libclang();
    let snapshot = extract(
        [Input::new("strings.hpp", source)],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )?;
    let before = snapshot.facts().to_vec();
    let before_annotations = snapshot.annotations().clone();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("test.dll");
    options.mutable_string_aliases = mutable_string_aliases;
    let rdl = if partitioned {
        let policy = HeaderPartitionPolicy::new()
            .with_traversed_header("strings.hpp", RootPartition::new("strings", "Test"));
        snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())?
            .emit_with_options(&options)?
            .into_values()
            .collect()
    } else {
        snapshot.emit_with_options(&options)?
    };
    assert_eq!(snapshot.facts(), before);
    assert_eq!(snapshot.annotations(), &before_annotations);
    Ok(rdl)
}

fn compile(mutable_string_aliases: bool, partitioned: bool) -> windows_metadata::reader::Index {
    let rdl = emit(SOURCE, mutable_string_aliases, partitioned).unwrap();
    compile_rdl(&rdl, &format!("{mutable_string_aliases}-{partitioned}"))
}

fn compile_rdl(rdl: &str, tag: &str) -> windows_metadata::reader::Index {
    let output = std::env::temp_dir().join(format!(
        "windows-clang-string-projection-{}-{tag}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_text(include_str!("../../../../metadata/metadata.rdl"))
        .input_text(rdl)
        .output(&output)
        .write()
        .unwrap_or_else(|error| panic!("{error}\n{rdl}"));
    let index = windows_metadata::reader::Index::read(&output).unwrap();
    std::fs::remove_file(output).unwrap();
    index
}

fn assert_nominal_strings(index: &windows_metadata::reader::Index) {
    let narrow = Type::value_named("Test", "TEXT_HANDLE_ANSI");
    let wide = Type::value_named("Test", "TEXT_HANDLE_W");
    assert_eq!(
        index.expect("Test", "TEXT_HANDLE_ANSI").underlying_type(),
        Some(Type::value_named("Test", "PCSTR"))
    );
    assert_eq!(
        index.expect("Test", "TEXT_HANDLE_W").underlying_type(),
        Some(Type::value_named("Test", "PCWSTR"))
    );
    let Item::Fn(function) = index.expect_item("Test", "UseNominalStrings") else {
        panic!("UseNominalStrings is not a function");
    };
    assert_eq!(
        function.signature(&[]).types,
        [
            narrow.clone(),
            wide.clone(),
            narrow.clone(),
            wide.clone(),
            narrow.clone(),
            wide.clone(),
        ]
    );
    let rows = function.params_by_sequence(6).unwrap();
    for row in rows.params() {
        let row = row.unwrap();
        assert_eq!(row.flags(), ParamAttributes::In);
        assert!(!row.has_attribute("ConstAttribute"));
        assert!(!row.has_attribute("NotNullTerminatedAttribute"));
        assert!(!row.has_attribute("NullNullTerminatedAttribute"));
    }
    let fields = index
        .expect("Test", "NominalStrings")
        .fields()
        .collect::<Vec<_>>();
    assert_eq!(fields[0].ty(), narrow);
    assert_eq!(fields[1].ty(), wide);
    assert!(
        fields
            .iter()
            .all(|field| !field.has_attribute("ConstAttribute"))
    );
    for (name, expected) in [("GetNominalString", narrow), ("GetNominalWideString", wide)] {
        let Item::Fn(function) = index.expect_item("Test", name) else {
            panic!("{name} is not a function");
        };
        assert_eq!(function.signature(&[]).return_type, expected);
        assert!(
            function
                .params_by_sequence(0)
                .unwrap()
                .return_param()
                .is_none()
        );
    }
}

fn assert_constant_counts(index: &windows_metadata::reader::Index, compatibility: bool) {
    for (name, counts) in [
        ("ConstantCounts", vec![8, 4, 10]),
        ("AlteredConstantCounts", vec![12, 6]),
    ] {
        let Item::Fn(function) = index.expect_item("Test", name) else {
            panic!("{name} is not a function");
        };
        let rows = function.params_by_sequence(counts.len()).unwrap();
        for (position, count) in counts.into_iter().enumerate() {
            let row = rows.params()[position].unwrap();
            assert_eq!(
                row.find_attribute("NativeArrayInfoAttribute")
                    .unwrap()
                    .value(),
                [(
                    "CountConst".to_string(),
                    windows_metadata::Value::I32(count)
                )]
            );
            assert!(!row.has_attribute("MemorySizeAttribute"));
            assert_eq!(row.has_attribute("ConstAttribute"), compatibility);
            assert_eq!(
                row.has_attribute("NotNullTerminatedAttribute"),
                compatibility
            );
            assert_eq!(
                function.signature(&[]).types[position],
                Type::value_named(
                    "Test",
                    match (position == 2, compatibility) {
                        (true, true) => "PSTR",
                        (true, false) => "PCSTR",
                        (false, true) => "PWSTR",
                        (false, false) => "PCWSTR",
                    }
                )
            );
        }
    }
}

#[test]
fn mutable_string_aliases_preserve_physical_use_contracts() {
    let index = compile(true, true);
    assert_nominal_strings(&index);
    assert_constant_counts(&index, true);
    assert_eq!(
        index.expect("Test", "PSTR").underlying_type(),
        Some(Type::PtrMut(Box::new(Type::I8), 1))
    );
    assert_eq!(
        index.expect("Test", "PWSTR").underlying_type(),
        Some(Type::PtrMut(Box::new(Type::U16), 1))
    );
    assert_eq!(
        index.expect("Test", "PCWSTR").underlying_type(),
        Some(Type::PtrConst(Box::new(Type::U16), 1))
    );
    let Item::Fn(function) = index.expect_item("Test", "UseStrings") else {
        panic!("UseStrings is not a function");
    };
    let pstr = Type::value_named("Test", "PSTR");
    let pwstr = Type::value_named("Test", "PWSTR");
    assert_eq!(
        function.signature(&[]).types,
        [
            pstr.clone(),
            pwstr.clone(),
            pwstr.clone(),
            Type::U32,
            pstr.clone(),
            pstr.clone(),
            Type::PtrConst(Box::new(Type::I8), 1),
            pwstr.clone(),
            pstr.clone(),
            pstr.clone(),
            pwstr.clone(),
            Type::PtrMut(Box::new(Type::value_named("Test", "PCWSTR")), 1),
            Type::class_named("Test", "StringCallback"),
        ]
    );
    let rows = function.params_by_sequence(13).unwrap();
    assert!(rows.return_param().is_none());
    for (index, row) in rows.params().iter().enumerate() {
        let row = row.unwrap();
        assert_eq!(
            row.has_attribute("ConstAttribute"),
            [0, 2, 4, 7, 8, 9, 10].contains(&index),
            "parameter {index}"
        );
        assert_eq!(
            row.has_attribute("NotNullTerminatedAttribute"),
            [4, 5, 8, 9, 10].contains(&index),
            "parameter {index}"
        );
        assert_eq!(row.has_attribute("NullNullTerminatedAttribute"), index == 7);
    }
    assert!(
        rows.params()[0]
            .unwrap()
            .flags()
            .contains(ParamAttributes::In)
    );
    assert!(
        rows.params()[1]
            .unwrap()
            .flags()
            .contains(ParamAttributes::Out)
    );
    assert!(
        rows.params()[2]
            .unwrap()
            .flags()
            .contains(ParamAttributes::Optional)
    );
    for (position, name, property) in [
        (4, "NativeArrayInfoAttribute", "CountParamIndex"),
        (5, "MemorySizeAttribute", "BytesParamIndex"),
        (6, "NativeArrayInfoAttribute", "CountParamIndex"),
    ] {
        let attribute = rows.params()[position]
            .unwrap()
            .find_attribute(name)
            .unwrap();
        assert_eq!(
            attribute.value(),
            [(property.to_string(), windows_metadata::Value::I16(3))]
        );
    }
    assert!(!function.has_attribute("ConstAttribute"));

    let fields = index.expect("Test", "Strings").fields().collect::<Vec<_>>();
    assert_eq!(fields[0].ty(), pstr);
    assert!(fields[0].has_attribute("ConstAttribute"));
    assert_eq!(fields[1].ty(), pwstr);
    assert!(!fields[1].has_attribute("ConstAttribute"));
    assert_eq!(fields[2].ty(), pwstr);
    assert!(fields[2].has_attribute("ConstAttribute"));
    assert!(fields[2].has_attribute("NotNullTerminatedAttribute"));
    assert_eq!(fields[3].ty(), pstr);
    assert!(fields[3].has_attribute("ConstAttribute"));
    assert!(fields[3].has_attribute("NullNullTerminatedAttribute"));
    assert_eq!(
        fields[4].ty(),
        Type::PtrMut(Box::new(Type::value_named("Test", "PCWSTR")), 1)
    );
    assert!(!fields[4].has_attribute("ConstAttribute"));
    assert_eq!(fields[5].ty(), Type::PtrConst(Box::new(pwstr.clone()), 1));
    assert_eq!(fields[6].ty(), Type::PtrConst(Box::new(Type::U16), 1));

    for (name, expected, multi) in [
        ("GetString", Type::value_named("Test", "PSTR"), false),
        ("GetMulti", pwstr.clone(), true),
    ] {
        let Item::Fn(function) = index.expect_item("Test", name) else {
            panic!("{name} is not a function");
        };
        assert_eq!(function.signature(&[]).return_type, expected);
        let rows = function.params_by_sequence(0).unwrap();
        let result = rows.return_param().unwrap();
        assert!(result.has_attribute("ConstAttribute"));
        assert_eq!(result.has_attribute("NullNullTerminatedAttribute"), multi);
        assert!(!function.has_attribute("ConstAttribute"));
    }
    let callback = index.expect("Test", "StringCallback");
    let invoke = callback
        .methods()
        .find(|method| method.name() == "Invoke")
        .unwrap();
    assert_eq!(invoke.signature(&[]).return_type, pwstr);
    assert_eq!(
        invoke.signature(&[]).types,
        [Type::value_named("Test", "PSTR"), pwstr]
    );
    let rows = invoke.params_by_sequence(2).unwrap();
    assert!(rows.return_param().unwrap().has_attribute("ConstAttribute"));
    assert!(rows.params()[0].unwrap().has_attribute("ConstAttribute"));
    assert!(!rows.params()[1].unwrap().has_attribute("ConstAttribute"));
    assert!(
        rows.params()[1]
            .unwrap()
            .flags()
            .contains(ParamAttributes::Out)
    );
    assert!(!callback.has_attribute("ConstAttribute"));

    let Item::Fn(more) = index.expect_item("Test", "MoreStrings") else {
        panic!("MoreStrings is not a function");
    };
    let pstr = Type::value_named("Test", "PSTR");
    let pwstr = Type::value_named("Test", "PWSTR");
    assert_eq!(
        more.signature(&[]).types,
        [
            pstr.clone(),
            pstr.clone(),
            pstr.clone(),
            pstr.clone(),
            pstr.clone(),
            pwstr.clone(),
            pwstr.clone(),
            Type::U32,
            pwstr.clone(),
            Type::PtrConst(Box::new(Type::U8), 1),
            pstr.clone(),
            pwstr.clone(),
            pstr.clone(),
            pstr.clone(),
            pwstr.clone(),
            pwstr.clone(),
            pwstr.clone(),
        ]
    );
    let rows = more.params_by_sequence(17).unwrap();
    for (position, row) in rows.params().iter().enumerate() {
        let row = row.unwrap();
        assert_eq!(
            row.has_attribute("ConstAttribute"),
            [4, 6, 10, 11, 13, 15, 16].contains(&position)
        );
        assert_eq!(
            row.has_attribute("NotNullTerminatedAttribute"),
            [8, 10, 16].contains(&position)
        );
        assert_eq!(
            row.has_attribute("NullNullTerminatedAttribute"),
            [12, 13, 14, 15].contains(&position)
        );
        assert!(
            row.attributes()
                .filter(|attribute| attribute.name() == "ConstAttribute")
                .count()
                <= 1
        );
    }
    for (position, flags) in [
        (0, ParamAttributes::In),
        (1, ParamAttributes::Out),
        (2, ParamAttributes::Out | ParamAttributes::Optional),
        (3, ParamAttributes::In),
        (8, ParamAttributes::In | ParamAttributes::Out),
    ] {
        assert_eq!(rows.params()[position].unwrap().flags(), flags);
    }
    assert_eq!(
        rows.params()[16]
            .unwrap()
            .find_attribute("NativeArrayInfoAttribute")
            .unwrap()
            .value(),
        [("CountConst".to_string(), windows_metadata::Value::I32(4))]
    );
    assert_eq!(
        rows.params()[11]
            .unwrap()
            .find_attribute("NativeArrayInfoAttribute")
            .unwrap()
            .value(),
        [(
            "CountParamIndex".to_string(),
            windows_metadata::Value::I16(7)
        )]
    );

    let record = index.expect("Test", "CountedFields");
    let fields = record.fields().collect::<Vec<_>>();
    assert_eq!(fields[1].ty(), pstr);
    assert_eq!(
        fields[1]
            .find_attribute("NativeArrayInfoAttribute")
            .unwrap()
            .value(),
        [(
            "CountFieldName".to_string(),
            windows_metadata::Value::Utf8("length".to_string())
        )]
    );
    assert!(fields[1].has_attribute("ConstAttribute"));
    assert!(fields[1].has_attribute("NotNullTerminatedAttribute"));
    assert_eq!(fields[2].ty(), pwstr);
    assert!(fields[2].has_attribute("NotNullTerminatedAttribute"));
    assert!(!fields[2].has_attribute("ConstAttribute"));
    assert_eq!(
        fields[3]
            .attributes()
            .filter(|attribute| attribute.name() == "ConstAttribute")
            .count(),
        1
    );
    let inner = index.nested(record).next().unwrap();
    let fields = inner.fields().collect::<Vec<_>>();
    assert_eq!(fields[0].ty(), pstr);
    assert!(fields[0].has_attribute("ConstAttribute"));
    assert_eq!(fields[1].ty(), pwstr);
    assert!(fields[1].has_attribute("NotNullTerminatedAttribute"));
    assert!(!fields[1].has_attribute("ConstAttribute"));
    assert!(!record.has_attribute("ConstAttribute"));
    assert!(!inner.has_attribute("ConstAttribute"));
    let array = record
        .fields()
        .find(|field| field.name() == "array")
        .unwrap();
    assert_eq!(
        array.ty(),
        Type::ArrayFixed(Box::new(Type::value_named("Test", "PCSTR")), 2)
    );
    assert!(!array.has_attribute("ConstAttribute"));

    let interface = index.expect("Test", "IStrings");
    for (name, return_type, param_type, const_return, const_param, direction) in [
        (
            "Read",
            pwstr.clone(),
            pstr.clone(),
            true,
            true,
            ParamAttributes::In,
        ),
        (
            "Write",
            pstr.clone(),
            pstr,
            false,
            false,
            ParamAttributes::Out,
        ),
    ] {
        let method = interface
            .methods()
            .find(|method| method.name() == name)
            .unwrap();
        assert_eq!(method.signature(&[]).return_type, return_type);
        assert_eq!(method.signature(&[]).types, [param_type]);
        let rows = method.params_by_sequence(1).unwrap();
        assert_eq!(
            rows.return_param()
                .is_some_and(|row| row.has_attribute("ConstAttribute")),
            const_return
        );
        assert_eq!(
            rows.params()[0].unwrap().has_attribute("ConstAttribute"),
            const_param
        );
        assert_eq!(rows.params()[0].unwrap().flags(), direction);
        assert!(!method.has_attribute("ConstAttribute"));
    }
    assert!(!interface.has_attribute("ConstAttribute"));

    for (name, expected, const_return, multi) in [
        (
            "GetMutable",
            Type::value_named("Test", "PSTR"),
            false,
            false,
        ),
        (
            "GetNested",
            Type::PtrConst(Box::new(Type::value_named("Test", "PCSTR")), 1),
            false,
            false,
        ),
        ("GetRawMulti", pwstr, true, true),
    ] {
        let Item::Fn(function) = index.expect_item("Test", name) else {
            panic!("{name} is not a function");
        };
        assert_eq!(function.signature(&[]).return_type, expected);
        let rows = function.params_by_sequence(0).unwrap();
        assert_eq!(
            rows.return_param()
                .is_some_and(|row| row.has_attribute("ConstAttribute")),
            const_return
        );
        assert_eq!(
            rows.return_param()
                .is_some_and(|row| row.has_attribute("NullNullTerminatedAttribute")),
            multi
        );
        assert!(!function.has_attribute("ConstAttribute"));
    }
    for (name, expected, multi) in [
        ("PlainRaw", Type::PtrConst(Box::new(Type::U16), 1), false),
        ("MultiRaw", Type::value_named("Test", "PWSTR"), true),
    ] {
        let Item::Fn(function) = index.expect_item("Test", name) else {
            panic!("{name} is not a function");
        };
        assert_eq!(function.signature(&[]).types, [expected]);
        let rows = function.params_by_sequence(1).unwrap();
        assert_eq!(
            rows.params()[0].unwrap().has_attribute("ConstAttribute"),
            multi
        );
        assert_eq!(
            rows.params()[0]
                .unwrap()
                .has_attribute("NullNullTerminatedAttribute"),
            multi
        );
    }
    let Item::Fn(function) = index.expect_item("Test", "PointerGates") else {
        panic!("PointerGates is not a function");
    };
    assert_eq!(
        function.signature(&[]).types,
        [
            Type::PtrMut(Box::new(Type::Void), 1),
            Type::PtrMut(Box::new(Type::Void), 1),
            Type::value_named("Test", "HANDLE"),
            Type::PtrConst(Box::new(Type::value_named("Test", "LPVOID")), 1),
            Type::value_named("Test", "TEXT_HANDLE"),
        ]
    );
    let rows = function.params_by_sequence(5).unwrap();
    assert_eq!(rows.params()[0].unwrap().flags(), ParamAttributes::Out);
    assert_eq!(rows.params()[1].unwrap().flags(), ParamAttributes::In);
    assert_eq!(rows.params()[2].unwrap().flags(), ParamAttributes::In);
    assert!(
        rows.params()
            .iter()
            .all(|row| !row.unwrap().has_attribute("ConstAttribute"))
    );
}

#[test]
fn default_string_projection_keeps_const_aliases() {
    let index = compile(false, true);
    assert_nominal_strings(&index);
    assert_constant_counts(&index, false);
    let Item::Fn(function) = index.expect_item("Test", "UseStrings") else {
        panic!("UseStrings is not a function");
    };
    assert_eq!(
        function.signature(&[]).types[0],
        Type::value_named("Test", "PCSTR")
    );
    assert_eq!(
        function.signature(&[]).types[2],
        Type::value_named("Test", "PCWSTR")
    );
    assert_eq!(
        index
            .expect("Test", "Strings")
            .fields()
            .next()
            .unwrap()
            .ty(),
        Type::value_named("Test", "PCSTR")
    );
    let rows = function.params_by_sequence(13).unwrap();
    assert!(
        rows.params()
            .iter()
            .all(|row| !row.unwrap().has_attribute("ConstAttribute"))
    );
    assert_eq!(
        index.expect("Test", "PCSTR").underlying_type(),
        Some(Type::PtrConst(Box::new(Type::I8), 1))
    );
}

#[test]
fn mutable_string_aliases_work_without_header_partitioning() {
    let index = compile(true, false);
    assert_nominal_strings(&index);
    assert_constant_counts(&index, true);
    let Item::Fn(function) = index.expect_item("Test", "UseStrings") else {
        panic!("UseStrings is not a function");
    };
    assert_eq!(
        function.signature(&[]).types[0],
        Type::value_named("Test", "PSTR")
    );
    assert!(
        function.params_by_sequence(13).unwrap().params()[0]
            .unwrap()
            .has_attribute("ConstAttribute")
    );
    assert_eq!(
        index.expect("Test", "PSTR").underlying_type(),
        Some(Type::PtrMut(Box::new(Type::I8), 1))
    );
    assert_eq!(
        index.expect("Test", "PCWSTR").underlying_type(),
        Some(Type::PtrConst(Box::new(Type::U16), 1))
    );
}

#[test]
fn string_projection_uses_existing_metadata_definitions() {
    helpers::ensure_libclang();
    let reference = std::env::temp_dir().join(format!(
        "windows-clang-string-reference-{}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_text(
            r#"
            #[win32]
            mod Sdk {
                type PSTR = *mut i8;
                type PCSTR = *const i8;
                type PWSTR = *mut u16;
                type PCWSTR = *const u16;
            }
        "#,
        )
        .output(&reference)
        .write()
        .unwrap();
    let reference_index = windows_metadata::reader::Index::read(&reference).unwrap();
    assert_eq!(
        reference_index.expect("Sdk", "PSTR").underlying_type(),
        Some(Type::PtrMut(Box::new(Type::I8), 1))
    );
    assert_eq!(
        reference_index.expect("Sdk", "PCWSTR").underlying_type(),
        Some(Type::PtrConst(Box::new(Type::U16), 1))
    );
    let references = windows_clang::MetadataReferences::new([windows_metadata::reader::File::new(
        std::fs::read(&reference).unwrap(),
    )
    .unwrap()]);
    let snapshot = extract(
        [Input::new("strings.hpp", SOURCE)],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let mut options = EmitOptions::new("Test", references.types());
    references.apply_reference_exclusions(&mut options);
    options.library = Some("test.dll");
    options.mutable_string_aliases = true;
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header("strings.hpp", RootPartition::new("strings", "Test"));
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let output = std::env::temp_dir().join(format!(
        "windows-clang-string-reference-output-{}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_text(include_str!("../../../../metadata/metadata.rdl"))
        .input_texts(partitions.values())
        .reference(&reference)
        .output(&output)
        .write()
        .unwrap_or_else(|error| panic!("{error}\n{partitions:?}"));
    let index = windows_metadata::reader::Index::read(&output).unwrap();
    assert!(!index.contains("Test", "PSTR"));
    assert!(!index.contains("Test", "PWSTR"));
    let Item::Fn(function) = index.expect_item("Test", "UseStrings") else {
        panic!("UseStrings is not a function");
    };
    let types = function.signature(&[]).types;
    assert_eq!(types[0], Type::value_named("Sdk", "PSTR"));
    assert_eq!(types[1], Type::value_named("Sdk", "PWSTR"));
    assert_eq!(
        types[11],
        Type::PtrMut(Box::new(Type::value_named("Sdk", "PCWSTR")), 1)
    );
    let rows = function.params_by_sequence(13).unwrap();
    assert!(rows.params()[0].unwrap().has_attribute("ConstAttribute"));
    assert!(!rows.params()[11].unwrap().has_attribute("ConstAttribute"));
    std::fs::remove_file(output).unwrap();
    std::fs::remove_file(reference).unwrap();
}

#[test]
fn string_projection_reports_missing_or_invalid_native_definitions() {
    for (source, expected) in [
        (
            r#"
            extern "C" void Use(__attribute__((annotate("_In_z_"))) const char* value);
        "#,
            "PSTR",
        ),
        (
            r#"
            typedef const char* PSTR;
            extern "C" void Use(__attribute__((annotate("_In_z_"))) const char* value);
        "#,
            "native mutable single-character pointer typedef",
        ),
        (
            r#"
            typedef unsigned short* PSTR;
            extern "C" void Use(__attribute__((annotate("_In_z_"))) const char* value);
        "#,
            "native mutable single-character pointer typedef",
        ),
    ] {
        for partitioned in [false, true] {
            let error = emit(source, true, partitioned).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("strings.hpp"), "{error}");
            if expected.starts_with("cannot preserve SAL count expression") {
                assert!(error.contains("`Use` parameter `value`"), "{error}");
            }
        }
    }
}

#[test]
fn constant_counts_keep_independent_tu_values() {
    helpers::ensure_libclang();
    let source = |name: &str, count: i32| {
        format!(
            "#define CAPACITY {count}\n\
         typedef char* PSTR;\n\
         extern \"C\" void {name}(\
         __attribute__((annotate(\"_In_reads_(CAPACITY)\"))) PSTR value);"
        )
    };
    let snapshot = extract(
        [
            Input::new("first.hpp", source("First", 8)),
            Input::new("second.hpp", source("Second", 12)),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let excluded_constants = std::collections::BTreeSet::from(["CAPACITY".to_string()]);
    let mut options = EmitOptions::new("Test", &references);
    options.excluded_constants = Some(&excluded_constants);
    options.library = Some("test.dll");
    options.mutable_string_aliases = true;
    let before = snapshot.facts().to_vec();
    let rdl = snapshot.emit_with_options(&options).unwrap();
    assert_eq!(snapshot.facts(), before);
    let output = std::env::temp_dir().join(format!(
        "windows-clang-sal-two-tu-{}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_text(include_str!("../../../../metadata/metadata.rdl"))
        .input_text(&rdl)
        .output(&output)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&output).unwrap();
    for (name, count) in [("First", 8), ("Second", 12)] {
        let Item::Fn(function) = index.expect_item("Test", name) else {
            panic!("{name} is not a function");
        };
        assert_eq!(
            function.signature(&[]).types,
            [Type::value_named("Test", "PSTR")]
        );
        assert_eq!(
            function.params_by_sequence(1).unwrap().params()[0]
                .unwrap()
                .find_attribute("NativeArrayInfoAttribute")
                .unwrap()
                .value(),
            [(
                "CountConst".to_string(),
                windows_metadata::Value::I32(count)
            )],
        );
    }
    std::fs::remove_file(output).unwrap();

    let snapshot = extract(
        [
            Input::new("first.hpp", source("Same", 8)),
            Input::new("second.hpp", source("Same", 12)),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    options.mutable_string_aliases = false;
    snapshot.emit_with_options(&options).unwrap();
    options.mutable_string_aliases = true;
    let rdl = snapshot.emit_with_options(&options).unwrap();
    let index = compile_rdl(&rdl, "independent-selected");
    let Item::Fn(function) = index.expect_item("Test", "Same") else {
        panic!("Same is not a function");
    };
    assert_eq!(
        function.params_by_sequence(1).unwrap().params()[0]
            .unwrap()
            .find_attribute("NativeArrayInfoAttribute")
            .unwrap()
            .value(),
        [("CountConst".to_string(), windows_metadata::Value::I32(8))],
    );
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header("first.hpp", RootPartition::new("first", "First"))
        .with_traversed_header("second.hpp", RootPartition::new("second", "Second"));
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let rdl = partitions.into_values().collect::<String>();
    let index = compile_rdl(&rdl, "independent-scoped");
    for (namespace, count) in [("First", 8), ("Second", 12)] {
        let Item::Fn(function) = index.expect_item(namespace, "Same") else {
            panic!("{namespace}.Same is not a function");
        };
        assert_eq!(
            function.signature(&[]).types,
            [Type::value_named("First", "PSTR")]
        );
        assert_eq!(
            function.params_by_sequence(1).unwrap().params()[0]
                .unwrap()
                .find_attribute("NativeArrayInfoAttribute")
                .unwrap()
                .value(),
            [(
                "CountConst".to_string(),
                windows_metadata::Value::I32(count)
            )],
        );
    }
}

#[test]
fn constant_counts_reject_repeated_selected_native_uses() {
    helpers::ensure_libclang();
    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-shared-sal-counts-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&scratch).unwrap();
    let common = scratch.join("common.h");
    for (name, declaration) in [
        (
            "Same",
            r#"extern "C" void Same(__attribute__((annotate("_In_reads_(CAPACITY)"))) PSTR value);"#,
        ),
        (
            "CountCallback",
            r#"typedef void (*CountCallback)(__attribute__((annotate("_In_reads_(CAPACITY)"))) PSTR value);"#,
        ),
        (
            "ICount",
            r#"struct __declspec(uuid("12345678-1234-1234-1234-123456789abc")) ICount { virtual void Count(__attribute__((annotate("_In_reads_(CAPACITY)"))) PSTR value) = 0; };"#,
        ),
    ] {
        std::fs::write(&common, format!("typedef char* PSTR;\n{declaration}\n")).unwrap();
        let snapshot = extract(
            [
                Input::new(
                    "first.cpp",
                    format!("#define CAPACITY 8\n#include \"{}\"\n", common.display()),
                )
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
                Input::new(
                    "second.cpp",
                    format!("#define CAPACITY 12\n#include \"{}\"\n", common.display()),
                )
                .with_root_dirs([scratch.to_string_lossy().to_string()]),
            ],
            &[
                "-x",
                "c++",
                "-fms-extensions",
                "--target=x86_64-pc-windows-msvc",
            ],
        )
        .unwrap();
        let references = BTreeMap::new();
        let excluded_constants = std::collections::BTreeSet::from(["CAPACITY".to_string()]);
        let mut options = EmitOptions::new("Test", &references);
        options.excluded_constants = Some(&excluded_constants);
        options.library = Some("test.dll");
        let policy = HeaderPartitionPolicy::new()
            .with_traversed_header_for_input(
                "first.cpp",
                common.to_string_lossy(),
                RootPartition::new("shared", "Test"),
            )
            .with_traversed_header_for_input(
                "second.cpp",
                common.to_string_lossy(),
                RootPartition::new("shared", "Test"),
            );
        let plan = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap();
        for partitioned in [false, true] {
            for compatibility in [false, true] {
                options.mutable_string_aliases = compatibility;
                let error = if partitioned {
                    plan.clone().emit_with_options(&options).unwrap_err()
                } else {
                    snapshot.emit_with_options(&options).unwrap_err()
                }
                .to_string();
                for expected in [
                    "conflicting native SAL count contexts",
                    "common.h",
                    "first.cpp",
                    "second.cpp",
                    "selected_target=",
                    "policy=",
                    "Constant(8)",
                    "Constant(12)",
                ] {
                    assert!(error.contains(expected), "{name}: {error}");
                }
            }
        }
    }
    std::fs::remove_file(common).unwrap();
    std::fs::remove_dir(scratch).unwrap();
}

#[test]
fn constant_counts_keep_native_line_macro_context() {
    let source = concat!(
        "typedef char* PSTR;\n",
        "#define SOURCE_LINE (__LINE__ + 2)\n",
        "extern \"C\" void DirectLine(__attribute__((annotate(\"_In_reads_(__LINE__)\"))) PSTR value);\n",
        "extern \"C\" void MacroLine(__attribute__((annotate(\"_In_reads_(SOURCE_LINE)\"))) PSTR value);\n",
        "#line 80 \"presumed-source.h\"\n",
        "extern \"C\" void PresumedLine(__attribute__((annotate(\"_In_reads_(SOURCE_LINE)\"))) PSTR value);\n",
    );
    for partitioned in [false, true] {
        for compatibility in [false, true] {
            let rdl = emit(source, compatibility, partitioned).unwrap();
            let index = compile_rdl(&rdl, &format!("line-counts-{compatibility}-{partitioned}"));
            for (name, expected) in [("DirectLine", 3), ("MacroLine", 6), ("PresumedLine", 82)] {
                let Item::Fn(function) = index.expect_item("Test", name) else {
                    panic!("{name} is not a function");
                };
                let row = function.params_by_sequence(1).unwrap().params()[0].unwrap();
                assert_eq!(
                    row.find_attribute("NativeArrayInfoAttribute")
                        .unwrap()
                        .value(),
                    [(
                        "CountConst".to_string(),
                        windows_metadata::Value::I32(expected)
                    )],
                    "{name}",
                );
            }
        }
    }
}

#[test]
fn constant_counts_keep_multiline_parameter_source() {
    let source = concat!(
        "typedef char* PSTR;\n",
        "extern \"C\" void Multi(\n",
        "__attribute__((annotate(\"_In_reads_(__LINE__)\"))) PSTR first,\n",
        "__attribute__((annotate(\"_In_reads_(__LINE__)\"))) PSTR second);\n",
        "#define SOURCE_LINE (__LINE__ + 2)\n",
        "extern \"C\" void MacroMulti(\n",
        "__attribute__((annotate(\"_In_reads_(SOURCE_LINE)\"))) PSTR first,\n",
        "__attribute__((annotate(\"_In_reads_(SOURCE_LINE)\"))) PSTR second);\n",
        "#line 200 \"native-line.h\"\n",
        "extern \"C\" void TrailingMulti(\n",
        "PSTR first\n",
        "__attribute__((annotate(\"_In_reads_(__LINE__)\"))),\n",
        "PSTR second\n",
        "__attribute__((annotate(\"_In_reads_(__LINE__)\"))));\n",
        "#define READS(value) __attribute__((annotate(\"_In_reads_(\" #value \")\")))\n",
        "#line 300 \"native-line.h\"\n",
        "extern \"C\" void WrappedMulti(\n",
        "READS(SOURCE_LINE) PSTR first,\n",
        "READS(SOURCE_LINE) PSTR second);\n",
        "#line 400 \"native-line.h\"\n",
        "typedef void (*MultilineCallback)(\n",
        "READS(__LINE__) PSTR first,\n",
        "READS(__LINE__) PSTR second);\n",
        "#line 500 \"native-line.h\"\n",
        "struct __declspec(uuid(\"12345678-1234-1234-1234-123456789abc\")) IMultiline {\n",
        "virtual void Count(\n",
        "READS(__LINE__) PSTR first,\n",
        "READS(__LINE__) PSTR second) = 0; };\n",
        "#line 100 \"native-line.h\"\n",
        "extern \"C\" void PresumedMulti(\n",
        "__attribute__((annotate(\"_In_reads_(SOURCE_LINE)\"))) PSTR first,\n",
        "__attribute__((annotate(\"_In_reads_(SOURCE_LINE)\"))) PSTR second);\n",
    );
    for partitioned in [false, true] {
        for compatibility in [false, true] {
            let rdl = emit(source, compatibility, partitioned).unwrap();
            let index = compile_rdl(&rdl, &format!("multiline-{compatibility}-{partitioned}"));
            for (name, expected) in [
                ("Multi", [3, 4]),
                ("MacroMulti", [9, 10]),
                ("PresumedMulti", [103, 104]),
                ("TrailingMulti", [202, 204]),
                ("WrappedMulti", [303, 304]),
                ("MultilineCallback", [401, 402]),
                ("IMultiline", [502, 503]),
            ] {
                let function = if matches!(name, "MultilineCallback" | "IMultiline") {
                    index
                        .expect("Test", name)
                        .methods()
                        .find(|method| {
                            method.name()
                                == if name == "IMultiline" {
                                    "Count"
                                } else {
                                    "Invoke"
                                }
                        })
                        .unwrap()
                } else {
                    let Item::Fn(function) = index.expect_item("Test", name) else {
                        panic!("{name} is not a function");
                    };
                    function
                };
                {
                    let actual = function
                        .params_by_sequence(2)
                        .unwrap()
                        .params()
                        .iter()
                        .map(|row| {
                            row.unwrap()
                                .find_attribute("NativeArrayInfoAttribute")
                                .unwrap()
                                .value()
                        })
                        .collect::<Vec<_>>();
                    let expected = expected
                        .into_iter()
                        .map(|count| {
                            vec![(
                                "CountConst".to_string(),
                                windows_metadata::Value::I32(count),
                            )]
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(actual, expected, "{name}");
                }
            }
        }
    }
}

#[test]
fn constant_count_errors_only_affect_eligible_string_uses() {
    let source = r#"
        #define ODD 3
        #define COUNT 8
        typedef char* PSTR;
        typedef unsigned short* PWSTR;
        typedef const unsigned short* PCWSTR;
        typedef PCWSTR TEXT_HANDLE_W;
        extern "C" void NativeAlias(PCWSTR value);
        extern "C" void Unrelated(
            __attribute__((annotate("_In_reads_(COUNT)"))) void* binary,
            __attribute__((annotate("_In_z_")))
            __attribute__((annotate("_In_reads_(COUNT)"))) TEXT_HANDLE_W nominal,
            __attribute__((annotate("_In_reads_bytes_(ODD)"))) void* bytes,
            __attribute__((annotate("_In_reads_bytes_(ODD)"))) TEXT_HANDLE_W nominal_bytes);
    "#;
    for partitioned in [false, true] {
        for compatibility in [false, true] {
            let rdl = emit(source, compatibility, partitioned).unwrap();
            let index = compile_rdl(&rdl, &format!("ineligible-{compatibility}-{partitioned}"));
            let Item::Fn(function) = index.expect_item("Test", "Unrelated") else {
                panic!("Unrelated is not a function");
            };
            let types = function.signature(&[]).types;
            assert_eq!(types[1], Type::value_named("Test", "TEXT_HANDLE_W"));
            assert_eq!(types[3], Type::value_named("Test", "TEXT_HANDLE_W"));
            for (position, row) in function
                .params_by_sequence(4)
                .unwrap()
                .params()
                .iter()
                .enumerate()
            {
                let row = row.unwrap();
                assert_eq!(
                    row.buffer_relationship(),
                    Some(if position < 2 {
                        windows_metadata::reader::BufferRelationship::ElementsConst(8)
                    } else {
                        windows_metadata::reader::BufferRelationship::BytesConst(3)
                    })
                );
            }
        }
        let source = r#"
            #define ODD 3
            typedef unsigned short* PWSTR;
            typedef const unsigned short* PCWSTR;
            extern "C" void Use(__attribute__((annotate("_In_reads_bytes_(ODD)"))) PCWSTR value);
        "#;
        for compatibility in [false, true] {
            let rdl = emit(source, compatibility, partitioned).unwrap();
            let index = compile_rdl(&rdl, &format!("odd-bytes-{compatibility}-{partitioned}"));
            let Item::Fn(function) = index.expect_item("Test", "Use") else {
                panic!("{rdl}")
            };
            assert_eq!(
                function.params_by_sequence(1).unwrap().params()[0]
                    .unwrap()
                    .buffer_relationship(),
                Some(windows_metadata::reader::BufferRelationship::BytesConst(3))
            );
            assert_eq!(
                function.signature(&[]).types[0],
                Type::value_named("Test", if compatibility { "PWSTR" } else { "PCWSTR" })
            );
            let unknown = source.replace("ODD)", "UNKNOWN_COUNT)");
            if compatibility {
                let error = emit(&unknown, compatibility, partitioned)
                    .unwrap_err()
                    .to_string();
                assert!(
                    error.contains("UNKNOWN_COUNT") && error.contains("value"),
                    "{error}"
                );
            } else {
                let rdl = emit(&unknown, compatibility, partitioned).unwrap();
                let index = compile_rdl(&rdl, &format!("unknown-default-{partitioned}"));
                let Item::Fn(function) = index.expect_item("Test", "Use") else {
                    panic!("{rdl}")
                };
                assert_eq!(
                    function.signature(&[]).types[0],
                    Type::value_named("Test", "PCWSTR")
                );
                assert_eq!(
                    function.params().next().unwrap().buffer_relationship(),
                    None
                );
            }
        }
    }
}

#[test]
fn string_projection_does_not_take_a_definition_from_another_input() {
    helpers::ensure_libclang();
    let mut provider = Input::new("provider.hpp", "typedef char* PSTR;");
    provider.roots.clear();
    let snapshot = extract(
        [
            Input::new(
                "consumer.hpp",
                r#"extern "C" void Use(__attribute__((annotate("_In_z_"))) const char* value);"#,
            ),
            provider,
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("test.dll");
    options.mutable_string_aliases = true;
    let error = snapshot
        .emit_with_options(&options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("PSTR"), "{error}");
    assert!(error.contains("consumer.hpp"), "{error}");
}

#[test]
fn string_projection_rejects_unrepresentable_counts_and_raw_qualifier_ranks() {
    for (source, expected) in [
        (
            r#"
            typedef char* PSTR;
            extern "C" void Use(unsigned count,
                __attribute__((annotate("_In_reads_(count + 1)"))) PSTR value);
        "#,
            "cannot preserve SAL count expression `count + 1`",
        ),
        (
            r#"
            typedef char* PSTR;
            extern "C" void Use(__attribute__((annotate("_In_reads_(UNKNOWN_COUNT)"))) PSTR value);
        "#,
            "SAL count expression `UNKNOWN_COUNT` is not a supported compiler constant",
        ),
        (
            r#"
            extern "C" const char** GetMixed();
        "#,
            "different const qualifiers at each pointer rank",
        ),
        (
            r#"
            struct Mixed { char* const* value; };
        "#,
            "different const qualifiers at each pointer rank",
        ),
    ] {
        for partitioned in [false, true] {
            let error = emit(source, true, partitioned).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("strings.hpp"), "{error}");
            if expected.contains("count expression") {
                assert!(error.contains("`Use` parameter `value`"), "{error}");
            }
        }
    }
}
