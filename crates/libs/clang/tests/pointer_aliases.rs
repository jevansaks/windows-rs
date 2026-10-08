use std::collections::BTreeMap;
use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition,
    Snapshot, TypeRef, extract,
};
use windows_metadata::{Type, reader::Item};

const BASE_SOURCE: &str = r#"
typedef unsigned short WCHAR;
typedef WCHAR *PWSTR, *LPWSTR;
typedef const WCHAR *LPCWSTR, *PCWSTR;
typedef PCWSTR *PPCWSTR;
"#;

const API_SOURCE: &str = r#"
typedef struct POINTER_ALIAS_FIELDS {
    PWSTR *mutableMutable;
    PWSTR const *constMutable;
    PCWSTR *mutableConst;
    PCWSTR const *constConst;
    LPCWSTR *legacyMutableConst;
    PPCWSTR aliasChain;
} POINTER_ALIAS_FIELDS;

extern "C" void PointerAliasParameters(
    PWSTR *mutableMutable,
    PWSTR const *constMutable,
    PCWSTR *mutableConst,
    PCWSTR const *constConst,
    LPCWSTR *legacyMutableConst,
    PPCWSTR aliasChain);
"#;

const TCHAR_BASE_SOURCE: &str = r#"
typedef char CHAR;
typedef unsigned short WCHAR;
typedef CHAR *LPSTR, *PSTR;
typedef const CHAR *LPCSTR, *PCSTR;
typedef WCHAR *LPWSTR, *PWSTR;
typedef const WCHAR *LPCWSTR, *PCWSTR;
#ifdef UNICODE
typedef LPWSTR LPTSTR;
typedef LPCWSTR LPCTSTR;
#else
typedef LPSTR LPTSTR;
typedef LPCSTR LPCTSTR;
#endif
"#;

const TCHAR_API_SOURCE: &str = r#"
typedef struct PROVIDER_NAME {
    LPCTSTR value;
    LPTSTR mutableValue;
} PROVIDER_NAME;

typedef void (*OPEN_PROVIDER)(LPCTSTR provider, LPTSTR mutableProvider);
extern "C" void RegisterProvider(OPEN_PROVIDER callback);
"#;

