use std::path::{Path, PathBuf};
use windows_metadata::{self as metadata, HasAttributes};

fn scratch(name: &str) -> PathBuf {
    let path = Path::new(env!("OUT_DIR"))
        .join("annotation_contracts")
        .join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn compile(name: &str, source: &str) -> (String, metadata::reader::Index) {
    let dir = scratch(name);
    let header = dir.join("input.h");
    let rdl = dir.join("input.rdl");
    let winmd = dir.join("output.winmd");
    std::fs::write(&header, source).unwrap();
    {
        let _guard = test_clang::libclang_guard();
        windows_clang::clang()
            .args([
                "-x",
                "c++",
                "--target=x86_64-pc-windows-msvc",
                "-fms-extensions",
            ])
            .input(&header)
            .namespace("Test")
            .library("test.dll")
            .output(&rdl)
            .write()
            .unwrap();
    }
    windows_rdl::reader()
        .input(&rdl)
        .input_text(windows_rdl::WIN32_METADATA_RDL)
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    (
        std::fs::read_to_string(rdl).unwrap(),
        metadata::reader::Index::read(&winmd).unwrap(),
    )
}

fn method<'a>(index: &'a metadata::reader::Index, name: &str) -> metadata::reader::MethodDef<'a> {
    index
        .expect("Test", "Apis")
        .methods()
        .find(|m| m.name() == name)
        .unwrap()
}

fn callback_method<'a>(
    index: &'a metadata::reader::Index,
    name: &str,
) -> metadata::reader::MethodDef<'a> {
    index
        .expect("Test", name)
        .methods()
        .find(|m| m.name() == "Invoke")
        .unwrap()
}

#[test]
fn callback_typedefs_preserve_source_sal() {
    let source = r#"
        #define CALLBACK __stdcall
        #define _In_
        #define _In_opt_
        #define _Inout_opt_
        #define _In_reads_bytes_(n)
        typedef unsigned long DWORD;
        typedef void *PVOID;
        typedef void *LPARAM;
        typedef int BOOLEAN;
        typedef struct POLICY { DWORD value; } POLICY, *PPOLICY;
        #define TEST_CONSTANT 7

        typedef BOOLEAN CALLBACK_V1(
            _In_ DWORD Index,
            _In_ DWORD NameSize,
            _In_reads_bytes_(NameSize) char *Name,
            _In_ DWORD DescriptionSize,
            _In_reads_bytes_(DescriptionSize) char *Description,
            _In_ PPOLICY Policy,
            _Inout_opt_ LPARAM Context);
        typedef CALLBACK_V1 *CALLBACK_ALIAS;

        #define ASSOCIATED(name) __attribute__((annotate("win32metadata:associated_enum=" #name)))
        #define SUPPORTED(version) __attribute__((annotate("win32metadata:supported_os=" version)))
        SUPPORTED("windows8.0")
        typedef ASSOCIATED(WIN32_ERROR) DWORD DEVICE_CALLBACK(
            _In_opt_ PVOID Context,
            _In_ DWORD Type,
            _In_ PVOID Setting);
        typedef DEVICE_CALLBACK *PDEVICE_CALLBACK;

        SUPPORTED("windows8.0")
        typedef struct RECORD { DWORD value; } RECORD, *PRECORD;
        SUPPORTED("windows8.0")
        typedef enum KIND { KIND_NONE = 0 } KIND, *PKIND;
    "#;

    let (_, index) = compile("callback_typedef_sal", source);

    let callback = callback_method(&index, "CALLBACK_V1");
    let signature = callback.signature(&[]);
    assert!(matches!(signature.types[2], metadata::Type::PtrMut(_, 1)));
    assert!(matches!(signature.types[4], metadata::Type::PtrMut(_, 1)));
    let params = callback.params_by_sequence(7).unwrap();
    assert_eq!(
        params.params()[2].unwrap().buffer_relationship(),
        Some(metadata::reader::BufferRelationship::BytesParam(1))
    );
    assert_eq!(
        params.params()[4].unwrap().buffer_relationship(),
        Some(metadata::reader::BufferRelationship::BytesParam(3))
    );
    assert_eq!(
        params.params()[5].unwrap().flags(),
        metadata::ParamAttributes::In
    );
    assert_eq!(
        params.params()[6].unwrap().flags(),
        metadata::ParamAttributes::In
            | metadata::ParamAttributes::Out
            | metadata::ParamAttributes::Optional
    );

    let callback = callback_method(&index, "DEVICE_CALLBACK");
    assert!(
        index
            .expect("Test", "DEVICE_CALLBACK")
            .has_attribute("SupportedOSPlatformAttribute")
    );
    let params = callback.params_by_sequence(3).unwrap();
    assert_eq!(
        params.params()[0].unwrap().flags(),
        metadata::ParamAttributes::In | metadata::ParamAttributes::Optional
    );
    assert_eq!(
        params.params()[2].unwrap().flags(),
        metadata::ParamAttributes::In
    );
    assert_eq!(
        params
            .return_param()
            .unwrap()
            .find_attribute("AssociatedEnumAttribute")
            .unwrap()
            .value(),
        [(
            String::new(),
            metadata::Value::Utf8("WIN32_ERROR".to_string())
        )]
    );
    for name in ["RECORD", "PRECORD", "KIND", "PKIND"] {
        assert!(
            index
                .expect("Test", name)
                .has_attribute("SupportedOSPlatformAttribute"),
            "{name}"
        );
    }
    let constant = index
        .expect("Test", "Apis")
        .fields()
        .find(|field| field.name() == "TEST_CONSTANT")
        .unwrap();
    assert!(
        constant
            .flags()
            .contains(metadata::FieldAttributes::HasDefault)
    );
    assert_eq!(
        constant.constant().unwrap().value(),
        metadata::Value::I32(7)
    );
}

