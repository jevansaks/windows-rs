use windows_clang::{CallingConvention, FactData, Input, Snapshot, TypeRef, extract};
use windows_metadata::{
    Type, Value,
    reader::{HasAttributes, TypeCategory},
};

const NAMESPACE: &str = "Windows.Win32.Web.InternetExplorer";

const IE_SOURCE: &str = r#"
#define WINAPI __stdcall

typedef void* HMODULE;

typedef void (WINAPI STDCALL_CALLBACK)(void);
typedef STDCALL_CALLBACK* PSTDCALL_CALLBACK;
typedef STDCALL_CALLBACK* (*CallbackFactory_t)(void);

typedef HMODULE (*IEGetProcessModule_t)();

struct IE80TabWindowExports {
    void (WINAPI *TLSFreeImmutableTabData)();
};

#define IETabWindowExports IE80TabWindowExports
typedef const struct IETabWindowExports* (*IEGetTabWindowExports_t)();
"#;

#[test]
fn ie_callback_contract_survives_all_producer_stages() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-ie-callback-contract-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    for (architecture, target) in [
        ("x86", "i686-pc-windows-msvc"),
        ("x64", "x86_64-pc-windows-msvc"),
        ("arm64", "aarch64-pc-windows-msvc"),
    ] {
        let snapshot = extract(
            [Input::new("IEProcess.h", IE_SOURCE)],
            &[
                "-x",
                "c++",
                "-fms-extensions",
                &format!("--target={target}"),
            ],
        )
        .unwrap();

        assert_callback(
            &snapshot,
            "PSTDCALL_CALLBACK",
            CallingConvention::Platform,
            0,
            |result| assert_eq!(result, &TypeRef::Void),
        );
        assert_callback(
            &snapshot,
            "CallbackFactory_t",
            CallingConvention::C,
            0,
            |_| {},
        );
        assert_callback(
            &snapshot,
            "IE80TabWindowExports_TLSFreeImmutableTabData",
            CallingConvention::Platform,
            0,
            |result| assert_eq!(result, &TypeRef::Void),
        );
        assert_callback(
            &snapshot,
            "IEGetProcessModule_t",
            CallingConvention::C,
            0,
            |result| {
                assert!(
                    matches!(result, TypeRef::Named { name, .. } if name == "HMODULE"),
                    "{architecture}: {result:?}"
                );
            },
        );
        assert_callback(
            &snapshot,
            "IEGetTabWindowExports_t",
            CallingConvention::C,
            0,
            |result| {
                assert!(
                    matches!(
                        result,
                        TypeRef::Pointer {
                            mutable: false,
                            target,
                        } if matches!(
                            target.as_ref(),
                            TypeRef::Named { name, .. } if name == "IE80TabWindowExports"
                        )
                    ),
                    "{architecture}: {result:?}"
                );
            },
        );

        let rdl = snapshot.emit(NAMESPACE).unwrap();
        assert!(
            rdl.contains("extern fn PSTDCALL_CALLBACK()"),
            "{architecture}: {rdl}"
        );
        assert!(
            rdl.contains("extern fn IE80TabWindowExports_TLSFreeImmutableTabData()"),
            "{architecture}: {rdl}"
        );
        assert!(
            rdl.contains("extern \"C\" fn IEGetProcessModule_t() -> HMODULE"),
            "{architecture}: {rdl}"
        );
        assert!(
            rdl.contains(
                "extern \"C\" fn IEGetTabWindowExports_t() -> *const IE80TabWindowExports"
            ),
            "{architecture}: {rdl}"
        );

        let output = scratch.join(format!("{architecture}.winmd"));
        windows_rdl::reader()
            .input_text(&rdl)
            .output(&output)
            .write()
            .unwrap();
        let index = windows_metadata::reader::Index::read(&output).unwrap();

        assert_physical_callback(
            &index,
            "IE80TabWindowExports_TLSFreeImmutableTabData",
            Type::Void,
            1,
        );
        assert_physical_callback(
            &index,
            "IEGetProcessModule_t",
            Type::value_named(NAMESPACE, "HMODULE"),
            2,
        );
        assert_physical_callback(
            &index,
            "IEGetTabWindowExports_t",
            Type::PtrConst(
                Box::new(Type::value_named(NAMESPACE, "IE80TabWindowExports")),
                1,
            ),
            2,
        );
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

fn assert_callback(
    snapshot: &Snapshot,
    name: &str,
    convention: CallingConvention,
    arity: usize,
    assert_result: impl FnOnce(&TypeRef),
) {
    let fact = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == name)
        .unwrap_or_else(|| panic!("missing callback `{name}`\n{}", snapshot.dump()));
    let FactData::Callback {
        convention: actual_convention,
        params,
        result,
    } = &fact.data
    else {
        panic!("`{name}` is not a callback: {:?}", fact.data);
    };
    assert_eq!(*actual_convention, convention, "{name}");
    assert_eq!(params.len(), arity, "{name}: {params:?}");
    assert_result(result);
}

fn assert_physical_callback(
    index: &windows_metadata::reader::Index,
    name: &str,
    return_type: Type,
    convention: i32,
) {
    let callback = index.expect(NAMESPACE, name);
    assert_eq!(callback.category(), TypeCategory::Delegate);

    let attribute = callback
        .find_attribute("UnmanagedFunctionPointerAttribute")
        .unwrap()
        .value();
    let [(_, Value::EnumValue(_, value))] = attribute.as_slice() else {
        panic!("unexpected calling-convention attribute for `{name}`: {attribute:?}");
    };
    assert_eq!(value.as_ref(), &Value::I32(convention));

    let invokes: Vec<_> = callback
        .methods()
        .filter(|method| method.name() == "Invoke")
        .collect();
    let [invoke] = invokes.as_slice() else {
        panic!("expected one physical Invoke for `{name}`, got {invokes:?}");
    };
    let signature = invoke.signature(&[]);
    assert!(signature.types.is_empty(), "{name}: {signature:?}");
    assert_eq!(signature.return_type, return_type, "{name}");
}
