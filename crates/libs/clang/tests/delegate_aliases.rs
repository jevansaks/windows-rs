use std::collections::BTreeMap;
use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition,
    Snapshot, TypeRef, TypeReference, TypeReferenceKind, extract,
};
use windows_metadata::{ParamAttributes, Type};

const NAMESPACE: &str = "Windows.Win32.Security.Authentication.Identity";

const SOURCE: &str = r#"
#define NTAPI __stdcall
#define INOUT __attribute__((annotate("win32metadata:in"))) \
              __attribute__((annotate("win32metadata:out")))
#define OUT __attribute__((annotate("win32metadata:out")))

typedef long NTSTATUS;
typedef void VOID;
typedef unsigned char BYTE;
typedef unsigned long ULONG;
typedef ULONG *PULONG;
typedef void *HANDLE;
typedef HANDLE *PHANDLE;
typedef void *PVOID;
typedef BYTE *PBYTE;
typedef void *PSID;
typedef HANDLE LSA_SEC_HANDLE;

typedef struct GUID {
    unsigned long Data1;
    unsigned short Data2;
    unsigned short Data3;
    unsigned char Data4[8];
} GUID;

typedef struct UNICODE_STRING {
    unsigned short Length;
    unsigned short MaximumLength;
    unsigned short *Buffer;
} UNICODE_STRING;

typedef struct LUID {
    unsigned long LowPart;
    long HighPart;
} LUID;

typedef struct SECPKG_SUPPLEMENTAL_CRED_ARRAY {
    ULONG Count;
} SECPKG_SUPPLEMENTAL_CRED_ARRAY, *PSECPKG_SUPPLEMENTAL_CRED_ARRAY;

typedef NTSTATUS
(NTAPI LSA_REDIRECTED_LOGON_INIT)(
    HANDLE RedirectedLogonHandle,
    const UNICODE_STRING* PackageName,
    ULONG SessionId,
    const LUID* LogonId
    );

typedef NTSTATUS
(NTAPI LSA_REDIRECTED_LOGON_CALLBACK)(
    HANDLE RedirectedLogonHandle,
    INOUT PVOID Buffer,
    ULONG BufferLength,
    INOUT PVOID* ReturnBuffer,
    INOUT ULONG* ReturnBufferLength
    );

typedef VOID
(NTAPI LSA_REDIRECTED_LOGON_CLEANUP_CALLBACK)(
    HANDLE RedirectedLogonHandle
    );

typedef NTSTATUS
(NTAPI LSA_REDIRECTED_LOGON_GET_LOGON_CREDS)(
    HANDLE RedirectedLogonHandle,
    INOUT PBYTE* LogonBuffer,
    INOUT PULONG LogonBufferLength
    );

typedef NTSTATUS
(NTAPI LSA_REDIRECTED_LOGON_GET_SUPP_CREDS)(
    HANDLE RedirectedLogonHandle,
    INOUT PSECPKG_SUPPLEMENTAL_CRED_ARRAY* SupplementalCredentials
    );

typedef NTSTATUS
(NTAPI LSA_REDIRECTED_LOGON_GET_SID)(
    HANDLE RedirectedLogonHandle,
    INOUT PSID* Sid
    );

typedef LSA_REDIRECTED_LOGON_INIT *PLSA_REDIRECTED_LOGON_INIT;
typedef LSA_REDIRECTED_LOGON_CALLBACK *PLSA_REDIRECTED_LOGON_CALLBACK;
typedef LSA_REDIRECTED_LOGON_GET_LOGON_CREDS *PLSA_REDIRECTED_LOGON_GET_LOGON_CREDS;
typedef LSA_REDIRECTED_LOGON_GET_SUPP_CREDS *PLSA_REDIRECTED_LOGON_GET_SUPP_CREDS;
typedef LSA_REDIRECTED_LOGON_CLEANUP_CALLBACK *PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK;
typedef LSA_REDIRECTED_LOGON_GET_SID *PLSA_REDIRECTED_LOGON_GET_SID;

