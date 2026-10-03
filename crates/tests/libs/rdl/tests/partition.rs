use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use windows_metadata::HasAttributes;
use windows_rdl::{ArchInput, RdlItemName};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("windows-rdl-{name}-{}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn record(namespace: &str, name: &str, field_type: &str) -> String {
    let segments: Vec<_> = namespace.split('.').collect();
    let mut source = String::from("#[win32]\n");
    for segment in &segments {
        source.push_str(&format!("mod {segment} {{\n"));
    }
    source.push_str(&format!("struct {name} {{\n    value: {field_type},\n}}\n"));
    for _ in segments {
        source.push_str("}\n");
    }
    source
}

fn create_arch(root: &Path, name: &str, bits: i32, files: &[(&str, String)]) -> ArchInput {
    let arch_dir = root.join(name);
    let rdl_dir = arch_dir.join("rdl");
    std::fs::create_dir_all(&rdl_dir).unwrap();
    for (stem, source) in files {
        std::fs::write(rdl_dir.join(format!("{stem}.rdl")), source).unwrap();
    }
    let winmd = arch_dir.join("Windows.Win32.winmd");
    windows_rdl::reader()
        .input(&rdl_dir)
        .output(&winmd)
        .write()
        .unwrap();
    ArchInput {
        rdl_dir,
        winmd,
        bits,
    }
}

fn qualified(namespace: &str, name: &str) -> RdlItemName {
    RdlItemName::new(namespace, name)
}

#[test]
fn qualified_item_names_include_all_namespaces() {
    let scratch = Scratch::new("qualified-item-names");
    let path = scratch.0.join("names.rdl");
    std::fs::write(
        &path,
        r#"
#[win32]
mod Windows {
    mod Win32 {
        struct Root {
            value: u32,
        }
        mod Child {
            struct Shared {
                value: u32,
            }
            #[library("test.dll")]
            extern fn ChildFunction() -> u32;
            const CHILD_CONSTANT: u32 = 1;
            mod Nested {
                struct Deep {
                    value: u32,
                }
            }
        }
    }
}
"#,
    )
    .unwrap();

    let expected = BTreeSet::from([
        qualified("Windows.Win32", "Root"),
        qualified("Windows.Win32.Child", "CHILD_CONSTANT"),
        qualified("Windows.Win32.Child", "ChildFunction"),
        qualified("Windows.Win32.Child", "Shared"),
        qualified("Windows.Win32.Child.Nested", "Deep"),
    ]);
    let first = windows_rdl::qualified_item_names(&path).unwrap();
    let second = windows_rdl::qualified_item_names(&path).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.into_iter().collect::<BTreeSet<_>>(), expected);

    assert_eq!(
        windows_rdl::item_names(&path, "Windows.Win32").unwrap(),
        ["Root"]
    );
    assert_eq!(
        windows_rdl::item_names(&path, "Windows.Win32.Child.Nested").unwrap(),
        ["Deep"]
    );
}

#[test]
fn flat_and_qualified_partitions_are_distinct_modes() {
    let scratch = Scratch::new("partition-modes");
    let source = scratch.0.join("flat.rdl");
    std::fs::write(
        &source,
        r#"
#[win32]
mod Flat {
    struct First {
        value: u32,
    }
    struct Second {
        value: u32,
    }
}
"#,
    )
    .unwrap();
    let winmd = scratch.0.join("flat.winmd");
    windows_rdl::reader()
        .input(&source)
        .output(&winmd)
        .write()
        .unwrap();

    let output = scratch.0.join("flat-output");
    let flat = HashMap::from([
        ("First".to_string(), "one".to_string()),
        ("Second".to_string(), "two".to_string()),
    ]);
    windows_rdl::writer()
        .input(&winmd)
        .partition(flat.clone())
        .output(&output)
        .write()
        .unwrap();
    assert_eq!(
        windows_rdl::item_names(output.join("one.rdl"), "Flat").unwrap(),
        ["First"]
    );
    assert_eq!(
        windows_rdl::item_names(output.join("two.rdl"), "Flat").unwrap(),
        ["Second"]
    );

    let error = windows_rdl::writer()
        .input(&winmd)
        .partition(flat)
        .partition_qualified(HashMap::from([(
            qualified("Flat", "First"),
            "one".to_string(),
        )]))
        .output(scratch.0.join("mixed-output"))
        .write()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("partition and partition_qualified cannot be combined")
    );
}

