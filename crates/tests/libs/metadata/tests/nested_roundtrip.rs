use windows_metadata::*;

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

fn type_ref<'a>(index: &'a reader::Index, namespace: &str, name: &str) -> reader::TypeRef<'a> {
    index
        .type_refs()
        .find(|type_ref| {
            let qualified = type_ref.qualified_name();
            qualified.namespace == namespace && qualified.name == name
        })
        .unwrap_or_else(|| panic!("missing TypeRef {namespace}.{name}"))
}

fn assert_type_ref_chain(index: &reader::Index, namespace: &str, name: &str) {
    let row = type_ref(index, namespace, name);
    if let Some((parent, leaf)) = name.rsplit_once('/') {
        assert_eq!(row.namespace(), "");
        assert_eq!(row.name(), leaf);
        let expected = type_ref(index, namespace, parent);
        match row.scope() {
            reader::ResolutionScope::TypeRef(actual) => assert_eq!(actual, expected),
            scope => panic!("nested TypeRef {namespace}.{name} has scope {scope:?}"),
        }
    } else {
        assert_eq!(row.namespace(), namespace);
        assert_eq!(row.name(), name);
        assert!(
            matches!(row.scope(), reader::ResolutionScope::Module(_)),
            "top-level TypeRef {namespace}.{name} must have module scope"
        );
    }
}

fn colliding_nested_winmd(path: &std::path::Path) {
    let mut file = writer::File::new("colliding-nested");
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));

    for (namespace, outer_name, member) in [
        ("Test", "First", "first"),
        ("Test", "Second", "second"),
        ("Other", "First", "other"),
    ] {
        let outer = file.TypeDef(
            namespace,
            outer_name,
            value_type,
            TypeAttributes::Public | TypeAttributes::SequentialLayout | TypeAttributes::Sealed,
        );
        file.Field(
            "Anonymous",
            &Type::value_named(namespace, &format!("{outer_name}/Anonymous")),
            FieldAttributes::Public,
        );
        let nested = file.TypeDef(
            "",
            "Anonymous",
            value_type,
            TypeAttributes::NestedPublic | TypeAttributes::ExplicitLayout | TypeAttributes::Sealed,
        );
        file.NestedClass(nested, outer);
        file.Field(member, &Type::I32, FieldAttributes::Public);
    }

    std::fs::write(path, file.into_stream()).unwrap();
}

/// `Outer` must decompose into a top-level type with one anonymous nested struct,
/// which in turn contains one anonymous nested union, expressed as real
/// `NestedClass` rows (nested types living in the empty namespace, `NestedPublic`).
fn assert_nested(index: &reader::Index) {
    let outer = index
        .types()
        .find(|t| t.name() == "Outer")
        .expect("Outer type present");
    assert!(
        !outer.flags().is_nested(),
        "top-level Outer must not be nested"
    );
    assert_eq!(outer.qualified_name().namespace, "Test");
    assert_eq!(outer.qualified_name().name, "Outer");

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
    assert_eq!(child.qualified_name().namespace, "Test");
    assert_eq!(child.qualified_name().name, "Outer/Outer_2");

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
    assert_eq!(grandchildren[0].qualified_name().namespace, "Test");
    assert_eq!(
        grandchildren[0].qualified_name().name,
        "Outer/Outer_2/Outer_2_2"
    );

    let outer_fields: Vec<_> = outer.fields().collect();
    assert!(
        matches!(
            outer_fields[0].ty(),
            Type::ValueName(tn) if &tn == ("Test", "Helper")
        ),
        "ordinary top-level references must remain top-level"
    );
    assert!(
        matches!(
            outer_fields[2].ty(),
            Type::ValueName(tn) if &tn == ("Test", "Outer/Outer_2")
        ),
        "inline child must retain its full enclosing path"
    );

    let child_fields: Vec<_> = child.fields().collect();
    assert!(
        matches!(
            child_fields[1].ty(),
            Type::PtrMut(inner, 1)
                if matches!(inner.as_ref(), Type::ValueName(tn) if tn == ("Test", "Outer"))
        ),
        "a nested record must retain an exact pointer back to its outer type"
    );
    assert!(
        matches!(
            child_fields[2].ty(),
            Type::ValueName(tn) if &tn == ("Test", "Outer/Outer_2/Outer_2_2")
        ),
        "depth-two child must retain its full enclosing path"
    );

    assert_type_ref_chain(index, "Test", "Helper");
    assert_type_ref_chain(index, "Test", "Outer");
    assert_type_ref_chain(index, "Test", "Outer/Outer_2");
    assert_type_ref_chain(index, "Test", "Outer/Outer_2/Outer_2_2");
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
        struct Helper { value: i32 } \
        struct Outer { \
            helper: Helper, \
            header: u32, \
            Anonymous: struct { \
                x: i32, \
                owner: *mut Outer, \
                Anonymous: union { a: i32, b: f32 }, \
            }, \
            tail: u16, \
        } \
    }";
    let winmd_path = winmd(&dir, "nested", src);

    // 1. The reader produced real NestedClass rows.
    let index = reader::Index::read(&winmd_path).unwrap();
    assert_nested(&index);

    // 2. merge() preserves the nested structure.
    let merged = dir.join("merged.winmd");
    merge().input(&winmd_path).output(&merged).merge().unwrap();
    let merged_index = reader::Index::read(merged.to_string_lossy().as_ref()).unwrap();
    assert_nested(&merged_index);

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
    assert_nested(&roundtrip_index);
}