typedef struct SECPKG_REDIRECTED_LOGON_BUFFER {
    GUID RedirectedLogonGuid;
    HANDLE RedirectedLogonHandle;
    PLSA_REDIRECTED_LOGON_INIT Init;
    PLSA_REDIRECTED_LOGON_CALLBACK Callback;
    PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK CleanupCallback;
    PLSA_REDIRECTED_LOGON_GET_LOGON_CREDS GetLogonCreds;
    PLSA_REDIRECTED_LOGON_GET_SUPP_CREDS GetSupplementalCreds;
    PLSA_REDIRECTED_LOGON_GET_SID GetRedirectedLogonSid;
} SECPKG_REDIRECTED_LOGON_BUFFER, *PSECPKG_REDIRECTED_LOGON_BUFFER;

typedef NTSTATUS
(NTAPI SpGetRemoteCredGuardLogonBufferFn)(
    LSA_SEC_HANDLE CredHandle,
    LSA_SEC_HANDLE ContextHandle,
    const UNICODE_STRING* TargetName,
    OUT PHANDLE RedirectedLogonHandle,
    OUT PLSA_REDIRECTED_LOGON_CALLBACK* Callback,
    OUT PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK* CleanupCallback,
    OUT PULONG LogonBufferSize,
    OUT PVOID* LogonBuffer
    );

typedef NTSTATUS
(NTAPI SpGetRemoteCredGuardSupplementalCredsFn)(
    LSA_SEC_HANDLE CredHandle,
    const UNICODE_STRING* TargetName,
    OUT PHANDLE RedirectedLogonHandle,
    OUT PLSA_REDIRECTED_LOGON_CALLBACK* Callback,
    OUT PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK* CleanupCallback,
    OUT PULONG SupplementalCredsSize,
    OUT PVOID* SupplementalCreds
    );

typedef int (NTAPI *ACMDRIVERPROC)(int value);
typedef ACMDRIVERPROC *LPACMDRIVERPROC;

typedef int (NTAPI *_ENTITY_METHOD)(int value);
typedef _ENTITY_METHOD RTM_ENTITY_EXPORT_METHOD, *PRTM_ENTITY_EXPORT_METHOD;

typedef int (NTAPI *_EVENT_CALLBACK)(int value);
typedef _EVENT_CALLBACK RTM_EVENT_CALLBACK, *PRTM_EVENT_CALLBACK;

typedef int (NTAPI *INTERNET_STATUS_CALLBACK)(int value);
typedef INTERNET_STATUS_CALLBACK *LPINTERNET_STATUS_CALLBACK;

typedef int (NTAPI *INSTALLUI_HANDLER_RECORD)(int value);
typedef INSTALLUI_HANDLER_RECORD *PINSTALLUI_HANDLER_RECORD;

typedef void (NTAPI *RPC_DISPATCH_FUNCTION)(int value);
typedef struct RPC_DISPATCH_TABLE {
    RPC_DISPATCH_FUNCTION *DispatchTable;
} RPC_DISPATCH_TABLE;
"#;

#[test]
fn pointer_callback_aliases_keep_delegate_identity_and_source_depth() {
    helpers::ensure_libclang();

    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-delegate-aliases-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let header = scratch.join("NTSecPKG.h");
    std::fs::write(&header, SOURCE).unwrap();

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
    assert_extracted_contracts(&snapshot);

    let root = RootPartition::new("ntsecpkg", NAMESPACE)
        .with_preserved_auto_function_pointer_level("PLSA_REDIRECTED_LOGON_CALLBACK")
        .with_preserved_auto_function_pointer_level("PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK");
    let policy = HeaderPartitionPolicy::new().with_traversed_header(header.to_string_lossy(), root);
    let references = BTreeMap::from([(
        "EXTERNAL_TYPE".to_string(),
        TypeReference::new("Example.External", "EXTERNAL_TYPE", TypeReferenceKind::Type),
    )]);
    let options = EmitOptions::new("Windows.Win32", &references);
    let partitions = snapshot
        .plan_header_partitions(&policy, &NamespaceAuthorities::new())
        .unwrap()
        .emit_with_options(&options)
        .unwrap();
    let rdl = partitions.values().cloned().collect::<String>();
    assert_rdl_contracts(&rdl);

    let winmd = scratch.join("delegate-aliases.winmd");
    windows_rdl::reader()
        .input_texts(partitions.values())
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    assert_physical_contracts(&index);

    std::fs::remove_dir_all(scratch).unwrap();
}

