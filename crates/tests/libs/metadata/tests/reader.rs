use windows_metadata::*;

#[test]
fn type_index() {
    let index = reader::Index::new(vec![
        reader::File::new(windows_default::WINRT.to_vec()).unwrap(),
    ]);

    let def = index.expect("Windows.Foundation", "Point");
    assert_eq!(def.namespace(), "Windows.Foundation");
    assert_eq!(def.name(), "Point");

    let extends = def.extends().unwrap();
    assert_eq!(extends.namespace(), "System");
    assert_eq!(extends.name(), "ValueType");

    let fields: Vec<_> = def.fields().collect();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name(), "X");
    assert_eq!(fields[1].name(), "Y");
    assert_eq!(fields[0].ty(), Type::F32);
    assert_eq!(fields[1].ty(), Type::F32);
}

#[test]
fn item_index() {
    let index = reader::Index::new(vec![
        reader::File::new(windows_default::WINRT.to_vec()).unwrap(),
        reader::File::new(windows_default::WIN32.to_vec()).unwrap(),
    ]);

    let reader::Item::Type(ty) = index.expect_item("Windows.Foundation", "Point") else {
        panic!()
    };
    assert_eq!(ty.namespace(), "Windows.Foundation");
    assert_eq!(ty.name(), "Point");

    let reader::Item::Fn(function) = index.expect_item("Windows.Win32", "ReadFileEx") else {
        panic!()
    };
    assert_eq!(function.name(), "ReadFileEx");

    let reader::Item::Const(constant) = index.expect_item("Windows.Win32", "CONTROL_C_EXIT") else {
        panic!()
    };
    assert_eq!(constant.name(), "CONTROL_C_EXIT");
}

#[test]
fn array() {
    let index = reader::Index::new(vec![
        reader::File::new(windows_default::WIN32.to_vec()).unwrap(),
    ]);
    let def = index
        .types()
        .find(|def| def.name() == "SID_IDENTIFIER_AUTHORITY")
        .unwrap();

    let field = def.fields().find(|field| field.name() == "Value").unwrap();

    assert_eq!(field.ty(), Type::ArrayFixed(Box::new(Type::U8), 6));
}

#[test]
fn nested() {
    let index = reader::Index::new(vec![
        reader::File::new(windows_default::WIN32.to_vec()).unwrap(),
    ]);

    let def = index
        .types()
        .find(|def| def.name() == "D3D10_BUFFER_RTV")
        .unwrap();

    let fields: Vec<reader::Field> = def.fields().collect();
    assert_eq!(fields.len(), 2);

    assert_eq!(fields[0].name(), "Anonymous");
    assert_eq!(fields[1].name(), "Anonymous2");

    assert_eq!(fields[0].ty(), Type::value_named("", "D3D10_BUFFER_RTV_0"));
    assert_eq!(fields[1].ty(), Type::value_named("", "D3D10_BUFFER_RTV_1"));

    let types: Vec<reader::TypeDef> = index.nested(def).collect();
    assert_eq!(types.len(), 2);

    assert_eq!(types[0].namespace(), "");
    assert_eq!(types[1].namespace(), "");

    assert_eq!(types[0].name(), "D3D10_BUFFER_RTV_0");
    assert_eq!(types[1].name(), "D3D10_BUFFER_RTV_1");

    let fields: Vec<reader::Field> = types[0].fields().collect();
    assert_eq!(fields.len(), 2);

    assert_eq!(fields[0].name(), "FirstElement");
    assert_eq!(fields[1].name(), "ElementOffset");

    let fields: Vec<reader::Field> = types[1].fields().collect();
    assert_eq!(fields.len(), 2);

    assert_eq!(fields[0].name(), "NumElements");
    assert_eq!(fields[1].name(), "ElementWidth");
}

#[test]
fn unsupported_table_error() {
    let mut bytes = writer::File::new("test").into_stream();
    let metadata = bytes
        .windows(4)
        .position(|window| window == b"BSJB")
        .unwrap();
    let stream_name = bytes
        .windows(4)
        .position(|window| window == b"#~\0\0")
        .unwrap();
    let stream_offset =
        u32::from_le_bytes(bytes[stream_name - 8..stream_name - 4].try_into().unwrap()) as usize;
    let tables = metadata + stream_offset;
    let valid = u64::from_le_bytes(bytes[tables + 8..tables + 16].try_into().unwrap());

    let table = 3;
    let row_count = tables + 24 + (valid & ((1 << table) - 1)).count_ones() as usize * 4;
    bytes.splice(row_count..row_count, 1u32.to_le_bytes());
    bytes[tables + 8..tables + 16].copy_from_slice(&(valid | 1 << table).to_le_bytes());

    let error = match reader::File::try_new(bytes.clone()) {
        Ok(_) => panic!("unsupported table should fail"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        reader::FileError::UnsupportedTable {
            table: "FieldPtr",
            rows: 1
        }
    ));
    assert_eq!(
        error.to_string(),
        "unsupported metadata table `FieldPtr` has 1 rows"
    );
    assert!(reader::File::new(bytes).is_none());
}

#[test]
fn committed_metadata_uses_supported_tables() {
    fn collect(path: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, files);
            } else if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("winmd"))
            {
                files.push(path);
            }
        }
    }

    let mut files = vec![];
    collect(std::path::Path::new("../../.."), &mut files);
    assert!(!files.is_empty());

    for path in files {
        let file = reader::File::try_read(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let index = reader::Index::new(vec![file]);
        for method_impl in index.types().flat_map(|ty| ty.method_impls()) {
            for method in [method_impl.body(), method_impl.declaration()] {
                match method {
                    reader::MethodDefOrRef::MethodDef(method) => {
                        let _ = method.name();
                    }
                    reader::MethodDefOrRef::MemberRef(method) => {
                        match method.parent() {
                            reader::MemberRefParent::TypeDef(parent) => {
                                let _ = (parent.namespace(), parent.name());
                            }
                            reader::MemberRefParent::TypeRef(parent) => {
                                let _ = (parent.namespace(), parent.name());
                            }
                            reader::MemberRefParent::TypeSpec(parent) => {
                                let _ = parent.ty(&[]);
                            }
                            reader::MemberRefParent::ModuleRef(parent) => {
                                let _ = parent.name();
                            }
                            reader::MemberRefParent::MethodDef(parent) => {
                                let _ = parent.name();
                            }
                        }
                        let _ = method.name();
                    }
                }
            }
        }
    }
}
