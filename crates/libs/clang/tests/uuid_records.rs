use windows_clang::{
    EmitOptions, FactData, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition,
    extract,
};
use windows_metadata::{
    Value,
    reader::{HasAttributes, TypeCategory},
};

const NAMESPACE: &str = "Windows.Win32.System.Iis";

const SUPPORT_RDL: &str = r#"
#[win32]
mod Windows {
    mod Win32 {
        mod System {
            mod Iis {
                type PCWSTR = *const u16;
            }
        }
    }
}
"#;

const SOURCE: &str = r#"
typedef unsigned short WCHAR;
typedef WCHAR *BSTR;
typedef const WCHAR *LPCWSTR;
typedef unsigned long DWORD;
typedef unsigned long long ULONGLONG;

struct GUID {
    unsigned long Data1;
    unsigned short Data2;
    unsigned short Data3;
    unsigned char Data4[8];
};

struct FILETIME {
    DWORD dwLowDateTime;
    DWORD dwHighDateTime;
};

struct __declspec(uuid("11111111-1111-1111-1111-111111111111"))
CONFIGURATION_ENTRY {
    BSTR key;
    BSTR value;
};

class __declspec(uuid("22222222-2222-2222-2222-222222222222"))
LOGGING_PARAMETERS {
public:
    LPCWSTR path;
    DWORD flags;
};

struct __declspec(uuid("33333333-3333-3333-3333-333333333333"))
FORWARD_DATA;

struct __declspec(uuid("33333333-3333-3333-3333-333333333333"))
FORWARD_DATA {
    DWORD value;
};

struct __declspec(uuid("44444444-4444-4444-4444-444444444444"))
PRE_PROCESS_PARAMETERS {
    LPCWSTR pszSessionId;
    LPCWSTR pszSiteName;
    LPCWSTR pszUserName;
    LPCWSTR pszHostName;
    LPCWSTR pszRemoteIpAddress;
    DWORD dwRemoteIpPort;
    LPCWSTR pszLocalIpAddress;
    DWORD dwLocalIpPort;
    LPCWSTR pszCommand;
    LPCWSTR pszCommandParameters;
    FILETIME SessionStartTime;
    ULONGLONG BytesSentPerSession;
    ULONGLONG BytesReceivedPerSession;
};

struct __declspec(uuid("55555555-5555-5555-5555-555555555555"))
POST_PROCESS_PARAMETERS {
    const PRE_PROCESS_PARAMETERS *pPreProcessParameters;
    DWORD dwErrorCode;
};

struct __declspec(uuid("00000000-0000-0000-c000-000000000046"))
IUnknown {
    virtual long QueryInterface(const GUID &iid, void **object) = 0;
};

struct __declspec(uuid("66666666-6666-6666-6666-666666666666"))
IFtpPreprocessProvider : IUnknown {
    virtual long Handle(const PRE_PROCESS_PARAMETERS *parameters) = 0;
};

typedef class FtpProvider FtpProvider;
class __declspec(uuid("77777777-7777-7777-7777-777777777777"))
FtpProvider;
"#;

#[test]
fn uuid_data_records_preserve_record_shape_and_guid() {
    helpers::ensure_libclang();

    let scratch =
        std::env::temp_dir().join(format!("windows-clang-uuid-records-{}", std::process::id()));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    let header = scratch.join("ftpext.h");
    std::fs::write(&header, SOURCE).unwrap();

    for (target, size, align, offsets) in [
        (
            "i686-pc-windows-msvc",
            64,
            8,
            vec![0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 384, 448],
        ),
        (
            "x86_64-pc-windows-msvc",
            104,
            8,
            vec![0, 64, 128, 192, 256, 320, 384, 448, 512, 576, 640, 704, 768],
        ),
    ] {
        let snapshot = extract(
            [Input::new(
                "aggregate.cpp",
                format!("#include \"{}\"\n", header.to_string_lossy()),
            )
            .with_roots([header.to_string_lossy().to_string()])],
            &[
                "-x",
                "c++",
                "-std=c++17",
                "-fms-extensions",
                &format!("--target={target}"),
            ],
        )
        .unwrap();

        assert_record(&snapshot, "CONFIGURATION_ENTRY", 2);
        assert_record(&snapshot, "LOGGING_PARAMETERS", 2);
        assert_record(&snapshot, "FORWARD_DATA", 1);
        assert_record(&snapshot, "POST_PROCESS_PARAMETERS", 2);

        let pre = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "PRE_PROCESS_PARAMETERS" && fact.definition)
            .unwrap();
        let FactData::Record {
            fields,
            size: actual_size,
            align: actual_align,
            ..
        } = &pre.data
        else {
            panic!("{pre:#?}");
        };
        assert_eq!((*actual_size, *actual_align), (size, align));
        assert_eq!(
            fields.iter().map(|field| field.offset).collect::<Vec<_>>(),
            offsets
        );

        for name in ["IUnknown", "IFtpPreprocessProvider"] {
            let fact = snapshot
                .facts()
                .iter()
                .find(|fact| fact.name == name && fact.definition)
                .unwrap();
            assert!(matches!(fact.data, FactData::Interface { .. }), "{fact:#?}");
        }
        let coclass = snapshot
            .facts()
            .iter()
            .find(|fact| fact.name == "FtpProvider" && matches!(fact.data, FactData::Class { .. }))
            .unwrap();
        assert!(
            matches!(coclass.data, FactData::Class { .. }),
            "{coclass:#?}"
        );

        if target == "x86_64-pc-windows-msvc" {
            emit_and_read_back(&snapshot, &header, &scratch);
        }
    }

    std::fs::remove_dir_all(scratch).unwrap();
}