#[test]
fn dependency_typedef_and_macro_return_identities_use_reference_metadata() {
    let dir = scratch("dependency_return_identities");
    let dependency = dir.join("dependency.h");
    let source = dir.join("input.h");
    let rdl = dir.join("input.rdl");
    let winmd = dir.join("output.winmd");
    std::fs::write(
        &dependency,
        "#define NTSTATUS LONG\n\
         typedef long LONG;\n\
         typedef unsigned char BOOLEAN;\n",
    )
    .unwrap();
    std::fs::write(
        &source,
        "#include \"dependency.h\"\n\
         typedef struct CAPABILITIES { BOOLEAN Flag; } CAPABILITIES;\n\
         extern \"C\" NTSTATUS GetStatus(void);\n\
         extern \"C\" BOOLEAN GetBoolean(void);\n\
         extern \"C\" void GetCapabilities(CAPABILITIES *value);\n",
    )
    .unwrap();
    {
        let _guard = test_clang::libclang_guard();
        windows_clang::clang()
            .args([
                "-x",
                "c++",
                "--target=x86_64-pc-windows-msvc",
                "-fms-extensions",
            ])
            .input(&source)
            .namespace("Test")
            .library("test.dll")
            .output(&rdl)
            .write()
            .unwrap();
    }

    let text = std::fs::read_to_string(&rdl).unwrap();
    assert!(text.contains("fn GetStatus() -> NTSTATUS"));
    assert!(text.contains("fn GetBoolean() -> BOOLEAN"));
    assert!(text.contains("Flag: BOOLEAN"));

    windows_rdl::reader()
        .input(&rdl)
        .input_text("#[win32] mod Test { type BOOLEAN = u8; type NTSTATUS = i32; }")
        .input_text(windows_rdl::WIN32_METADATA_RDL)
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = metadata::reader::Index::read(&winmd).unwrap();
    assert_eq!(
        method(&index, "GetStatus").signature(&[]).return_type,
        metadata::Type::value_named("Test", "NTSTATUS")
    );
    assert_eq!(
        method(&index, "GetBoolean").signature(&[]).return_type,
        metadata::Type::value_named("Test", "BOOLEAN")
    );
}

