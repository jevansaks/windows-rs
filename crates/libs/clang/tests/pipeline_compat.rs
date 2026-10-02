use windows_clang::{Input, extract};

#[test]
fn pipeline_scalar_aliases_use_their_abi_shapes() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [Input::new(
            "pipeline_aliases.h",
            "typedef long long HNSTIME;\n\
             typedef unsigned long mdToken;\n\
             typedef mdToken mdFieldDef;\n\
             typedef mdToken mdMemberRef;\n\
             typedef unsigned long SCRIPTTHREADID;\n\
             typedef unsigned char COR_SIGNATURE;\n\
             typedef COR_SIGNATURE* PCOR_SIGNATURE;\n\
             typedef const COR_SIGNATURE* PCCOR_SIGNATURE;\n\
             typedef void* HCORENUM;\n\
             typedef const char* MDUTF8CSTR;\n\
             typedef struct PIPELINE_TYPES {\n\
                 HNSTIME time;\n\
                 mdFieldDef field;\n\
                 mdMemberRef member;\n\
                 SCRIPTTHREADID thread;\n\
                 PCOR_SIGNATURE mutable_signature;\n\
                 PCCOR_SIGNATURE signature;\n\
                 HCORENUM enumerator;\n\
                 MDUTF8CSTR name;\n\
             } PIPELINE_TYPES;\n",
        )],
        &["-x", "c++"],
    )
    .unwrap();
    let rdl = snapshot.emit("Pipeline").unwrap();

    assert!(rdl.contains("time: i64"), "{rdl}");
    assert!(rdl.contains("field: u32"), "{rdl}");
    assert!(rdl.contains("member: u32"), "{rdl}");
    assert!(rdl.contains("thread: u32"), "{rdl}");
    assert!(rdl.contains("mutable_signature: *mut u8"), "{rdl}");
    assert!(rdl.contains("signature: *const u8"), "{rdl}");
    assert!(rdl.contains("enumerator: *mut void"), "{rdl}");
    assert!(rdl.contains("name: *const i8"), "{rdl}");
    assert!(!rdl.contains("type HNSTIME"), "{rdl}");
    assert!(!rdl.contains("type mdToken"), "{rdl}");
}

#[test]
fn colliding_type_and_constant_names_are_both_emitted() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [Input::new(
            "colliding_names.h",
            "typedef struct AE_ACLMOD { unsigned long action; } AE_ACLMOD;\n\
             #define AE_ACLMOD 12\n",
        )],
        &["-x", "c++"],
    )
    .unwrap();
    let rdl = snapshot.emit("Pipeline").unwrap();

    assert!(rdl.contains("struct AE_ACLMOD"), "{rdl}");
    assert!(rdl.contains("const AE_ACLMOD: i32 = 12"), "{rdl}");
}

#[test]
fn recoverable_msvc_enum_mask_diagnostic_is_accepted() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [Input::new(
            "negative_shift.h",
            "#define TEST_BIT_MASK(n) (~((~0) << n))\n\
             typedef enum TEST_VALUES {\n\
                 TEST_VALUE = TEST_BIT_MASK(5),\n\
             } TEST_VALUES;\n",
        )],
        &["-x", "c++", "-std=c++17"],
    )
    .unwrap();
    let rdl = snapshot.emit("Pipeline").unwrap();

    assert!(rdl.contains("enum TEST_VALUES"), "{rdl}");
    assert!(rdl.contains("TEST_VALUE = 0"), "{rdl}");
}