fn assert_extracted_contracts(snapshot: &Snapshot) {
    for name in [
        "LSA_REDIRECTED_LOGON_INIT",
        "LSA_REDIRECTED_LOGON_CALLBACK",
        "LSA_REDIRECTED_LOGON_CLEANUP_CALLBACK",
        "LSA_REDIRECTED_LOGON_GET_LOGON_CREDS",
        "LSA_REDIRECTED_LOGON_GET_SUPP_CREDS",
        "LSA_REDIRECTED_LOGON_GET_SID",
        "PLSA_REDIRECTED_LOGON_INIT",
        "PLSA_REDIRECTED_LOGON_CALLBACK",
        "PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK",
        "PLSA_REDIRECTED_LOGON_GET_LOGON_CREDS",
        "PLSA_REDIRECTED_LOGON_GET_SUPP_CREDS",
        "PLSA_REDIRECTED_LOGON_GET_SID",
    ] {
        assert!(
            snapshot
                .facts()
                .iter()
                .any(|fact| fact.name == name && matches!(fact.data, FactData::Callback { .. })),
            "{name} was not extracted as a callback"
        );
    }

    let record = snapshot
        .facts()
        .iter()
        .find_map(|fact| {
            (fact.name == "SECPKG_REDIRECTED_LOGON_BUFFER")
                .then_some(&fact.data)
                .and_then(|data| match data {
                    FactData::Record { fields, .. } => Some(fields),
                    _ => None,
                })
        })
        .unwrap();
    let fields = record
        .iter()
        .map(|field| (field.name.as_str(), &field.ty))
        .collect::<BTreeMap<_, _>>();
    for (field, name) in [
        ("Init", "PLSA_REDIRECTED_LOGON_INIT"),
        ("Callback", "PLSA_REDIRECTED_LOGON_CALLBACK"),
        ("CleanupCallback", "PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK"),
        ("GetLogonCreds", "PLSA_REDIRECTED_LOGON_GET_LOGON_CREDS"),
        (
            "GetSupplementalCreds",
            "PLSA_REDIRECTED_LOGON_GET_SUPP_CREDS",
        ),
        ("GetRedirectedLogonSid", "PLSA_REDIRECTED_LOGON_GET_SID"),
    ] {
        assert_named(fields[field], name);
    }

    for (callback, slots) in [
        (
            "SpGetRemoteCredGuardLogonBufferFn",
            [
                (4, "PLSA_REDIRECTED_LOGON_CALLBACK"),
                (5, "PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK"),
            ],
        ),
        (
            "SpGetRemoteCredGuardSupplementalCredsFn",
            [
                (3, "PLSA_REDIRECTED_LOGON_CALLBACK"),
                (4, "PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK"),
            ],
        ),
    ] {
        let params = snapshot
            .facts()
            .iter()
            .find_map(|fact| {
                (fact.name == callback)
                    .then_some(&fact.data)
                    .and_then(|data| match data {
                        FactData::Callback { params, .. } => Some(params),
                        _ => None,
                    })
            })
            .unwrap();
        for (index, name) in slots {
            assert_mut_pointer_to_named(&params[index].ty, name);
            assert!(params[index].annotation.output);
        }
    }

    for (alias, target) in [
        ("LPACMDRIVERPROC", "ACMDRIVERPROC"),
        ("PRTM_ENTITY_EXPORT_METHOD", "_ENTITY_METHOD"),
        ("PRTM_EVENT_CALLBACK", "_EVENT_CALLBACK"),
        ("LPINTERNET_STATUS_CALLBACK", "INTERNET_STATUS_CALLBACK"),
        ("PINSTALLUI_HANDLER_RECORD", "INSTALLUI_HANDLER_RECORD"),
    ] {
        let ty = snapshot
            .facts()
            .iter()
            .find_map(|fact| {
                (fact.name == alias)
                    .then_some(&fact.data)
                    .and_then(|data| match data {
                        FactData::Typedef { target } => Some(target),
                        _ => None,
                    })
            })
            .unwrap_or_else(|| panic!("{alias} was not extracted as a pointer typedef"));
        assert_mut_pointer_to_named(ty, target);
    }

    let dispatch = snapshot
        .facts()
        .iter()
        .find_map(|fact| {
            (fact.name == "RPC_DISPATCH_TABLE")
                .then_some(&fact.data)
                .and_then(|data| match data {
                    FactData::Record { fields, .. } => fields
                        .iter()
                        .find(|field| field.name == "DispatchTable")
                        .map(|field| &field.ty),
                    _ => None,
                })
        })
        .unwrap();
    assert_mut_pointer_to_named(dispatch, "RPC_DISPATCH_FUNCTION");
}

