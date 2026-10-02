use windows_clang::{FactData, Input, TypeRef, extract};
use windows_metadata::Type;

const SUPPORT_RDL: &str = r#"
#[win32]
mod Test {
    type PCWSTR = *const u16;
}
"#;

const SOURCE: &str = r#"
typedef void *HWND;
typedef unsigned short WCHAR;
typedef const WCHAR *LPCWSTR;

typedef struct GUID {
    unsigned long Data1;
    unsigned short Data2;
    unsigned short Data3;
    unsigned char Data4[8];
} GUID;

typedef const GUID &REFGUID;
typedef GUID &MUTABLE_REFGUID;

typedef struct _WEBAUTHN_PLUGIN_USER_VERIFICATION_REQUEST {
    HWND hwnd;
    REFGUID rguidTransactionId;
    LPCWSTR pwszUsername;
    LPCWSTR pwszDisplayHint;
} WEBAUTHN_PLUGIN_USER_VERIFICATION_REQUEST;

typedef struct _REFERENCE_PAIR {
    REFGUID immutable;
    MUTABLE_REFGUID mutableValue;
} REFERENCE_PAIR;

extern "C" int InspectGuid(const GUID &value);
"#;

#[test]
fn reference_data_members_use_pointer_storage() {
    helpers::ensure_libclang();

    for (name, target, pointer_size) in [
        ("x86", "i686-pc-windows-msvc", 4),
        ("x64", "x86_64-pc-windows-msvc", 8),
        ("arm64", "aarch64-pc-windows-msvc", 8),
    ] {
        let snapshot = extract(
            [Input::new("webauthnplugin.h", SOURCE)],
            &[
                "-x",
                "c++",
                "-std=c++17",
                "-fms-extensions",
                &format!("--target={target}"),
            ],
        )
        .unwrap();

        assert_reference_alias(&snapshot, "REFGUID", false);
        assert_reference_alias(&snapshot, "MUTABLE_REFGUID", true);

        let request = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "_WEBAUTHN_PLUGIN_USER_VERIFICATION_REQUEST")
            .unwrap();
        let FactData::Record {
            fields,
            size,
            align,
            ..
        } = &request.data
        else {
            panic!("{request:#?}");
        };
        assert_eq!((*size, *align), (pointer_size * 4, pointer_size));
        assert_eq!(fields.len(), 4);
        for (index, field) in fields.iter().enumerate() {
            assert_eq!(field.offset, index as i64 * pointer_size * 8);
            assert_eq!(field.size, pointer_size);
            assert_eq!(field.align, pointer_size);
        }

        let pair = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "_REFERENCE_PAIR")
            .unwrap();
        let FactData::Record {
            fields,
            size,
            align,
            ..
        } = &pair.data
        else {
            panic!("{pair:#?}");
        };
        assert_eq!((*size, *align), (pointer_size * 2, pointer_size));
        assert_eq!(
            fields
                .iter()
                .map(|field| (field.offset, field.size, field.align))
                .collect::<Vec<_>>(),
            vec![
                (0, pointer_size, pointer_size),
                (pointer_size * 8, pointer_size, pointer_size),
            ]
        );

        let function = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "InspectGuid")
            .unwrap();
        let FactData::Function { params, .. } = &function.data else {
            panic!("{function:#?}");
        };
        assert!(matches!(
            params.as_slice(),
            [parameter]
                if matches!(
                    &parameter.ty,
                    TypeRef::Reference {
                        mutable: false,
                        target,
                    } if matches!(
                        &**target,
                        TypeRef::Named { name, .. } if name == "GUID"
                    )
                )
        ));

        let rdl = snapshot.emit_with_library("Test", "test.dll").unwrap();
        assert!(rdl.contains("type REFGUID = *const GUID;"));
        assert!(rdl.contains("type MUTABLE_REFGUID = *mut GUID;"));
        assert!(rdl.contains("rguidTransactionId: REFGUID,"));
        assert!(rdl.contains("value: *const GUID"));

        let output = std::env::temp_dir().join(format!(
            "windows-clang-reference-fields-{}-{name}.winmd",
            std::process::id()
        ));
        windows_rdl::reader()
            .input_text(SUPPORT_RDL)
            .input_text(&rdl)
            .output(&output)
            .write()
            .unwrap();
        let index = windows_metadata::reader::Index::read(&output).unwrap();

        let request = index.expect("Test", "WEBAUTHN_PLUGIN_USER_VERIFICATION_REQUEST");
        assert_eq!(
            request
                .fields()
                .find(|field| field.name() == "rguidTransactionId")
                .unwrap()
                .ty(),
            Type::value_named("Test", "REFGUID")
        );
        assert_eq!(
            index.expect("Test", "REFGUID").underlying_type(),
            Some(Type::PtrConst(
                Box::new(Type::value_named("Test", "GUID")),
                1
            ))
        );

        let pair = index.expect("Test", "REFERENCE_PAIR");
        assert_eq!(
            pair.fields()
                .find(|field| field.name() == "immutable")
                .unwrap()
                .ty(),
            Type::value_named("Test", "REFGUID")
        );
        assert_eq!(
            pair.fields()
                .find(|field| field.name() == "mutableValue")
                .unwrap()
                .ty(),
            Type::value_named("Test", "MUTABLE_REFGUID")
        );
        assert_eq!(
            index.expect("Test", "MUTABLE_REFGUID").underlying_type(),
            Some(Type::PtrMut(Box::new(Type::value_named("Test", "GUID")), 1))
        );
        std::fs::remove_file(output).unwrap();
    }
    assert_function_reference_data_member_unsupported();
}

fn assert_function_reference_data_member_unsupported() {
    let snapshot = extract(
        [Input::new(
            "callback.h",
            r#"
                typedef int CALLBACK(int value);
                typedef struct _BAD_REFERENCE {
                    CALLBACK &callback;
                } BAD_REFERENCE;
            "#,
        )],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let fact = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == "_BAD_REFERENCE")
        .unwrap();
    let FactData::Unsupported { reason } = &fact.data else {
        panic!("{fact:#?}");
    };
    assert_eq!(
        reason,
        "field `callback` has unsupported function reference type `CALLBACK &`"
    );
}

fn assert_reference_alias(snapshot: &windows_clang::Snapshot, name: &str, mutable: bool) {
    let fact = snapshot
        .facts()
        .iter()
        .find(|fact| fact.name == name)
        .unwrap();
    let FactData::Typedef {
        target: TypeRef::Reference {
            mutable: actual,
            target,
        },
    } = &fact.data
    else {
        panic!("{fact:#?}");
    };
    assert_eq!(*actual, mutable);
    assert!(matches!(
        &**target,
        TypeRef::Named { name, .. } if name == "GUID"
    ));
}