#[test]
fn sal_source_and_attributes_preserve_buffer_contracts() {
    for (name, annotation) in [
        ("empty_sal", ""),
        ("portable_sal", "__attribute__((annotate(text)))"),
        ("legacy_sal", "__attribute__((annotate(\"SAL_pre\")))"),
    ] {
        let source = format!(
            r#"
            #define SAL(text) {annotation}
            #define _In_ SAL("_In_")
            #define _Out_ SAL("_Out_")
            #define _Inout_ SAL("_Inout_")
            #define _In_reads_bytes_opt_(n) SAL("_In_reads_bytes_opt_(" #n ")")
            #define _Out_writes_bytes_opt_(n) SAL("_Out_writes_bytes_opt_(" #n ")")
            #define _Out_writes_to_opt_(n, written) SAL("_Out_writes_to_opt_(" #n "," #written ")")
            typedef unsigned long DWORD;
            typedef void *PVOID;
            typedef void *HANDLE;
            typedef struct {{ DWORD value; }} CAPABILITIES, *PCAPABILITIES;
            typedef struct {{ DWORD Data1; }} GUID;
            extern "C" long __stdcall Exchange(
                DWORD level,
                _In_reads_bytes_opt_(inputLength) PVOID input,
                DWORD inputLength,
                _Out_writes_bytes_opt_(outputLength) PVOID output,
                DWORD outputLength);
            static_assert(__is_same(decltype(&Exchange),
                long (__stdcall *)(DWORD, PVOID, DWORD, PVOID, DWORD)));
            void GetCapabilities(_Out_ PCAPABILITIES value);
            void GetScalar(_Out_ DWORD *value);
            void UpdateHandle(_Inout_ HANDLE handle);
            void GetPartial(_Out_writes_to_opt_(*count, *count) DWORD *values,
                _Inout_ DWORD *count);
            void Defaults(DWORD value, DWORD *writable, const DWORD *readonly,
                void *buffer, const void *constant);
            void Nested(void (*callback)(_Out_ DWORD *nested), DWORD plain);
            void CommentPriority(/* [out] */ _In_ DWORD *value);
            void NativeConstness(_In_ CAPABILITIES *record, _In_ DWORD *scalar,
                _In_ PVOID buffer, _In_ PCAPABILITIES alias,
                _In_ const DWORD *readonly, _In_ const GUID *guid);
            "#
        );
        let (rdl, index) = compile(name, &source);
        assert!(rdl.contains("#[size_param(2)]"));
        assert!(rdl.contains("#[size_param(4)]"));
        let exchange = method(&index, "Exchange");
        let signature = exchange.signature(&[]);
        assert_eq!(signature.return_type, metadata::Type::I32);
        assert_eq!(signature.types.len(), 5);
        // SAL projection constness is separate from the native signature asserted in the TU.
        assert_eq!(
            signature.types[1],
            metadata::Type::PtrMut(Box::new(metadata::Type::Void), 1)
        );
        assert_eq!(
            signature.types[3],
            metadata::Type::PtrMut(Box::new(metadata::Type::Void), 1)
        );
        let params = exchange.params_by_sequence(5).unwrap();
        for (position, flags, count) in [
            (1, metadata::ParamAttributes::In, 2),
            (3, metadata::ParamAttributes::Out, 4),
        ] {
            let param = params.params()[position].unwrap();
            assert_eq!(param.flags(), flags | metadata::ParamAttributes::Optional);
            assert_eq!(
                param.buffer_relationship(),
                Some(metadata::reader::BufferRelationship::BytesParam(count))
            );
        }
        for name in ["GetCapabilities", "GetScalar"] {
            assert_eq!(
                method(&index, name).params().next().unwrap().flags(),
                metadata::ParamAttributes::Out
            );
        }
        assert_eq!(
            method(&index, "UpdateHandle")
                .params()
                .next()
                .unwrap()
                .flags(),
            metadata::ParamAttributes::In | metadata::ParamAttributes::Out
        );
        let partial = method(&index, "GetPartial");
        let params = partial.params_by_sequence(2).unwrap();
        assert_eq!(
            params.params()[0].unwrap().buffer_relationship(),
            Some(metadata::reader::BufferRelationship::ElementsParam(1))
        );
        assert_eq!(
            params.params()[0].unwrap().flags(),
            metadata::ParamAttributes::Out | metadata::ParamAttributes::Optional
        );
        let defaults = method(&index, "Defaults");
        let flags: Vec<_> = defaults.params().map(|p| p.flags()).collect();
        assert_eq!(
            flags,
            [
                metadata::ParamAttributes::In,
                metadata::ParamAttributes::Out,
                metadata::ParamAttributes::In,
                metadata::ParamAttributes::Out,
                metadata::ParamAttributes::In,
            ]
        );
        assert!(defaults.params().all(|p| p.buffer_relationship().is_none()));
        let nested: Vec<_> = method(&index, "Nested")
            .params()
            .map(|p| p.flags())
            .collect();
        assert_eq!(nested[1], metadata::ParamAttributes::In);
        assert!(!nested[1].contains(metadata::ParamAttributes::Out));
        assert_eq!(
            method(&index, "CommentPriority")
                .params()
                .next()
                .unwrap()
                .flags(),
            metadata::ParamAttributes::In
        );
        let native = method(&index, "NativeConstness").signature(&[]).types;
        assert!(matches!(native[0], metadata::Type::PtrMut(_, 1)));
        assert_eq!(
            native[1],
            metadata::Type::PtrMut(Box::new(metadata::Type::U32), 1)
        );
        assert_eq!(
            native[2],
            metadata::Type::PtrMut(Box::new(metadata::Type::Void), 1)
        );
        assert!(matches!(native[3], metadata::Type::PtrMut(_, 1)));
        assert_eq!(
            native[4],
            metadata::Type::PtrConst(Box::new(metadata::Type::U32), 1)
        );
        assert!(matches!(native[5], metadata::Type::PtrConst(_, 1)));
    }
}

