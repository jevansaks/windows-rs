use windows_metadata::*;

fn attributes(method: reader::MethodDef) -> Vec<(String, String, Vec<(String, Value)>)> {
    method
        .attributes()
        .filter(|a| a.name() != "SupportedArchitectureAttribute")
        .map(|a| (a.namespace().to_string(), a.name().to_string(), a.value()))
        .collect()
}

fn arch(method: reader::MethodDef) -> Option<i32> {
    let attributes: Vec<_> = method
        .attributes()
        .filter(|a| a.name() == "SupportedArchitectureAttribute")
        .collect();
    assert!(attributes.len() <= 1, "duplicate architecture attributes");
    attributes.first().map(|a| match a.value().as_slice() {
        [(_, Value::I32(bits))] => *bits,
        value => panic!("unexpected architecture {value:?}"),
    })
}

#[test]
fn arch_merge_preserves_method_and_parameter_contracts() {
    let dir = std::env::temp_dir().join("win_method_contract_merge");
    std::fs::create_dir_all(&dir).unwrap();
    let mut inputs = vec![];
    for (name, bit) in [("X64", 2), ("X86", 1), ("Arm64", 4)] {
        let legacy = bit == 2;
        let os = if legacy {
            "windows5.2.3790"
        } else {
            "windows6.0.6000"
        };
        let optional = if legacy { "#[opt]" } else { "" };
        let direction = if legacy { "#[in]" } else { "#[out]" };
        let enum_name = if legacy { "First" } else { "Second" };
        let import = if legacy { "EntryA" } else { "EntryB" };
        let library = if legacy { "one.dll" } else { "two.dll" };
        let last_error = if legacy { ", set_last_error" } else { "" };
        let preserve_sig = if legacy { "#[preserve_sig]" } else { "" };
        let source = format!(
            r#"#[win32] mod Test {{
                #[library("test.dll")] #[arch({name})] #[supported_os("{os}")]
                extern "system" fn Version(value: u32) -> u32;
                #[library("test.dll")] #[arch({name})]
                extern "system" fn Parameter({optional} {direction} value: *mut u32) -> u32;
                #[library("test.dll")] #[arch({name})]
                extern "system" fn Association(#[associated_enum("{enum_name}")] value: u32)
                    -> #[associated_enum("{enum_name}")] u32;
                #[library("{library}", import = "{import}" {last_error})] #[arch({name})]
                extern "system" fn Import(value: u32) -> u32;
                #[library("test.dll")] #[arch({name})] {preserve_sig}
                extern "system" fn Implementation(value: u32) -> HRESULT;
                #[library("test.dll")] #[arch({name})] #[supported_os("windows5.1.2600")]
                extern "system" fn Identical(#[opt] value: *mut u32) -> u32;
            }}"#
        );
        let path = dir.join(format!("{name}.winmd"));
        windows_rdl::reader()
            .input_text(windows_rdl::WIN32_METADATA_RDL)
            .input_text(&source)
            .output(&path)
            .write()
            .unwrap();
        inputs.push((path, bit));
    }

    let output = dir.join("merged.winmd");
    let mut merger = merge();
    for (path, bit) in &inputs {
        merger.arch_input(path, *bit);
    }
    merger.output(&output).merge().unwrap();
    assert_contracts(
        &inputs,
        &output,
        &[
            "Version",
            "Parameter",
            "Association",
            "Import",
            "Implementation",
            "Identical",
        ],
    );

    let rdl = dir.join("rdl");
    windows_rdl::writer()
        .input(&output)
        .output(&rdl)
        .split()
        .write()
        .unwrap();
    let roundtrip = dir.join("roundtrip.winmd");
    windows_rdl::reader()
        .input(&rdl)
        .output(&roundtrip)
        .write()
        .unwrap();
    assert_contracts(
        &inputs,
        &roundtrip,
        &[
            "Version",
            "Parameter",
            "Association",
            "Import",
            "Implementation",
            "Identical",
        ],
    );
}

fn assert_contracts(
    inputs: &[(std::path::PathBuf, i32)],
    output: &std::path::Path,
    names: &[&str],
) {
    let merged = reader::Index::read(output).unwrap();
    let apis = merged.expect("Test", "Apis");
    for name in names.iter().filter(|name| **name != "Identical") {
        let mut masks: Vec<_> = apis
            .methods()
            .filter(|m| m.name() == *name)
            .map(arch)
            .collect();
        masks.sort();
        assert_eq!(masks, [Some(2), Some(5)], "{name}");
    }
    let identical: Vec<_> = apis.methods().filter(|m| m.name() == "Identical").collect();
    assert_eq!(identical.len(), 1);
    assert_eq!(arch(identical[0]), None);

    for (input, bit) in inputs {
        let source = reader::Index::read(input).unwrap();
        for expected in source
            .expect("Test", "Apis")
            .methods()
            .filter(|method| names.contains(&method.name()))
        {
            let matches: Vec<_> = apis
                .methods()
                .filter(|m| m.name() == expected.name() && arch(*m).is_none_or(|a| a & bit != 0))
                .collect();
            assert_eq!(matches.len(), 1, "{} on {bit}", expected.name());
            let actual = matches[0];
            let actual_signature = actual.signature(&[]);
            let expected_signature = expected.signature(&[]);
            assert_eq!(actual_signature.flags, expected_signature.flags);
            assert_eq!(actual_signature.return_type, expected_signature.return_type);
            assert_eq!(actual_signature.types, expected_signature.types);
            assert_eq!(actual.flags(), expected.flags());
            assert_eq!(actual.impl_flags(), expected.impl_flags());
            assert_eq!(attributes(actual), attributes(expected));
            let params = |method: reader::MethodDef| {
                method
                    .params()
                    .map(|p| {
                        (
                            p.sequence(),
                            p.name().to_string(),
                            p.flags(),
                            p.attributes()
                                .map(|a| (a.name().to_string(), a.value()))
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(params(actual), params(expected));
            let import = |method: reader::MethodDef| {
                method.impl_map().map(|m| {
                    (
                        m.flags(),
                        m.import_name().to_string(),
                        m.import_scope().name().to_string(),
                    )
                })
            };
            assert_eq!(import(actual), import(expected));
        }
    }
}
