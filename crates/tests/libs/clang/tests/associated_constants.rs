use std::path::{Path, PathBuf};
use windows_metadata::{self as metadata, HasAttributes};

const NAMESPACE: &str = "Test.Canonical";

fn fixture(name: &str, associations: &[&str]) -> PathBuf {
    let dir = Path::new(env!("OUT_DIR"))
        .join("associated_constants")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let mut source = "enum ".to_string();
    for name in associations {
        source.push_str(&format!(
            "__attribute__((annotate(\"win32metadata:associated_constant={name}\"))) "
        ));
    }
    source.push_str("ERROR_KIND : unsigned long { ERROR_LOCAL = 1 };\n");
    std::fs::write(dir.join("group.h"), source).unwrap();
    dir
}

fn builder(dir: &Path) -> windows_clang::Clang {
    let mut clang = windows_clang::clang();
    clang
        .args(["-x", "c++", "-fms-extensions"])
        .target("x86_64-pc-windows-msvc")
        .input(dir.join("group.h"))
        .namespace(NAMESPACE)
        .scope("__dependency_only__")
        .scope_header("group")
        .output(dir.join("rdl"));
    clang
}

fn compile(dir: &Path, output: &Path) {
    windows_rdl::reader()
        .input(dir)
        .input_text(windows_rdl::WIN32_METADATA_RDL)
        .reference_default()
        .output(output)
        .write()
        .unwrap();
}

fn verify(image: &Path, namespace: &str) {
    let index = metadata::reader::Index::read(image).unwrap();
    let group = index.expect(namespace, "ERROR_KIND");
    let mut associations: Vec<_> = group
        .attributes()
        .filter(|a| a.name() == "AssociatedConstantAttribute")
        .map(|a| a.value())
        .collect();
    associations.sort_by_key(|value| format!("{value:?}"));
    assert_eq!(associations.len(), 3);
    let apis = index.expect(namespace, "Apis");
    assert_eq!(apis.methods().count(), 0);
    let fields: Vec<_> = apis.fields().collect();
    assert_eq!(fields.len(), 3);
    for (name, ty, value) in [
        (
            "ERROR_TARGET",
            metadata::Type::U32,
            metadata::Value::U32(0xe0000001),
        ),
        (
            "ERROR_ALIAS",
            metadata::Type::U32,
            metadata::Value::U32(0xe0000001),
        ),
        ("ERROR_SMALL", metadata::Type::U8, metadata::Value::U8(7)),
    ] {
        let field = fields.iter().find(|f| f.name() == name).unwrap();
        assert_eq!(field.ty(), ty);
        assert_eq!(field.constant().unwrap().value(), value);
        assert!(associations.contains(&vec![(
            String::new(),
            metadata::Value::Utf8(name.to_string())
        )]));
    }
}

#[test]
fn normal_partition_roots_resolve_associations_and_preserve_aliases_all_arches() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture(
        "all_arches",
        &["ERROR_TARGET", "ERROR_ALIAS", "ERROR_SMALL"],
    );
    let provider = dir.join("provider.h");
    std::fs::write(
        &provider,
        r#"
        #define ERROR_TARGET (0xe0000001UL)
        #define ERROR_ALIAS ERROR_TARGET
        #define ERROR_SMALL ((unsigned char)7)
        #define ERROR_NOISE 19
        extern "C" int UnrelatedApi();
        struct UnrelatedType { int value; };
    "#,
    )
    .unwrap();
    for namespace in ["Windows.Win32", "Windows.Win32.Foundation"] {
        let outputs = dir.join(namespace);
        let mut arches = vec![];
        for name in ["x64", "x86", "arm64"] {
            let arch = windows_clang::Arch::known(name).unwrap();
            let rdl = outputs.join(name);
            let image = outputs.join(format!("{name}.winmd"));
            builder(&dir)
                .input(&provider)
                .input(&provider)
                .namespace(namespace)
                .target(&arch.triple)
                .output(&rdl)
                .write_by_header()
                .unwrap();
            let source = std::fs::read_to_string(rdl.join("provider.rdl")).unwrap();
            assert!(!source.contains("NOISE") && !source.contains("Unrelated"));
            compile(&rdl, &image);
            verify(&image, namespace);
            arches.push(windows_rdl::ArchInput {
                rdl_dir: rdl,
                winmd: image,
                bits: arch.bits,
            });
        }
        let merged = outputs.join("merged");
        windows_rdl::merge_arch_rdl(&arches, None, namespace, &merged).unwrap();
        let image = outputs.join("merged.winmd");
        compile(&merged, &image);
        verify(&image, namespace);
    }
}

#[test]
fn provider_partition_can_precede_its_associated_enum() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture("provider_first", &["ERROR_ALIAS"]);
    let provider = dir.join("provider.h");
    std::fs::write(&provider, "const unsigned short ERROR_ALIAS = 7;\n").unwrap();
    windows_clang::clang()
        .args(["-x", "c++", "-fms-extensions"])
        .target("x86_64-pc-windows-msvc")
        .input(&provider)
        .input(dir.join("group.h"))
        .namespace(NAMESPACE)
        .scope("__dependency_only__")
        .scope_header("group")
        .output(dir.join("rdl"))
        .write_by_header()
        .unwrap();
    let image = dir.join("provider_first.winmd");
    compile(&dir.join("rdl"), &image);
    let index = metadata::reader::Index::read(&image).unwrap();
    let fields: Vec<_> = index.expect(NAMESPACE, "Apis").fields().collect();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name(), "ERROR_ALIAS");
    assert_eq!(fields[0].ty(), metadata::Type::U16);
    assert_eq!(
        fields[0].constant().unwrap().value(),
        metadata::Value::U16(7)
    );
}

