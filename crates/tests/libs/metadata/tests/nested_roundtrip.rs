use windows_metadata::*;

fn assert_nested_field(parent: reader::TypeDef, field_name: &str, namespace: &str, names: &[&str]) {
    let field = parent.fields().find(|f| f.name() == field_name).unwrap();
    let mut signature = field.blob(2);
    assert_eq!(signature.read_u8(), 0x06);
    assert_eq!(signature.read_u8(), 0x11);
    let reader::TypeDefOrRef::TypeRef(mut reference) = signature.decode() else {
        panic!("expected a nested TypeRef in {field_name}");
    };
    for name in names[1..].iter().rev() {
        assert_eq!(reference.name(), *name);
        assert_eq!(reference.namespace(), "");
        let reader::ResolutionScope::TypeRef(enclosing) = reference.scope() else {
            panic!("nested TypeRef {name} must resolve through its enclosing TypeRef");
        };
        reference = enclosing;
    }
    assert_eq!(reference.name(), names[0]);
    assert_eq!(reference.namespace(), namespace);
    assert!(matches!(
        reference.scope(),
        reader::ResolutionScope::Module(_)
    ));
}

/// Compiles inline RDL into `dir/name.winmd` and returns its path.
fn winmd(dir: &std::path::Path, name: &str, rdl: &str) -> String {
    let rdl_path = dir.join(format!("{name}.rdl"));
    std::fs::write(&rdl_path, rdl).unwrap();
    let out = dir.join(format!("{name}.winmd"));
    windows_rdl::reader()
        .input(&rdl_path)
        .output(&out)
        .write()
        .unwrap();
    out.to_string_lossy().into_owned()
}

/// `Outer` must decompose into a top-level type with one anonymous nested struct,
/// which in turn contains one anonymous nested union, expressed as real
/// `NestedClass` rows (nested types living in the empty namespace, `NestedPublic`).
fn assert_nested(index: &reader::Index, namespace: &str) {
    let outer = index
        .types()
        .find(|t| t.name() == "Outer")
        .expect("Outer type present");
    assert!(
        !outer.flags().is_nested(),
        "top-level Outer must not be nested"
    );

    let children: Vec<_> = index.nested(outer).collect();
    assert_eq!(
        children.len(),
        1,
        "Outer should have exactly one nested type"
    );
    let child = children[0];
    assert!(
        child.flags().is_nested(),
        "nested struct must be NestedPublic"
    );
    assert!(
        child.namespace().is_empty(),
        "nested type must live in the empty namespace"
    );
    assert!(
        !child.flags().contains(TypeAttributes::ExplicitLayout),
        "the inner anonymous aggregate should be a struct (sequential layout)"
    );

    let grandchildren: Vec<_> = index.nested(child).collect();
    assert_eq!(
        grandchildren.len(),
        1,
        "the nested struct should itself contain one nested union"
    );
    assert!(
        grandchildren[0]
            .flags()
            .contains(TypeAttributes::ExplicitLayout),
        "the deepest anonymous aggregate should be a union (explicit layout)"
    );
    assert_nested_field(outer, "Anonymous", namespace, &["Outer", "Outer_1"]);
    assert_nested_field(
        child,
        "Anonymous",
        namespace,
        &["Outer", "Outer_1", "Outer_1_1"],
    );
    assert_eq!(
        grandchildren[0].type_name(),
        TypeName::named(namespace, "Outer/Outer_1/Outer_1_1")
    );
    assert_eq!(
        child
            .fields()
            .find(|field| field.name() == "Anonymous")
            .unwrap()
            .ty(),
        Type::value_named(namespace, "Outer/Outer_1/Outer_1_1")
    );
}

/// Inline anonymous nested struct/union syntax must survive the whole pipeline:
/// the reader emits real `NestedClass` rows, `merge` preserves them, and the
/// writer decompiles them back to inline syntax that the reader re-encodes
/// identically.
#[test]
fn nested_types_survive_rdl_merge_and_writer() {
    let dir = std::env::temp_dir().join("win_nested_roundtrip");
    std::fs::create_dir_all(&dir).unwrap();

    let src = "#[win32] mod Test { \
        struct Outer { \
            header: u32, \
            Anonymous: struct { x: i32, Anonymous: union { a: i32, b: f32 } }, \
            tail: u16, \
        } \
    }";
    let winmd_path = winmd(&dir, "nested", src);

    // 1. The reader produced real NestedClass rows.
    let index = reader::Index::read(&winmd_path).unwrap();
    assert_nested(&index, "Test");

    // 2. merge() preserves the nested structure.
    let merged = dir.join("merged.winmd");
    merge().input(&winmd_path).output(&merged).merge().unwrap();
    let merged_index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    assert_nested(&merged_index, "Test");

    let merged_arches = dir.join("merged_arches.winmd");
    merge()
        .arch_input(&winmd_path, 1)
        .arch_input(&winmd_path, 2)
        .arch_input(&winmd_path, 4)
        .output(&merged_arches)
        .merge()
        .unwrap();
    assert_nested(&reader::Index::read(&merged_arches).unwrap(), "Test");

    let remapped = dir.join("remapped.winmd");
    remap()
        .input(&winmd_path)
        .source("Test")
        .route("Outer", "Routed")
        .fallback("Fallback")
        .output(&remapped)
        .remap()
        .unwrap();
    assert_nested(&reader::Index::read(&remapped).unwrap(), "Routed");

    // 3. writer -> RDL emits inline nested syntax (not hoisted flat siblings).
    let rdl_dir = dir.join("rdl");
    std::fs::create_dir_all(&rdl_dir).unwrap();
    windows_rdl::writer()
        .input(&winmd_path)
        .output(&rdl_dir)
        .split()
        .write()
        .unwrap();
    let rdl_text = std::fs::read_to_string(rdl_dir.join("Test.rdl")).unwrap();
    assert!(
        rdl_text.contains("Anonymous: struct {"),
        "inline nested struct syntax missing:\n{rdl_text}"
    );
    assert!(
        rdl_text.contains("Anonymous: union {"),
        "inline nested union syntax missing:\n{rdl_text}"
    );

    // 4. Reading that RDL back reproduces the nested structure.
    let roundtrip = dir.join("roundtrip.winmd");
    windows_rdl::reader()
        .input(&rdl_dir)
        .output(&roundtrip)
        .write()
        .unwrap();
    let roundtrip_index = reader::Index::read(roundtrip.to_string_lossy().as_ref()).unwrap();
    assert_nested(&roundtrip_index, "Test");
}