fn assert_rdl_contracts(rdl: &str) {
    for name in [
        "PLSA_REDIRECTED_LOGON_INIT",
        "PLSA_REDIRECTED_LOGON_CALLBACK",
        "PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK",
        "PLSA_REDIRECTED_LOGON_GET_LOGON_CREDS",
        "PLSA_REDIRECTED_LOGON_GET_SUPP_CREDS",
        "PLSA_REDIRECTED_LOGON_GET_SID",
    ] {
        assert!(
            rdl.lines()
                .any(|line| line.trim_start().starts_with(&format!("extern fn {name}("))),
            "{name} was not emitted as a delegate:\n{rdl}"
        );
    }

    for (field, name) in [
        ("Init", "PLSA_REDIRECTED_LOGON_INIT"),
        ("Callback", "PLSA_REDIRECTED_LOGON_CALLBACK"),
        ("CleanupCallback", "PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK"),
        ("GetLogonCreds", "PLSA_REDIRECTED_LOGON_GET_LOGON_CREDS"),
        (
            "GetSupplementalCreds",
            "PLSA_REDIRECTED_LOGON_GET_SUPP_CREDS",
        ),
        ("GetRedirectedLogonSid", "PLSA_REDIRECTED_LOGON_GET_SID"),
    ] {
        let line = rdl
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("{field}:")))
            .unwrap();
        assert_eq!(line.trim(), format!("{field}: {name},"));
    }

    for name in [
        "SpGetRemoteCredGuardLogonBufferFn",
        "SpGetRemoteCredGuardSupplementalCredsFn",
    ] {
        let line = rdl
            .lines()
            .find(|line| line.contains(&format!("extern fn {name}(")))
            .unwrap();
        assert!(
            line.contains("Callback: *mut PLSA_REDIRECTED_LOGON_CALLBACK"),
            "{line}"
        );
        assert!(
            line.contains("CleanupCallback: *mut PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK"),
            "{line}"
        );
        assert!(!line.contains("*mut *mut PLSA_REDIRECTED_LOGON"), "{line}");
    }

    for (alias, target) in [
        ("LPACMDRIVERPROC", "ACMDRIVERPROC"),
        ("PRTM_ENTITY_EXPORT_METHOD", "_ENTITY_METHOD"),
        ("PRTM_EVENT_CALLBACK", "_EVENT_CALLBACK"),
        ("LPINTERNET_STATUS_CALLBACK", "INTERNET_STATUS_CALLBACK"),
        ("PINSTALLUI_HANDLER_RECORD", "INSTALLUI_HANDLER_RECORD"),
    ] {
        assert!(
            rdl.contains(&format!("type {alias} = *mut {target};")),
            "{alias} lost its pointer-to-delegate depth:\n{rdl}"
        );
    }
    assert!(
        rdl.lines()
            .any(|line| { line.trim() == "DispatchTable: *mut RPC_DISPATCH_FUNCTION," }),
        "RPC_DISPATCH_TABLE lost its pointer-to-delegate field:\n{rdl}"
    );
}

