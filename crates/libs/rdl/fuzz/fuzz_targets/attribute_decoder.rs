#![no_main]

mod support;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let bytes = support::metadata_input(data);
    let Ok(file) = windows_metadata::reader::File::try_new(bytes) else {
        return;
    };
    let index = windows_metadata::reader::Index::new(vec![file]);

    for attribute in index.attributes() {
        let _ = attribute.try_args();
        let _ = attribute.try_value();
    }
});