/// The architecture of a nested type is always that of its enclosing type, so the
/// RDL omits the redundant `#[arch]` on inline nested records, but the winmd must
/// still carry it (bindgen hoists nested types to arch-gated flat helpers). The
/// reader therefore inherits the parent's architecture onto a nested type that has
/// no explicit `#[arch]`, and the writer omits it again on the way out.
#[test]
fn nested_type_inherits_parent_arch() {
    let dir = std::env::temp_dir().join("win_nested_arch");
    std::fs::create_dir_all(&dir).unwrap();

    // `Foo` is x64-only; its inline nested struct carries no `#[arch]` of its own.
    let src = "#[win32] mod Test { \
        #[arch(X64)] struct Foo { \
            flags: u32, \
            Anonymous: struct { lo: u32, hi: u32 }, \
        } \
    }";
    let winmd_path = winmd(&dir, "arch", src);
    let index = reader::Index::read(&winmd_path).unwrap();

    let arch_bits = |t: reader::TypeDef| -> Option<i32> {
        let attr = t.find_attribute("SupportedArchitectureAttribute")?;
        match attr.value().first() {
            Some((_, Value::I32(v))) => Some(*v),
            Some((_, Value::EnumValue(_, inner))) => match inner.as_ref() {
                Value::I32(v) => Some(*v),
                _ => None,
            },
            _ => None,
        }
    };

    let foo = index
        .types()
        .find(|t| t.name() == "Foo")
        .expect("Foo present");
    assert_eq!(
        arch_bits(foo),
        Some(2),
        "top-level Foo must be tagged x64 (2)"
    );

    let child = index.nested(foo).next().expect("Foo has a nested type");
    assert_eq!(
        arch_bits(child),
        Some(2),
        "the nested type must inherit its parent's x64 architecture in the winmd"
    );

    // The writer omits the redundant `#[arch]` on the inline nested record but keeps
    // it on the top-level type.
    let rdl_dir = dir.join("rdl");
    std::fs::create_dir_all(&rdl_dir).unwrap();
    windows_rdl::writer()
        .input(&winmd_path)
        .output(&rdl_dir)
        .split()
        .write()
        .unwrap();
    let rdl_text = std::fs::read_to_string(rdl_dir.join("Test.rdl")).unwrap();
    assert_eq!(
        rdl_text.matches("#[arch(").count(),
        1,
        "exactly one #[arch] (on the top-level type, not the nested one):\n{rdl_text}"
    );
}

#[test]
fn nested_field_scopes_distinguish_namespaces_and_top_level_names() {
    let dir = std::env::temp_dir().join("win_nested_scopes");
    std::fs::create_dir_all(&dir).unwrap();
    let source = r#"
        #[win32] mod First {
            struct Outer { Anonymous: union { Block: struct { value: i32 } } }
            struct Outer_0 { value: u64 }
        }
        #[win32] mod Second {
            struct Outer { Anonymous: union { Block: struct { value: f32 } } }
        }
    "#;
    let path = winmd(&dir, "scopes", source);
    let index = reader::Index::read(&path).unwrap();
    for namespace in ["First", "Second"] {
        let outer = index.expect(namespace, "Outer");
        let child = index.nested(outer).next().unwrap();
        let grandchild = index.nested(child).next().unwrap();
        assert_eq!(child.name(), "Outer_0");
        assert_eq!(grandchild.name(), "Outer_0_0");
        assert_nested_field(outer, "Anonymous", namespace, &["Outer", "Outer_0"]);
        assert_nested_field(
            child,
            "Block",
            namespace,
            &["Outer", "Outer_0", "Outer_0_0"],
        );
    }
    assert!(!index.expect("First", "Outer_0").flags().is_nested());
}
