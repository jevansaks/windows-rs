use std::collections::BTreeMap;
use std::path::PathBuf;
use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities, RdlPartition,
    RootPartition, extract,
};
use windows_metadata::{Type, reader::Item};

const OPENGL_NAMESPACE: &str = "Windows.Win32.Graphics.OpenGL";

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-opaque-classes-{name}-{}",
        std::process::id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn references() -> windows_clang::MetadataReferences {
    windows_clang::MetadataReferences::new([windows_metadata::reader::File::new(
        windows_default::WINRT.to_vec(),
    )
    .unwrap()])
}

fn output<'a>(partitions: &'a BTreeMap<RdlPartition, String>, namespace: &str) -> &'a str {
    partitions
        .iter()
        .find(|(partition, _)| partition.namespace == namespace)
        .unwrap()
        .1
}

#[test]
fn glu_style_forward_classes_preserve_metadata_identity() {
    helpers::ensure_libclang();

    let scratch = scratch("glu");
    let header = scratch.join("glu.h");
    std::fs::write(
        &header,
        "class GLUnurbs;\n\
         class GLUquadric;\n\
         class GLUtesselator;\n\
         struct GLUmesh;\n\
         class __declspec(uuid(\"12345678-1234-5678-90ab-cdef12345678\")) GLUfactory;\n\
         typedef GLUnurbs GLUnurbsObj;\n\
         typedef GLUquadric GLUquadricObj;\n\
         typedef GLUtesselator GLUtesselatorObj;\n\
         typedef GLUtesselator GLUtriangulatorObj;\n\
         typedef GLUmesh GLUmeshObj;\n\
         extern \"C\" GLUnurbs* gluNewNurbsRenderer();\n\
         extern \"C\" void gluDeleteNurbsRenderer(GLUnurbs* value);\n\
         extern \"C\" GLUquadric* gluNewQuadric();\n\
         extern \"C\" void gluDeleteQuadric(GLUquadric* value);\n\
         extern \"C\" GLUtesselator* gluNewTess();\n\
         extern \"C\" void gluDeleteTess(GLUtesselator* value);\n\
         extern \"C\" GLUmesh* gluNewMesh();\n\
         extern \"C\" void gluDeleteMesh(GLUmesh* value);\n",
    )
    .unwrap();

    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!("#include \"{}\"\n", header.to_string_lossy()),
        )
        .with_roots([header.to_string_lossy().to_string()])],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();

    for name in ["GLUnurbs", "GLUquadric", "GLUtesselator", "GLUmesh"] {
        let fact = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == name)
            .unwrap();
        let FactData::Record { fields, .. } = &fact.data else {
            panic!("{fact:#?}");
        };
        assert!(!fact.definition, "{fact:#?}");
        assert!(fields.is_empty(), "{fact:#?}");
    }
    let coclass = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == "GLUfactory")
        .unwrap();
    assert!(
        matches!(coclass.data, FactData::Class { .. }),
        "{coclass:#?}"
    );

    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("opengl", OPENGL_NAMESPACE),
    );
    let references = references();
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.library = Some("glu32.dll");
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let rdl = output(&partitions, OPENGL_NAMESPACE);

    for name in ["GLUnurbs", "GLUquadric", "GLUtesselator", "GLUmesh"] {
        assert_eq!(rdl.matches(&format!("struct {name}")).count(), 1, "{rdl}");
    }
    for (alias, target) in [
        ("GLUnurbsObj", "GLUnurbs"),
        ("GLUquadricObj", "GLUquadric"),
        ("GLUtesselatorObj", "GLUtesselator"),
        ("GLUtriangulatorObj", "GLUtesselator"),
        ("GLUmeshObj", "GLUmesh"),
    ] {
        assert!(rdl.contains(&format!("type {alias} = {target};")), "{rdl}");
    }
    assert!(
        rdl.contains("const GLUfactory: GUID = 0x12345678_1234_5678_90ab_cdef12345678;"),
        "{rdl}"
    );
    assert!(
        rdl.contains("extern \"C\" fn gluNewNurbsRenderer() -> *mut GLUnurbs;"),
        "{rdl}"
    );
    assert!(
        rdl.contains("extern \"C\" fn gluDeleteNurbsRenderer(value: *mut GLUnurbs);"),
        "{rdl}"
    );

    let winmd = scratch.join("glu.winmd");
    let mut compiler = windows_rdl::reader();
    for output in partitions.values() {
        compiler.input_text(output);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();

    for name in ["GLUnurbs", "GLUquadric", "GLUtesselator", "GLUmesh"] {
        assert_eq!(index.expect(OPENGL_NAMESPACE, name).fields().count(), 0);
    }
    for (alias, target) in [
        ("GLUnurbsObj", "GLUnurbs"),
        ("GLUquadricObj", "GLUquadric"),
        ("GLUtesselatorObj", "GLUtesselator"),
        ("GLUtriangulatorObj", "GLUtesselator"),
        ("GLUmeshObj", "GLUmesh"),
    ] {
        assert_eq!(
            index.expect(OPENGL_NAMESPACE, alias).underlying_type(),
            Some(Type::value_named(OPENGL_NAMESPACE, target))
        );
    }

    for (create, delete, target) in [
        ("gluNewNurbsRenderer", "gluDeleteNurbsRenderer", "GLUnurbs"),
        ("gluNewQuadric", "gluDeleteQuadric", "GLUquadric"),
        ("gluNewTess", "gluDeleteTess", "GLUtesselator"),
        ("gluNewMesh", "gluDeleteMesh", "GLUmesh"),
    ] {
        let Item::Fn(create) = index.expect_item(OPENGL_NAMESPACE, create) else {
            panic!("{create} was not emitted as a function");
        };
        assert_eq!(create.signature(&[]).return_type, pointer_to(target));
        let Item::Fn(delete) = index.expect_item(OPENGL_NAMESPACE, delete) else {
            panic!("{delete} was not emitted as a function");
        };
        assert_eq!(delete.signature(&[]).types, [pointer_to(target)]);
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn included_namespaced_forward_class_emits_only_as_dependency() {
    helpers::ensure_libclang();

    let scratch = scratch("namespaced");
    let dependency = scratch.join("dependency.h");
    let public = scratch.join("public.h");
    std::fs::write(
        &dependency,
        "namespace Native {\n\
             class OPAQUE_CONTEXT;\n\
             class UNUSED_CONTEXT;\n\
             typedef OPAQUE_CONTEXT OPAQUE_CONTEXT_OBJ;\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        &public,
        format!(
            "#include \"{}\"\n\
             typedef Native::OPAQUE_CONTEXT PUBLIC_CONTEXT;\n\
             extern \"C\" Native::OPAQUE_CONTEXT* OpenContext();\n",
            dependency.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!("#include \"{}\"\n", public.to_string_lossy()),
        )
        .with_roots([public.to_string_lossy().to_string()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        public.to_string_lossy(),
        RootPartition::new("public", OPENGL_NAMESPACE),
    );
    let references = references();
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.library = Some("test.dll");
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let common = output(&partitions, "Windows.Win32");
    let public = output(&partitions, OPENGL_NAMESPACE);

    assert!(common.contains("struct OPAQUE_CONTEXT"), "{common}");
    assert!(!common.contains("UNUSED_CONTEXT"), "{common}");
    assert!(!common.contains("OPAQUE_CONTEXT_OBJ"), "{common}");
    assert!(
        public.contains("type PUBLIC_CONTEXT = Windows::Win32::OPAQUE_CONTEXT;"),
        "{public}"
    );
    assert!(
        public.contains("extern \"C\" fn OpenContext() -> *mut Windows::Win32::OPAQUE_CONTEXT;"),
        "{public}"
    );

    let winmd = scratch.join("namespaced.winmd");
    windows_rdl::reader()
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    assert_eq!(
        index
            .expect(OPENGL_NAMESPACE, "PUBLIC_CONTEXT")
            .underlying_type(),
        Some(Type::value_named("Windows.Win32", "OPAQUE_CONTEXT"))
    );
    let Item::Fn(open) = index.expect_item(OPENGL_NAMESPACE, "OpenContext") else {
        panic!("OpenContext was not emitted as a function");
    };
    assert_eq!(
        open.signature(&[]).return_type,
        Type::PtrMut(
            Box::new(Type::value_named("Windows.Win32", "OPAQUE_CONTEXT")),
            1
        )
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn definitions_and_by_value_validation_keep_existing_behavior() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [Input::new(
            "definitions.hpp",
            "class PUBLIC_DATA;\n\
             typedef PUBLIC_DATA PUBLIC_DATA_ALIAS;\n\
             class PUBLIC_DATA { public: unsigned value; };\n\
             class PRIVATE_IMPLEMENTATION;\n\
             class PRIVATE_IMPLEMENTATION { int value; };\n\
             extern \"C\" PUBLIC_DATA* GetPublicData();\n\
             extern \"C\" PRIVATE_IMPLEMENTATION* GetPrivateImplementation();\n",
        )],
        &["-x", "c++"],
    )
    .unwrap();
    let definition = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == "PUBLIC_DATA" && fact.definition)
        .unwrap();
    let FactData::Record { fields, .. } = &definition.data else {
        panic!("{definition:#?}");
    };
    assert_eq!(fields.len(), 1, "{definition:#?}");

    let rdl = snapshot
        .emit_with_library("Definitions", "test.dll")
        .unwrap();

    assert_eq!(rdl.matches("struct PUBLIC_DATA_ALIAS").count(), 1, "{rdl}");
    assert!(rdl.contains("value: u32"), "{rdl}");
    assert!(
        rdl.contains("fn GetPublicData() -> *mut PUBLIC_DATA_ALIAS"),
        "{rdl}"
    );
    assert!(
        rdl.contains("fn GetPrivateImplementation() -> *mut void"),
        "{rdl}"
    );
    assert!(!rdl.contains("struct PRIVATE_IMPLEMENTATION"), "{rdl}");

    let error = extract(
        [Input::new(
            "incomplete-by-value.hpp",
            "class OPAQUE_VALUE;\n\
             extern \"C\" void UseOpaqueValue(OPAQUE_VALUE value);\n",
        )],
        &["-x", "c++"],
    )
    .unwrap()
    .emit_with_library("Incomplete", "test.dll")
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("incomplete record `OPAQUE_VALUE` is used by value"),
        "{error}"
    );
}

fn pointer_to(name: &str) -> Type {
    Type::PtrMut(Box::new(Type::value_named(OPENGL_NAMESPACE, name)), 1)
}