#[test]
fn associated_enum_targets_parameter_and_return_rows() {
    let (rdl, index) = compile(
        "associated_enum",
        r#"
        #define ASSOCIATED(name) __attribute__((annotate("win32metadata:associated_enum=" #name)))
        typedef unsigned long DWORD;
        typedef unsigned long ULONG;
        ASSOCIATED(ERROR_KIND) DWORD __stdcall Select(ASSOCIATED(FLAGS) DWORD *flags);
        static_assert(__is_same(decltype(&Select), DWORD (__stdcall *)(DWORD *)));
        enum FLAGS : unsigned long { FIRST = 1, SECOND = 2 };
        enum ERROR_KIND : ULONG { SUCCESS = 0, FAILURE = 5 };
        struct __declspec(uuid("12345678-1234-1234-1234-123456789abc")) ISelector {
            virtual ASSOCIATED(ERROR_KIND) DWORD Select(ASSOCIATED(FLAGS) DWORD flags) = 0;
        };
        "#,
    );
    assert!(rdl.contains("-> #[associated_enum(\"ERROR_KIND\")] u32"));
    assert!(rdl.contains("#[repr(u32)]\n    enum ERROR_KIND"));
    index.expect(
        "Windows.Win32.Foundation.Metadata",
        "AssociatedEnumAttribute",
    );
    assert_eq!(
        index.expect("Test", "FLAGS").category(),
        metadata::reader::TypeCategory::Enum
    );
    assert_eq!(
        index.expect("Test", "ERROR_KIND").category(),
        metadata::reader::TypeCategory::Enum
    );
    let error_kind = index.expect("Test", "ERROR_KIND");
    let value = error_kind
        .fields()
        .find(|field| field.name() == "value__")
        .unwrap();
    assert_eq!(value.ty(), metadata::Type::U32);
    assert_eq!(
        value.flags(),
        metadata::FieldAttributes::Public
            | metadata::FieldAttributes::SpecialName
            | metadata::FieldAttributes::RTSpecialName
    );
    for field in error_kind
        .fields()
        .filter(|field| field.name() != "value__")
    {
        assert_eq!(
            field.flags(),
            metadata::FieldAttributes::Public
                | metadata::FieldAttributes::Static
                | metadata::FieldAttributes::Literal
                | metadata::FieldAttributes::HasDefault
        );
    }
    for method in [
        method(&index, "Select"),
        index.expect("Test", "ISelector").methods().next().unwrap(),
    ] {
        assert_eq!(method.signature(&[]).return_type, metadata::Type::U32);
        assert!(!method.has_attribute("AssociatedEnumAttribute"));
        let params = method.params_by_sequence(1).unwrap();
        for (row, name) in [
            (params.return_param().unwrap(), "ERROR_KIND"),
            (params.params()[0].unwrap(), "FLAGS"),
        ] {
            assert_eq!(
                row.find_attribute("AssociatedEnumAttribute")
                    .unwrap()
                    .value(),
                [(String::new(), metadata::Value::Utf8(name.to_string()))]
            );
        }
    }
    assert_eq!(
        method(&index, "Select").signature(&[]).types,
        [metadata::Type::PtrMut(Box::new(metadata::Type::U32), 1)]
    );
}

