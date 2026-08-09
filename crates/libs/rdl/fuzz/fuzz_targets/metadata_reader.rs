#![no_main]

mod support;

use libfuzzer_sys::fuzz_target;
use windows_metadata::HasAttributes;

fuzz_target!(|data: &[u8]| {
    let bytes = support::metadata_input(data);
    let Ok(file) = windows_metadata::reader::File::try_new(bytes) else {
        return;
    };
    let index = windows_metadata::reader::Index::new(vec![file]);

    for (_, _, ty) in index.iter() {
        let _ = ty.flags();
        let _ = ty.extends();
        for field in ty.fields() {
            let _ = field.ty();
            let _ = field.attributes().count();
        }
        for method in ty.methods() {
            let _ = method.signature(&[]);
            let _ = method.attributes().count();
        }
        for attribute in ty.attributes() {
            let _ = attribute.try_args();
        }
    }
    let _ = windows_metadata::validator::validate(&index);
});
