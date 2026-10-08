use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition,
    TypeRef, TypeReference, TypeReferenceKind, extract,
};
use windows_metadata::{
    Type,
    reader::{HasAttributes, TypeCategory},
};

const METADATA_RDL: &str = include_str!("../../../../metadata/metadata.rdl");

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-clang-interface-self-typedefs-{name}-{}",
        std::process::id()
    ));
    if path.exists() {
        std::fs::remove_dir_all(&path).unwrap();
    }
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn include(path: &Path) -> String {
    format!("#include \"{}\"\n", path.to_string_lossy())
}

fn output(partitions: &BTreeMap<windows_clang::RdlPartition, String>, namespace: &str) -> String {
    partitions
        .iter()
        .filter(|(partition, _)| partition.namespace == namespace)
        .map(|(_, rdl)| rdl.as_str())
        .collect()
}

#[test]
fn redundant_interface_self_typedefs_bind_to_canonical_providers() {
    helpers::ensure_libclang();

    let scratch = scratch("providers");
    let support = scratch.join("support.h");
    let direct2d_aliases = scratch.join("direct2d_aliases.h");
    let imaging_aliases = scratch.join("imaging_aliases.h");
    let imaging = scratch.join("imaging.h");
    let direct2d = scratch.join("direct2d.h");
    let color_consumer = scratch.join("color_consumer.h");
    let encoder = scratch.join("encoder.h");

    std::fs::write(
        &support,
        "#pragma once\n\
         #define interface struct\n\
         #define W32M(text) __attribute__((annotate(text)))\n",
    )
    .unwrap();
    std::fs::write(
        &direct2d_aliases,
        "#pragma once\n\
         #include \"support.h\"\n\
         interface IColorContext;\n\
         typedef interface IColorContext IColorContext;\n\
         typedef void *HANDLE;\n\
         typedef HANDLE HLOCAL;\n\
         W32M(\"win32metadata:raii_free=CloseHandle\")\n\
         typedef HANDLE OWNED_HANDLE;\n",
    )
    .unwrap();
    std::fs::write(
        &imaging_aliases,
        "#pragma once\n\
         #include \"support.h\"\n\
         interface IImage;\n\
         typedef interface IImage IImage;\n\
         typedef interface IImage IImageAlias;\n\
         typedef unsigned long DWORD;\n\
         typedef DWORD *PDWORD;\n\
         typedef PDWORD PLCID;\n",
    )
    .unwrap();
    std::fs::write(
        &imaging,
        "#pragma once\n\
         #include \"support.h\"\n\
         struct __declspec(uuid(\"11111111-1111-1111-1111-111111111111\")) IColorContext {\n\
             virtual unsigned GetColorSpace() = 0;\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &direct2d,
        "#pragma once\n\
         #include \"support.h\"\n\
         struct __declspec(uuid(\"22222222-2222-2222-2222-222222222222\")) IImage {\n\
             virtual unsigned GetBounds() = 0;\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &color_consumer,
        "#pragma once\n\
         #include \"direct2d_aliases.h\"\n\
         struct COLOR_CONTEXT_USE {\n\
             IColorContext *value;\n\
             HLOCAL local;\n\
             OWNED_HANDLE owned;\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &encoder,
        "#pragma once\n\
         #include \"imaging_aliases.h\"\n\
         struct __declspec(uuid(\"33333333-3333-3333-3333-333333333333\")) IImageEncoder {\n\
             virtual unsigned WriteFrame(IImage *image, PLCID locale) = 0;\n\
         };\n",
    )
    .unwrap();

    let source = [
        &direct2d_aliases,
        &imaging_aliases,
        &imaging,
        &direct2d,
        &color_consumer,
        &encoder,
    ]
    .into_iter()
    .map(|path| include(path))
    .collect::<String>();
    let include_dir = format!("-I{}", scratch.display());
    let snapshot = extract(
        [Input::new("aggregate.cpp", source)
            .with_root_dirs([scratch.to_string_lossy().to_string()])],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
            include_dir.as_str(),
        ],
    )
    .unwrap();

    for name in ["IColorContext", "IImage"] {
        assert!(
            !snapshot
                .facts()
                .iter()
                .any(|fact| { fact.name == name && matches!(fact.data, FactData::Typedef { .. }) }),
            "{name} retained a redundant typedef:\n{}",
            snapshot.dump()
        );
    }
    for (alias, target) in [
        ("HLOCAL", "HANDLE"),
        ("OWNED_HANDLE", "HANDLE"),
        ("PLCID", "PDWORD"),
        ("IImageAlias", "IImage"),
    ] {
        let fact = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == alias && matches!(fact.data, FactData::Typedef { .. }))
            .unwrap_or_else(|| panic!("{alias} was not retained:\n{}", snapshot.dump()));
        let FactData::Typedef {
            target: TypeRef::Named { name, .. },
        } = &fact.data
        else {
            panic!("{alias} no longer targets its named alias: {fact:#?}");
        };
        assert_eq!(name, target, "{alias}");
    }

    let provider = |name: &str| {
        snapshot
            .facts()
            .iter()
            .find(|fact| {
                fact.name == name
                    && fact.definition
                    && matches!(fact.data, FactData::Interface { .. })
            })
            .unwrap_or_else(|| panic!("{name} provider was not extracted:\n{}", snapshot.dump()))
    };
    let color_provider = provider("IColorContext");
    let image_provider = provider("IImage");
    let color_use = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == "COLOR_CONTEXT_USE")
        .unwrap();
    let FactData::Record { fields, .. } = &color_use.data else {
        panic!("COLOR_CONTEXT_USE was not extracted as a record");
    };
    let TypeRef::Pointer { target, .. } = &fields[0].ty else {
        panic!("COLOR_CONTEXT_USE.value was not extracted as a pointer");
    };
    let TypeRef::Named { declaration, .. } = target.as_ref() else {
        panic!("COLOR_CONTEXT_USE.value did not retain an interface declaration");
    };
    assert_eq!(declaration, &color_provider.spelling);

    let image_encoder = provider("IImageEncoder");
    let FactData::Interface { methods, .. } = &image_encoder.data else {
        unreachable!()
    };
    let TypeRef::Pointer { target, .. } = &methods[0].params[0].ty else {
        panic!("IImageEncoder.WriteFrame image was not extracted as a pointer");
    };
    let TypeRef::Named { declaration, .. } = target.as_ref() else {
        panic!("IImageEncoder.WriteFrame image did not retain an interface declaration");
    };
    assert_eq!(declaration, &image_provider.spelling);

    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            direct2d_aliases.to_string_lossy(),
            RootPartition::new("direct2d", "Example.Direct2D"),
        )
        .with_traversed_header(
            imaging_aliases.to_string_lossy(),
            RootPartition::new("imaging", "Example.Imaging"),
        )
        .with_traversed_header(
            imaging.to_string_lossy(),
            RootPartition::new("imaging", "Example.Imaging"),
        )
        .with_traversed_header(
            direct2d.to_string_lossy(),
            RootPartition::new("direct2d", "Example.Direct2D"),
        )
        .with_traversed_header(
            color_consumer.to_string_lossy(),
            RootPartition::new("direct2d", "Example.Direct2D"),
        )
        .with_traversed_header(
            encoder.to_string_lossy(),
            RootPartition::new("imaging-d2d", "Example.Imaging.D2D"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let authorities = NamespaceAuthorities::new().with_exact("IColorContext", "Example.Imaging");
    let plan = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();

    assert!(
        partitions
            .values()
            .all(|rdl| !rdl.contains("type IColorContext =") && !rdl.contains("type IImage =")),
        "{partitions:#?}"
    );
    let direct2d_rdl = output(&partitions, "Example.Direct2D");
    assert!(direct2d_rdl.contains("interface IImage"), "{direct2d_rdl}");
    assert!(
        direct2d_rdl.contains("value: Example::Imaging::IColorContext"),
        "{direct2d_rdl}"
    );
    assert!(direct2d_rdl.contains("type HLOCAL"), "{direct2d_rdl}");
    assert!(
        direct2d_rdl.contains("#[raii_free(\"CloseHandle\")]"),
        "{direct2d_rdl}"
    );
    let imaging_rdl = output(&partitions, "Example.Imaging");
    assert!(
        imaging_rdl.contains("interface IColorContext"),
        "{imaging_rdl}"
    );
    assert!(imaging_rdl.contains("type PLCID"), "{imaging_rdl}");
    assert!(
        imaging_rdl.contains("type IImageAlias = Example::Direct2D::IImage"),
        "{imaging_rdl}"
    );
    let encoder_rdl = output(&partitions, "Example.Imaging.D2D");
    assert!(
        encoder_rdl.contains(
            "fn WriteFrame(&self, image: Example::Direct2D::IImage, locale: \
             Example::Imaging::PLCID)"
        ),
        "{encoder_rdl}"
    );

    let winmd = scratch.join("interface-self-typedefs.winmd");
    windows_rdl::reader()
        .input_text(METADATA_RDL)
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    for (namespace, name) in [
        ("Example.Imaging", "IColorContext"),
        ("Example.Direct2D", "IImage"),
        ("Example.Imaging.D2D", "IImageEncoder"),
    ] {
        let interface = index.expect(namespace, name);
        assert_eq!(interface.category(), TypeCategory::Interface);
        assert_eq!(interface.methods().len(), 1);
        assert!(interface.find_attribute("GuidAttribute").is_some());
    }
    assert!(!index.contains("Example.Direct2D", "IColorContext"));
    assert!(!index.contains("Example.Imaging", "IImage"));
    assert!(
        index
            .expect("Example.Direct2D", "OWNED_HANDLE")
            .has_attribute("RAIIFreeAttribute")
    );
    assert!(
        index
            .expect("Example.Direct2D", "HLOCAL")
            .underlying_type()
            .is_some()
    );
    assert!(
        index
            .expect("Example.Imaging", "PLCID")
            .underlying_type()
            .is_some()
    );
    assert_eq!(
        index
            .expect("Example.Imaging", "IImageAlias")
            .underlying_type(),
        Some(Type::class_named("Example.Direct2D", "IImage"))
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn interface_self_typedef_identity_does_not_cross_source_declarations() {
    helpers::ensure_libclang();

    let scratch = scratch("distinct");
    let alias = scratch.join("alias.h");
    let provider = scratch.join("provider.h");
    let annotated = scratch.join("annotated.h");
    std::fs::write(
        &alias,
        "struct IShared;\n\
         typedef struct IShared IShared;\n\
         struct SHARED_USE { IShared *value; };\n",
    )
    .unwrap();
    std::fs::write(
        &provider,
        "struct __declspec(uuid(\"44444444-4444-4444-4444-444444444444\")) IShared {\n\
             virtual unsigned Method() = 0;\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &annotated,
        "#define interface struct\n\
         #define W32M(text) __attribute__((annotate(text)))\n\
         interface IAnnotated;\n\
         W32M(\"win32metadata:raii_free=ReleaseAnnotated\")\n\
         typedef interface IAnnotated IAnnotated;\n\
         struct __declspec(uuid(\"55555555-5555-5555-5555-555555555555\")) IAnnotated {\n\
             virtual unsigned Method() = 0;\n\
         };\n",
    )
    .unwrap();

    let roots = [scratch.to_string_lossy().to_string()];
    let snapshot = extract(
        [
            Input::new("alias.cpp", include(&alias)).with_root_dirs(roots.clone()),
            Input::new("provider.cpp", include(&provider)).with_root_dirs(roots.clone()),
            Input::new("annotated.cpp", include(&annotated)).with_root_dirs(roots),
        ],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();

    let shared_alias = snapshot
        .facts()
        .iter()
        .find(|fact| {
            fact.origin.tu == "alias.cpp"
                && fact.name == "IShared"
                && matches!(fact.data, FactData::Typedef { .. })
        })
        .unwrap_or_else(|| {
            panic!(
                "source-distinct IShared alias was suppressed:\n{}",
                snapshot.dump()
            )
        });
    let shared_use = snapshot
        .facts()
        .iter()
        .find(|fact| fact.origin.tu == "alias.cpp" && fact.name == "SHARED_USE")
        .unwrap();
    let FactData::Record { fields, .. } = &shared_use.data else {
        unreachable!()
    };
    let TypeRef::Pointer { target, .. } = &fields[0].ty else {
        unreachable!()
    };
    let TypeRef::Named { declaration, .. } = target.as_ref() else {
        unreachable!()
    };
    assert_eq!(declaration, &shared_alias.spelling);
    assert!(
        snapshot.facts().iter().any(|fact| {
            fact.origin.tu == "provider.cpp"
                && fact.name == "IShared"
                && fact.definition
                && matches!(fact.data, FactData::Interface { .. })
        }),
        "{}",
        snapshot.dump()
    );
    assert!(
        snapshot.facts().iter().any(|fact| {
            fact.origin.tu == "annotated.cpp"
                && fact.name == "IAnnotated"
                && matches!(fact.data, FactData::Typedef { .. })
        }),
        "annotated interface typedef was suppressed:\n{}",
        snapshot.dump()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn nested_interface_self_typedef_routes_to_provider_without_dangling_parent() {
    helpers::ensure_libclang();

    let scratch = scratch("nested-parent");
    let support = scratch.join("support.h");
    let alias = scratch.join("alias.h");
    let provider = scratch.join("provider.h");
    let consumer = scratch.join("consumer.h");
    std::fs::write(&support, "#pragma once\n#define interface struct\n").unwrap();
    std::fs::write(
        &alias,
        "#pragma once\n\
         #include \"support.h\"\n\
         typedef interface IFoo IFoo;\n",
    )
    .unwrap();
    std::fs::write(
        &provider,
        "#pragma once\n\
         #include \"support.h\"\n\
         struct __declspec(uuid(\"66666666-6666-6666-6666-666666666666\")) IFoo {\n\
             virtual unsigned GetValue() = 0;\n\
         };\n",
    )
    .unwrap();
    std::fs::write(
        &consumer,
        "#pragma once\n\
         #include \"alias.h\"\n\
         struct FOO_USE { IFoo *value; };\n",
    )
    .unwrap();

    let include_dir = format!("-I{}", scratch.display());
    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            [&alias, &provider, &consumer]
                .into_iter()
                .map(|path| include(path))
                .collect::<String>(),
        )
        .with_root_dirs([scratch.to_string_lossy().to_string()])],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
            include_dir.as_str(),
        ],
    )
    .unwrap();

    assert!(
        !snapshot
            .facts()
            .iter()
            .any(|fact| fact.name == "IFoo" && matches!(fact.data, FactData::Typedef { .. })),
        "{}",
        snapshot.dump()
    );
    let provider_fact = snapshot
        .facts()
        .iter()
        .find(|fact| {
            fact.name == "IFoo"
                && fact.definition
                && matches!(fact.data, FactData::Interface { .. })
        })
        .unwrap_or_else(|| panic!("IFoo provider was not extracted:\n{}", snapshot.dump()));
    let nested_forward = snapshot
        .facts()
        .iter()
        .find(|fact| {
            fact.name == "IFoo"
                && !fact.definition
                && matches!(fact.data, FactData::Interface { .. })
        })
        .unwrap_or_else(|| {
            panic!(
                "nested IFoo declaration was not extracted:\n{}",
                snapshot.dump()
            )
        });
    let origins = snapshot
        .facts()
        .iter()
        .map(|fact| &fact.origin)
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        snapshot.facts().iter().all(|fact| fact
            .parent
            .as_ref()
            .is_none_or(|parent| origins.contains(parent))),
        "{}",
        snapshot.dump()
    );
    assert!(nested_forward.parent.is_none(), "{nested_forward:#?}");

    let use_fact = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == "FOO_USE")
        .unwrap();
    let FactData::Record { fields, .. } = &use_fact.data else {
        panic!("FOO_USE was not extracted as a record");
    };
    let TypeRef::Pointer { target, .. } = &fields[0].ty else {
        panic!("FOO_USE.value was not extracted as a pointer");
    };
    let TypeRef::Named { declaration, .. } = target.as_ref() else {
        panic!("FOO_USE.value did not retain an interface declaration");
    };
    assert_eq!(declaration, &provider_fact.spelling);

    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header(
            alias.to_string_lossy(),
            RootPartition::new("alias", "Example.Alias"),
        )
        .with_traversed_header(
            provider.to_string_lossy(),
            RootPartition::new("provider", "Example.Provider"),
        )
        .with_traversed_header(
            consumer.to_string_lossy(),
            RootPartition::new("consumer", "Example.Consumer"),
        );
    let references = BTreeMap::from([(
        "EXTERNAL_TYPE".to_string(),
        TypeReference::new("Example.External", "EXTERNAL_TYPE", TypeReferenceKind::Type),
    )]);
    let options = EmitOptions::new("Example.Common", &references);
    let authorities = NamespaceAuthorities::new().with_exact("IFoo", "Example.Provider");
    let plan = snapshot
        .plan_header_partitions(&policy, &authorities)
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();

    assert!(
        !partitions
            .keys()
            .any(|partition| partition.namespace == "Example.Alias"),
        "{partitions:#?}"
    );
    let provider_rdl = output(&partitions, "Example.Provider");
    assert_eq!(provider_rdl.matches("interface IFoo").count(), 1);
    assert!(
        provider_rdl.contains("fn GetValue(&self) -> u32"),
        "{provider_rdl}"
    );
    let consumer_rdl = output(&partitions, "Example.Consumer");
    assert!(
        consumer_rdl.contains("value: Example::Provider::IFoo"),
        "{consumer_rdl}"
    );

    let winmd = scratch.join("nested-interface-self-typedef.winmd");
    windows_rdl::reader()
        .input_text(METADATA_RDL)
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    let interface = index.expect("Example.Provider", "IFoo");
    assert_eq!(interface.category(), TypeCategory::Interface);
    assert_eq!(interface.methods().len(), 1);
    assert!(interface.find_attribute("GuidAttribute").is_some());
    assert!(!index.contains("Example.Alias", "IFoo"));

    std::fs::remove_dir_all(scratch).unwrap();
}
