use std::collections::BTreeMap;
use windows_clang::{EmitOptions, Input, extract};
use windows_metadata::HasAttributes;

const METADATA_RDL: &str = include_str!("../../../../metadata/metadata.rdl");

fn attribute_strings<'a>(item: impl HasAttributes<'a>, name: &str) -> Vec<String> {
    item.attributes()
        .filter(|attribute| attribute.name() == name)
        .map(|attribute| {
            let values = attribute.value();
            let windows_metadata::Value::Utf8(value) = &values[0].1 else {
                panic!("{name} did not contain a string");
            };
            value.clone()
        })
        .collect()
}

#[test]
fn annotations_compile_to_final_metadata() {
    helpers::ensure_libclang();

    let source = r#"
        #define W32M(text) __attribute__((annotate(text)))
        typedef long HRESULT;
        typedef void* HANDLE;
        #define INVALID_HANDLE_VALUE ((HANDLE)(__int64)-1)
        #define EXTRA_FLAG 4

        enum W32M("win32metadata:associated_constant=EXTRA_FLAG")
             W32M("win32metadata:supported_os=windows10.0.26100")
             FLAGS : unsigned long {
            FLAGS_NONE = 0,
            FLAGS_ONE = 1,
            FLAGS_TWO W32M("win32metadata:associated_enum=FLAGS") = 2,
        };

        W32M("win32metadata:also_usable_for=HANDLE")
        W32M("win32metadata:raii_free=CloseHandle, INVALID_HANDLE_VALUE, 0")
        W32M("win32metadata:supported_os=windows10.0.26100")
        typedef HANDLE RESOURCE_HANDLE;

        struct W32M("win32metadata:supported_os=windows10.0.26100") RECORD {
            W32M("win32metadata:associated_enum=FLAGS")
            unsigned long flags;
        };

        W32M("win32metadata:set_last_error")
        W32M("win32metadata:import_library=override.dll")
        W32M("win32metadata:preserve_result")
        W32M("win32metadata:supported_os=windows10.0.26100")
        extern "C" HRESULT CreateResource(
            W32M("win32metadata:raii_free=CloseHandle, INVALID_HANDLE_VALUE, 0")
            W32M("win32metadata:retained")
            W32M("win32metadata:retval")
            HANDLE* value);

        W32M("win32metadata:raii_free=CloseHandle, INVALID_HANDLE_VALUE, 0")
        extern "C" HANDLE OpenResource();

        extern "C" void Direction(
            W32M("win32metadata:in")
            W32M("win32metadata:out")
            void* value);

        struct
            W32M("win32metadata:supported_os=windows10.0.26100")
            __declspec(uuid("12345678-1234-1234-1234-123456789abc"))
            IAnnotated {
            virtual
                W32M("win32metadata:preserve_result")
                W32M("win32metadata:associated_enum=FLAGS")
                unsigned long GetFlags(
                    W32M("win32metadata:retval")
                    unsigned long* value) = 0;
        };
    "#;
    let snapshot = extract(
        [Input::new("annotations.hpp", source)],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("fallback.dll");
    let rdl = snapshot.emit_with_options(&options).unwrap();

    assert!(
        rdl.contains("#[library(\"override.dll\", set_last_error)]"),
        "{rdl}"
    );
    assert!(rdl.contains("#[preserve_sig]"), "{rdl}");
    assert!(rdl.contains("#[raii_free(\"CloseHandle\")]"), "{rdl}");
    assert!(rdl.contains("#[invalid_handle(-1)]"), "{rdl}");
    assert!(rdl.contains("#[invalid_handle(0)]"), "{rdl}");
    assert!(rdl.contains("#[associated_constant(\"EXTRA_FLAG\")]"));
    assert!(rdl.contains("const EXTRA_FLAG: i32 = 4;"), "{rdl}");

    let output = std::env::temp_dir().join(format!(
        "windows-clang-annotations-{}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_text(METADATA_RDL)
        .input_text(&rdl)
        .output(&output)
        .write()
        .unwrap_or_else(|error| panic!("{error}\n{rdl}"));
    let index = windows_metadata::reader::Index::read(&output).unwrap();

    let windows_metadata::reader::Item::Fn(create) = index.expect_item("Test", "CreateResource")
    else {
        panic!("CreateResource was not emitted as a function");
    };
    let import = create.impl_map().unwrap();
    assert_eq!(import.import_scope().name(), "override.dll");
    assert!(
        import
            .flags()
            .contains(windows_metadata::PInvokeAttributes::SupportsLastError)
    );
    assert!(
        create
            .impl_flags()
            .contains(windows_metadata::MethodImplAttributes::PreserveSig)
    );
    assert_eq!(
        attribute_strings(create, "SupportedOSPlatformAttribute"),
        ["windows10.0.26100"]
    );
    let params = create.params_by_sequence(1).unwrap();
    let [Some(value)] = params.params() else {
        panic!("CreateResource parameter metadata is incomplete");
    };
    assert!(value.has_attribute("RAIIFreeAttribute"));
    assert!(value.has_attribute("RetainedAttribute"));
    assert!(value.has_attribute("RetValAttribute"));
    let mut invalid: Vec<_> = value
        .attributes()
        .filter(|attribute| attribute.name() == "InvalidHandleValueAttribute")
        .map(|attribute| attribute.value()[0].1.clone())
        .collect();
    invalid.sort_by_key(|value| match value {
        windows_metadata::Value::I64(value) => *value,
        _ => i64::MAX,
    });
    assert_eq!(
        invalid,
        [
            windows_metadata::Value::I64(-1),
            windows_metadata::Value::I64(0)
        ]
    );

    let resource = index.expect("Test", "RESOURCE_HANDLE");
    assert!(resource.has_attribute("RAIIFreeAttribute"));
    assert!(resource.has_attribute("AlsoUsableForAttribute"));
    assert_eq!(
        attribute_strings(resource, "SupportedOSPlatformAttribute"),
        ["windows10.0.26100"]
    );

    let windows_metadata::reader::Item::Fn(direction) = index.expect_item("Test", "Direction")
    else {
        panic!("Direction was not emitted as a function");
    };
    assert_eq!(
        direction.params().next().unwrap().direction(),
        windows_metadata::reader::ParamDirection::InputOutput
    );

    let flags = index.expect("Test", "FLAGS");
    assert!(flags.has_attribute("AssociatedConstantAttribute"));
    assert_eq!(
        attribute_strings(flags, "SupportedOSPlatformAttribute"),
        ["windows10.0.26100"]
    );
    assert!(
        flags
            .fields()
            .find(|field| field.name() == "FLAGS_TWO")
            .unwrap()
            .has_attribute("AssociatedEnumAttribute")
    );

    let record = index.expect("Test", "RECORD");
    let field = record
        .fields()
        .find(|field| field.name() == "flags")
        .unwrap();
    assert!(field.has_attribute("AssociatedEnumAttribute"));

    let interface = index.expect("Test", "IAnnotated");
    let method = interface
        .methods()
        .find(|method| method.name() == "GetFlags")
        .unwrap();
    assert!(
        method
            .impl_flags()
            .contains(windows_metadata::MethodImplAttributes::PreserveSig)
    );
    let params = method.params_by_sequence(1).unwrap();
    assert!(
        params
            .return_param()
            .unwrap()
            .has_attribute("AssociatedEnumAttribute")
    );
    assert!(params.params()[0].unwrap().has_attribute("RetValAttribute"));

    std::fs::remove_file(output).unwrap();
}

#[test]
fn malformed_annotations_are_errors() {
    helpers::ensure_libclang();

    for (name, annotation, error) in [
        (
            "unknown",
            "win32metadata:not_supported",
            "unknown win32metadata annotation `not_supported`",
        ),
        (
            "missing",
            "win32metadata:import_library",
            "win32metadata annotation `import_library` requires a value",
        ),
        (
            "valued",
            "win32metadata:preserve_result=yes",
            "win32metadata annotation `preserve_result` does not accept a value",
        ),
        (
            "unsupported",
            "win32metadata:reduce_pointer_level",
            "unknown win32metadata annotation `reduce_pointer_level`",
        ),
    ] {
        let source =
            format!("__attribute__((annotate(\"{annotation}\"))) extern \"C\" int Invalid();");
        let result = extract(
            [Input::new(format!("{name}.hpp"), source)],
            &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
        )
        .unwrap_err();
        assert!(result.to_string().contains(error), "{result}");
    }

    let result = extract(
        [Input::new(
            "misplaced.hpp",
            r#"
                __attribute__((annotate("win32metadata:agile")))
                extern "C" int Invalid();
            "#,
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap_err();
    assert!(
        result
            .to_string()
            .contains("win32metadata annotation `agile` is not valid on this declaration"),
        "{result}"
    );
}

#[test]
fn compatibility_annotations_compile_to_final_metadata() {
    helpers::ensure_libclang();

    let source = r#"
        #define W32M(text) __attribute__((annotate(text)))
        typedef long HRESULT;
        enum FLAGS : unsigned long { FLAGS_NONE = 0 };

        struct
            W32M("win32metadata:agile")
            W32M("win32metadata:native_inheritance=BASE")
            W32M("win32metadata:struct_size_field=size")
            EXTENDED_RECORD {
            unsigned long size;
            W32M("win32metadata:array_count_field=count")
            W32M("win32metadata:const")
            int* values;
            unsigned long count;
            W32M("win32metadata:native_encoding=utf-8")
            W32M("win32metadata:ansi")
            char* text;
            union {
                W32M("win32metadata:associated_enum=FLAGS")
                unsigned long nested_flags;
            };
        };

        W32M("win32metadata:associated_enum=FLAGS")
        const float FLOAT_VALUE = 1.0f;

        W32M("win32metadata:static_library=extended.lib")
        W32M("win32metadata:preserve_result")
        W32M("win32metadata:can_return_errors_as_success")
        W32M("win32metadata:can_return_multiple_success_values")
        W32M("win32metadata:ansi")
        extern "C" HRESULT Extended(
            W32M("win32metadata:array_count_param=1")
            W32M("win32metadata:memory_size_param=2")
            W32M("win32metadata:free_with=FreeBuffer")
            W32M("win32metadata:do_not_release")
            W32M("win32metadata:not_null_terminated")
            W32M("win32metadata:optional")
            W32M("win32metadata:in")
            W32M("win32metadata:const")
            void* value,
            unsigned long count,
            unsigned long bytes,
            W32M("win32metadata:ignore_if_return=-1")
            W32M("win32metadata:out")
            void** result);

        W32M("win32metadata:unicode")
        extern "C" int ExtendedW();
    "#;
    let snapshot = extract(
        [Input::new("compatibility.hpp", source)],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("test.dll");
    let rdl = snapshot.emit_with_options(&options).unwrap();
    let output = std::env::temp_dir().join(format!(
        "windows-clang-compatibility-{}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_text(METADATA_RDL)
        .input_text(&rdl)
        .output(&output)
        .write()
        .unwrap_or_else(|error| panic!("{error}\n{rdl}"));
    let index = windows_metadata::reader::Index::read(&output).unwrap();

    let record = index.expect("Test", "EXTENDED_RECORD");
    assert!(record.has_attribute("AgileAttribute"));
    assert!(record.has_attribute("NativeInheritanceAttribute"));
    assert!(record.has_attribute("StructSizeFieldAttribute"));
    let values = record
        .fields()
        .find(|field| field.name() == "values")
        .unwrap();
    assert!(values.has_attribute("NativeArrayInfoAttribute"));
    assert!(values.has_attribute("ConstAttribute"));
    let text = record
        .fields()
        .find(|field| field.name() == "text")
        .unwrap();
    assert!(text.has_attribute("NativeEncodingAttribute"));
    assert!(text.has_attribute("AnsiAttribute"));
    let nested = index
        .nested(record)
        .flat_map(|ty| ty.fields())
        .find(|field| field.name() == "nested_flags")
        .unwrap();
    assert!(nested.has_attribute("AssociatedEnumAttribute"));
    let windows_metadata::reader::Item::Const(value) = index.expect_item("Test", "FLOAT_VALUE")
    else {
        panic!("FLOAT_VALUE was not emitted as a constant");
    };
    assert!(value.has_attribute("AssociatedEnumAttribute"));

    let windows_metadata::reader::Item::Fn(function) = index.expect_item("Test", "Extended") else {
        panic!("Extended was not emitted as a function");
    };
    assert!(function.has_attribute("StaticLibraryAttribute"));
    assert!(function.has_attribute("CanReturnErrorsAsSuccessAttribute"));
    assert!(function.has_attribute("CanReturnMultipleSuccessValuesAttribute"));
    assert!(function.has_attribute("AnsiAttribute"));
    assert!(
        function
            .impl_flags()
            .contains(windows_metadata::MethodImplAttributes::PreserveSig)
    );
    let params = function.params_by_sequence(4).unwrap();
    let [Some(value), Some(_), Some(_), Some(result)] = params.params() else {
        panic!("Extended parameter metadata is incomplete");
    };
    assert!(value.has_attribute("NativeArrayInfoAttribute"));
    assert!(value.has_attribute("MemorySizeAttribute"));
    assert!(value.has_attribute("FreeWithAttribute"));
    assert!(value.has_attribute("DoNotReleaseAttribute"));
    assert!(value.has_attribute("NotNullTerminatedAttribute"));
    assert!(value.has_attribute("ConstAttribute"));
    assert!(
        value
            .flags()
            .contains(windows_metadata::ParamAttributes::In)
    );
    assert!(
        value
            .flags()
            .contains(windows_metadata::ParamAttributes::Optional)
    );
    assert!(result.has_attribute("IgnoreIfReturnAttribute"));
    assert!(
        result
            .flags()
            .contains(windows_metadata::ParamAttributes::Out)
    );

    let windows_metadata::reader::Item::Fn(function) = index.expect_item("Test", "ExtendedW")
    else {
        panic!("ExtendedW was not emitted as a function");
    };
    assert!(function.has_attribute("UnicodeAttribute"));

    std::fs::remove_file(output).unwrap();
}

#[test]
fn symbolic_invalid_handles_resolve_on_all_architectures() {
    helpers::ensure_libclang();

    let source = r#"
        typedef __int64 LONG_PTR;
        typedef void* HANDLE;
        #define INVALID_HANDLE_BASE -2
        #define INVALID_HANDLE_VALUE ((HANDLE)(LONG_PTR)(INVALID_HANDLE_BASE + 1))
        __attribute__((annotate("win32metadata:raii_free=CloseHandle, INVALID_HANDLE_VALUE")))
        extern "C" HANDLE OpenResource();
        #undef INVALID_HANDLE_VALUE
        #define INVALID_HANDLE_VALUE ((HANDLE)0)
    "#;
    for target in [
        "x86_64-pc-windows-msvc",
        "i686-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ] {
        let snapshot = extract(
            [Input::new("symbolic.hpp", source)],
            &[
                "-x",
                "c++",
                "-fms-extensions",
                &format!("--target={target}"),
            ],
        )
        .unwrap();
        let references = BTreeMap::new();
        let mut options = EmitOptions::new("Test", &references);
        options.library = Some("test.dll");
        let rdl = snapshot.emit_with_options(&options).unwrap();
        assert!(
            rdl.contains(
                "OpenResource() -> #[raii_free(\"CloseHandle\")] #[invalid_handle(-1)] HANDLE"
            ),
            "{target}\n{rdl}"
        );
    }

    let error = extract(
        [Input::new(
            "unresolved.hpp",
            r#"
                typedef void* HANDLE;
                __attribute__((annotate(
                    "win32metadata:raii_free=CloseHandle, MISSPELLED_INVALID_HANDLE_VALUE")))
                extern "C" HANDLE OpenResource();
            "#,
        )],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("could not evaluate invalid-handle sentinel"),
        "{error}"
    );
}

#[test]
fn associated_constants_include_only_the_provider() {
    helpers::ensure_libclang();

    let group = Input::new(
        "group.hpp",
        r#"
            enum
                __attribute__((annotate(
                    "win32metadata:associated_constant=ERROR_ALIAS")))
                ERROR_KIND : unsigned long {
                ERROR_LOCAL = 1,
            };
        "#,
    );
    let mut provider = Input::new(
        "provider.hpp",
        r#"
            #define ERROR_TARGET 42UL
            #define ERROR_ALIAS ERROR_TARGET
            #define ERROR_NOISE 19
            extern "C" int UnrelatedApi();
            struct UnrelatedType { int value; };
        "#,
    );
    provider.roots.clear();
    let snapshot = extract(
        [group, provider],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let references = BTreeMap::new();
    let rdl = snapshot
        .emit_with_options(&EmitOptions::new("Test", &references))
        .unwrap();
    assert!(rdl.contains("const ERROR_ALIAS: u32 = 42;"), "{rdl}");
    assert!(!rdl.contains("ERROR_TARGET"), "{rdl}");
    assert!(!rdl.contains("ERROR_NOISE"), "{rdl}");
    assert!(!rdl.contains("Unrelated"), "{rdl}");

    let missing = extract(
        [Input::new(
            "missing.hpp",
            r#"
                enum
                    __attribute__((annotate(
                        "win32metadata:associated_constant=MISSING")))
                    ERROR_KIND : unsigned long {
                    ERROR_LOCAL = 1,
                };
            "#,
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap_err();
    assert!(
        missing
            .to_string()
            .contains("associated constant `MISSING` has no source provider"),
        "{missing}"
    );
}

#[test]
fn redeclarations_union_repeatable_annotations_and_reject_conflicts() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [Input::new(
            "redecl.hpp",
            r#"
                __attribute__((annotate("win32metadata:supported_os=windows6.0.6000")))
                __attribute__((annotate("win32metadata:supported_os=windows6.0.6000")))
                extern "C" int Pick();
                __attribute__((annotate("win32metadata:supported_os=windowsserver2003")))
                extern "C" int Pick();
            "#,
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("test.dll");
    let rdl = snapshot.emit_with_options(&options).unwrap();
    assert_eq!(rdl.matches("#[supported_os(").count(), 2, "{rdl}");

    let conflict = extract(
        [Input::new(
            "conflict.hpp",
            r#"
                __attribute__((annotate("win32metadata:raii_free=CloseFirst")))
                extern "C" int Pick();
                __attribute__((annotate("win32metadata:raii_free=CloseSecond")))
                extern "C" int Pick();
            "#,
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap_err();
    assert!(
        conflict
            .to_string()
            .contains("conflicting redeclaration annotation `raii_free`"),
        "{conflict}"
    );

    let snapshot = extract(
        [Input::new(
            "libraries.hpp",
            r#"
                __attribute__((annotate("win32metadata:import_library=first.dll")))
                extern "C" int Pick();
                __attribute__((annotate("win32metadata:import_library=second.dll")))
                extern "C" int Pick();
            "#,
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let rdl = snapshot
        .emit_with_options(&EmitOptions::new("Test", &references))
        .unwrap();
    assert!(rdl.contains("#[library(\"first.dll\")]"), "{rdl}");
}

#[test]
fn compatible_redeclarations_merge_singleton_and_repeatable_annotations() {
    helpers::ensure_libclang();

    let mut foreign = Input::new(
        "Redeclarations.hpp",
        r#"
            __attribute__((annotate("win32metadata:set_last_error")))
            __attribute__((annotate(
                "win32metadata:supported_os=windows6.1")))
            extern "C" int SampleMergedContract();
        "#,
    );
    foreign.roots.clear();
    let snapshot = extract(
        [
            Input::new(
                "SampleApi.hpp",
                r#"
                    __attribute__((annotate(
                        "win32metadata:import_library=samplemerged.dll")))
                    extern "C" int SampleMergedContract();
                "#,
            ),
            foreign,
        ],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let rdl = snapshot
        .emit_with_options(&EmitOptions::new("Test", &references))
        .unwrap();
    assert_eq!(rdl.matches("fn SampleMergedContract").count(), 1, "{rdl}");
    assert!(
        rdl.contains("#[library(\"samplemerged.dll\", set_last_error)]"),
        "{rdl}"
    );
    assert!(rdl.contains("#[supported_os(\"windows6.1\")]"), "{rdl}");

    let snapshot = extract(
        [Input::new(
            "parameter-redecl.hpp",
            r#"
                extern "C" int Fill(void* value);
                extern "C" int Fill(
                    __attribute__((annotate("win32metadata:out")))
                    void* renamed);

                struct
                    __attribute__((annotate(
                        "win32metadata:supported_os=windows6.1")))
                    FORWARD_RECORD;
                struct FORWARD_RECORD { int value; };
            "#,
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("test.dll");
    let rdl = snapshot.emit_with_options(&options).unwrap();
    assert!(rdl.contains("fn Fill(#[out] value: *mut void)"), "{rdl}");
    assert!(
        rdl.contains("#[supported_os(\"windows6.1\")]\n    struct FORWARD_RECORD"),
        "{rdl}"
    );

    let root = std::env::temp_dir().join(format!(
        "windows-clang-redeclaration-{}",
        std::process::id()
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    let foreign = root.join("Redeclarations.h");
    std::fs::write(
        &foreign,
        r#"
            __attribute__((annotate("win32metadata:set_last_error")))
            extern "C" int SampleMergedContract(void* buffer, unsigned long length);
        "#,
    )
    .unwrap();
    let owner = root.join("SampleApi.h");
    std::fs::write(
        &owner,
        r#"
            #include "Redeclarations.h"
            __attribute__((annotate(
                "win32metadata:import_library=samplemerged.dll")))
            __attribute__((annotate(
                "win32metadata:supported_os=windows6.1")))
            extern "C" int SampleMergedContract(void* buffer, unsigned long length);
        "#,
    )
    .unwrap();
    let input = Input::new("main.cpp", "#include <SampleApi.h>")
        .with_roots([owner.to_string_lossy().as_ref()]);
    let include = format!("-I{}", root.to_string_lossy());
    let snapshot = extract(
        [input],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
            &include,
        ],
    )
    .unwrap();
    let rdl = snapshot
        .emit_with_options(&EmitOptions::new("Test", &references))
        .unwrap();
    assert_eq!(rdl.matches("fn SampleMergedContract").count(), 1, "{rdl}");
    assert!(
        rdl.contains("#[library(\"samplemerged.dll\", set_last_error)]"),
        "{rdl}"
    );
    assert!(rdl.contains("#[supported_os(\"windows6.1\")]"), "{rdl}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn anonymous_callback_and_explicit_layout_survive_rdl() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [Input::new(
            "SampleApi.hpp",
            r#"
                typedef int (__stdcall SAMPLE_CALLBACK)(int code);
                typedef SAMPLE_CALLBACK* PSAMPLE_CALLBACK;
                typedef struct SAMPLE_CALLBACKS {
                    PSAMPLE_CALLBACK chained;
                    PSAMPLE_CALLBACK* pointer;
                    int (__stdcall *anonymous)(int code);
                } SAMPLE_CALLBACKS;

                #pragma pack(push, 2)
                typedef struct __declspec(align(8)) SAMPLE_PACKED {
                    char tag;
                    int value;
                } SAMPLE_PACKED;
                #pragma pack(pop)
            "#,
        )],
        &[
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ],
    )
    .unwrap();
    let references = BTreeMap::new();
    let rdl = snapshot
        .emit_with_options(&EmitOptions::new("Test", &references))
        .unwrap();
    assert!(
        rdl.contains(
            "struct SAMPLE_CALLBACKS {\n        chained: PSAMPLE_CALLBACK,\n        pointer: *mut PSAMPLE_CALLBACK,\n        anonymous: SAMPLE_CALLBACKS_anonymous,"
        ),
        "{rdl}"
    );
    assert!(
        rdl.contains("extern fn SAMPLE_CALLBACKS_anonymous(arg0: i32) -> i32;"),
        "{rdl}"
    );
    assert!(
        rdl.contains("#[packed(2)]\n    #[align(8)]\n    struct SAMPLE_PACKED"),
        "{rdl}"
    );

    let output = std::env::temp_dir().join(format!(
        "windows-clang-layout-callback-{}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_text(METADATA_RDL)
        .input_text(&rdl)
        .output(&output)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&output).unwrap();
    let callbacks = index.expect("Test", "SAMPLE_CALLBACKS");
    let anonymous = callbacks
        .fields()
        .find(|field| field.name() == "anonymous")
        .unwrap();
    assert_eq!(
        anonymous.ty(),
        windows_metadata::Type::class_named("Test", "SAMPLE_CALLBACKS_anonymous")
    );
    assert_eq!(
        index
            .expect("Test", "SAMPLE_CALLBACKS_anonymous")
            .methods()
            .find(|method| method.name() == "Invoke")
            .unwrap()
            .signature(&[])
            .types,
        [windows_metadata::Type::I32]
    );
    let packed = index.expect("Test", "SAMPLE_PACKED");
    assert_eq!(packed.class_layout().unwrap().packing_size(), 2);
    assert_eq!(
        packed.find_attribute("AlignmentAttribute").unwrap().value(),
        [(String::new(), windows_metadata::Value::I32(8))]
    );
    std::fs::remove_file(output).unwrap();
}

#[test]
fn architecture_varying_type_merges_x86_and_wide_layouts() {
    helpers::ensure_libclang();

    let root = std::env::temp_dir().join(format!(
        "windows-clang-arch-annotations-{}",
        std::process::id()
    ));
    if root.exists() {
        std::fs::remove_dir_all(&root).unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    let source = r#"
        #if defined(_M_IX86)
        typedef struct SAMPLE_ARCH_VALUE { int value; } SAMPLE_ARCH_VALUE;
        #else
        typedef struct SAMPLE_ARCH_VALUE { __int64 value; } SAMPLE_ARCH_VALUE;
        #endif

        #pragma pack(push, 2)
        typedef struct __declspec(align(8)) SAMPLE_PACKED {
            char tag;
            int value;
        } SAMPLE_PACKED;
        #pragma pack(pop)
    "#;
    let references = BTreeMap::new();
    let mut arches = vec![];
    for (name, target, bits) in [
        ("x64", "x86_64-pc-windows-msvc", 2),
        ("arm64", "aarch64-pc-windows-msvc", 4),
        ("x86", "i686-pc-windows-msvc", 1),
    ] {
        let snapshot = extract(
            [Input::new("SampleApi.hpp", source)],
            &[
                "-x",
                "c++",
                "-fms-extensions",
                &format!("--target={target}"),
            ],
        )
        .unwrap();
        let rdl = snapshot
            .emit_with_options(&EmitOptions::new("Windows.Win32", &references))
            .unwrap();
        let arch_dir = root.join(name);
        let rdl_dir = arch_dir.join("rdl");
        std::fs::create_dir_all(&rdl_dir).unwrap();
        std::fs::write(rdl_dir.join("sampleapi.rdl"), rdl).unwrap();
        let winmd = arch_dir.join("Windows.Win32.winmd");
        windows_rdl::reader()
            .input_text(METADATA_RDL)
            .input(&rdl_dir)
            .output(&winmd)
            .write()
            .unwrap();
        arches.push(windows_rdl::ArchInput {
            rdl_dir,
            winmd,
            bits,
        });
    }

    let merged_rdl = root.join("merged");
    windows_rdl::merge_arch_rdl(&arches, None, &merged_rdl).unwrap();
    let output = root.join("merged.winmd");
    windows_rdl::reader()
        .input(&merged_rdl)
        .output(&output)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&output).unwrap();
    let mut values: Vec<_> = index
        .types()
        .filter(|ty| ty.namespace() == "Windows.Win32" && ty.name() == "SAMPLE_ARCH_VALUE")
        .map(|ty| {
            (
                ty.arches(),
                ty.fields()
                    .find(|field| field.name() == "value")
                    .unwrap()
                    .ty(),
            )
        })
        .collect();
    values.sort_by_key(|(arches, _)| *arches);
    assert_eq!(
        values,
        [
            (1, windows_metadata::Type::I32),
            (6, windows_metadata::Type::I64)
        ]
    );
    let packed = index.expect("Windows.Win32", "SAMPLE_PACKED");
    assert_eq!(packed.class_layout().unwrap().packing_size(), 2);
    assert_eq!(
        packed.find_attribute("AlignmentAttribute").unwrap().value(),
        [(String::new(), windows_metadata::Value::I32(8))]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn supported_os_payloads_and_attribute_usage_are_exact() {
    helpers::ensure_libclang();

    let platforms = [
        "windows5.0",
        "windows5.1.2600",
        "windows6.0.6000",
        "windows6.0.6001",
        "windows6.1",
        "windows8.0",
        "windows8.1",
        "windows10.0.10240",
        "windows10.0.10586",
        "windows10.0.14393",
        "windows10.0.15063",
        "windows10.0.16299",
        "windows10.0.17134",
        "windows10.0.17763",
        "windows10.0.18362",
        "windows10.0.19041",
        "windows10.0.19041.662",
        "windows10.0.20348",
        "windows10.0.22631",
        "windows10.0.26100",
        "windowsserver2000",
        "windowsserver2003",
        "windowsserver2008",
        "windowsserver2012",
        "windowsserver2016",
    ];
    let annotations = platforms
        .iter()
        .map(|platform| {
            format!("__attribute__((annotate(\"win32metadata:supported_os={platform}\")))")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let source = format!("{annotations}\nextern \"C\" int Supported();");
    let snapshot = extract(
        [Input::new("supported.hpp", source)],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let references = BTreeMap::new();
    let mut options = EmitOptions::new("Test", &references);
    options.library = Some("test.dll");
    let rdl = snapshot.emit_with_options(&options).unwrap();
    let output = std::env::temp_dir().join(format!(
        "windows-clang-supported-os-{}.winmd",
        std::process::id()
    ));
    windows_rdl::reader()
        .input_text(METADATA_RDL)
        .input_text(&rdl)
        .output(&output)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&output).unwrap();
    let windows_metadata::reader::Item::Fn(function) = index.expect_item("Test", "Supported")
    else {
        panic!("Supported was not emitted as a function");
    };
    let mut actual = attribute_strings(function, "SupportedOSPlatformAttribute");
    actual.sort();
    let mut expected = platforms.map(str::to_string).to_vec();
    expected.sort();
    assert_eq!(actual, expected);

    let definition = index.expect(
        "Windows.Win32.Foundation.Metadata",
        "SupportedOSPlatformAttribute",
    );
    let usage = definition.attributes().next().unwrap();
    assert_eq!(usage.namespace(), "System");
    assert_eq!(usage.name(), "AttributeUsageAttribute");
    assert_eq!(usage.named_arg_kinds(), [0x54]);
    let values = usage.value();
    let windows_metadata::Value::EnumValue(_, mask) = &values[0].1 else {
        panic!("AttributeUsage target was not encoded as an enum");
    };
    assert_eq!(
        mask.as_ref(),
        &windows_metadata::Value::I32(64 | 8 | 16 | 1024 | 4096)
    );
    assert_eq!(
        values[1],
        (
            "AllowMultiple".to_string(),
            windows_metadata::Value::Bool(true)
        )
    );
    std::fs::remove_file(output).unwrap();
}
