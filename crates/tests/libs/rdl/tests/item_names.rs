#[test]
fn includes_descendant_namespaces() {
    let path =
        std::env::temp_dir().join(format!("windows-rdl-item-names-{}.rdl", std::process::id()));
    std::fs::write(
        &path,
        r#"
#[win32]
mod Windows {
    mod Win32 {
        mod Foundation {
            struct FOUNDATION_TYPE {
                value: i32,
            }
        }
        mod System {
            mod Power {
                const POWER_VALUE: i32 = 1;
            }
        }
    }
}
"#,
    )
    .unwrap();

    let mut names = windows_rdl::item_names(&path, "Windows.Win32").unwrap();
    names.sort();
    assert_eq!(names, ["FOUNDATION_TYPE", "POWER_VALUE"]);
    std::fs::remove_file(path).ok();
}
