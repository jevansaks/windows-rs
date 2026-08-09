#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(formatted) = windows_rdl::formatter::format_named("fuzz.rdl", source) {
        let second = windows_rdl::formatter::format_named("fuzz.rdl", &formatted).unwrap();
        assert_eq!(formatted, second);
    }
});
