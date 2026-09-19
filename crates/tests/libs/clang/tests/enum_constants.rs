use std::path::{Path, PathBuf};
use windows_metadata::{self as metadata, HasAttributes};

fn fixture(name: &str, source: &str) -> PathBuf {
    let dir = Path::new(env!("OUT_DIR")).join("enum_constants").join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("input.h"), source).unwrap();
    dir
}

fn compile(rdl: &Path, image: &Path) {
    windows_rdl::reader()
        .input(rdl)
        .input_text(windows_rdl::WIN32_METADATA_RDL)
        .reference_default()
        .output(image)
        .write()
        .unwrap();
}

fn verify_high_bits(image: &Path) {
    let index = metadata::reader::Index::read(image).unwrap();
    for (name, variant, ty, value) in [
        (
            "FLAGS8",
            "HIGH8",
            metadata::Type::I8,
            metadata::Value::I8(-128),
        ),
        (
            "FLAGS16",
            "HIGH16",
            metadata::Type::U16,
            metadata::Value::U16(0x8000),
        ),
        (
            "FLAGS32",
            "HIGH32",
            metadata::Type::I32,
            metadata::Value::I32(i32::MIN),
        ),
        (
            "FLAGS64",
            "HIGH64",
            metadata::Type::I64,
            metadata::Value::I64(i64::MIN),
        ),
    ] {
        let group = index.expect("Test", name);
        assert!(group.has_attribute("FlagsAttribute"));
        assert_eq!(
            group.fields().find(|f| f.name() == "value__").unwrap().ty(),
            ty
        );
        let field = group.fields().find(|f| f.name() == variant).unwrap();
        assert_eq!(field.constant().unwrap().value(), value);
    }
    let fields: Vec<_> = index.expect("Test", "Apis").fields().collect();
    assert_eq!(
        fields.iter().map(|f| f.name()).collect::<Vec<_>>(),
        ["HIGH16_ALIAS", "UNRELATED"]
    );
    assert_eq!(
        fields[0].constant().unwrap().value(),
        metadata::Value::I32(0x8000)
    );
    assert_eq!(
        fields[1].constant().unwrap().value(),
        metadata::Value::I32(9)
    );
}

#[test]
fn flag_high_bits_drop_only_matching_loose_names_all_arches() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture(
        "high_bits",
        r#"
        enum [[clang::flag_enum]] FLAGS8 : signed char { HIGH8 = (signed char)0x80 };
        typedef unsigned short WORD;
        #define HIGH16 0x8000
        #pragma push_macro("HIGH16")
        #undef HIGH16
        enum [[clang::flag_enum]] FLAGS16 : WORD { HIGH16 = 0x8000 };
        #pragma pop_macro("HIGH16")
        enum [[clang::flag_enum]] FLAGS32 : int { HIGH32 = (int)0x80000000U };
        enum [[clang::flag_enum]] FLAGS64 : long long { HIGH64 = (long long)0x8000000000000000ULL };
        #define HIGH8 0x80
        #define HIGH32 0x80000000U
        #define HIGH64 0x8000000000000000ULL
        #define HIGH16_ALIAS HIGH16
        #define UNRELATED 9
        "#,
    );
    let mut inputs = vec![];
    for name in ["x64", "x86", "arm64"] {
        let arch = windows_clang::Arch::known(name).unwrap();
        let rdl = dir.join(name);
        let image = dir.join(format!("{name}.winmd"));
        windows_clang::clang()
            .args(["-x", "c++", "-fms-extensions"])
            .target(&arch.triple)
            .input(dir.join("input.h"))
            .namespace("Test")
            .output(&rdl)
            .write_by_header()
            .unwrap();
        compile(&rdl, &image);
        verify_high_bits(&image);
        inputs.push(windows_rdl::ArchInput {
            rdl_dir: rdl,
            winmd: image,
            bits: arch.bits,
        });
    }
    let merged = dir.join("merged");
    windows_rdl::merge_arch_rdl(&inputs, None, "Test", &merged).unwrap();
    let image = dir.join("merged.winmd");
    compile(&merged, &image);
    verify_high_bits(&image);
}

#[test]
fn differing_values_and_signed_nonflags_keep_loose_constants() {
    let _guard = test_clang::libclang_guard();
    let dir = fixture(
        "different_values",
        r#"
        enum [[clang::flag_enum]] FLAGS : short { DIFFERENT = (short)0x8000, WIDER = 0 };
        enum SIGNED : int { NEGATIVE = -1 };
        enum WIDE : long long { LOW_BITS = 0x100000001LL };
        #define DIFFERENT 0x8001
        #define WIDER 0x10000
        #define NEGATIVE 0xffffffffU
        #define LOW_BITS 1
        "#,
    );
    let rdl = dir.join("rdl");
    windows_clang::clang()
        .args(["-x", "c++", "-fms-extensions"])
        .target("x86_64-pc-windows-msvc")
        .input(dir.join("input.h"))
        .namespace("Test")
        .output(&rdl)
        .write_by_header()
        .unwrap();
    let image = dir.join("output.winmd");
    compile(&rdl, &image);
    let index = metadata::reader::Index::read(&image).unwrap();
    let fields: Vec<_> = index.expect("Test", "Apis").fields().collect();
    assert_eq!(fields.len(), 4);
    for (name, value) in [
        ("DIFFERENT", metadata::Value::I32(0x8001)),
        ("WIDER", metadata::Value::I32(0x10000)),
        ("NEGATIVE", metadata::Value::U32(u32::MAX)),
        ("LOW_BITS", metadata::Value::I32(1)),
    ] {
        let field = fields.iter().find(|f| f.name() == name).unwrap();
        assert_eq!(field.constant().unwrap().value(), value);
    }
}