#[test]
fn arch_merge_restores_qualified_partitions() {
    let scratch = Scratch::new("qualified-arch-merge");
    let x64 = create_arch(
        &scratch.0,
        "x64",
        2,
        &[
            ("root", record("Windows.Win32", "RootValue", "u32")),
            (
                "alpha",
                format!(
                    "{}{}",
                    record("Windows.Win32.Alpha", "Shared", "u64"),
                    record("Windows.Win32.Alpha.Deep", "Shared", "u16")
                ),
            ),
            ("beta", record("Windows.Win32.Beta", "Shared", "u32")),
            (
                "x64_owner",
                record("Windows.Win32.Precedence", "Owner", "u32"),
            ),
            ("outside", record("Outside.Namespace", "Ignored", "u32")),
        ],
    );
    let arm64 = create_arch(
        &scratch.0,
        "arm64",
        4,
        &[
            (
                "arm64_alpha",
                record("Windows.Win32.Alpha", "Shared", "u64"),
            ),
            (
                "arm64_owner",
                record("Windows.Win32.Precedence", "Owner", "u32"),
            ),
            ("arm64_only", record("Windows.Win32.ArmOnly", "Only", "u32")),
        ],
    );
    let x86 = create_arch(
        &scratch.0,
        "x86",
        1,
        &[
            ("x86_alpha", record("Windows.Win32.Alpha", "Shared", "u32")),
            (
                "x86_owner",
                record("Windows.Win32.Precedence", "Owner", "u32"),
            ),
            ("x86_only", record("Windows.Win32.X86Only", "Only", "u32")),
        ],
    );

    let output = scratch.0.join("merged-rdl");
    windows_rdl::merge_arch_rdl(&[x64, arm64, x86], None, &output).unwrap();

    let stems: BTreeSet<_> = std::fs::read_dir(&output)
        .unwrap()
        .flatten()
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        stems,
        BTreeSet::from([
            "alpha".to_string(),
            "arm64_only".to_string(),
            "beta".to_string(),
            "root".to_string(),
            "x64_owner".to_string(),
            "x86_only".to_string(),
        ])
    );

    let mut exported = BTreeSet::new();
    for entry in std::fs::read_dir(&output).unwrap().flatten() {
        exported.extend(windows_rdl::qualified_item_names(entry.path()).unwrap());
    }
    assert_eq!(
        exported,
        BTreeSet::from([
            qualified("Windows.Win32", "RootValue"),
            qualified("Windows.Win32.Alpha", "Shared"),
            qualified("Windows.Win32.Alpha.Deep", "Shared"),
            qualified("Windows.Win32.ArmOnly", "Only"),
            qualified("Windows.Win32.Beta", "Shared"),
            qualified("Windows.Win32.Precedence", "Owner"),
            qualified("Windows.Win32.X86Only", "Only"),
        ])
    );
    assert_eq!(
        windows_rdl::qualified_item_names(output.join("alpha.rdl")).unwrap(),
        [
            qualified("Windows.Win32.Alpha", "Shared"),
            qualified("Windows.Win32.Alpha.Deep", "Shared"),
        ]
    );
    assert_eq!(
        windows_rdl::qualified_item_names(output.join("beta.rdl")).unwrap(),
        [qualified("Windows.Win32.Beta", "Shared")]
    );
    assert_eq!(
        windows_rdl::qualified_item_names(output.join("x64_owner.rdl")).unwrap(),
        [qualified("Windows.Win32.Precedence", "Owner")]
    );

    let recompiled = scratch.0.join("Windows.Win32.recompiled.winmd");
    windows_rdl::reader()
        .input(&output)
        .output(&recompiled)
        .write()
        .unwrap();
    let index = windows_metadata::reader::Index::read(&recompiled).unwrap();
    assert_eq!(index.expect("Windows.Win32", "RootValue").arches(), 2);

    let mut alpha: Vec<_> = index
        .get("Windows.Win32.Alpha", "Shared")
        .map(|ty| {
            (
                ty.arches(),
                ty.fields()
                    .find(|field| field.name() == "value")
                    .unwrap()
                    .ty(),
            )
        })
        .collect();
    alpha.sort_by_key(|(arches, _)| *arches);
    assert_eq!(
        alpha,
        [
            (1, windows_metadata::Type::U32),
            (6, windows_metadata::Type::U64),
        ]
    );
    assert_eq!(
        index.expect("Windows.Win32.Alpha.Deep", "Shared").arches(),
        2
    );
    assert_eq!(index.expect("Windows.Win32.Beta", "Shared").arches(), 2);
    assert_eq!(
        index.expect("Windows.Win32.Precedence", "Owner").arches(),
        0
    );
    assert_eq!(index.expect("Windows.Win32.X86Only", "Only").arches(), 1);
    assert_eq!(index.expect("Windows.Win32.ArmOnly", "Only").arches(), 4);
    assert!(!index.contains("Outside.Namespace", "Ignored"));
}
