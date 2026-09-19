use std::path::{Path, PathBuf};
use windows_metadata::{self as metadata, HasAttributes};

const VISTA: &str = "windows6.0.6000";
const SERVER2003: &str = "windowsserver2003";

fn fixture(name: &str) -> PathBuf {
    let dir = Path::new(env!("OUT_DIR")).join("supported_os").join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn annotations(tags: &[&str]) -> String {
    tags.iter()
        .map(|tag| format!("__attribute__((annotate(\"win32metadata:supported_os={tag}\"))) "))
        .collect()
}

fn builder(header: &Path) -> windows_clang::Clang {
    let mut clang = windows_clang::clang();
    clang
        .args(["-x", "c++", "-fms-extensions"])
        .target("x86_64-pc-windows-msvc")
        .namespace("Test")
        .library("test.dll")
        .input(header);
    clang
}

fn compile(rdl: &Path, image: &Path) {
    windows_rdl::reader()
        .input(rdl)
        .input_text(windows_rdl::WIN32_METADATA_RDL)
        .reference_default()
        .output(image)
        .write()
        .unwrap();
}

fn verify_attribute_definition(image: &Path) {
    let index = metadata::reader::Index::read(image).unwrap();
    let definition = index.expect(
        "Windows.Win32.Foundation.Metadata",
        "SupportedOSPlatformAttribute",
    );
    let attributes: Vec<_> = definition.attributes().collect();
    assert_eq!(attributes.len(), 1);

    let usage = attributes[0];
    assert_eq!(usage.namespace(), "System");
    assert_eq!(usage.name(), "AttributeUsageAttribute");

    let metadata::reader::AttributeType::MemberRef(ctor) = usage.ctor() else {
        panic!("AttributeUsage must use a typed member reference");
    };
    let metadata::reader::MemberRefParent::TypeRef(provider) = ctor.parent() else {
        panic!("AttributeUsage constructor must be provided by a type reference");
    };
    assert_eq!(provider.namespace(), "System");
    assert_eq!(provider.name(), "AttributeUsageAttribute");
    let metadata::reader::ResolutionScope::AssemblyRef(_) = provider.scope() else {
        panic!("System.AttributeUsageAttribute must resolve through an assembly reference");
    };

    let targets = metadata::TypeName::named("System", "AttributeTargets");
    let signature = ctor.signature(&[]);
    assert_eq!(signature.flags, metadata::MethodCallAttributes::HASTHIS);
    assert_eq!(signature.return_type, metadata::Type::Void);
    assert_eq!(
        signature.types,
        [metadata::Type::ValueName(targets.clone())]
    );
    assert_eq!(
        usage.value(),
        [
            (
                String::new(),
                metadata::Value::EnumValue(targets, Box::new(metadata::Value::I32(4216))),
            ),
            ("AllowMultiple".to_string(), metadata::Value::Bool(true)),
        ]
    );
    assert_eq!(usage.named_arg_kinds(), [0x54]);
}

fn tags<'a>(item: impl HasAttributes<'a>) -> Vec<String> {
    let mut values: Vec<_> = item
        .attributes()
        .filter(|a| a.name() == "SupportedOSPlatformAttribute")
        .map(|a| {
            let args = a.value();
            assert_eq!(args.len(), 1);
            let metadata::Value::Utf8(value) = &args[0].1 else {
                panic!("supported_os must keep its exact string payload");
            };
            value.clone()
        })
        .collect();
    values.sort();
    values
}

fn verify_method(image: &Path, expected: &[&str]) {
    let index = metadata::reader::Index::read(image).unwrap();
    let methods: Vec<_> = index.expect("Test", "Apis").methods().collect();
    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0].name(), "Pick");
    assert_eq!(tags(methods[0]), expected);
    assert_eq!(methods[0].signature(&[]).return_type, metadata::Type::U32);
    assert_eq!(
        methods[0].impl_map().unwrap().import_scope().name(),
        "test.dll"
    );
}

#[test]
fn same_declaration_preserves_distinct_tags_and_deduplicates_identical_tags() {
    let _guard = test_clang::libclang_guard();
    for (case, values) in [
        ("forward", vec![VISTA, SERVER2003, VISTA]),
        ("reverse", vec![SERVER2003, VISTA, SERVER2003]),
        ("duplicate", vec![VISTA, VISTA, VISTA]),
    ] {
        let expected: Vec<_> = if case == "duplicate" {
            vec![VISTA]
        } else {
            vec![VISTA, SERVER2003]
        };
        for mode in ["single", "header", "selected"] {
            let dir = fixture(&format!("{case}_{mode}"));
            let header = dir.join("api.h");
            std::fs::write(
                &header,
                format!("extern \"C\" {}unsigned long Pick();", annotations(&values)),
            )
            .unwrap();
            let rdl = dir.join(if mode == "single" {
                "output.rdl"
            } else {
                "rdl"
            });
            let mut clang = builder(&header);
            clang.output(&rdl);
            if mode == "single" {
                clang.write().unwrap();
            } else {
                if mode == "selected" {
                    clang.symbol("Pick");
                }
                clang.write_by_header().unwrap();
            }
            let text = std::fs::read_to_string(if mode == "single" {
                rdl.clone()
            } else {
                rdl.join("api.rdl")
            })
            .unwrap();
            assert_eq!(
                text.matches("#[supported_os(").count(),
                expected.len(),
                "{text}"
            );
            let image = dir.join("output.winmd");
            compile(&rdl, &image);
            verify_method(&image, &expected);
        }
    }
}