fn assert_physical_contracts(index: &windows_metadata::reader::Index) {
    let record = index.expect(NAMESPACE, "SECPKG_REDIRECTED_LOGON_BUFFER");
    let fields = record
        .fields()
        .map(|field| (field.name().to_string(), field.ty()))
        .collect::<BTreeMap<_, _>>();
    for (field, name) in [
        ("Init", "PLSA_REDIRECTED_LOGON_INIT"),
        ("Callback", "PLSA_REDIRECTED_LOGON_CALLBACK"),
        ("CleanupCallback", "PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK"),
        ("GetLogonCreds", "PLSA_REDIRECTED_LOGON_GET_LOGON_CREDS"),
        (
            "GetSupplementalCreds",
            "PLSA_REDIRECTED_LOGON_GET_SUPP_CREDS",
        ),
        ("GetRedirectedLogonSid", "PLSA_REDIRECTED_LOGON_GET_SID"),
    ] {
        assert_eq!(fields[field], Type::class_named(NAMESPACE, name));
    }

    for (callback, count, slots) in [
        (
            "SpGetRemoteCredGuardLogonBufferFn",
            8,
            [
                (4, "PLSA_REDIRECTED_LOGON_CALLBACK"),
                (5, "PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK"),
            ],
        ),
        (
            "SpGetRemoteCredGuardSupplementalCredsFn",
            7,
            [
                (3, "PLSA_REDIRECTED_LOGON_CALLBACK"),
                (4, "PLSA_REDIRECTED_LOGON_CLEANUP_CALLBACK"),
            ],
        ),
    ] {
        let item = index.expect(NAMESPACE, callback);
        let invoke = item
            .methods()
            .find(|method| method.name() == "Invoke")
            .unwrap();
        let signature = invoke.signature(&[]);
        let params = invoke.params_by_sequence(count).unwrap();
        let params = params.params();
        for (slot, name) in slots {
            assert_eq!(
                signature.types[slot],
                Type::PtrMut(Box::new(Type::class_named(NAMESPACE, name)), 1)
            );
            assert!(params[slot].unwrap().flags().contains(ParamAttributes::Out));
        }
    }

    for (alias, target) in [
        ("LPACMDRIVERPROC", "ACMDRIVERPROC"),
        ("PRTM_ENTITY_EXPORT_METHOD", "_ENTITY_METHOD"),
        ("PRTM_EVENT_CALLBACK", "_EVENT_CALLBACK"),
        ("LPINTERNET_STATUS_CALLBACK", "INTERNET_STATUS_CALLBACK"),
        ("PINSTALLUI_HANDLER_RECORD", "INSTALLUI_HANDLER_RECORD"),
    ] {
        assert_eq!(
            index.expect(NAMESPACE, alias).underlying_type(),
            Some(Type::PtrMut(
                Box::new(Type::class_named(NAMESPACE, target)),
                1
            ))
        );
    }

    let dispatch = index.expect(NAMESPACE, "RPC_DISPATCH_TABLE");
    assert_eq!(
        dispatch
            .fields()
            .find(|field| field.name() == "DispatchTable")
            .unwrap()
            .ty(),
        Type::PtrMut(
            Box::new(Type::class_named(NAMESPACE, "RPC_DISPATCH_FUNCTION")),
            1
        )
    );
}

fn assert_named(ty: &TypeRef, expected: &str) {
    assert!(
        matches!(ty, TypeRef::Named { name, .. } if name == expected),
        "expected `{expected}`, got {ty:?}"
    );
}

fn assert_mut_pointer_to_named(ty: &TypeRef, expected: &str) {
    assert!(
        matches!(
            ty,
            TypeRef::Pointer { mutable: true, target }
                if matches!(target.as_ref(), TypeRef::Named { name, .. } if name == expected)
        ),
        "expected `*mut {expected}`, got {ty:?}"
    );
}