#[test]
fn dependency_pointer_aliases_preserve_nested_constness() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-pointer-aliases-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let base = scratch.join("base.h");
    let api = scratch.join("api.h");
    std::fs::write(&base, BASE_SOURCE).unwrap();
    std::fs::write(
        &api,
        format!("#include \"{}\"\n{API_SOURCE}", base.to_string_lossy()),
    )
    .unwrap();

    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!("#include \"{}\"\n", api.to_string_lossy()),
        )
        .with_roots([api.to_string_lossy().to_string()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let legacy = snapshot.emit_with_library("Legacy", "test.dll").unwrap();
    assert!(!legacy.contains("type PCWSTR"));
    assert!(!legacy.contains("type PWSTR"));
    assert!(legacy.contains("mutableConst: *mut PCWSTR,"));

    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        api.to_string_lossy(),
        RootPartition::new("api", "Windows.Win32.Test"),
    );
    let references = windows_clang::MetadataReferences::new([windows_metadata::reader::File::new(
        windows_default::WINRT.to_vec(),
    )
    .unwrap()]);
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.library = Some("test.dll");
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let output = partitions.values().cloned().collect::<String>();

    assert_eq!(output.matches("type PCWSTR = *const u16;").count(), 1);
    assert_eq!(output.matches("type PWSTR = *mut u16;").count(), 1);
    assert_eq!(output.matches("type PPCWSTR = *mut PCWSTR;").count(), 1);
    assert!(!output.contains("type LPCWSTR"));
    assert!(output.contains("mutableMutable: *mut Windows::Win32::PWSTR,"));
    assert!(output.contains("constMutable: *const Windows::Win32::PWSTR,"));
    assert!(output.contains("mutableConst: *mut Windows::Win32::PCWSTR,"));
    assert!(output.contains("constConst: *const Windows::Win32::PCWSTR,"));
    assert!(output.contains("legacyMutableConst: *mut Windows::Win32::PCWSTR,"));
    assert!(
        output.contains("aliasChain: Windows::Win32::PPCWSTR,"),
        "{output}"
    );
    assert!(output.contains(
        "extern \"C\" fn PointerAliasParameters(mutableMutable: *mut Windows::Win32::PWSTR, \
         constMutable: *const Windows::Win32::PWSTR, mutableConst: *mut \
         Windows::Win32::PCWSTR, constConst: *const Windows::Win32::PCWSTR, \
         legacyMutableConst: *mut Windows::Win32::PCWSTR, aliasChain: \
         Windows::Win32::PPCWSTR);"
    ));

    let winmd = scratch.join("pointer-aliases.winmd");
    let mut compiler = windows_rdl::reader();
    for output in partitions.values() {
        compiler.input_text(output);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();

    assert_eq!(
        index.expect("Windows.Win32", "PWSTR").underlying_type(),
        Some(Type::PtrMut(Box::new(Type::U16), 1))
    );
    assert_eq!(
        index.expect("Windows.Win32", "PCWSTR").underlying_type(),
        Some(Type::PtrConst(Box::new(Type::U16), 1))
    );
    assert_eq!(
        index.expect("Windows.Win32", "PPCWSTR").underlying_type(),
        Some(pointer_to_alias(false, "PCWSTR"))
    );

    let record = index.expect("Windows.Win32.Test", "POINTER_ALIAS_FIELDS");
    let fields = record
        .fields()
        .map(|field| (field.name().to_string(), field.ty()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(fields["mutableMutable"], pointer_to_alias(false, "PWSTR"));
    assert_eq!(fields["constMutable"], pointer_to_alias(true, "PWSTR"));
    assert_eq!(fields["mutableConst"], pointer_to_alias(false, "PCWSTR"));
    assert_eq!(fields["constConst"], pointer_to_alias(true, "PCWSTR"));
    assert_eq!(
        fields["legacyMutableConst"],
        pointer_to_alias(false, "PCWSTR")
    );
    assert_eq!(
        fields["aliasChain"],
        Type::value_named("Windows.Win32", "PPCWSTR")
    );

    let function = index
        .iter_items()
        .find_map(|(namespace, name, item)| {
            (namespace == "Windows.Win32.Test" && name == "PointerAliasParameters").then_some(item)
        })
        .unwrap();
    let Item::Fn(function) = function else {
        panic!("PointerAliasParameters was not emitted as a function");
    };
    assert_eq!(
        function.signature(&[]).types,
        [
            pointer_to_alias(false, "PWSTR"),
            pointer_to_alias(true, "PWSTR"),
            pointer_to_alias(false, "PCWSTR"),
            pointer_to_alias(true, "PCWSTR"),
            pointer_to_alias(false, "PCWSTR"),
            Type::value_named("Windows.Win32", "PPCWSTR"),
        ]
    );

    let unsupported = scratch.join("unsupported.winmd");
    let error = windows_rdl::reader()
        .input_text("#[win32]\nmod Test {\n    struct BAD { value: *mut *const u16 }\n}\n")
        .output(&unsupported)
        .write()
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("mixed `*mut` and `*const` pointer chains are not representable"),
        "{error}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn retained_canonical_aliases_follow_their_emitted_namespace() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-canonical-alias-routes-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let base = scratch.join("base.h");
    let api = scratch.join("clusapi.h");
    std::fs::write(&base, BASE_SOURCE).unwrap();
    std::fs::write(
        &api,
        format!(
            "#include \"{}\"\n\
             typedef struct CLUSTER_BATCH_COMMAND {{\n\
                 LPCWSTR wzName;\n\
                 PCWSTR directName;\n\
                 PCWSTR *nestedName;\n\
             }} CLUSTER_BATCH_COMMAND;\n\
             extern \"C\" void UseNames(\n\
                 LPCWSTR legacyName,\n\
                 PCWSTR directName,\n\
                 PCWSTR *nestedName);\n",
            base.to_string_lossy()
        ),
    )
    .unwrap();

    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            format!("#include \"{}\"\n", api.to_string_lossy()),
        )
        .with_roots([api.to_string_lossy().to_string()])],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = windows_clang::MetadataReferences::new([windows_metadata::reader::File::new(
        windows_default::WINRT.to_vec(),
    )
    .unwrap()]);
    let mut options = EmitOptions::new("Windows.Win32", references.types());
    options.library = Some("test.dll");

    let cross_policy = HeaderPartitionPolicy::new().with_traversed_header(
        api.to_string_lossy(),
        RootPartition::new("clustering", "Windows.Win32.Networking.Clustering"),
    );
    let cross_partitions = snapshot
        .plan_header_partitions(&cross_policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let cross = cross_partitions.values().cloned().collect::<String>();
    assert_eq!(cross.matches("type PCWSTR = *const u16;").count(), 1);
    assert!(cross.contains("wzName: Windows::Win32::PCWSTR,"), "{cross}");
    assert!(
        cross.contains("directName: Windows::Win32::PCWSTR,"),
        "{cross}"
    );
    assert!(
        cross.contains("nestedName: *mut Windows::Win32::PCWSTR,"),
        "{cross}"
    );
    assert!(
        cross.contains(
            "fn UseNames(legacyName: Windows::Win32::PCWSTR, directName: \
             Windows::Win32::PCWSTR, nestedName: *mut Windows::Win32::PCWSTR)"
        ),
        "{cross}"
    );
    let cross_winmd = scratch.join("cross.winmd");
    windows_rdl::reader()
        .input_texts(cross_partitions.values())
        .reference_default()
        .output(&cross_winmd)
        .write()
        .unwrap();
    assert_canonical_alias_metadata(
        &windows_metadata::reader::Index::read(&cross_winmd).unwrap(),
        "Windows.Win32.Networking.Clustering",
    );

    let same_policy = HeaderPartitionPolicy::new().with_traversed_header(
        api.to_string_lossy(),
        RootPartition::new("clustering", "Windows.Win32"),
    );
    let same_partitions = snapshot
        .plan_header_partitions(&same_policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let same = same_partitions.values().cloned().collect::<String>();
    assert_eq!(same.matches("type PCWSTR = *const u16;").count(), 1);
    assert!(same.contains("wzName: PCWSTR,"), "{same}");
    assert!(same.contains("directName: PCWSTR,"), "{same}");
    assert!(same.contains("nestedName: *mut PCWSTR,"), "{same}");
    assert!(
        same.contains(
            "fn UseNames(legacyName: PCWSTR, directName: PCWSTR, nestedName: *mut PCWSTR)"
        ),
        "{same}"
    );
    assert!(!same.contains("Windows::Win32::PCWSTR"), "{same}");
    let same_winmd = scratch.join("same.winmd");
    windows_rdl::reader()
        .input_texts(same_partitions.values())
        .reference_default()
        .output(&same_winmd)
        .write()
        .unwrap();
    assert_canonical_alias_metadata(
        &windows_metadata::reader::Index::read(&same_winmd).unwrap(),
        "Windows.Win32",
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn canonical_pointer_alias_identity_survives_isolated_translation_units() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-isolated-canonical-alias-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let common = scratch.join("common.h");
    let foundation = scratch.join("foundation.h");
    let consumer = scratch.join("consumer.h");
    std::fs::write(&common, "typedef void *LPVOID;\n").unwrap();
    std::fs::write(&foundation, "typedef LPVOID *PFOUNDATION_POINTER;\n").unwrap();
    std::fs::write(
        &consumer,
        "typedef LPVOID CONSUMER_POINTER;\n\
         extern \"C\" CONSUMER_POINTER UseConsumerPointer(CONSUMER_POINTER value);\n\
         extern \"C\" void UseRawPointer(void *value);\n",
    )
    .unwrap();

    let include = |header: &std::path::Path| format!("#include \"{}\"\n", header.to_string_lossy());
    let roots = [scratch.to_string_lossy().to_string()];
    let aggregate = extract(
        [Input::new(
            "aggregate.cpp",
            format!(
                "{}{}{}",
                include(&common),
                include(&foundation),
                include(&consumer)
            ),
        )
        .with_root_dirs(roots.clone())],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let isolated = extract(
        [
            Input::new(
                "foundation.cpp",
                format!("{}{}", include(&common), include(&foundation)),
            )
            .with_root_dirs(roots.clone()),
            Input::new(
                "consumer.cpp",
                format!("{}{}", include(&common), include(&consumer)),
            )
            .with_root_dirs(roots),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();

    let aggregate_target = named_alias_target(&aggregate, "aggregate.cpp", "CONSUMER_POINTER");
    let isolated_target = named_alias_target(&isolated, "consumer.cpp", "CONSUMER_POINTER");
    assert_eq!(aggregate_target.0, "LPVOID");
    assert_eq!(isolated_target.0, "LPVOID");
    assert_eq!(aggregate_target.1, isolated_target.1);
    assert_eq!(
        aggregate_target.1.file,
        common.to_string_lossy().replace('\\', "/")
    );
    let isolated_common_aliases = isolated
        .facts()
        .iter()
        .filter(|fact| fact.name == "LPVOID")
        .collect::<Vec<_>>();
    assert_eq!(isolated_common_aliases.len(), 2);
    assert_eq!(
        isolated_common_aliases[0].spelling,
        isolated_common_aliases[1].spelling
    );
    assert_eq!(
        isolated_common_aliases[0].data,
        isolated_common_aliases[1].data
    );

    let foundation_partition = RootPartition::new("foundation", "Example.Foundation");
    let consumer_partition = RootPartition::new("consumer", "Example.Consumer");
    let aggregate_policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "aggregate.cpp",
            common.to_string_lossy(),
            foundation_partition.clone(),
        )
        .with_traversed_header_for_input(
            "aggregate.cpp",
            foundation.to_string_lossy(),
            foundation_partition.clone(),
        )
        .with_traversed_header_for_input(
            "aggregate.cpp",
            consumer.to_string_lossy(),
            consumer_partition.clone(),
        );
    let isolated_policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "foundation.cpp",
            common.to_string_lossy(),
            foundation_partition.clone(),
        )
        .with_traversed_header_for_input(
            "foundation.cpp",
            foundation.to_string_lossy(),
            foundation_partition,
        )
        .with_traversed_header_for_input(
            "consumer.cpp",
            consumer.to_string_lossy(),
            consumer_partition,
        );
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Example.Common", &references);
    options.library = Some("test.dll");
    let emit = |snapshot: Snapshot, policy: &HeaderPartitionPolicy| {
        let plan = snapshot
            .plan_header_partitions(policy, &NamespaceAuthorities::new())
            .unwrap();
        assert!(plan.audit(&options).unwrap().is_clean());
        plan.emit_with_options(&options).unwrap()
    };
    let aggregate_partitions = emit(aggregate, &aggregate_policy);
    let isolated_partitions = emit(isolated, &isolated_policy);
    assert_eq!(isolated_partitions, aggregate_partitions);

    let foundation_rdl = isolated_partitions
        .values()
        .find(|rdl| rdl.contains("type LPVOID"))
        .unwrap()
        .as_str();
    assert!(
        foundation_rdl.contains("type LPVOID = *mut void;"),
        "{foundation_rdl}"
    );
    let consumer_rdl = isolated_partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Consumer")
        .unwrap()
        .1;
    assert!(
        consumer_rdl.contains("type CONSUMER_POINTER = Example::Foundation::LPVOID;"),
        "{consumer_rdl}"
    );
    assert!(
        consumer_rdl.contains("fn UseRawPointer(value: *mut void)"),
        "{consumer_rdl}"
    );

    let winmd = scratch.join("isolated-canonical-alias.winmd");
    windows_rdl::reader()
        .input_texts(isolated_partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    assert_eq!(
        index
            .expect("Example.Consumer", "CONSUMER_POINTER")
            .underlying_type(),
        Some(Type::value_named("Example.Foundation", "LPVOID"))
    );
    let Item::Fn(use_raw_pointer) = index.expect_item("Example.Consumer", "UseRawPointer") else {
        panic!("UseRawPointer was not emitted as a function");
    };
    assert_eq!(
        use_raw_pointer.signature(&[]).types,
        [Type::PtrMut(Box::new(Type::Void), 1)]
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn canonical_pointer_alias_identity_requires_equivalent_source_declarations() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-variant-canonical-alias-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let common = scratch.join("common.h");
    let foundation = scratch.join("foundation.h");
    let consumer = scratch.join("consumer.h");
    std::fs::write(&common, "typedef POINTER_TARGET *LPVOID;\n").unwrap();
    std::fs::write(&foundation, "typedef LPVOID *PFOUNDATION_POINTER;\n").unwrap();
    std::fs::write(&consumer, "typedef LPVOID CONSUMER_POINTER;\n").unwrap();
    let include = |header: &std::path::Path| format!("#include \"{}\"\n", header.to_string_lossy());
    let roots = [scratch.to_string_lossy().to_string()];
    let snapshot = extract(
        [
            Input::new(
                "foundation.cpp",
                format!(
                    "#define POINTER_TARGET void\n{}{}",
                    include(&common),
                    include(&foundation)
                ),
            )
            .with_root_dirs(roots.clone()),
            Input::new(
                "consumer.cpp",
                format!(
                    "#define POINTER_TARGET unsigned long\n{}{}",
                    include(&common),
                    include(&consumer)
                ),
            )
            .with_root_dirs(roots),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let consumer_target = named_alias_target(&snapshot, "consumer.cpp", "CONSUMER_POINTER");
    assert_eq!(consumer_target.0, "LPVOID");
    assert_eq!(
        consumer_target.1.file,
        common.to_string_lossy().replace('\\', "/")
    );
    let common_aliases = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.name == "LPVOID")
        .collect::<Vec<_>>();
    assert_eq!(common_aliases.len(), 2);
    assert_eq!(common_aliases[0].spelling, common_aliases[1].spelling);
    assert_ne!(common_aliases[0].data, common_aliases[1].data);

    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "foundation.cpp",
            common.to_string_lossy(),
            RootPartition::new("foundation", "Example.Foundation"),
        )
        .with_traversed_header_for_input(
            "foundation.cpp",
            foundation.to_string_lossy(),
            RootPartition::new("foundation", "Example.Foundation"),
        )
        .with_traversed_header_for_input(
            "consumer.cpp",
            consumer.to_string_lossy(),
            RootPartition::new("consumer", "Example.Consumer"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let consumer_rdl = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Consumer")
        .unwrap()
        .1;
    assert!(
        consumer_rdl.contains("type CONSUMER_POINTER = *mut void;"),
        "{consumer_rdl}"
    );
    assert!(!consumer_rdl.contains("Example::Foundation::LPVOID"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn canonical_pointer_alias_identity_requires_matching_native_scope() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-scoped-canonical-alias-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let common = scratch.join("common.h");
    let foundation = scratch.join("foundation.h");
    let consumer = scratch.join("consumer.h");
    std::fs::write(&common, "typedef void *LPVOID;\n").unwrap();
    std::fs::write(&foundation, "typedef A::LPVOID *PFOUNDATION_POINTER;\n").unwrap();
    std::fs::write(&consumer, "typedef B::LPVOID CONSUMER_POINTER;\n").unwrap();
    let include = |header: &std::path::Path| format!("#include \"{}\"\n", header.to_string_lossy());
    let roots = [scratch.to_string_lossy().to_string()];
    let snapshot = extract(
        [
            Input::new(
                "foundation.cpp",
                format!(
                    "namespace A {{\n{}}}\n{}",
                    include(&common),
                    include(&foundation)
                ),
            )
            .with_root_dirs(roots.clone()),
            Input::new(
                "consumer.cpp",
                format!(
                    "namespace B {{\n{}}}\n{}",
                    include(&common),
                    include(&consumer)
                ),
            )
            .with_root_dirs(roots),
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    assert_eq!(fact_parent_name(&snapshot, "foundation.cpp", "LPVOID"), "A");
    assert_eq!(fact_parent_name(&snapshot, "consumer.cpp", "LPVOID"), "B");

    let policy = HeaderPartitionPolicy::new()
        .with_traversed_header_for_input(
            "foundation.cpp",
            common.to_string_lossy(),
            RootPartition::new("foundation", "Example.Foundation"),
        )
        .with_traversed_header_for_input(
            "foundation.cpp",
            foundation.to_string_lossy(),
            RootPartition::new("foundation", "Example.Foundation"),
        )
        .with_traversed_header_for_input(
            "consumer.cpp",
            consumer.to_string_lossy(),
            RootPartition::new("consumer", "Example.Consumer"),
        );
    let references = BTreeMap::new();
    let options = EmitOptions::new("Example.Common", &references);
    let plan = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap();
    assert!(plan.audit(&options).unwrap().is_clean());
    let partitions = plan.emit_with_options(&options).unwrap();
    let consumer_rdl = partitions
        .iter()
        .find(|(partition, _)| partition.namespace == "Example.Consumer")
        .unwrap()
        .1;
    assert!(
        consumer_rdl.contains("type CONSUMER_POINTER = *mut void;"),
        "{consumer_rdl}"
    );
    assert!(!consumer_rdl.contains("Example::Foundation::LPVOID"));

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn direct_tchar_aliases_are_retained_or_resolved_from_metadata() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-direct-tchar-aliases-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let base = scratch.join("base.h");
    let api = scratch.join("resapi.h");
    std::fs::write(&base, TCHAR_BASE_SOURCE).unwrap();
    std::fs::write(
        &api,
        format!(
            "#include \"{}\"\n{TCHAR_API_SOURCE}",
            base.to_string_lossy()
        ),
    )
    .unwrap();

    let external_winmd = scratch.join("external.winmd");
    windows_rdl::reader()
        .input_text(
            "#[win32]\nmod External {\n    type PCSTR = *const i8;\n    type PSTR = *mut i8;\n    \
             type PCWSTR = *const u16;\n    type PWSTR = *mut u16;\n}\n",
        )
        .output(&external_winmd)
        .write()
        .unwrap();
    let external_references =
        windows_clang::MetadataReferences::new([windows_metadata::reader::File::new(
            std::fs::read(&external_winmd).unwrap(),
        )
        .unwrap()]);
    let no_references = BTreeMap::new();

    for (case, define, alias, mutable_alias, primitive) in [
        ("ansi", None, "PCSTR", "PSTR", "i8"),
        ("unicode", Some("-DUNICODE"), "PCWSTR", "PWSTR", "u16"),
    ] {
        let mut args = vec!["-x", "c++", "--target=x86_64-pc-windows-msvc"];
        if let Some(define) = define {
            args.push(define);
        }
        let snapshot = extract(
            [Input::new(
                format!("aggregate-{case}.cpp"),
                format!("#include \"{}\"\n", api.to_string_lossy()),
            )
            .with_roots([api.to_string_lossy().to_string()])],
            &args,
        )
        .unwrap();
        let policy = HeaderPartitionPolicy::new().with_traversed_header(
            api.to_string_lossy(),
            RootPartition::new("clustering", "Windows.Win32.Networking.Clustering"),
        );

        let mut local_options = EmitOptions::new("Windows.Win32", &no_references);
        local_options.library = Some("test.dll");
        let local_partitions = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap()
            .emit_with_options(&local_options)
            .unwrap();
        let local = local_partitions.values().cloned().collect::<String>();
        assert_eq!(
            local
                .matches(&format!("type {alias} = *const {primitive};"))
                .count(),
            1,
            "{local}"
        );
        assert_eq!(
            local
                .matches(&format!("type {mutable_alias} = *mut {primitive};"))
                .count(),
            1,
            "{local}"
        );
        assert!(
            local.contains(&format!("value: Windows::Win32::{alias},")),
            "{local}"
        );
        assert!(
            local.contains(&format!("mutableValue: Windows::Win32::{mutable_alias},")),
            "{local}"
        );
        assert!(
            local.contains(&format!(
                "extern \"C\" fn OPEN_PROVIDER(provider: Windows::Win32::{alias}, \
                 mutableProvider: Windows::Win32::{mutable_alias})"
            )),
            "{local}"
        );
        let local_winmd = scratch.join(format!("{case}-local.winmd"));
        windows_rdl::reader()
            .input_texts(local_partitions.values())
            .reference_default()
            .output(&local_winmd)
            .write()
            .unwrap();
        assert_tchar_alias_metadata(
            &windows_metadata::reader::Index::read(&local_winmd).unwrap(),
            "Windows.Win32",
            alias,
            mutable_alias,
        );

        let mut external_options = EmitOptions::new("Windows.Win32", external_references.types());
        external_options.library = Some("test.dll");
        let external_partitions = snapshot
            .plan_header_partitions(&policy, &NamespaceAuthorities::new())
            .unwrap()
            .emit_with_options(&external_options)
            .unwrap();
        let external = external_partitions.values().cloned().collect::<String>();
        assert!(!external.contains(&format!("type {alias} =")), "{external}");
        assert!(
            !external.contains(&format!("type {mutable_alias} =")),
            "{external}"
        );
        assert!(
            external.contains(&format!("value: External::{alias},")),
            "{external}"
        );
        assert!(
            external.contains(&format!("mutableValue: External::{mutable_alias},")),
            "{external}"
        );
        assert!(
            external.contains(&format!(
                "extern \"C\" fn OPEN_PROVIDER(provider: External::{alias}, \
                 mutableProvider: External::{mutable_alias})"
            )),
            "{external}"
        );
        let external_output = scratch.join(format!("{case}-external.winmd"));
        windows_rdl::reader()
            .input_texts(external_partitions.values())
            .reference(&external_winmd)
            .reference_default()
            .output(&external_output)
            .write()
            .unwrap();
        assert_tchar_alias_metadata(
            &windows_metadata::reader::Index::read(&external_output).unwrap(),
            "External",
            alias,
            mutable_alias,
        );
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

fn assert_canonical_alias_metadata(index: &windows_metadata::reader::Index, namespace: &str) {
    let alias = Type::value_named("Windows.Win32", "PCWSTR");
    let record = index.expect(namespace, "CLUSTER_BATCH_COMMAND");
    let fields = record
        .fields()
        .map(|field| (field.name().to_string(), field.ty()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(fields["wzName"], alias);
    assert_eq!(fields["directName"], alias);
    assert_eq!(fields["nestedName"], pointer_to_alias(false, "PCWSTR"));

    let Item::Fn(function) = index.expect_item(namespace, "UseNames") else {
        panic!("UseNames was not emitted as a function");
    };
    assert_eq!(
        function.signature(&[]).types,
        [
            Type::value_named("Windows.Win32", "PCWSTR"),
            Type::value_named("Windows.Win32", "PCWSTR"),
            pointer_to_alias(false, "PCWSTR"),
        ]
    );
}

fn assert_tchar_alias_metadata(
    index: &windows_metadata::reader::Index,
    alias_namespace: &str,
    alias: &str,
    mutable_alias: &str,
) {
    let fields = index
        .expect("Windows.Win32.Networking.Clustering", "PROVIDER_NAME")
        .fields()
        .map(|field| (field.name().to_string(), field.ty()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(fields["value"], Type::value_named(alias_namespace, alias));
    assert_eq!(
        fields["mutableValue"],
        Type::value_named(alias_namespace, mutable_alias)
    );
}

fn pointer_to_alias(outer_const: bool, name: &str) -> Type {
    let target = Box::new(Type::value_named("Windows.Win32", name));
    if outer_const {
        Type::PtrConst(target, 1)
    } else {
        Type::PtrMut(target, 1)
    }
}

fn named_alias_target<'a>(
    snapshot: &'a Snapshot,
    translation_unit: &str,
    alias: &str,
) -> (&'a str, &'a windows_clang::Location) {
    let fact = snapshot
        .facts()
        .iter()
        .find(|fact| fact.origin.tu == translation_unit && fact.name == alias)
        .unwrap();
    let FactData::Typedef {
        target: TypeRef::Named { name, declaration },
    } = &fact.data
    else {
        panic!("{alias} did not retain a named alias target");
    };
    (name, declaration)
}

fn fact_parent_name<'a>(snapshot: &'a Snapshot, translation_unit: &str, name: &str) -> &'a str {
    let fact = snapshot
        .facts()
        .iter()
        .find(|fact| fact.origin.tu == translation_unit && fact.name == name)
        .unwrap();
    let parent = fact.parent.as_ref().unwrap();
    &snapshot
        .facts()
        .iter()
        .find(|fact| &fact.origin == parent)
        .unwrap()
        .name
}
