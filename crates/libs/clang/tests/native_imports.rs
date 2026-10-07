use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use windows_clang::{
    EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities, NativeImport, NativeImports,
    RootPartition, extract,
};
use windows_metadata::reader::Item;
use windows_rdl::implib::{ImportName, ImportType};

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-native-imports-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn coff_import_contracts_reach_physical_impl_maps() {
    helpers::ensure_libclang();

    let archive = archive([
        short_import(
            "FileIconInit",
            "shell32.dll",
            ImportType::Code,
            0,
            660,
            None,
        ),
        short_import(
            "NamedApi",
            "named.dll",
            ImportType::Code,
            4,
            7,
            Some("ExactExport"),
        ),
        short_import("NamedSame", "named-same.dll", ImportType::Code, 1, 8, None),
        short_import("DataOnly", "data.dll", ImportType::Data, 0, 12, None),
    ]);
    let contracts = windows_rdl::implib::read_contracts(&archive).unwrap();
    assert_eq!(contracts.len(), 4);

    let mut imports = NativeImports::new();
    let mut libraries = BTreeMap::new();
    let mut functions = BTreeSet::new();
    for contract in contracts {
        if contract.import_type != ImportType::Code {
            continue;
        }
        libraries.insert(contract.symbol.clone(), contract.dll.clone());
        let import = match contract.import_name {
            ImportName::Ordinal(ordinal) => NativeImport::ordinal(contract.dll, ordinal),
            ImportName::Name => NativeImport::named(contract.dll, &contract.symbol),
            ImportName::ExportAs(name) => NativeImport::named(contract.dll, name),
            ImportName::NameNoPrefix | ImportName::NameUndecorate => {
                panic!("the fixture requires an exact native entry point")
            }
        };
        functions.insert(contract.symbol.clone());
        imports.insert(contract.symbol, import).unwrap();
    }
    assert!(imports.get("DataOnly").is_none());

    let root = scratch("physical");
    let header = root.join("imports.h");
    std::fs::write(
        &header,
        "extern \"C\" int FileIconInit(int restore);\n\
         extern \"C\" int NamedApi(int value);\n\
         extern \"C\" int NamedSame(int value);\n\
         extern \"C\" int DataOnly(int value);\n",
    )
    .unwrap();
    let snapshot = extract(
        [Input::new(
            root.join("aggregate.cpp").to_string_lossy(),
            format!("#include \"{}\"", header.display()),
        )
        .with_roots([header.to_string_lossy()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("imports", "Example.Imports"),
    );
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.functions = Some(&functions);
    options.libraries = Some(&libraries);
    options.native_imports = Some(&imports);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let rdl = partitions.values().next().unwrap();

    assert!(
        rdl.contains("#[library(\"shell32.dll\", import = \"#660\")]"),
        "{rdl}"
    );
    assert!(
        rdl.contains("#[library(\"named.dll\", import = \"ExactExport\")]"),
        "{rdl}"
    );
    assert!(rdl.contains("#[library(\"named-same.dll\")]"), "{rdl}");
    assert!(!rdl.contains("DataOnly"), "{rdl}");

    let winmd = root.join("imports.winmd");
    windows_rdl::reader()
        .input_text(rdl)
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    for (function, library, import_name) in [
        ("FileIconInit", "shell32.dll", "#660"),
        ("NamedApi", "named.dll", "ExactExport"),
        ("NamedSame", "named-same.dll", "NamedSame"),
    ] {
        let Item::Fn(function) = index.expect_item("Example.Imports", function) else {
            panic!("Example.Imports.{function} was not emitted as a function");
        };
        let impl_map = function.impl_map().unwrap();
        assert_eq!(impl_map.import_scope().name(), library);
        assert_eq!(impl_map.import_name(), import_name);
    }

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_import_conflicts_fail_before_emission() {
    helpers::ensure_libclang();

    let mut imports = NativeImports::new();
    imports
        .insert("Conflict", NativeImport::ordinal("first.dll", 10))
        .unwrap()
        .insert("Conflict", NativeImport::ordinal("first.dll", 10))
        .unwrap();
    let error = imports
        .insert("Conflict", NativeImport::ordinal("first.dll", 11))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "conflicting native import contracts for `Conflict`: first.dll!#10 and first.dll!#11"
    );

    let root = scratch("conflict");
    let header = root.join("conflict.h");
    std::fs::write(&header, "extern \"C\" int Conflict();\n").unwrap();
    let snapshot = extract(
        [Input::new(
            root.join("aggregate.cpp").to_string_lossy(),
            format!("#include \"{}\"", header.display()),
        )
        .with_roots([header.to_string_lossy()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("conflict", "Example.Conflict").with_library("Conflict", "other.dll"),
    );
    let references = BTreeMap::new();
    let functions = BTreeSet::from(["Conflict".to_string()]);
    let mut options = EmitOptions::new("Example.Common", &references);
    options.functions = Some(&functions);
    options.native_imports = Some(&imports);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    let error = plan.audit(&options).unwrap_err();
    assert_eq!(
        error.to_string(),
        "function `Conflict` has conflicting import libraries `other.dll` and `first.dll` for \
         linker symbol `Conflict`"
    );

    std::fs::remove_dir_all(root).unwrap();
}

fn archive<const N: usize>(members: [Vec<u8>; N]) -> Vec<u8> {
    let mut result = b"!<arch>\n".to_vec();
    for (index, member) in members.into_iter().enumerate() {
        let name = format!("member{index}/");
        let header = format!(
            "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            0,
            0,
            0,
            0,
            member.len()
        );
        assert_eq!(header.len(), 60);
        result.extend_from_slice(header.as_bytes());
        result.extend_from_slice(&member);
        if member.len() & 1 != 0 {
            result.push(b'\n');
        }
    }
    result
}

fn short_import(
    symbol: &str,
    dll: &str,
    import_type: ImportType,
    name_type: u16,
    ordinal_or_hint: u16,
    export: Option<&str>,
) -> Vec<u8> {
    let mut strings = vec![];
    for value in [Some(symbol), Some(dll), export].into_iter().flatten() {
        strings.extend_from_slice(value.as_bytes());
        strings.push(0);
    }
    let import_type = match import_type {
        ImportType::Code => 0,
        ImportType::Data => 1,
        ImportType::Const => 2,
    };
    let mut result = vec![0, 0, 0xFF, 0xFF, 0, 0, 0x64, 0x86, 0, 0, 0, 0];
    result.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    result.extend_from_slice(&ordinal_or_hint.to_le_bytes());
    result.extend_from_slice(&(import_type | name_type << 2).to_le_bytes());
    result.extend_from_slice(&strings);
    result
}
