use windows_clang::{Input, extract};

#[test]
fn x86_raw_mangling_is_separate_from_legacy_link_name() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [Input::new(
            "raw-mangling.hpp",
            "extern \"C\" int __cdecl CCall(int value);\n\
             extern \"C\" int __stdcall SystemCall(int value);\n",
        )],
        &["-x", "c++", "--target=i686-pc-windows-msvc"],
    )
    .unwrap();

    let identities = snapshot
        .function_source_identities()
        .map(|identity| (identity.name, identity.link_name, identity.raw_link_name))
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        [
            ("CCall", "CCall", "_CCall"),
            ("SystemCall", "SystemCall", "_SystemCall@4"),
        ]
    );
    assert_eq!(
        snapshot.resolve_function_link_name("_CCall").unwrap(),
        Some("CCall")
    );
    assert_eq!(
        snapshot
            .resolve_function_link_name("_SystemCall@4")
            .unwrap(),
        Some("SystemCall")
    );
}

#[test]
fn equivalent_raw_mangling_duplicates_are_retained() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [
            Input::new(
                "duplicate-a.hpp",
                "extern \"C\" int __cdecl Duplicate(int value);\n",
            ),
            Input::new(
                "duplicate-b.hpp",
                "extern \"C\" int __cdecl Duplicate(int value);\n",
            ),
        ],
        &["-x", "c++", "--target=i686-pc-windows-msvc"],
    )
    .unwrap();

    let identities = snapshot
        .function_source_identities()
        .map(|identity| {
            (
                identity.origin.tu.as_str(),
                identity.spelling.file.as_str(),
                identity.link_name,
                identity.raw_link_name,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        [
            (
                "duplicate-a.hpp",
                "duplicate-a.hpp",
                "Duplicate",
                "_Duplicate",
            ),
            (
                "duplicate-b.hpp",
                "duplicate-b.hpp",
                "Duplicate",
                "_Duplicate",
            ),
        ]
    );
    assert_eq!(
        snapshot.resolve_function_link_name("_Duplicate").unwrap(),
        Some("Duplicate")
    );
}

#[test]
fn distinct_raw_decorations_for_one_link_name_are_rejected() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [
            Input::new(
                "cdecl.hpp",
                "extern \"C\" int __cdecl Conflict(int value);\n",
            ),
            Input::new(
                "stdcall.hpp",
                "extern \"C\" int __stdcall Conflict(int value);\n",
            ),
        ],
        &["-x", "c++", "--target=i686-pc-windows-msvc"],
    )
    .unwrap();

    let identities = snapshot
        .function_source_identities()
        .map(|identity| (identity.link_name, identity.raw_link_name))
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        [("Conflict", "_Conflict"), ("Conflict", "_Conflict@4"),]
    );
    let error = snapshot
        .resolve_function_link_name("_Conflict")
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "normalized linker symbol `Conflict` has conflicting raw COFF symbols: `_Conflict`, \
         `_Conflict@4`"
    );
}

#[test]
fn one_raw_mangling_for_distinct_link_names_is_rejected() {
    helpers::ensure_libclang();

    let snapshot = extract(
        [
            Input::new(
                "normalized.hpp",
                "extern \"C\" int __cdecl Shared(int value);\n",
            ),
            Input::new(
                "asm-label.hpp",
                "extern \"C\" int __cdecl Other(int value) asm(\"_Shared\");\n",
            ),
        ],
        &["-x", "c++", "--target=i686-pc-windows-msvc"],
    )
    .unwrap();

    let identities = snapshot
        .function_source_identities()
        .map(|identity| (identity.name, identity.link_name, identity.raw_link_name))
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        [
            ("Other", "_Shared", "_Shared"),
            ("Shared", "Shared", "_Shared"),
        ]
    );
    let error = snapshot.resolve_function_link_name("_Shared").unwrap_err();
    assert_eq!(
        error.to_string(),
        "raw COFF linker symbol `_Shared` maps to multiple normalized linker symbols: `Shared`, \
         `_Shared`"
    );
}
