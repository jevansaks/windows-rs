use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::process::Command;
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

#[test]
fn mixed_large_macro_cohort_preserves_valid_neighbors() {
    const CHILD: &str = "WINDOWS_CLANG_MIXED_CONSTANT_CHILD";
    if std::env::var_os(CHILD).is_some() {
        run_mixed_large_macro_cohort();
        return;
    }

    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "mixed_large_macro_cohort_preserves_valid_neighbors",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("WINDOWS_CLANG_TIMINGS", "1")
        .output()
        .unwrap();
    let log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{log}");
    let metrics = log
        .lines()
        .find(|line| {
            line.contains("phase=constant-probes ") && line.contains("tu=\"mixed_constants.h\"")
        })
        .unwrap_or_else(|| panic!("missing constant-probe metrics:\n{log}"));
    for (name, expected) in [
        ("candidates", 2050),
        ("evaluated", 2040),
        ("initial_batches", 1),
        ("recovery_batches", 0),
        ("isolation_batches", 0),
        ("singleton_probes", 0),
        ("synthetic_tus", 1),
        ("retry_tus", 0),
    ] {
        assert_eq!(metric(metrics, name), expected, "{metrics}");
    }
}

fn run_mixed_large_macro_cohort() {
    helpers::ensure_libclang();

    const COUNT: usize = 2048;
    let invalid = BTreeSet::from([127, 382, 637, 892, 1151, 1406, 1661, 1916]);
    let mut source = String::from(
        "#define MIXED_BIT_MASK(n) (~((~0) << n))\n\
         typedef enum MIXED_ENUM {\n\
             MIXED_ENUM_VALUE = MIXED_BIT_MASK(5),\n\
         } MIXED_ENUM;\n\
         typedef struct MIXED_PAIR { int first; int second; } MIXED_PAIR;\n\
         MIXED_PAIR mixed_pair(void);\n\
         typedef int (*MIXED_CALLBACK)(int);\n",
    );
    for index in 0..COUNT {
        let value = if invalid.contains(&index) {
            match index % 4 {
                0 => "(1 | 2 4)",
                1 => "mixed_pair()",
                2 => "unsigned long",
                _ => "((void)0)",
            }
            .to_string()
        } else {
            (index + 7).to_string()
        };
        writeln!(source, "#define MIXED_VALUE_{index:04} {value}").unwrap();
    }
    source.push_str(
        "#define MIXED_UNSUPPORTED_CALLBACK ((MIXED_CALLBACK)0)\n\
         #define MIXED_UNSUPPORTED_COMMA 1, 2\n",
    );

    let snapshot = extract(
        [Input::new("mixed_constants.h", source)],
        &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
    )
    .unwrap();
    let constants: BTreeMap<_, _> = snapshot
        .constants()
        .iter()
        .filter(|constant| constant.name.starts_with("MIXED_VALUE_"))
        .map(|constant| (constant.name.as_str(), &constant.value))
        .collect();

    assert_eq!(constants.len(), COUNT - invalid.len());
    for index in 0..COUNT {
        let name = format!("MIXED_VALUE_{index:04}");
        if invalid.contains(&index) {
            assert!(!constants.contains_key(name.as_str()), "{name}");
        } else {
            assert_eq!(
                constants.get(name.as_str()),
                Some(&&Value::Signed((index + 7) as i64)),
                "{name}"
            );
        }
    }
    assert!(
        snapshot.constants().iter().all(|constant| {
            constant.name != "MIXED_UNSUPPORTED_CALLBACK"
                && constant.name != "MIXED_UNSUPPORTED_COMMA"
        }),
        "{:#?}",
        snapshot.constants()
    );
}

fn metric(line: &str, name: &str) -> usize {
    let prefix = format!("{name}=");
    line.split_ascii_whitespace()
        .find_map(|part| part.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("missing metric `{name}` in `{line}`"))
        .parse()
        .unwrap()
}