#[test]
fn nested_type_refs_survive_namespace_remap() {
    let dir = std::env::temp_dir().join("win_nested_remap");
    std::fs::create_dir_all(&dir).unwrap();

    let input = winmd(
        &dir,
        "input",
        "#[win32] mod Flat { \
            struct Outer { \
                Anonymous: struct { owner: *mut Outer }, \
            } \
        }",
    );
    let output = dir.join("output.winmd");

    remap()
        .input(input)
        .source("Flat")
        .route("Outer", "Routed")
        .fallback("Fallback")
        .output(&output)
        .remap()
        .unwrap();

    let index = reader::Index::read(output.to_string_lossy().as_ref()).unwrap();
    let outer = index.get("Routed", "Outer").next().unwrap();
    let child = index.nested(outer).next().unwrap();
    assert_eq!(child.qualified_name().namespace, "Routed");
    assert_eq!(child.qualified_name().name, "Outer/Outer_0");
    assert!(
        matches!(
            outer.fields().next().unwrap().ty(),
            Type::ValueName(tn) if &tn == ("Routed", "Outer/Outer_0")
        ),
        "the nested path must route with its outer type"
    );
    assert!(
        matches!(
            child.fields().next().unwrap().ty(),
            Type::PtrMut(inner, 1)
                if matches!(inner.as_ref(), Type::ValueName(tn) if tn == ("Routed", "Outer"))
        ),
        "the pointer back to the outer type must use the remapped namespace"
    );
    assert_type_ref_chain(&index, "Routed", "Outer");
    assert_type_ref_chain(&index, "Routed", "Outer/Outer_0");
}

#[test]
fn nested_type_ref_paths_disambiguate_matching_leaf_names() {
    let dir = std::env::temp_dir().join("win_nested_collisions");
    std::fs::create_dir_all(&dir).unwrap();

    let path = winmd(
        &dir,
        "collisions",
        "#[win32] mod Left { \
            struct Outer { Anonymous: struct { left: i32 } } \
        } \
        #[win32] mod Right { \
            struct Outer { Anonymous: struct { right: i32 } } \
        }",
    );
    let index = reader::Index::read(path).unwrap();

    for namespace in ["Left", "Right"] {
        let outer = index.get(namespace, "Outer").next().unwrap();
        let child = index.nested(outer).next().unwrap();
        assert_eq!(child.name(), "Outer_0");
        assert_eq!(child.qualified_name().namespace, namespace);
        assert_eq!(child.qualified_name().name, "Outer/Outer_0");
        assert_type_ref_chain(&index, namespace, "Outer");
        assert_type_ref_chain(&index, namespace, "Outer/Outer_0");
    }
}

#[test]
fn nested_type_ref_paths_disambiguate_parents_and_namespaces() {
    let dir = std::env::temp_dir().join("win_nested_parent_collisions");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.winmd");
    colliding_nested_winmd(&input);

    let assert_collisions = |index: &reader::Index| {
        for (namespace, outer_name) in [("Test", "First"), ("Test", "Second"), ("Other", "First")] {
            let outer = index.get(namespace, outer_name).next().unwrap();
            let child = index.nested(outer).next().unwrap();
            let path = format!("{outer_name}/Anonymous");
            assert_eq!(child.name(), "Anonymous");
            assert_eq!(child.qualified_name().namespace, namespace);
            assert_eq!(child.qualified_name().name, path);
            assert!(
                matches!(
                    outer.fields().next().unwrap().ty(),
                    Type::ValueName(tn)
                        if tn.namespace == namespace && tn.name == path
                ),
                "{namespace}.{outer_name}"
            );
            assert_type_ref_chain(index, namespace, outer_name);
            assert_type_ref_chain(index, namespace, &path);
        }
    };

    let index = reader::Index::read(&input).unwrap();
    assert_collisions(&index);

    let merged = dir.join("merged.winmd");
    merge().input(&input).output(&merged).merge().unwrap();
    let merged_index = reader::Index::read(merged).unwrap();
    assert_collisions(&merged_index);
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
