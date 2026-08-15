use windows_metadata::HasAttributes;
use windows_metadata::reader::{Index, Item};

#[test]
fn project_as_round_trips_to_winmd() {
    let scratch =
        std::env::temp_dir().join(format!("windows_rdl_project_as_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let input = scratch.join("project_as.rdl");
    let output = scratch.join("project_as.winmd");

    std::fs::write(
        &input,
        r#"
#[win32]
mod Windows {
    mod Win32 {
        mod Metadata {
            attribute ProjectAsAttribute {
                fn(typeName: String);
            }
        }
        type BOOL = i32;
        struct USE_SITES {
            #[project_as("BOOL")]
            field: i32,
        }
        #[library("test.dll")]
        extern fn Projected(
            #[project_as("BOOL")] value: i32
        ) -> #[project_as("BOOL")] i32;
    }
}
"#,
    )
    .unwrap();

    windows_rdl::reader()
        .input(&input)
        .output(&output)
        .write()
        .unwrap();

    let index = Index::read(output.to_string_lossy().as_ref()).unwrap();
    let Item::Type(use_sites) = index.expect_item("Windows.Win32", "USE_SITES") else {
        panic!("USE_SITES should be emitted");
    };
    let field = use_sites.fields().next().expect("field should be emitted");
    let attribute = field
        .find_attribute("ProjectAsAttribute")
        .expect("field ProjectAsAttribute should be emitted");
    assert_eq!(
        attribute.value(),
        &[(String::new(), windows_metadata::Value::Utf8("BOOL".into()))]
    );

    let Item::Fn(projected) = index.expect_item("Windows.Win32", "Projected") else {
        panic!("Projected should be emitted");
    };
    let params = projected.params_by_sequence(1).unwrap();
    assert!(
        params.params()[0]
            .expect("parameter row should be emitted")
            .has_attribute("ProjectAsAttribute")
    );
    assert!(
        params
            .return_param()
            .expect("return row should be emitted")
            .has_attribute("ProjectAsAttribute")
    );
}
