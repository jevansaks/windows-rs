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
    let rdl = dir.join("input.rdl");
    let winmd = dir.join("output.winmd");
    {
        let _guard = test_clang::libclang_guard();
        windows_clang::clang()
            .args([
                "-x",
                "c++",
                "--target=x86_64-pc-windows-msvc",
                "-fms-extensions",
            ])
            .input_text(source)
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
    assert!(text.contains("fn GetStatus() -> Windows::Win32::Foundation::NTSTATUS"));
    assert!(text.contains("fn GetBoolean() -> Windows::Win32::Foundation::BOOLEAN"));
    assert!(text.contains("Flag: Windows::Win32::Foundation::BOOLEAN"));

    windows_rdl::reader()
        .input(&rdl)
        .input_text(
            "#[win32] mod Windows { mod Win32 { mod Foundation {\n\
                 type BOOLEAN = u8;\n\
                 type NTSTATUS = i32;\n\
             } } }",
        )
        .input_text(windows_rdl::WIN32_METADATA_RDL)
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = metadata::reader::Index::read(&winmd).unwrap();
    assert_eq!(
        method(&index, "GetStatus").signature(&[]).return_type,
        metadata::Type::value_named("Windows.Win32.Foundation", "NTSTATUS")
    );
    assert_eq!(
        method(&index, "GetBoolean").signature(&[]).return_type,
        metadata::Type::value_named("Windows.Win32.Foundation", "BOOLEAN")
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
