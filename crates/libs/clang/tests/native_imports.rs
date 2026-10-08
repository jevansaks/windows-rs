use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use windows_clang::{
    EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities, NativeImport, NativeImports,
    RootPartition, Snapshot, extract, extract_partitioned,
};
use windows_metadata::reader::Item;
use windows_rdl::implib::{ImportContract, ImportEntryPoint, ImportName, ImportType};

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
        short_import("OrdinalApi", "ordinal.dll", ImportType::Code, 0, 660, None),
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
    assert!(contracts.iter().all(|contract| contract.machine == 0x8664));

    let mut imports = NativeImports::new();
    let mut libraries = BTreeMap::new();
    let mut functions = BTreeSet::new();
    for contract in contracts {
        if contract.import_type != ImportType::Code {
            continue;
        }
        libraries.insert(contract.symbol.clone(), contract.dll.clone());
        let import = match contract.resolve_entry_point().unwrap() {
            ImportEntryPoint::Name(name) => NativeImport::named(&contract.dll, name),
            ImportEntryPoint::Ordinal(ordinal) => NativeImport::ordinal(&contract.dll, ordinal),
        };
        functions.insert(contract.symbol.clone());
        imports.insert(contract.symbol, import).unwrap();
    }
    assert!(imports.get("DataOnly").is_none());

    let root = scratch("physical");
    let header = root.join("imports.h");
    std::fs::write(
        &header,
        "extern \"C\" int OrdinalApi(int restore);\n\
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
        rdl.contains("#[library(\"ordinal.dll\", import = \"#660\")]"),
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
        ("OrdinalApi", "ordinal.dll", "#660"),
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
fn library_scoped_imports_emit_distinct_owner_impl_maps() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned(
        [
            Input::new("audio.h", "extern \"C\" int GetDeviceID(int device);\n")
                .partitioned("audio-input")
                .with_root_partition(
                    "audio.h",
                    RootPartition::new("audio", "Example.Audio")
                        .with_library("GetDeviceID", "DSOUND.dll"),
                ),
            Input::new(
                "tbs.h",
                "extern \"C\" long long GetDeviceID(long long device);\n",
            )
            .partitioned("tbs-input")
            .with_root_partition(
                "tbs.h",
                RootPartition::new("tbs", "Example.Tbs").with_library("GetDeviceID", "tbs.dll"),
            ),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let mut imports = NativeImports::new();
    imports
        .insert_for_library("GetDeviceID", NativeImport::ordinal("DSOUND.dll", 17))
        .unwrap()
        .insert_for_library(
            "GetDeviceID",
            NativeImport::named("tbs.dll", "TbsGetDeviceID"),
        )
        .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.native_imports = Some(&imports);
    let partitions = snapshot.emit_partitioned_with_options(&options).unwrap();
    let root = scratch("owner-scoped");

    for (namespace, library, import_name, expected_rdl) in [
        (
            "Example.Audio",
            "DSOUND.dll",
            "#17",
            "#[library(\"DSOUND.dll\", import = \"#17\")]",
        ),
        (
            "Example.Tbs",
            "tbs.dll",
            "TbsGetDeviceID",
            "#[library(\"tbs.dll\", import = \"TbsGetDeviceID\")]",
        ),
    ] {
        let rdl = partitions
            .iter()
            .find(|(partition, _)| partition.namespace == namespace)
            .unwrap()
            .1;
        assert!(rdl.contains(expected_rdl), "{rdl}");

        let winmd = root.join(format!("{namespace}.winmd"));
        windows_rdl::reader()
            .input_text(rdl)
            .output(&winmd)
            .write()
            .unwrap();
        let index = windows_metadata::reader::Index::read(&winmd).unwrap();
        let Item::Fn(function) = index.expect_item(namespace, "GetDeviceID") else {
            panic!("{namespace}.GetDeviceID was not emitted as a function");
        };
        let impl_map = function.impl_map().unwrap();
        assert_eq!(impl_map.import_scope().name(), library);
        assert_eq!(impl_map.import_name(), import_name);
    }

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn all_coff_name_modes_reach_physical_impl_maps() {
    helpers::ensure_libclang();

    let archive = archive([
        short_import_for_machine(
            "_OrdinalApi@4",
            "ordinal.dll",
            ImportType::Code,
            0,
            321,
            None,
            0x014C,
        ),
        short_import_for_machine(
            "_ExactName",
            "name.dll",
            ImportType::Code,
            1,
            155,
            None,
            0x014C,
        ),
        short_import_for_machine(
            "_ExportAsApi",
            "export.dll",
            ImportType::Code,
            4,
            7,
            Some("ExactExport"),
            0x014C,
        ),
        short_import_for_machine(
            "_NoPrefixApi",
            "prefix.dll",
            ImportType::Code,
            2,
            8,
            None,
            0x014C,
        ),
        short_import_for_machine(
            "_UndecorateApi@4",
            "undecorate.dll",
            ImportType::Code,
            3,
            9,
            None,
            0x014C,
        ),
        short_import_for_machine(
            "_DataOnly",
            "data.dll",
            ImportType::Data,
            1,
            10,
            None,
            0x014C,
        ),
        short_import_for_machine(
            "_ConstOnly",
            "const.dll",
            ImportType::Const,
            1,
            11,
            None,
            0x014C,
        ),
    ]);
    let contracts = windows_rdl::implib::read_contracts(&archive).unwrap();

    let root = scratch("all-name-modes");
    let header = root.join("imports.h");
    std::fs::write(
        &header,
        "extern \"C\" int __stdcall OrdinalApi(int value);\n\
         extern \"C\" int __cdecl ExactName(int value);\n\
         extern \"C\" int __cdecl ExportAsApi(int value);\n\
         extern \"C\" int __cdecl NoPrefixApi(int value);\n\
         extern \"C\" int __stdcall UndecorateApi(int value);\n\
         extern \"C\" int __cdecl DataOnly(int value);\n\
         extern \"C\" int __cdecl ConstOnly(int value);\n",
    )
    .unwrap();
    let snapshot = extract(
        [Input::new(
            root.join("aggregate.cpp").to_string_lossy(),
            format!("#include \"{}\"", header.display()),
        )
        .with_roots([header.to_string_lossy()])],
        &["-x", "c++", "--target=i686-pc-windows-msvc"],
    )
    .unwrap();
    let selected = BTreeSet::from([
        "OrdinalApi".to_string(),
        "ExactName".to_string(),
        "ExportAsApi".to_string(),
        "NoPrefixApi".to_string(),
        "UndecorateApi".to_string(),
    ]);

    let wrong_machine = native_imports_for_machine(
        &snapshot,
        &contracts,
        0x8664,
        &BTreeSet::from(["OrdinalApi".to_string()]),
    )
    .unwrap_err();
    assert_eq!(
        wrong_machine,
        "selected function `OrdinalApi` has no matching CODE import contract for machine 0x8664"
    );
    for rejected in ["DataOnly", "ConstOnly"] {
        let error = native_imports_for_machine(
            &snapshot,
            &contracts,
            0x014C,
            &BTreeSet::from([rejected.to_string()]),
        )
        .unwrap_err();
        assert_eq!(
            error,
            format!(
                "selected function `{rejected}` has no matching CODE import contract for machine \
                 0x014c"
            )
        );
    }

    let imports = native_imports_for_machine(&snapshot, &contracts, 0x014C, &selected).unwrap();
    assert_eq!(imports.len(), selected.len());
    assert!(
        imports
            .get_for_library("OrdinalApi", "ORDINAL.DLL")
            .is_some()
    );
    assert!(imports.library_imports("_OrdinalApi@4").next().is_none());
    assert!(imports.library_imports("DataOnly").next().is_none());
    assert!(imports.library_imports("ConstOnly").next().is_none());

    let libraries = BTreeMap::from([
        ("OrdinalApi".to_string(), "ordinal.dll".to_string()),
        ("ExactName".to_string(), "name.dll".to_string()),
        ("ExportAsApi".to_string(), "export.dll".to_string()),
        ("NoPrefixApi".to_string(), "prefix.dll".to_string()),
        ("UndecorateApi".to_string(), "undecorate.dll".to_string()),
    ]);
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.X86Imports", &references);
    options.functions = Some(&selected);
    options.libraries = Some(&libraries);
    options.native_imports = Some(&imports);
    let rdl = snapshot.emit_with_options(&options).unwrap();
    assert!(!rdl.contains("DataOnly"), "{rdl}");
    assert!(!rdl.contains("ConstOnly"), "{rdl}");

    let winmd = root.join("imports.winmd");
    windows_rdl::reader()
        .input_text(&rdl)
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    for (function, library, import_name) in [
        ("OrdinalApi", "ordinal.dll", "#321"),
        ("ExactName", "name.dll", "_ExactName"),
        ("ExportAsApi", "export.dll", "ExactExport"),
        ("NoPrefixApi", "prefix.dll", "NoPrefixApi"),
        ("UndecorateApi", "undecorate.dll", "UndecorateApi"),
    ] {
        let Item::Fn(function) = index.expect_item("Example.X86Imports", function) else {
            panic!("Example.X86Imports.{function} was not emitted as a function");
        };
        let impl_map = function.impl_map().unwrap();
        assert_eq!(impl_map.import_scope().name(), library);
        assert_eq!(impl_map.import_name(), import_name);
    }

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn archive_only_machine_evidence_is_selected_by_exact_target() {
    helpers::ensure_libclang();

    // FileIconInit is archive-only evidence from SDK 10.0.28000.2270. The source declarations
    // below are synthetic mangling controls, not evidence of checked-in SDK header admission.
    let contracts = windows_rdl::implib::read_contracts(&archive([
        short_import_for_machine(
            "FileIconInit",
            "SHELL32.dll",
            ImportType::Code,
            0,
            660,
            None,
            0x8664,
        ),
        short_import_for_machine(
            "_FileIconInit@4",
            "SHELL32.dll",
            ImportType::Code,
            0,
            660,
            None,
            0x014C,
        ),
        short_import_for_machine(
            "FileIconInit",
            "SHELL32.dll",
            ImportType::Code,
            0,
            660,
            None,
            0xAA64,
        ),
        short_import_for_machine(
            "HybridApi",
            "SHCORE.dll",
            ImportType::Code,
            0,
            700,
            None,
            0xAA64,
        ),
        short_import_for_machine(
            "HybridApi",
            "SHCORE.dll",
            ImportType::Code,
            0,
            701,
            None,
            0xA641,
        ),
        short_import_for_machine(
            "Arm64EcOnlyApi",
            "SHCORE.dll",
            ImportType::Code,
            0,
            702,
            None,
            0xA641,
        ),
    ]))
    .unwrap();

    for (target, source, raw_name, machine) in [
        (
            "--target=x86_64-pc-windows-msvc",
            "extern \"C\" int FileIconInit(int restore);\n",
            "FileIconInit",
            0x8664,
        ),
        (
            "--target=i686-pc-windows-msvc",
            "extern \"C\" int __stdcall FileIconInit(int restore);\n",
            "_FileIconInit@4",
            0x014C,
        ),
        (
            "--target=aarch64-pc-windows-msvc",
            "extern \"C\" int FileIconInit(int restore);\n",
            "FileIconInit",
            0xAA64,
        ),
    ] {
        let snapshot = extract(
            [Input::new(format!("{machine:04x}.h"), source)],
            &["-x", "c++", target],
        )
        .unwrap();
        assert_eq!(
            snapshot.resolve_function_link_name(raw_name).unwrap(),
            Some("FileIconInit")
        );
        let imports = native_imports_for_machine(
            &snapshot,
            &contracts,
            machine,
            &BTreeSet::from(["FileIconInit".to_string()]),
        )
        .unwrap();
        assert_eq!(imports.len(), 1);
        let import = imports
            .get_for_library("FileIconInit", "shell32.DLL")
            .unwrap();
        assert_eq!(import.library(), "SHELL32.dll");
        assert_eq!(
            import.name(),
            &windows_clang::NativeImportName::Ordinal(660)
        );
    }

    let snapshot = extract(
        [Input::new(
            "hybrid.h",
            "extern \"C\" int HybridApi();\n\
             extern \"C\" int Arm64EcOnlyApi();\n",
        )],
        &["-x", "c++", "--target=aarch64-pc-windows-msvc"],
    )
    .unwrap();
    let imports = native_imports_for_machine(
        &snapshot,
        &contracts,
        0xAA64,
        &BTreeSet::from(["HybridApi".to_string()]),
    )
    .unwrap();
    assert_eq!(
        imports
            .get_for_library("HybridApi", "shcore.dll")
            .unwrap()
            .name(),
        &windows_clang::NativeImportName::Ordinal(700)
    );
    let error = native_imports_for_machine(
        &snapshot,
        &contracts,
        0xAA64,
        &BTreeSet::from(["Arm64EcOnlyApi".to_string()]),
    )
    .unwrap_err();
    assert_eq!(
        error,
        "selected function `Arm64EcOnlyApi` has no matching CODE import contract for machine \
         0xaa64"
    );
}

#[test]
fn sdk_shell_ordinal_contracts_reach_physical_impl_maps() {
    helpers::ensure_libclang();

    const FUNCTIONS: [(&str, u16); 5] = [
        ("SHCreatePropSheetExtArray", 168),
        ("DAD_DragEnterEx", 131),
        ("DAD_DragEnterEx2", 22),
        ("SHDefExtractIconA", 3),
        ("SHDefExtractIconW", 6),
    ];
    const TARGETS: [(&str, &str, u16, [&str; 5]); 3] = [
        (
            "x64",
            "--target=x86_64-pc-windows-msvc",
            0x8664,
            [
                "SHCreatePropSheetExtArray",
                "DAD_DragEnterEx",
                "DAD_DragEnterEx2",
                "SHDefExtractIconA",
                "SHDefExtractIconW",
            ],
        ),
        (
            "x86",
            "--target=i686-pc-windows-msvc",
            0x014C,
            [
                "_SHCreatePropSheetExtArray@12",
                "_DAD_DragEnterEx@12",
                "_DAD_DragEnterEx2@16",
                "_SHDefExtractIconA@24",
                "_SHDefExtractIconW@24",
            ],
        ),
        (
            "arm64",
            "--target=aarch64-pc-windows-msvc",
            0xAA64,
            [
                "SHCreatePropSheetExtArray",
                "DAD_DragEnterEx",
                "DAD_DragEnterEx2",
                "SHDefExtractIconA",
                "SHDefExtractIconW",
            ],
        ),
    ];

    let mut members = vec![];
    for (_, _, machine, raw_names) in TARGETS {
        for ((_, ordinal), raw_name) in FUNCTIONS.iter().zip(raw_names) {
            members.push(short_import_for_machine(
                raw_name,
                "SHELL32.dll",
                ImportType::Code,
                0,
                *ordinal,
                None,
                machine,
            ));
        }
    }
    let contracts = windows_rdl::implib::read_contracts(&archive(members)).unwrap();

    let root = scratch("sdk-shell-ordinals");
    let header = root.join("shell_ordinals.h");
    let source = "typedef struct POINT { long x; long y; } POINT;\n\
                  extern \"C\" void* __stdcall SHCreatePropSheetExtArray(\n\
                      void* hkey, const unsigned short* subkey, unsigned max_iface);\n\
                  extern \"C\" int __stdcall DAD_DragEnterEx(void* target, POINT start);\n\
                  extern \"C\" int __stdcall DAD_DragEnterEx2(\n\
                      void* target, POINT start, void* data_object);\n\
                  extern \"C\" long __stdcall SHDefExtractIconA(\n\
                      const char* path, int index, unsigned flags, void** large,\n\
                      void** small, unsigned size);\n\
                  extern \"C\" long __stdcall SHDefExtractIconW(\n\
                      const unsigned short* path, int index, unsigned flags, void** large,\n\
                      void** small, unsigned size);\n";
    std::fs::write(&header, source).unwrap();
    let normalized_header = header.to_string_lossy().replace('\\', "/");
    let selected = FUNCTIONS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect::<BTreeSet<_>>();

    for (label, target, machine, raw_names) in TARGETS {
        let snapshot = extract(
            [Input::new(
                root.join(format!("{label}.cpp")).to_string_lossy(),
                format!("#include \"{}\"", header.display()),
            )
            .with_roots([header.to_string_lossy()])],
            &["-x", "c++", target],
        )
        .unwrap();
        let identities = snapshot
            .function_source_identities()
            .map(|identity| (identity.name, identity))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(identities.len(), FUNCTIONS.len());
        for ((name, _), raw_name) in FUNCTIONS.iter().zip(raw_names) {
            let identity = identities.get(*name).unwrap();
            assert_eq!(identity.link_name, *name);
            assert_eq!(identity.raw_link_name, raw_name);
            assert_eq!(identity.spelling.file, normalized_header);
            assert_eq!(identity.spelling.offset, source.find(*name).unwrap() as u32);
        }

        let imports =
            native_imports_for_machine(&snapshot, &contracts, machine, &selected).unwrap();
        assert_eq!(imports.len(), FUNCTIONS.len());
        let references = BTreeMap::new();
        let mut options = EmitOptions::new("Example.Shell", &references);
        options.functions = Some(&selected);
        options.library = Some("shell32.dll");
        options.native_imports = Some(&imports);
        let rdl = snapshot.emit_with_options(&options).unwrap();
        for (_, ordinal) in FUNCTIONS {
            assert!(
                rdl.contains(&format!(
                    "#[library(\"SHELL32.dll\", import = \"#{ordinal}\")]"
                )),
                "{rdl}"
            );
        }

        let winmd = root.join(format!("{label}.winmd"));
        windows_rdl::reader()
            .input_text(&rdl)
            .output(&winmd)
            .write()
            .unwrap();
        let index = windows_metadata::reader::Index::read(&winmd).unwrap();
        for (function, ordinal) in FUNCTIONS {
            let Item::Fn(function) = index.expect_item("Example.Shell", function) else {
                panic!("Example.Shell.{function} was not emitted as a function");
            };
            let impl_map = function.impl_map().unwrap();
            assert_eq!(impl_map.import_scope().name(), "SHELL32.dll");
            assert_eq!(impl_map.import_name(), format!("#{ordinal}"));
        }
    }

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn router_name_undecorate_hint_is_a_named_physical_impl_map() {
    helpers::ensure_libclang();

    const RAW: &str = "?RouterUnregisterForPrintAsyncNotifications@@YAJPEAX@Z";
    const NAME: &str = "RouterUnregisterForPrintAsyncNotifications";
    let archive = archive([short_import_for_machine(
        RAW,
        "SPOOLSS.DLL",
        ImportType::Code,
        3,
        155,
        None,
        0x8664,
    )]);
    let contracts = windows_rdl::implib::read_contracts(&archive).unwrap();
    assert_eq!(contracts[0].import_name, ImportName::NameUndecorate);
    assert_eq!(
        contracts[0].resolve_entry_point().unwrap(),
        ImportEntryPoint::Name(NAME.to_string())
    );

    let root = scratch("router-name-undecorate");
    let header = root.join("router.hpp");
    std::fs::write(&header, format!("long {NAME}(void* registration);\n")).unwrap();
    let snapshot = extract(
        [Input::new(
            root.join("aggregate.cpp").to_string_lossy(),
            format!("#include \"{}\"", header.display()),
        )
        .with_roots([header.to_string_lossy()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    assert_eq!(snapshot.resolve_function_link_name(RAW).unwrap(), Some(RAW));

    let selected = BTreeSet::from([RAW.to_string()]);
    let imports = native_imports_for_machine(&snapshot, &contracts, 0x8664, &selected).unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Router", &references);
    options.functions = Some(&selected);
    options.library = Some("spoolss.dll");
    options.native_imports = Some(&imports);
    let rdl = snapshot.emit_with_options(&options).unwrap();
    assert!(rdl.contains("#[library(\"SPOOLSS.DLL\")]"), "{rdl}");
    assert!(rdl.contains("fn RouterUnregisterForPrintAsyncNotifications("));
    assert!(!rdl.contains("#155"), "{rdl}");

    let winmd = root.join("router.winmd");
    windows_rdl::reader()
        .input_text(&rdl)
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    let Item::Fn(function) = index.expect_item("Example.Router", NAME) else {
        panic!("Example.Router.{NAME} was not emitted as a function");
    };
    let impl_map = function.impl_map().unwrap();
    assert_eq!(impl_map.import_scope().name(), "SPOOLSS.DLL");
    assert_eq!(impl_map.import_name(), NAME);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn library_scoped_native_import_conflicts_are_dll_local() {
    let mut imports = NativeImports::new();
    imports
        .insert_for_library("GetDeviceID", NativeImport::ordinal("DSOUND.dll", 17))
        .unwrap()
        .insert_for_library("GetDeviceID", NativeImport::ordinal("dsound.DLL", 17))
        .unwrap()
        .insert_for_library(
            "GetDeviceID",
            NativeImport::named("tbs.dll", "TbsGetDeviceID"),
        )
        .unwrap();

    assert_eq!(imports.len(), 2);
    assert_eq!(
        imports
            .get_for_library("GetDeviceID", "DsOuNd.DlL")
            .unwrap()
            .library(),
        "DSOUND.dll"
    );
    assert_eq!(
        imports
            .get_for_library("GetDeviceID", "TBS.DLL")
            .unwrap()
            .name(),
        &windows_clang::NativeImportName::Name("TbsGetDeviceID".to_string())
    );

    let error = imports
        .insert_for_library("GetDeviceID", NativeImport::ordinal("dsound.dll", 18))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "conflicting native import contracts for `GetDeviceID` in library `DSOUND.dll`: \
         DSOUND.dll!#17 and dsound.dll!#18"
    );

    let mut mixed = NativeImports::new();
    mixed
        .insert("Mixed", NativeImport::ordinal("first.dll", 1))
        .unwrap();
    let error = mixed
        .insert_for_library("Mixed", NativeImport::named("FIRST.DLL", "OtherEntry"))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "conflicting native import contracts for `Mixed` in library `first.dll`: first.dll!#1 and \
         FIRST.DLL!OtherEntry"
    );
}

#[test]
fn library_scoped_native_imports_require_a_matching_configured_library() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [Input::new("scoped.h", "extern \"C\" int ScopedImport();\n")],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let mut imports = NativeImports::new();
    imports
        .insert("ScopedImport", NativeImport::ordinal("fallback.dll", 99))
        .unwrap()
        .insert_for_library("ScopedImport", NativeImport::ordinal("First.dll", 10))
        .unwrap()
        .insert_for_library(
            "ScopedImport",
            NativeImport::named("second.dll", "SecondEntry"),
        )
        .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Scoped", &references);
    options.native_imports = Some(&imports);

    let error = snapshot.emit_with_options(&options).unwrap_err();
    assert_eq!(
        error.to_string(),
        "function `ScopedImport` has DLL-scoped native import contracts for linker symbol \
         `ScopedImport` but no import library is configured: `First.dll!#10`, \
         `second.dll!SecondEntry`"
    );

    options.library = Some("fallback.dll");
    let error = snapshot.emit_with_options(&options).unwrap_err();
    assert_eq!(
        error.to_string(),
        "function `ScopedImport` selects import library `fallback.dll` for linker symbol \
         `ScopedImport`, but DLL-scoped native import contracts are available only for \
         `First.dll!#10`, `second.dll!SecondEntry`"
    );

    options.library = Some("FIRST.DLL");
    let rdl = snapshot.emit_with_options(&options).unwrap();
    assert!(
        rdl.contains("#[library(\"First.dll\", import = \"#10\")]"),
        "{rdl}"
    );
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

fn native_imports_for_machine(
    snapshot: &Snapshot,
    contracts: &[ImportContract],
    machine: u16,
    selected: &BTreeSet<String>,
) -> Result<NativeImports, String> {
    let mut imports = NativeImports::new();
    let mut resolved = BTreeSet::new();
    for contract in contracts {
        if contract.import_type != ImportType::Code || contract.machine != machine {
            continue;
        }
        let Some(link_name) = snapshot
            .resolve_function_link_name(&contract.symbol)
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        if !selected.contains(link_name) {
            continue;
        }
        let import = match contract
            .resolve_entry_point()
            .map_err(|error| error.to_string())?
        {
            ImportEntryPoint::Name(name) => NativeImport::named(&contract.dll, name),
            ImportEntryPoint::Ordinal(ordinal) => NativeImport::ordinal(&contract.dll, ordinal),
        };
        imports
            .insert_for_library(link_name, import)
            .map_err(|error| error.to_string())?;
        resolved.insert(link_name.to_string());
    }
    if let Some(unresolved) = selected.difference(&resolved).next() {
        return Err(format!(
            "selected function `{unresolved}` has no matching CODE import contract for machine \
             0x{machine:04x}"
        ));
    }
    Ok(imports)
}

fn archive(members: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
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
    short_import_for_machine(
        symbol,
        dll,
        import_type,
        name_type,
        ordinal_or_hint,
        export,
        0x8664,
    )
}

fn short_import_for_machine(
    symbol: &str,
    dll: &str,
    import_type: ImportType,
    name_type: u16,
    ordinal_or_hint: u16,
    export: Option<&str>,
    machine: u16,
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
    let mut result = vec![0, 0, 0xFF, 0xFF, 0, 0];
    result.extend_from_slice(&machine.to_le_bytes());
    result.extend_from_slice(&[0, 0, 0, 0]);
    result.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    result.extend_from_slice(&ordinal_or_hint.to_le_bytes());
    result.extend_from_slice(&(import_type | name_type << 2).to_le_bytes());
    result.extend_from_slice(&strings);
    result
}
