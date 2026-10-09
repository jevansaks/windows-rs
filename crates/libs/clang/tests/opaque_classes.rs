use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities, RdlPartition,
    RootPartition, extract,
};
use windows_metadata::{HasAttributes, Type, reader::Item};

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

#[test]
fn forward_classes_preserve_pointer_identity() {
    helpers::ensure_libclang();

    let scratch = scratch("forward-classes");
    let first_types = scratch.join("first_types.h");
    let first_api = scratch.join("first_api.h");
    let first_definition = scratch.join("first_definition.h");
    let dependency_types = scratch.join("dependency_types.h");
    let second_types = scratch.join("second_types.h");
    let second_api = scratch.join("second_api.h");
    std::fs::write(
        &first_types,
        "#define W32M(text) __attribute__((annotate(text)))\n\
         class NativeEmpty;\n\
         class AliasedEmpty;\n\
         class AnotherEmpty;\n\
         class PlainImplementation {\n\
         public:\n\
             virtual ~PlainImplementation();\n\
             unsigned hidden;\n\
         };\n\
         typedef AliasedEmpty NATIVE_EMPTY_ALIAS;\n\
         typedef NativeEmpty* NATIVE_EMPTY_PTR;\n",
    )
    .unwrap();
    std::fs::write(&first_definition, "class AliasedEmpty;\n").unwrap();
    std::fs::write(&dependency_types, "class DependencyEmpty;\n").unwrap();
    std::fs::write(
        &first_api,
        format!(
            "#include \"{}\"\n\
             #include \"{}\"\n\
             extern \"C\" int __stdcall FirstUse(\n\
                 W32M(\"win32metadata:in\") NativeEmpty* value,\n\
                 W32M(\"win32metadata:in\") const NativeEmpty* input,\n\
                 W32M(\"win32metadata:out\") NativeEmpty** output,\n\
                 W32M(\"win32metadata:in\") NATIVE_EMPTY_PTR alias,\n\
                 W32M(\"win32metadata:in\") NATIVE_EMPTY_ALIAS* class_alias,\n\
                 W32M(\"win32metadata:in\") AnotherEmpty* another,\n\
                 W32M(\"win32metadata:in\") DependencyEmpty* dependency,\n\
                 W32M(\"win32metadata:in\") PlainImplementation* plain);\n",
            first_types.to_string_lossy(),
            dependency_types.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(&second_types, "class NativeEmpty;\n").unwrap();
    std::fs::write(
        &second_api,
        format!(
            "#include \"{}\"\n\
             extern \"C\" int __stdcall SecondUse(NativeEmpty** value);\n",
            second_types.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = extract(
        [
            Input::new(
                "first.cpp",
                format!(
                    "#include \"{}\"\n#include \"{}\"\n",
                    first_api.to_string_lossy(),
                    first_definition.to_string_lossy()
                ),
            )
            .with_roots([
                first_api.to_string_lossy().to_string(),
                first_types.to_string_lossy().to_string(),
                first_definition.to_string_lossy().to_string(),
            ]),
            Input::new(
                "second.cpp",
                format!("#include \"{}\"\n", second_api.to_string_lossy()),
            )
            .with_roots([
                second_api.to_string_lossy().to_string(),
                second_types.to_string_lossy().to_string(),
            ]),
        ],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    for name in ["NativeEmpty", "AliasedEmpty", "AnotherEmpty"] {
        let fact = snapshot
            .facts()
            .iter()
            .find(|fact| fact.origin.tu == "first.cpp" && fact.name == name && !fact.definition)
            .unwrap();
        let FactData::Record {
            base,
            fields,
            size,
            align,
            packing,
            alignment,
            union,
        } = &fact.data
        else {
            panic!("{fact:#?}");
        };
        assert!(base.is_none(), "{fact:#?}");
        assert!(fields.is_empty(), "{fact:#?}");
        assert!(*size < 0 && *align < 0, "{fact:#?}");
        assert!(packing.is_none() && alignment.is_none(), "{fact:#?}");
        assert!(!union, "{fact:#?}");
    }
    let plain = snapshot
        .facts()
        .iter()
        .find(|fact| {
            fact.origin.tu == "first.cpp" && fact.name == "PlainImplementation" && fact.definition
        })
        .unwrap();
    assert!(
        matches!(plain.data, FactData::Unsupported { .. }),
        "{plain:#?}"
    );
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            first_api.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            first_types.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            first_definition.to_string_lossy(),
            RootPartition::new("first", "Example.First"),
        )
        .with_traversed_header(
            second_api.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        )
        .with_traversed_header(
            second_types.to_string_lossy(),
            RootPartition::new("second", "Example.Second"),
        );
    let references = references();
    let functions = BTreeSet::from(["FirstUse".to_string(), "SecondUse".to_string()]);
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.functions = Some(&functions);
    options.library = Some("native.dll");
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let first = partitions
        .iter()
        .filter(|(partition, _)| partition.namespace == "Example.First")
        .map(|(_, rdl)| rdl.as_str())
        .collect::<String>();
    let second = partitions
        .iter()
        .filter(|(partition, _)| partition.namespace == "Example.Second")
        .map(|(_, rdl)| rdl.as_str())
        .collect::<String>();
    let common = partitions
        .iter()
        .filter(|(partition, _)| partition.namespace == "Windows.Win32")
        .map(|(_, rdl)| rdl.as_str())
        .collect::<String>();

    for name in ["NativeEmpty", "AnotherEmpty", "AliasedEmpty"] {
        assert!(first.contains(&format!("struct {name}")), "{first}");
    }
    assert_eq!(first.matches("struct NativeEmpty").count(), 1, "{first}");
    assert_eq!(second.matches("struct NativeEmpty").count(), 1, "{second}");
    assert!(
        first.contains("type NATIVE_EMPTY_PTR = *mut NativeEmpty"),
        "{first}"
    );
    assert!(
        first.contains("type NATIVE_EMPTY_ALIAS = AliasedEmpty"),
        "{first}"
    );
    assert!(
        first.contains("#[out] output: *mut *mut NativeEmpty"),
        "{first}"
    );
    assert!(first.contains("#[in] input: *const NativeEmpty"), "{first}");
    assert!(
        first.contains("dependency: *mut Windows::Win32::DependencyEmpty"),
        "{first}"
    );
    assert!(first.contains("plain: *mut void"), "{first}");
    for hidden in [
        "hidden",
        "Reset",
        "PlainImplementation",
        "native_inheritance",
    ] {
        assert!(!first.contains(hidden), "{first}");
    }
    assert!(common.contains("struct DependencyEmpty"), "{common}");
    assert!(!first.contains("struct DependencyEmpty"), "{first}");

    let winmd = scratch.join("forward-classes.winmd");
    windows_rdl::reader()
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    for (namespace, name) in [
        ("Example.First", "NativeEmpty"),
        ("Example.First", "AnotherEmpty"),
        ("Example.First", "AliasedEmpty"),
        ("Example.Second", "NativeEmpty"),
        ("Windows.Win32", "DependencyEmpty"),
    ] {
        let ty = index.expect(namespace, name);
        assert_eq!(ty.fields().count(), 0, "{namespace}.{name}");
        assert!(
            ty.attributes()
                .all(|attribute| attribute.name() != "NativeInheritanceAttribute"),
            "{namespace}.{name}"
        );
    }
    assert_eq!(
        index
            .expect("Example.First", "NATIVE_EMPTY_ALIAS")
            .underlying_type(),
        Some(Type::value_named("Example.First", "AliasedEmpty"))
    );
    assert_eq!(
        index
            .expect("Example.First", "NATIVE_EMPTY_PTR")
            .underlying_type(),
        Some(Type::PtrMut(
            Box::new(Type::value_named("Example.First", "NativeEmpty")),
            1
        ))
    );
    let Item::Fn(first_use) = index.expect_item("Example.First", "FirstUse") else {
        panic!("FirstUse was not emitted as a function");
    };
    assert_eq!(first_use.calling_convention(), "system");
    assert_eq!(
        first_use.signature(&[]).types,
        [
            Type::PtrMut(
                Box::new(Type::value_named("Example.First", "NativeEmpty")),
                1
            ),
            Type::PtrConst(
                Box::new(Type::value_named("Example.First", "NativeEmpty")),
                1
            ),
            Type::PtrMut(
                Box::new(Type::value_named("Example.First", "NativeEmpty")),
                2
            ),
            Type::value_named("Example.First", "NATIVE_EMPTY_PTR"),
            Type::PtrMut(
                Box::new(Type::value_named("Example.First", "NATIVE_EMPTY_ALIAS")),
                1
            ),
            Type::PtrMut(
                Box::new(Type::value_named("Example.First", "AnotherEmpty")),
                1
            ),
            Type::PtrMut(
                Box::new(Type::value_named("Windows.Win32", "DependencyEmpty")),
                1
            ),
            Type::PtrMut(Box::new(Type::Void), 1),
        ]
    );
    let Item::Fn(second_use) = index.expect_item("Example.Second", "SecondUse") else {
        panic!("SecondUse was not emitted as a function");
    };
    assert_eq!(
        second_use.signature(&[]).types,
        [Type::PtrMut(
            Box::new(Type::value_named("Example.Second", "NativeEmpty")),
            2
        )]
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn unsupported_annotations_and_incomplete_class_by_value_uses_are_rejected() {
    helpers::ensure_libclang();

    let error = extract(
        [Input::new(
            "unsupported.hpp",
            "#define W32M(text) __attribute__((annotate(text)))\n\
             class W32M(\"win32metadata:unsupported_key\") OpaqueValue;\n",
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("unknown win32metadata annotation `unsupported_key`"),
        "{error}"
    );

    for (name, usage) in [
        (
            "direct",
            "extern \"C\" void UseOpaqueValue(OpaqueValue value);",
        ),
        (
            "alias",
            "typedef OpaqueValue NativeAlias;\n\
             extern \"C\" void UseOpaqueValue(NativeAlias value);",
        ),
        (
            "callback",
            "typedef void (*NativeCallback)(OpaqueValue value);\n\
             extern \"C\" void UseOpaqueValue(NativeCallback callback);",
        ),
        ("return", "extern \"C\" OpaqueValue UseOpaqueValue();"),
    ] {
        let scratch = scratch(&format!("incomplete-class-by-value-{name}"));
        let header = scratch.join("native.h");
        std::fs::write(
            &header,
            format!(
                "class OpaqueValue;\n\
                 {usage}\n"
            ),
        )
        .unwrap();
        let snapshot = extract(
            [Input::new(
                "aggregate.cpp",
                format!("#include \"{}\"\n", header.to_string_lossy()),
            )
            .with_roots([header.to_string_lossy().to_string()])],
            &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
        )
        .unwrap();
        let policy = HeaderPartitionPolicy::new().with_traversed_header(
            header.to_string_lossy(),
            RootPartition::new("native", "Example.Native"),
        );
        let references = references();
        let functions = BTreeSet::from(["UseOpaqueValue".to_string()]);
        let mut options = EmitOptions::new("Windows.Win32", references.types());
        options.functions = Some(&functions);
        options.library = Some("native.dll");
        let error = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap()
            .audit(&options)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("incomplete record `OpaqueValue` is used by value"),
            "{name}: {error}"
        );
        std::fs::remove_dir_all(scratch).unwrap();
    }
}

#[test]
fn selected_native_geometry_classes_emit_as_pointer_dependencies() {
    helpers::ensure_libclang();

    for target in ["x86_64-pc-windows-msvc", "i686-pc-windows-msvc"] {
        let scratch = scratch(if target.starts_with("x86_64") {
            "native-geometry-x64"
        } else {
            "native-geometry-x86"
        });
        let header = scratch.join("geometry.h");
        std::fs::write(&header, native_geometry_source()).unwrap();
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
                &format!("--target={target}"),
            ],
        )
        .unwrap();
        for name in [
            "Point",
            "Rect",
            "Size",
            "PointF",
            "RectF",
            "PathData",
            "UnselectedData",
        ] {
            assert!(
                snapshot.unsupported().any(|(fact, _)| fact.name == name),
                "{name} did not retain its unsupported extracted fact for {target}"
            );
        }
        let policy = HeaderPartitionPolicy::new().with_traversed_header(
            header.to_string_lossy(),
            RootPartition::new("gdiplus", "Windows.Win32.Graphics.GdiPlus")
                .with_library("GdipUseGeometry", "gdiplus.dll"),
        );
        let references = references();
        for name in ["Point", "Rect", "Size"] {
            assert_eq!(
                references.types().get(name).unwrap().namespace,
                "Windows.Foundation"
            );
        }
        let functions = BTreeSet::from(["GdipUseGeometry".to_string()]);
        let mut options = EmitOptions::new("Windows.Win32", references.types());
        options.functions = Some(&functions);
        let plan = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap();
        assert!(plan.audit(&options).unwrap().is_clean());
        let partitions = plan.emit_with_options(&options).unwrap();
        let rdl = output(&partitions, "Windows.Win32.Graphics.GdiPlus");

        for (name, fields) in [
            ("Point", ["X: i32", "Y: i32"].as_slice()),
            (
                "Rect",
                ["X: i32", "Y: i32", "Width: i32", "Height: i32"].as_slice(),
            ),
            ("Size", ["Width: i32", "Height: i32"].as_slice()),
            ("PointF", ["X: REAL", "Y: REAL"].as_slice()),
            (
                "RectF",
                ["X: REAL", "Y: REAL", "Width: REAL", "Height: REAL"].as_slice(),
            ),
            (
                "PathData",
                ["Count: i32", "Points: *mut PointF", "Types: *mut u8"].as_slice(),
            ),
        ] {
            assert!(rdl.contains(&format!("struct {name}")), "{rdl}");
            for field in fields {
                assert!(rdl.contains(field), "{rdl}");
            }
            assert!(rdl.contains("type REAL = f32"), "{rdl}");
        }
        for (alias, target) in [
            ("GpPoint", "Point"),
            ("GpRect", "Rect"),
            ("GpSize", "Size"),
            ("GpPointF", "PointF"),
            ("GpRectF", "RectF"),
            ("GpPathData", "PathData"),
        ] {
            assert!(rdl.contains(&format!("type {alias} = {target}")), "{rdl}");
        }
        for method in ["Equals", "Clone", "operator", "Unselected"] {
            assert!(!rdl.contains(method), "{rdl}");
        }
        assert!(
            rdl.contains(
                "extern fn GdipUseGeometry(path: *mut void, point: *mut GpPoint, rect: *mut \
                 GpRect, size: *mut GpSize, point_f: *mut GpPointF, rect_f: *mut GpRectF, data: \
                 *mut GpPathData, external: *mut Windows::Foundation::Point)"
            ),
            "{rdl}"
        );

        let winmd = scratch.join("geometry.winmd");
        windows_rdl::reader()
            .input_texts(partitions.values())
            .reference_default()
            .output(&winmd)
            .write()
            .unwrap();
        let index = windows_metadata::reader::Index::read(&winmd).unwrap();
        assert_eq!(
            index
                .expect("Windows.Win32.Graphics.GdiPlus", "REAL")
                .underlying_type(),
            Some(Type::F32)
        );
        for (alias, target) in [
            ("GpPoint", "Point"),
            ("GpRect", "Rect"),
            ("GpSize", "Size"),
            ("GpPointF", "PointF"),
            ("GpRectF", "RectF"),
            ("GpPathData", "PathData"),
        ] {
            assert_eq!(
                index
                    .expect("Windows.Win32.Graphics.GdiPlus", alias)
                    .underlying_type(),
                Some(Type::value_named("Windows.Win32.Graphics.GdiPlus", target))
            );
        }
        let point_fields = index
            .expect("Windows.Win32.Graphics.GdiPlus", "PointF")
            .fields()
            .map(|field| (field.name().to_string(), field.ty()))
            .collect::<Vec<_>>();
        assert_eq!(
            point_fields,
            [
                (
                    "X".to_string(),
                    Type::value_named("Windows.Win32.Graphics.GdiPlus", "REAL")
                ),
                (
                    "Y".to_string(),
                    Type::value_named("Windows.Win32.Graphics.GdiPlus", "REAL")
                ),
            ]
        );
        let native_point_fields = index
            .expect("Windows.Win32.Graphics.GdiPlus", "Point")
            .fields()
            .map(|field| (field.name().to_string(), field.ty()))
            .collect::<Vec<_>>();
        assert_eq!(
            native_point_fields,
            [("X".to_string(), Type::I32), ("Y".to_string(), Type::I32),]
        );
        let native_rect_fields = index
            .expect("Windows.Win32.Graphics.GdiPlus", "Rect")
            .fields()
            .map(|field| (field.name().to_string(), field.ty()))
            .collect::<Vec<_>>();
        assert_eq!(
            native_rect_fields,
            [
                ("X".to_string(), Type::I32),
                ("Y".to_string(), Type::I32),
                ("Width".to_string(), Type::I32),
                ("Height".to_string(), Type::I32),
            ]
        );
        let native_size_fields = index
            .expect("Windows.Win32.Graphics.GdiPlus", "Size")
            .fields()
            .map(|field| (field.name().to_string(), field.ty()))
            .collect::<Vec<_>>();
        assert_eq!(
            native_size_fields,
            [
                ("Width".to_string(), Type::I32),
                ("Height".to_string(), Type::I32),
            ]
        );
        let path_fields = index
            .expect("Windows.Win32.Graphics.GdiPlus", "PathData")
            .fields()
            .map(|field| (field.name().to_string(), field.ty()))
            .collect::<Vec<_>>();
        assert_eq!(
            path_fields,
            [
                ("Count".to_string(), Type::I32),
                (
                    "Points".to_string(),
                    Type::PtrMut(
                        Box::new(Type::value_named(
                            "Windows.Win32.Graphics.GdiPlus",
                            "PointF"
                        )),
                        1
                    )
                ),
                ("Types".to_string(), Type::PtrMut(Box::new(Type::U8), 1)),
            ]
        );
        let Item::Fn(function) =
            index.expect_item("Windows.Win32.Graphics.GdiPlus", "GdipUseGeometry")
        else {
            panic!("GdipUseGeometry was not emitted as a function");
        };
        assert_eq!(function.calling_convention(), "system");
        assert_eq!(
            function.impl_map().unwrap().import_scope().name(),
            "gdiplus.dll"
        );
        assert_eq!(
            function.signature(&[]).types,
            [
                Type::PtrMut(Box::new(Type::Void), 1),
                pointer_to_namespace("GpPoint"),
                pointer_to_namespace("GpRect"),
                pointer_to_namespace("GpSize"),
                pointer_to_namespace("GpPointF"),
                pointer_to_namespace("GpRectF"),
                pointer_to_namespace("GpPathData"),
                Type::PtrMut(
                    Box::new(Type::value_named("Windows.Foundation", "Point")),
                    1
                ),
            ]
        );

        std::fs::remove_dir_all(scratch).unwrap();
    }
}

#[test]
fn selected_protected_storage_class_keeps_native_identity_over_external_short_name() {
    helpers::ensure_libclang();

    let scratch = scratch("protected-storage-identity");
    let header = scratch.join("gdiplus.h");
    std::fs::write(
        &header,
        r#"
            typedef unsigned char BYTE;
            typedef unsigned int ARGB;
            typedef int INT;

            namespace ABI { namespace Windows { namespace UI {
                struct Color {
                    BYTE A;
                    BYTE R;
                    BYTE G;
                    BYTE B;
                };
            } } }

            namespace Gdiplus {
                class Color {
                public:
                    Color();
                    Color(ARGB value);
                    static ARGB MakeARGB(BYTE a, BYTE r, BYTE g, BYTE b);
                protected:
                    ARGB Argb;
                };

                struct ColorMap {
                    Color oldColor;
                    Color newColor;
                };

                namespace DllExports {
                    extern "C" INT __stdcall GdipUseColorMap(
                        const ColorMap* map,
                        ABI::Windows::UI::Color* external);
                    extern "C" INT __stdcall GdipTakeColor(Color value);
                    extern "C" INT __stdcall GdipTakeColorMap(ColorMap value);
                }
            }
        "#,
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
    assert!(
        snapshot.unsupported().any(|(fact, _)| fact.name == "Color"),
        "the native class should remain unsupported outside selected planning"
    );

    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("gdiplus", "Windows.Win32.Graphics.GdiPlus")
            .with_library("GdipUseColorMap", "gdiplus.dll")
            .with_library("GdipTakeColor", "gdiplus.dll")
            .with_library("GdipTakeColorMap", "gdiplus.dll"),
    );
    let references = references();
    assert_eq!(
        references.types().get("Color").unwrap().namespace,
        "Windows.UI"
    );
    let functions = BTreeSet::from(["GdipUseColorMap".to_string()]);
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.functions = Some(&functions);
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let rdl = output(&partitions, "Windows.Win32.Graphics.GdiPlus");

    assert!(rdl.contains("struct Color"), "{rdl}");
    assert!(rdl.contains("Argb: ARGB"), "{rdl}");
    assert!(rdl.contains("oldColor: Color"), "{rdl}");
    assert!(rdl.contains("newColor: Color"), "{rdl}");
    assert!(rdl.contains("external: *mut Windows::UI::Color"), "{rdl}");
    assert!(!rdl.contains("oldColor: Windows::UI::Color"), "{rdl}");
    assert!(!rdl.contains("newColor: Windows::UI::Color"), "{rdl}");

    let winmd = scratch.join("protected-storage.winmd");
    windows_rdl::reader()
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    let namespace = "Windows.Win32.Graphics.GdiPlus";
    assert_eq!(
        index.expect(namespace, "ARGB").underlying_type(),
        Some(Type::U32)
    );
    assert_eq!(
        index
            .expect(namespace, "Color")
            .fields()
            .map(|field| (field.name().to_string(), field.ty()))
            .collect::<Vec<_>>(),
        [("Argb".to_string(), Type::value_named(namespace, "ARGB"))]
    );
    assert_eq!(
        index
            .expect(namespace, "ColorMap")
            .fields()
            .map(|field| (field.name().to_string(), field.ty()))
            .collect::<Vec<_>>(),
        [
            (
                "oldColor".to_string(),
                Type::value_named(namespace, "Color")
            ),
            (
                "newColor".to_string(),
                Type::value_named(namespace, "Color")
            ),
        ]
    );
    let Item::Fn(function) = index.expect_item(namespace, "GdipUseColorMap") else {
        panic!("GdipUseColorMap was not emitted as a function");
    };
    assert_eq!(
        function.signature(&[]).types,
        [
            Type::PtrConst(Box::new(Type::value_named(namespace, "ColorMap")), 1),
            Type::PtrMut(Box::new(Type::value_named("Windows.UI", "Color")), 1),
        ]
    );

    for function in ["GdipTakeColor", "GdipTakeColorMap"] {
        let functions = BTreeSet::from([function.to_string()]);
        let mut options = EmitOptions::new("Windows.Win32", references.types());
        options.functions = Some(&functions);
        options.library = Some("gdiplus.dll");
        let error = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap()
            .audit(&options)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("pointer-only native class `Color` is used by value"),
            "{function}: {error}"
        );
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn selected_nonrepresentable_native_class_does_not_use_external_short_name() {
    helpers::ensure_libclang();

    let scratch = scratch("unsupported-native-short-name");
    let header = scratch.join("native.h");
    std::fs::write(
        &header,
        "namespace Native {\n\
             class Color {\n\
             public:\n\
                 virtual void Reset();\n\
             protected:\n\
                 unsigned int Argb;\n\
             };\n\
             struct ColorMap { Color value; };\n\
             extern \"C\" int __stdcall UseNativeColorMap(ColorMap* value);\n\
         }\n",
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
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("native", "Windows.Win32.Native"),
    );
    let references = references();
    assert_eq!(
        references.types().get("Color").unwrap().namespace,
        "Windows.UI"
    );
    let functions = BTreeSet::from(["UseNativeColorMap".to_string()]);
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.functions = Some(&functions);
    options.library = Some("native.dll");
    let error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&options)
        .unwrap_err()
        .to_string();

    assert!(error.contains("unsupported type `Color`"), "{error}");
    assert!(
        error.contains("class is not a public data-only record"),
        "{error}"
    );
    assert!(!error.contains("Windows.UI.Color"), "{error}");

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn selected_native_class_and_owned_pod_same_name_keep_exact_routes() {
    helpers::ensure_libclang();

    let scratch = scratch("native-class-owned-pod-same-name");
    let geometry = scratch.join("geometry.h");
    let pod = scratch.join("pod.h");
    std::fs::write(&geometry, native_geometry_source()).unwrap();
    std::fs::write(
        &pod,
        "namespace Foo {\n\
             struct Point { short X; short Y; };\n\
             namespace Exports {\n\
                 extern \"C\" int __stdcall UseFooPoint(Point* point);\n\
             }\n\
         }\n",
    )
    .unwrap();
    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!(
                "#include \"{}\"\n#include \"{}\"\n",
                geometry.to_string_lossy(),
                pod.to_string_lossy()
            ),
        )
        .with_roots([
            geometry.to_string_lossy().to_string(),
            pod.to_string_lossy().to_string(),
        ])],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            geometry.to_string_lossy(),
            RootPartition::new("gdiplus", "Windows.Win32.Graphics.GdiPlus")
                .with_library("GdipUseGeometry", "gdiplus.dll"),
        )
        .with_traversed_header(
            pod.to_string_lossy(),
            RootPartition::new("foo", "Windows.Win32.Foo").with_library("UseFooPoint", "foo.dll"),
        );
    let references = references();
    let functions = BTreeSet::from(["GdipUseGeometry".to_string(), "UseFooPoint".to_string()]);
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.functions = Some(&functions);
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let gdiplus = output(&partitions, "Windows.Win32.Graphics.GdiPlus");
    let foo = output(&partitions, "Windows.Win32.Foo");

    assert!(gdiplus.contains("struct Point"), "{gdiplus}");
    assert!(gdiplus.contains("type GpPoint = Point"), "{gdiplus}");
    assert!(
        gdiplus.contains("external: *mut Windows::Foundation::Point"),
        "{gdiplus}"
    );
    assert!(foo.contains("struct Point"), "{foo}");
    assert!(foo.contains("X: i16"), "{foo}");
    assert!(foo.contains("Y: i16"), "{foo}");
    assert!(
        foo.contains("extern fn UseFooPoint(point: *mut Point)"),
        "{foo}"
    );
    assert!(!foo.contains("Windows::Foundation::Point"), "{foo}");

    let winmd = scratch.join("same-name.winmd");
    windows_rdl::reader()
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    let gdiplus_point = index
        .expect("Windows.Win32.Graphics.GdiPlus", "Point")
        .fields()
        .map(|field| (field.name().to_string(), field.ty()))
        .collect::<Vec<_>>();
    assert_eq!(
        gdiplus_point,
        [("X".to_string(), Type::I32), ("Y".to_string(), Type::I32),]
    );
    let foo_point = index
        .expect("Windows.Win32.Foo", "Point")
        .fields()
        .map(|field| (field.name().to_string(), field.ty()))
        .collect::<Vec<_>>();
    assert_eq!(
        foo_point,
        [("X".to_string(), Type::I16), ("Y".to_string(), Type::I16),]
    );
    let Item::Fn(use_foo) = index.expect_item("Windows.Win32.Foo", "UseFooPoint") else {
        panic!("UseFooPoint was not emitted as a function");
    };
    assert_eq!(
        use_foo.signature(&[]).types,
        [Type::PtrMut(
            Box::new(Type::value_named("Windows.Win32.Foo", "Point")),
            1
        )]
    );
    let Item::Fn(use_geometry) =
        index.expect_item("Windows.Win32.Graphics.GdiPlus", "GdipUseGeometry")
    else {
        panic!("GdipUseGeometry was not emitted as a function");
    };
    let geometry_types = use_geometry.signature(&[]).types;
    assert_eq!(geometry_types[1], pointer_to_namespace("GpPoint"));
    assert_eq!(
        geometry_types.last(),
        Some(&Type::PtrMut(
            Box::new(Type::value_named("Windows.Foundation", "Point")),
            1
        ))
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn selected_native_geometry_classes_reject_by_value_abi() {
    helpers::ensure_libclang();

    let scratch = scratch("native-geometry-by-value");
    let header = scratch.join("geometry.h");
    std::fs::write(&header, native_geometry_source()).unwrap();
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
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("gdiplus", "Windows.Win32.Graphics.GdiPlus"),
    );
    let references = references();

    for (function, class) in [
        ("GdipReturnPoint", "PointF"),
        ("GdipTakeRect", "RectF"),
        ("GdipTakeArray", "PointF"),
        ("GdipUseArray", "PointF"),
    ] {
        let functions = BTreeSet::from([function.to_string()]);
        let mut options = EmitOptions::new("Windows.Win32", references.types());
        options.functions = Some(&functions);
        options.library = Some("gdiplus.dll");
        let error = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap()
            .audit(&options)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!(
                "pointer-only native class `{class}` is used by value"
            )),
            "{error}"
        );
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn selected_native_geometry_classes_respect_owner_exclusions() {
    helpers::ensure_libclang();

    let scratch = scratch("native-geometry-exclusion");
    let header = scratch.join("geometry.h");
    std::fs::write(&header, native_geometry_source()).unwrap();
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
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("gdiplus", "Windows.Win32.Graphics.GdiPlus").with_exclusion("PathData"),
    );
    let references = references();
    let functions = BTreeSet::from(["GdipUseGeometry".to_string()]);
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.functions = Some(&functions);
    options.library = Some("gdiplus.dll");
    let error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&options)
        .unwrap_err()
        .to_string();

    assert!(
        error.contains(
            "owner-excluded local type `PathData` in partition `gdiplus` namespace \
             `Windows.Win32.Graphics.GdiPlus` is required without a retained public alias"
        ),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn selected_private_and_virtual_classes_remain_nonrepresentable() {
    helpers::ensure_libclang();

    let scratch = scratch("native-geometry-negative");
    let header = scratch.join("geometry.h");
    std::fs::write(&header, native_geometry_source()).unwrap();
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
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("gdiplus", "Windows.Win32.Graphics.GdiPlus"),
    );
    let references = references();
    let functions = BTreeSet::from(["GdipUsePrivate".to_string(), "GdipUseVirtual".to_string()]);
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.functions = Some(&functions);
    options.library = Some("gdiplus.dll");
    let error = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .audit(&options)
        .unwrap_err()
        .to_string();

    assert!(
        error.contains("header partition dependency closure found 2 blocker(s)"),
        "{error}"
    );
    assert!(error.contains("`PrivateGeometry`"), "{error}");
    assert!(error.contains("`VirtualGeometry`"), "{error}");
    assert_eq!(
        error
            .matches("class is not a public data-only record")
            .count(),
        2,
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

fn pointer_to(name: &str) -> Type {
    Type::PtrMut(Box::new(Type::value_named(OPENGL_NAMESPACE, name)), 1)
}

fn pointer_to_namespace(name: &str) -> Type {
    Type::PtrMut(
        Box::new(Type::value_named("Windows.Win32.Graphics.GdiPlus", name)),
        1,
    )
}

fn native_geometry_source() -> &'static str {
    r#"
        typedef unsigned char BYTE;
        typedef int INT;
        typedef float REAL;

        namespace ABI { namespace Windows { namespace Foundation {
            struct Point { float X; float Y; };
        } } }

        namespace Gdiplus {
            class GpPath {};

            class Point {
            public:
                Point();
                Point(const Point& other);
                INT X;
                INT Y;
            };

            class Rect {
            public:
                Rect();
                Rect(const Rect& other);
                Rect Clone() const;
                INT X;
                INT Y;
                INT Width;
                INT Height;
            };

            class Size {
            public:
                Size();
                Size(const Size& other);
                INT Width;
                INT Height;
            };

            class PointF {
            public:
                PointF();
                PointF(const PointF& other);
                PointF operator+(const PointF& other) const;
                bool Equals(const PointF& other) const;
                REAL X;
                REAL Y;
            };

            class RectF {
            public:
                RectF();
                RectF(const RectF& other);
                RectF Clone() const;
                REAL X;
                REAL Y;
                REAL Width;
                REAL Height;
            };

            class PathData {
            public:
                PathData();
                ~PathData();
            private:
                PathData(const PathData& other);
                PathData& operator=(const PathData& other);
            public:
                INT Count;
                PointF* Points;
                BYTE* Types;
            };

            typedef Point GpPoint;
            typedef Rect GpRect;
            typedef Size GpSize;
            typedef PointF GpPointF;
            typedef RectF GpRectF;
            typedef PathData GpPathData;
            struct PointArray { GpPointF Values[2]; };

            class UnselectedData {
            public:
                UnselectedData();
                REAL Value;
            };

            class PrivateGeometry {
                REAL Hidden;
            public:
                REAL Visible;
            };
            typedef PrivateGeometry GpPrivateGeometry;

            class VirtualGeometry {
            public:
                virtual void Reset();
                REAL Value;
            };
            typedef VirtualGeometry GpVirtualGeometry;

            namespace DllExports {
                extern "C" INT __stdcall GdipUseGeometry(
                    GpPath* path,
                    GpPoint* point,
                    GpRect* rect,
                    GpSize* size,
                    GpPointF* point_f,
                    GpRectF* rect_f,
                    GpPathData* data,
                    ABI::Windows::Foundation::Point* external);
                extern "C" INT __stdcall GdipUseUnselected(UnselectedData* value);
                extern "C" GpPointF __stdcall GdipReturnPoint();
                extern "C" INT __stdcall GdipTakeRect(GpRectF value);
                extern "C" INT __stdcall GdipTakeArray(PointArray value);
                extern "C" INT __stdcall GdipUseArray(PointArray* value);
                extern "C" INT __stdcall GdipUsePrivate(GpPrivateGeometry* value);
                extern "C" INT __stdcall GdipUseVirtual(GpVirtualGeometry* value);
            }
        }
    "#
}
