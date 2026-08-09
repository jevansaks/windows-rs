use std::sync::OnceLock;
use windows_metadata as metadata;

pub fn metadata_input(data: &[u8]) -> Vec<u8> {
    let Some(mutations) = data.strip_prefix(b"seed") else {
        return data.to_vec();
    };

    let mut bytes = metadata_seed().clone();
    for mutation in mutations.chunks_exact(3) {
        let offset = usize::from(u16::from_le_bytes([mutation[0], mutation[1]])) % bytes.len();
        bytes[offset] ^= mutation[2];
    }
    bytes
}

fn metadata_seed() -> &'static Vec<u8> {
    static SEED: OnceLock<Vec<u8>> = OnceLock::new();
    SEED.get_or_init(|| {
        let mut file = metadata::writer::File::new("fuzz");
        let target = file.TypeDef(
            "Fuzz",
            "Target",
            metadata::writer::TypeDefOrRef::default(),
            metadata::TypeAttributes::Public,
        );
        let attribute = file.TypeRef("Fuzz", "MarkerAttribute");
        let constructor = file.MemberRef(
            ".ctor",
            &metadata::Signature {
                flags: metadata::MethodCallAttributes::HASTHIS,
                return_type: metadata::Type::Void,
                types: vec![metadata::Type::String],
            },
            metadata::writer::MemberRefParent::TypeRef(attribute),
        );
        file.Attribute(
            metadata::writer::HasAttribute::TypeDef(target),
            metadata::writer::AttributeType::MemberRef(constructor),
            &[(String::new(), metadata::Value::Utf8("seed".to_string()))],
        );
        file.into_stream()
    })
}