fn assert_record(snapshot: &windows_clang::Snapshot, name: &str, expected_fields: usize) {
    let facts = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.name == name)
        .collect::<Vec<_>>();
    assert!(!facts.is_empty(), "{name}");
    for fact in facts {
        let FactData::Record { fields, .. } = &fact.data else {
            panic!("{fact:#?}");
        };
        if fact.definition {
            assert_eq!(fields.len(), expected_fields);
        }
    }
}

fn emit_and_read_back(
    snapshot: &windows_clang::Snapshot,
    header: &std::path::Path,
    scratch: &std::path::Path,
) {
    let policy = HeaderPartitionPolicy::new().with_traversed_header(
        header.to_string_lossy(),
        RootPartition::new("iis", NAMESPACE),
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
    let rdl = partitions.values().cloned().collect::<String>();

    for guid in [
        "0x11111111_1111_1111_1111_111111111111",
        "0x22222222_2222_2222_2222_222222222222",
        "0x33333333_3333_3333_3333_333333333333",
        "0x44444444_4444_4444_4444_444444444444",
        "0x55555555_5555_5555_5555_555555555555",
    ] {
        assert!(rdl.contains(&format!("#[guid({guid})]")), "{rdl}");
    }
    assert!(rdl.contains("struct PRE_PROCESS_PARAMETERS"), "{rdl}");
    assert!(rdl.contains("struct POST_PROCESS_PARAMETERS"), "{rdl}");
    assert!(rdl.contains("interface IFtpPreprocessProvider: IUnknown"));
    assert!(rdl.contains("const FtpProvider: GUID = 0x77777777_7777_7777_7777_777777777777;"));
    assert!(!rdl.contains("struct FtpProvider"));

    let winmd = scratch.join("uuid-records.winmd");
    let mut compiler = windows_rdl::reader();
    compiler.input_text(SUPPORT_RDL);
    for output in partitions.values() {
        compiler.input_text(output);
    }
    compiler.reference_default().output(&winmd).write().unwrap();
    let index = windows_metadata::reader::Index::read(&winmd).unwrap();

    let pre = index.expect(NAMESPACE, "PRE_PROCESS_PARAMETERS");
    assert_eq!(pre.category(), TypeCategory::Struct);
    assert_eq!(pre.fields().len(), 13);
    assert_guid(
        pre.find_attribute("GuidAttribute").unwrap(),
        0x44444444,
        0x4444,
        0x4444,
        0x44,
    );

    let post = index.expect(NAMESPACE, "POST_PROCESS_PARAMETERS");
    assert_eq!(post.category(), TypeCategory::Struct);
    assert_eq!(post.fields().len(), 2);
    assert_guid(
        post.find_attribute("GuidAttribute").unwrap(),
        0x55555555,
        0x5555,
        0x5555,
        0x55,
    );

    let interface = index.expect(NAMESPACE, "IFtpPreprocessProvider");
    assert_eq!(interface.category(), TypeCategory::Interface);
    assert_guid(
        interface.find_attribute("GuidAttribute").unwrap(),
        0x66666666,
        0x6666,
        0x6666,
        0x66,
    );
}

fn assert_guid(
    attribute: windows_metadata::reader::Attribute<'_>,
    data1: u32,
    data2: u16,
    data3: u16,
    data4: u8,
) {
    assert_eq!(attribute.namespace(), "Windows.Foundation.Metadata");
    assert_eq!(
        attribute.value(),
        [
            Value::U32(data1),
            Value::U16(data2),
            Value::U16(data3),
            Value::U8(data4),
            Value::U8(data4),
            Value::U8(data4),
            Value::U8(data4),
            Value::U8(data4),
            Value::U8(data4),
            Value::U8(data4),
            Value::U8(data4),
        ]
        .into_iter()
        .map(|value| (String::new(), value))
        .collect::<Vec<_>>()
    );
}