#[test]
fn referenced_alias_does_not_emit_unreferenced_target_or_dependency_api() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture("alias_only", &["ERROR_ALIAS"]);
    let provider = dir.join("provider.h");
    std::fs::write(
        &provider,
        r#"
        #define ERROR_TARGET 42UL
        #define ERROR_ALIAS ERROR_TARGET
        extern "C" int UnrelatedApi();
    "#,
    )
    .unwrap();
    builder(&dir).input(provider).write_by_header().unwrap();
    let rdl = dir.join("rdl");
    let image = dir.join("alias.winmd");
    compile(&rdl, &image);
    let index = metadata::reader::Index::read(&image).unwrap();
    let apis = index.expect(NAMESPACE, "Apis");
    let fields: Vec<_> = apis.fields().collect();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name(), "ERROR_ALIAS");
    assert_eq!(
        fields[0].constant().unwrap().value(),
        metadata::Value::U32(42)
    );
    assert_eq!(apis.methods().count(), 0);
}

#[test]
fn enum_typedef_alias_keeps_its_native_source_owner() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture("enum_alias", &["ERROR_ALIAS"]);
    std::fs::write(
        dir.join("group.h"),
        r#"
        typedef enum __attribute__((annotate("win32metadata:associated_constant=ERROR_ALIAS")))
            INTERNAL_TAG : unsigned long { ERROR_LOCAL = 1 } ERROR_KIND;
        "#,
    )
    .unwrap();
    let provider = dir.join("provider.h");
    std::fs::write(&provider, "#define ERROR_ALIAS 2UL\n").unwrap();
    builder(&dir).input(provider).write_by_header().unwrap();
    let image = dir.join("alias.winmd");
    compile(&dir.join("rdl"), &image);
    let index = metadata::reader::Index::read(&image).unwrap();
    assert!(
        index
            .expect(NAMESPACE, "ERROR_KIND")
            .has_attribute("AssociatedConstantAttribute")
    );
    let field = index.expect(NAMESPACE, "Apis").fields().next().unwrap();
    assert_eq!(field.name(), "ERROR_ALIAS");
}

#[test]
fn missing_provider_and_lost_alias_are_errors() {
    let _guard = test_clang::libclang_guard();
    for (case, name) in [("missing", "MISSING"), ("alias", "ERROR_ALIAS")] {
        let dir = fixture(case, &[name]);
        let provider = dir.join("provider.h");
        std::fs::write(&provider, "#define ERROR_TARGET 2\n").unwrap();
        let error = builder(&dir).input(provider).write_by_header().unwrap_err();
        assert!(
            error.message.contains("has no source provider"),
            "{error:?}"
        );
        assert!(error.message.contains(name), "{error:?}");
    }
}

#[test]
fn distinct_provider_owners_are_errors_even_with_identical_values() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture("owners", &["ERROR_TARGET"]);
    for subdir in ["first", "second"] {
        std::fs::create_dir(dir.join(subdir)).unwrap();
        std::fs::write(dir.join(subdir).join("same.h"), "#define ERROR_TARGET 2\n").unwrap();
    }
    let error = builder(&dir)
        .input(dir.join("first").join("same.h"))
        .input(dir.join("second").join("same.h"))
        .write_by_header()
        .unwrap_err();
    assert!(
        error.message.contains("multiple source owners"),
        "{error:?}"
    );
}

#[test]
fn conflicting_native_value_or_type_contexts_are_errors() {
    let _guard = test_clang::libclang_guard();
    for (case, second) in [("value", "2"), ("type", "1U")] {
        let dir = fixture(case, &["ERROR_TARGET"]);
        std::fs::write(dir.join("provider.h"), "#define ERROR_TARGET CONTEXT\n").unwrap();
        for (name, value) in [("first.h", "1"), ("second.h", second)] {
            std::fs::write(
                dir.join(name),
                format!("#define CONTEXT {value}\n#include \"provider.h\"\n"),
            )
            .unwrap();
        }
        let error = builder(&dir)
            .input(dir.join("first.h"))
            .input(dir.join("second.h"))
            .write_by_header()
            .unwrap_err();
        assert!(
            error.message.contains("conflicting native values or types"),
            "{error:?}"
        );
    }
}

#[test]
fn foreign_namespace_reference_does_not_satisfy_association() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture("namespace", &["ERROR_TARGET"]);
    let reference = dir.join("reference.winmd");
    windows_rdl::reader()
        .input_text("mod Wrong { const ERROR_TARGET: u32 = 2; }")
        .output(&reference)
        .write()
        .unwrap();
    let error = builder(&dir)
        .reference(reference)
        .write_by_header()
        .unwrap_err();
    assert!(
        error
            .message
            .contains("conflicts with reference provider `Wrong.ERROR_TARGET`"),
        "{error:?}"
    );
}

#[test]
fn non_integer_provider_and_conflicting_enum_contexts_are_errors() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture("non_integer", &["ERROR_TARGET"]);
    let provider = dir.join("provider.h");
    std::fs::write(&provider, "#define ERROR_TARGET \"text\"\n").unwrap();
    let error = builder(&dir).input(provider).write_by_header().unwrap_err();
    assert!(
        error.message.contains("not a native integer constant"),
        "{error:?}"
    );

    let dir = fixture("enum_context", &["ERROR_TARGET"]);
    let extra = dir.join("other.h");
    std::fs::copy(dir.join("group.h"), &extra).unwrap();
    let error = builder(&dir).input(extra).write_by_header().unwrap_err();
    assert!(
        error
            .message
            .contains("conflicting source contexts for associated enum"),
        "{error:?}"
    );
}
