#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    let _ = windows_rdl::reader()
        .input_text_named("fuzz.rdl", source)
        .check_all();
});
