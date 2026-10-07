use windows_clang::{Input, Value, extract};

#[test]
fn malformed_macro_expressions_are_not_accepted_from_recovered_prefixes() {
    helpers::ensure_libclang();

    let source = concat!(
        "#define WTS_SECURITY_SET_INFORMATION 0x00000002\n",
        "#define WTS_SECURITY_RESET 0x00000004\n",
        "#define WTS_SECURITY_VIRTUAL_CHANNELS 0x00000008\n",
        "#define WTS_SECURITY_LOGOFF 0x00000040\n",
        "#define WTS_SECURITY_DISCONNECT 0x00000200\n",
        "#define WTS_SECURITY_CURRENT_USER_ACCESS ",
        "(WTS_SECURITY_SET_INFORMATION | WTS_SECURITY_RESET \\\n",
        "             WTS_SECURITY_VIRTUAL_CHANNELS | WTS_SECURITY_LOGOFF \\\n",
        "             WTS_SECURITY_DISCONNECT)\n",
        "#define WTS_SECURITY_CORRECTED_USER_ACCESS ",
        "(WTS_SECURITY_SET_INFORMATION | WTS_SECURITY_RESET | \\\n",
        "             WTS_SECURITY_VIRTUAL_CHANNELS | WTS_SECURITY_LOGOFF | \\\n",
        "             WTS_SECURITY_DISCONNECT)\n",
        "#define WTS_SECURITY_CAST_ACCESS \\\n",
        "    ((unsigned long)(WTS_SECURITY_SET_INFORMATION | WTS_SECURITY_RESET))\n",
        "#define WTS_SECURITY_ALIAS_ACCESS WTS_SECURITY_CORRECTED_USER_ACCESS\n",
        "#define WTS_SECURITY_PAREN_ACCESS \\\n",
        "    (((WTS_SECURITY_SET_INFORMATION) | (WTS_SECURITY_RESET)))\n",
    );
    let snapshot = extract(
        [Input::new("WtsApi32.h", source)],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let constants = snapshot.constants();
    let rdl = snapshot.emit("ConstantExpressions").unwrap();

    assert!(
        constants
            .iter()
            .all(|constant| constant.name != "WTS_SECURITY_CURRENT_USER_ACCESS"),
        "{constants:#?}"
    );
    assert!(!rdl.contains("WTS_SECURITY_CURRENT_USER_ACCESS"), "{rdl}");
    assert!(
        rdl.contains("const WTS_SECURITY_CORRECTED_USER_ACCESS: i32 = 590"),
        "{rdl}"
    );
    for (name, value) in [
        ("WTS_SECURITY_CORRECTED_USER_ACCESS", Value::Signed(590)),
        ("WTS_SECURITY_CAST_ACCESS", Value::Unsigned(6)),
        ("WTS_SECURITY_ALIAS_ACCESS", Value::Signed(590)),
        ("WTS_SECURITY_PAREN_ACCESS", Value::Signed(6)),
    ] {
        assert_eq!(
            constants
                .iter()
                .find(|constant| constant.name == name)
                .map(|constant| &constant.value),
            Some(&value),
            "{constants:#?}"
        );
    }
}
