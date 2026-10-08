use windows_metadata::*;

fn explicit_layout_winmd(dir: &std::path::Path, name: &str, offsets: [Option<u32>; 2]) -> String {
    let mut file = writer::File::new(name);
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef(
        "Test",
        "EXPLICIT",
        value_type,
        TypeAttributes::ExplicitLayout | TypeAttributes::Sealed | TypeAttributes::Public,
    );
    let first = file.Field("first", &Type::I32, FieldAttributes::Public);
    if let Some(offset) = offsets[0] {
        file.FieldLayout(first, offset);
    }
    let second = file.Field("second", &Type::U64, FieldAttributes::Public);
    if let Some(offset) = offsets[1] {
        file.FieldLayout(second, offset);
    }
    let out = dir.join(format!("{name}.winmd"));
    std::fs::write(&out, file.into_stream()).unwrap();
    out.to_string_lossy().into_owned()
}

fn explicit_class_winmd(dir: &std::path::Path, name: &str, offset: u32) -> String {
    let mut file = writer::File::new(name);
    let object = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "Object"));
    file.TypeDef(
        "Test",
        "EXPLICIT_CLASS",
        object,
        TypeAttributes::ExplicitLayout | TypeAttributes::Public,
    );
    let field = file.Field("value", &Type::I32, FieldAttributes::Public);
    file.FieldLayout(field, offset);
    let out = dir.join(format!("{name}.winmd"));
    std::fs::write(&out, file.into_stream()).unwrap();
    out.to_string_lossy().into_owned()
}

fn arch_value<'a, R: HasAttributes<'a>>(row: R) -> i32 {
    row.attributes()
        .find_map(|attribute| {
            (attribute.ctor().parent().name() == "SupportedArchitectureAttribute").then(|| {
                match attribute.value().first() {
                    Some((_, Value::I32(value))) => *value,
                    _ => 0,
                }
            })
        })
        .unwrap_or(0)
}

fn merged_layout_variants(
    output: &std::path::Path,
    inputs: &[(&str, i32)],
) -> Vec<(i32, Vec<Option<u32>>)> {
    let mut merger = merge();
    for (input, arch) in inputs {
        merger.arch_input(input, *arch);
    }
    merger.output(output).merge().unwrap();

    let index = reader::Index::read(output).unwrap();
    let mut variants: Vec<_> = index
        .get("Test", "EXPLICIT")
        .map(|ty| {
            (
                arch_value(ty),
                ty.fields().map(|field| field.offset()).collect(),
            )
        })
        .collect();
    variants.sort();
    variants
}

#[test]
fn arch_merge_splits_offset_only_layout_variants_deterministically() {
    let dir = std::env::temp_dir().join("win_merge_offset_variants");
    std::fs::create_dir_all(&dir).unwrap();

    let x64 = explicit_layout_winmd(&dir, "x64", [Some(4), Some(12)]);
    let arm = explicit_layout_winmd(&dir, "arm", [Some(4), Some(12)]);
    let x86 = explicit_layout_winmd(&dir, "x86", [Some(8), Some(16)]);
    let expected = vec![(1, vec![Some(8), Some(16)]), (6, vec![Some(4), Some(12)])];

    assert_eq!(
        merged_layout_variants(
            &dir.join("forward.winmd"),
            &[(&x64, 2), (&arm, 4), (&x86, 1)]
        ),
        expected
    );
    assert_eq!(
        merged_layout_variants(
            &dir.join("reverse.winmd"),
            &[(&x86, 1), (&arm, 4), (&x64, 2)]
        ),
        expected
    );
}

#[test]
fn arch_merge_distinguishes_missing_and_zero_field_layouts() {
    let dir = std::env::temp_dir().join("win_merge_missing_layout");
    std::fs::create_dir_all(&dir).unwrap();

    let x64 = explicit_layout_winmd(&dir, "x64", [Some(0), Some(0)]);
    let arm = explicit_layout_winmd(&dir, "arm", [Some(0), Some(0)]);
    let x86 = explicit_layout_winmd(&dir, "x86", [None, None]);
    let expected = vec![(1, vec![None, None]), (6, vec![Some(0), Some(0)])];

    assert_eq!(
        merged_layout_variants(
            &dir.join("forward.winmd"),
            &[(&x64, 2), (&arm, 4), (&x86, 1)]
        ),
        expected
    );
    assert_eq!(
        merged_layout_variants(
            &dir.join("reverse.winmd"),
            &[(&x86, 1), (&arm, 4), (&x64, 2)]
        ),
        expected
    );
}

#[test]
fn arch_merge_keeps_explicit_class_offset_variants() {
    let dir = std::env::temp_dir().join("win_merge_class_offset_variants");
    std::fs::create_dir_all(&dir).unwrap();

    let x64 = explicit_class_winmd(&dir, "x64", 4);
    let arm = explicit_class_winmd(&dir, "arm", 4);
    let x86 = explicit_class_winmd(&dir, "x86", 8);
    let merged = dir.join("merged.winmd");
    merge()
        .arch_input(&x86, 1)
        .arch_input(&arm, 4)
        .arch_input(&x64, 2)
        .output(&merged)
        .merge()
        .unwrap();

    let index = reader::Index::read(merged).unwrap();
    let class = index.expect("Test", "EXPLICIT_CLASS");
    let mut fields: Vec<_> = class
        .fields()
        .map(|field| (arch_value(field), field.offset()))
        .collect();
    fields.sort();
    assert_eq!(fields, [(1, Some(8)), (6, Some(4))]);
}
