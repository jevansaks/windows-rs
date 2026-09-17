fn temp_path(name: &str, extension: &str) -> String {
    std::env::temp_dir()
        .join(format!("windows_rdl_{name}.{extension}"))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn default_input_resolves_default_metadata() {
    windows_rdl::reader()
        .input_text(
            r#"
use Windows::Foundation::*;

#[winrt]
mod Test {
    struct Wrapper {
        value: Point,
    }
}
"#,
        )
        .reference_default()
        .output(temp_path("default_input", "winmd"))
        .write()
        .unwrap();
}

#[test]
fn default_input_resolves_win32_pseudo_attributes() {
    let winmd = temp_path("default_win32_pseudo_attributes", "winmd");
    let rdl = temp_path("default_win32_pseudo_attributes", "rdl");

    windows_rdl::reader()
        .input_text(
            r#"
#[win32]
mod Test {
    #[repr(i32)]
    #[associated_constant("VALUE_ALL")]
    enum VALUE {
        VALUE_NONE = 0,
    }

    const VALUE_ALL: i32 = 1;

    #[supported_os("windows5.0")]
    extern fn Open(#[raii_free("Close")] #[invalid_handle(-1)] #[invalid_handle(0)] value: *mut isize);
}
"#,
        )
        .input_text(windows_rdl::WIN32_METADATA_RDL)
        .reference_default()
        .output(&winmd)
        .write()
        .unwrap();

    let index = windows_metadata::reader::Index::read(&winmd).unwrap();
    index.expect("Windows.Win32.Foundation.Metadata", "MemorySizeAttribute");

    windows_rdl::writer()
        .input(&winmd)
        .output(&rdl)
        .write()
        .unwrap();

    let output = std::fs::read_to_string(rdl).unwrap();
    assert!(output.contains("#[invalid_handle(-1)]"));
    assert!(output.contains("#[invalid_handle(0)]"));
}

#[test]
fn reference_bytes_resolve_metadata() {
    let reference = temp_path("reference_bytes_reference", "winmd");

    windows_rdl::reader()
        .input_text(
            r#"
#[winrt]
mod Other {
    struct Point {
        x: i32,
        y: i32,
    }
}
"#,
        )
        .output(&reference)
        .write()
        .unwrap();

    let bytes = std::fs::read(reference).unwrap();
    windows_rdl::reader()
        .input_text(
            r#"
use Other::*;

#[winrt]
mod Test {
    struct Wrapper {
        value: Point,
    }
}
"#,
        )
        .reference_byte_sets([bytes])
        .output(temp_path("reference_bytes", "winmd"))
        .write()
        .unwrap();
}

#[test]
fn reference_path_resolves_metadata() {
    let reference = temp_path("reference_path_reference", "winmd");

    windows_rdl::reader()
        .input_text(
            r#"
#[winrt]
mod Other {
    struct Point {
        x: i32,
        y: i32,
    }
}
"#,
        )
        .output(&reference)
        .write()
        .unwrap();

    windows_rdl::reader()
        .input_text(
            r#"
use Other::*;

#[winrt]
mod Test {
    struct Wrapper {
        value: Point,
    }
}
"#,
        )
        .reference(&reference)
        .output(temp_path("reference_path", "winmd"))
        .write()
        .unwrap();
}

#[test]
fn writer_accepts_metadata_bytes() {
    let winmd = temp_path("writer_bytes_input", "winmd");
    let rdl = temp_path("writer_bytes_output", "rdl");

    windows_rdl::reader()
        .input_text(
            r#"
#[win32]
mod Test {
    struct Value {
        value: u32,
    }
}
"#,
        )
        .output(&winmd)
        .write()
        .unwrap();

    let bytes = std::fs::read(&winmd).unwrap();
    windows_rdl::writer()
        .input_byte_sets([bytes])
        .output(&rdl)
        .write()
        .unwrap();

    assert!(
        std::fs::read_to_string(rdl)
            .unwrap()
            .contains("struct Value")
    );
}