#[test]
fn redeclarations_union_exact_os_tags_in_either_order_across_architectures() {
    let _guard = test_clang::libclang_guard();
    for (case, first, second) in [
        ("forward", VISTA, SERVER2003),
        ("reverse", SERVER2003, VISTA),
    ] {
        let dir = fixture(&format!("redeclarations_{case}"));
        std::fs::write(
            dir.join("foreign.h"),
            format!(
                "extern \"C\" {}unsigned long Pick();",
                annotations(&[first, first])
            ),
        )
        .unwrap();
        let header = dir.join("api.h");
        std::fs::write(
            &header,
            format!(
                "#include \"foreign.h\"\nextern \"C\" {}unsigned long Pick();",
                annotations(&[second, "windows10.0.10240", second])
            ),
        )
        .unwrap();
        let expected = ["windows10.0.10240", VISTA, SERVER2003];
        let mut inputs = vec![];
        for name in ["x64", "x86", "arm64"] {
            let arch = windows_clang::Arch::known(name).unwrap();
            let rdl = dir.join(name);
            let image = dir.join(format!("{name}.winmd"));
            builder(&header)
                .target(&arch.triple)
                .scope("__no_directory_scope__")
                .scope_header("api")
                .symbol("Pick")
                .output(&rdl)
                .write_by_header()
                .unwrap();
            assert!(!rdl.join("foreign.rdl").exists());
            compile(&rdl, &image);
            verify_method(&image, &expected);
            inputs.push(windows_rdl::ArchInput {
                rdl_dir: rdl,
                winmd: image,
                bits: arch.bits,
            });
        }
        let merged = dir.join("merged");
        windows_rdl::merge_arch_rdl(&inputs, None, "Test", &merged).unwrap();
        let image = dir.join("merged.winmd");
        compile(&merged, &image);
        verify_method(&image, &expected);
    }
}

#[test]
fn repeated_os_tags_on_types_remain_exact() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture("types");
    let header = dir.join("api.h");
    let attrs = annotations(&[SERVER2003, VISTA, VISTA]);
    std::fs::write(
        &header,
        format!(
            "struct {attrs} RECORD {{ int field; }};\n\
             union {attrs} UNION {{ int integer; void* pointer; }};\n\
             enum {attrs} KIND {{ KIND_ONE = 1 }};\n\
             typedef {attrs} unsigned long ALIAS;\n\
             typedef {attrs} unsigned long CALLBACK_TYPE();\n\
             struct {attrs} __declspec(uuid(\"12345678-1234-1234-1234-123456789abc\")) IANNOTATED {{ virtual unsigned long Pick() = 0; }};"
        ),
    )
    .unwrap();
    let rdl = dir.join("output.rdl");
    builder(&header).output(&rdl).write().unwrap();
    let image = dir.join("output.winmd");
    compile(&rdl, &image);
    verify_attribute_definition(&image);
    let index = metadata::reader::Index::read(&image).unwrap();
    for name in [
        "RECORD",
        "UNION",
        "KIND",
        "ALIAS",
        "CALLBACK_TYPE",
        "IANNOTATED",
    ] {
        assert_eq!(
            tags(index.expect("Test", name)),
            [VISTA, SERVER2003],
            "{name}"
        );
    }
    assert_eq!(
        index.expect("Test", "IANNOTATED").category(),
        metadata::reader::TypeCategory::Interface
    );
}

#[test]
fn invalid_supported_os_and_conflicting_singletons_remain_errors() {
    let _guard = test_clang::libclang_guard();
    for (case, source, error) in [
        (
            "missing",
            "extern \"C\" __attribute__((annotate(\"win32metadata:supported_os\"))) unsigned long Pick();",
            "requires a value",
        ),
        (
            "empty",
            "extern \"C\" __attribute__((annotate(\"win32metadata:supported_os=\"))) unsigned long Pick();",
            "requires a value",
        ),
        (
            "parameter",
            "extern \"C\" unsigned long Pick(__attribute__((annotate(\"win32metadata:supported_os=windows6.0.6000\"))) int value);",
            "not valid on this declaration",
        ),
        (
            "singleton",
            "extern \"C\" __attribute__((annotate(\"win32metadata:import_library=one.dll\"))) unsigned long Pick();\n\
             extern \"C\" __attribute__((annotate(\"win32metadata:import_library=two.dll\"))) unsigned long Pick();",
            "conflicting redeclaration annotation `import_library`",
        ),
    ] {
        let dir = fixture(case);
        let header = dir.join("api.h");
        std::fs::write(&header, source).unwrap();
        let result = builder(&header)
            .symbol("Pick")
            .output(dir.join("rdl"))
            .write_by_header()
            .unwrap_err();
        assert!(result.message.contains(error), "{result:?}");
    }
}