#[test]
fn associated_constants_preserve_canonical_namespace_across_partition_units() {
    let dir = scratch("associated_constant_partitions");
    let foundation_header = dir.join("foundation.h");
    let foundation_rdl = dir.join("foundation.rdl");
    let setupapi_header = dir.join("setupapi.h");
    let setupapi_rdl = dir.join("setupapi.rdl");
    let registry_header = dir.join("registry.h");
    let registry_rdl = dir.join("registry.rdl");
    let winmd = dir.join("output.winmd");

    std::fs::write(
        &foundation_header,
        r#"
        #define ASSOCIATED_CONSTANT(name) \
            __attribute__((annotate("win32metadata:associated_constant=" #name)))
        #define ERROR_CORE 1
        #pragma push_macro("ERROR_CORE")
        #undef ERROR_CORE
        enum [[clang::flag_enum]]
            ASSOCIATED_CONSTANT(ERROR_SETUPAPI)
            ASSOCIATED_CONSTANT(ERROR_ABSENT)
            ASSOCIATED_CONSTANT(ERROR_REGISTRY)
            WIN32_ERROR : unsigned long {
                ERROR_CORE = 1,
        };
        #pragma pop_macro("ERROR_CORE")
        "#,
    )
    .unwrap();
    std::fs::write(&setupapi_header, "#define ERROR_SETUPAPI 2\n").unwrap();
    std::fs::write(&registry_header, "#define ERROR_REGISTRY 3\n").unwrap();

    let emit = |input: &Path, output: &Path, namespace: &str| {
        let _guard = test_clang::libclang_guard();
        windows_clang::clang()
            .args([
                "-x",
                "c++",
                "--target=x86_64-pc-windows-msvc",
                "-fms-extensions",
            ])
            .input(input)
            .namespace(namespace)
            .library("test.dll")
            .output(output)
            .write()
            .unwrap();
    };
    emit(
        &foundation_header,
        &foundation_rdl,
        "Windows.Win32.Foundation",
    );
    emit(&setupapi_header, &setupapi_rdl, "Windows.Win32.Foundation");
    emit(
        &registry_header,
        &registry_rdl,
        "Windows.Win32.System.Registry",
    );

    let foundation = std::fs::read_to_string(&foundation_rdl).unwrap();
    assert!(foundation.contains("mod Foundation"));
    assert!(foundation.contains("#[flags]"));
    for name in ["ERROR_SETUPAPI", "ERROR_ABSENT", "ERROR_REGISTRY"] {
        assert!(foundation.contains(&format!("#[associated_constant(\"{name}\")]")));
    }

    windows_rdl::reader()
        .input(&foundation_rdl)
        .input(&setupapi_rdl)
        .input(&registry_rdl)
        .input_text(windows_rdl::WIN32_METADATA_RDL)
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = metadata::reader::Index::read(&winmd).unwrap();
    let error = index.expect("Windows.Win32.Foundation", "WIN32_ERROR");
    assert_eq!(error.category(), metadata::reader::TypeCategory::Enum);
    assert!(error.has_attribute("FlagsAttribute"));

    let mut associated: Vec<_> = error
        .attributes()
        .filter(|attribute| attribute.name() == "AssociatedConstantAttribute")
        .map(|attribute| match attribute.value().as_slice() {
            [(name, metadata::Value::Utf8(value))] if name.is_empty() => value.clone(),
            value => panic!("unexpected associated constant value: {value:?}"),
        })
        .collect();
    associated.sort();
    assert_eq!(
        associated,
        ["ERROR_ABSENT", "ERROR_REGISTRY", "ERROR_SETUPAPI"]
    );

    let foundation_constants = index.expect("Windows.Win32.Foundation", "Apis");
    assert!(
        foundation_constants
            .fields()
            .any(|field| field.name() == "ERROR_SETUPAPI")
    );
    assert!(
        !foundation_constants
            .fields()
            .any(|field| field.name() == "ERROR_ABSENT")
    );
    assert!(
        !foundation_constants
            .fields()
            .any(|field| field.name() == "ERROR_REGISTRY")
    );
    assert!(
        index
            .expect("Windows.Win32.System.Registry", "Apis")
            .fields()
            .any(|field| field.name() == "ERROR_REGISTRY")
    );

    let bindings = dir.join("bindings.rs");
    windows_bindgen::bindgen([
        "--in",
        winmd.to_str().unwrap(),
        "--out",
        bindings.to_str().unwrap(),
        "--filter",
        "Windows.Win32.Foundation.WIN32_ERROR",
    ]);
    let bindings = std::fs::read_to_string(bindings).unwrap();
    assert!(bindings.contains("pub type WIN32_ERROR = u32;"));
    assert!(bindings.contains("pub const ERROR_CORE: WIN32_ERROR = 1;"));
    assert!(!bindings.contains("ERROR_SETUPAPI"));
    assert!(!bindings.contains("ERROR_ABSENT"));
    assert!(!bindings.contains("ERROR_REGISTRY"));

    let namespace_bindings = dir.join("namespace_bindings.rs");
    windows_bindgen::bindgen([
        "--in",
        winmd.to_str().unwrap(),
        "--out",
        namespace_bindings.to_str().unwrap(),
        "--filter",
        "Windows.Win32.Foundation",
    ]);
    let namespace_bindings = std::fs::read_to_string(namespace_bindings).unwrap();
    assert!(namespace_bindings.contains("pub const ERROR_SETUPAPI: i32 = 2;"));
    assert!(!namespace_bindings.contains("ERROR_ABSENT"));
    assert!(!namespace_bindings.contains("ERROR_REGISTRY"));
}

#[test]
fn rejects_malformed_or_misplaced_enum_associations() {
    let _guard = test_clang::libclang_guard();
    for (source, message) in [
        (
            r#"__attribute__((annotate("win32metadata:associated_enum"))) unsigned long Missing();"#,
            "requires a value",
        ),
        (
            r#"__attribute__((annotate("win32metadata:associated_enum="))) unsigned long Empty();"#,
            "requires a value",
        ),
        (
            r#"typedef int VALUE __attribute__((annotate("win32metadata:associated_enum=FLAGS")));"#,
            "is not valid on this declaration",
        ),
        (
            r#"__attribute__((annotate("win32metadata:associated_enum=FLAGS"))) void NoReturn();"#,
            "requires a non-void return",
        ),
    ] {
        let error = windows_clang::clang()
            .args(["-x", "c++", "--target=x86_64-pc-windows-msvc"])
            .input_text(source)
            .output(scratch("invalid").join("output.rdl"))
            .write()
            .unwrap_err();
        assert!(error.message.contains(message), "{error:?}");
    }
}
