use windows_metadata::*;

const INTEROP: &str = "System.Runtime.InteropServices";
const COMPILER: &str = "System.Runtime.CompilerServices";

fn assert_assembly(reference: reader::TypeRef<'_>, expected: usize) -> reader::AssemblyRef<'_> {
    let reader::ResolutionScope::AssemblyRef(assembly) = reference.scope() else {
        panic!("{} must have an assembly scope", reference.name());
    };
    // File::new emits the core-library AssemblyRef first; explicit owners follow it.
    assert_eq!(assembly.pos(), expected);
    assembly
}

fn type_ref<'a>(blob: &mut reader::Blob<'a>) -> reader::TypeRef<'a> {
    let reader::TypeDefOrRef::TypeRef(reference) = blob.decode() else {
        panic!("expected TypeRef");
    };
    reference
}

fn reference_assembly(name: &str) -> Vec<u8> {
    let mut file = writer::File::new(name);
    let types = if name == "Windows" {
        &[("Windows.Foundation", "HResult")][..]
    } else {
        &[
            (COMPILER, "IsConst"),
            (INTEROP, "CallingConvention"),
            (INTEROP, "UnmanagedFunctionPointerAttribute"),
        ][..]
    };
    for &(namespace, name) in types {
        file.TypeDef(namespace, name, Default::default(), TypeAttributes::Public);
    }
    file.into_stream()
}

#[test]
fn writer_system_scopes_and_reference_precedence() {
    for owner in [None, Some("mscorlib"), Some("OtherRuntime")] {
        let mut file = writer::File::new("Test");
        if let Some(owner) = owner {
            file.set_reference(reader::Index::new(vec![
                reader::File::new(reference_assembly(owner)).unwrap(),
                reader::File::new(reference_assembly("Windows")).unwrap(),
            ]));
        }
        file.TypeDef("Test", "Holder", Default::default(), TypeAttributes::Public);
        for (name, ty) in [
            ("Pointer", Type::PtrConst(Box::new(Type::U8), 1)),
            ("Reference", Type::RefConst(Box::new(Type::U32))),
            (
                "Convention",
                Type::value_named(INTEROP, "CallingConvention"),
            ),
            (
                "Attribute",
                Type::class_named(INTEROP, "UnmanagedFunctionPointerAttribute"),
            ),
            ("Object", Type::class_named("System", "Object")),
            ("Local", Type::value_named("Systematic", "Local")),
        ] {
            file.Field(name, &ty, FieldAttributes::Public);
        }
        if owner.is_some() {
            file.Field(
                "HResult",
                &Type::value_named("Windows.Foundation", "HResult"),
                FieldAttributes::Public,
            );
        }
        let index = reader::Index::new(vec![reader::File::new(file.into_stream()).unwrap()]);
        let holder = index.expect("Test", "Holder");
        let mut core = None;
        for field in holder.fields() {
            let mut blob = field.blob(2);
            assert_eq!(blob.read_u8(), 0x06);
            let expected = usize::from(owner == Some("OtherRuntime"));
            match field.name() {
                "Pointer" | "Reference" => {
                    assert_eq!(blob.read_u8(), 0x1F, "required modifier must be retained");
                    let reference = type_ref(&mut blob);
                    assert_eq!(
                        (reference.namespace(), reference.name()),
                        (COMPILER, "IsConst")
                    );
                    let suffix = if field.name() == "Pointer" {
                        [0x0F, 0x05]
                    } else {
                        [0x10, 0x09]
                    };
                    assert_eq!([blob.read_u8(), blob.read_u8()], suffix);
                    let assembly = assert_assembly(reference, expected);
                    core = Some(assembly);
                }
                "Convention" | "Attribute" => {
                    assert_eq!(
                        blob.read_u8(),
                        if field.name() == "Convention" {
                            0x11
                        } else {
                            0x12
                        }
                    );
                    let reference = type_ref(&mut blob);
                    assert_eq!(reference.namespace(), INTEROP);
                    assert_eq!(assert_assembly(reference, expected), core.unwrap());
                }
                "Object" => {
                    assert_eq!(blob.read_u8(), 0x12);
                    let assembly = assert_assembly(type_ref(&mut blob), 0);
                    if owner != Some("OtherRuntime") {
                        assert_eq!(Some(assembly), core);
                    }
                }
                "Local" => {
                    assert_eq!(blob.read_u8(), 0x11);
                    assert!(matches!(
                        type_ref(&mut blob).scope(),
                        reader::ResolutionScope::Module(_)
                    ));
                }
                "HResult" => {
                    assert_eq!(blob.read_u8(), 0x11);
                    assert_assembly(type_ref(&mut blob), expected + 1);
                }
                _ => unreachable!(),
            }
        }
    }
}

#[test]
fn rdl_const_modifier_and_callback_scopes_survive_merge() {
    let dir = std::env::temp_dir().join("win_system_references");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.winmd");
    windows_rdl::reader()
        .input_text(
            r#"#[win32] mod Test {
            #[library("test.dll")] extern "system" fn Query(block: *const u8);
            extern "system" fn Callback(value: u32);
        }"#,
        )
        .output(&input)
        .write()
        .unwrap();
    let merged = dir.join("merged.winmd");
    merge().input(&input).output(&merged).merge().unwrap();
    for path in [&input, &merged] {
        let index = reader::Index::read(path).unwrap();
        let query = index.expect("Test", "Apis").methods().next().unwrap();
        let mut blob = query.blob(4);
        assert_eq!([blob.read_u8(), blob.read_u8(), blob.read_u8()], [0, 1, 1]);
        assert_eq!(blob.read_u8(), 0x1F);
        let modifier = type_ref(&mut blob);
        assert_eq!(
            (modifier.namespace(), modifier.name()),
            (COMPILER, "IsConst")
        );
        assert_eq!([blob.read_u8(), blob.read_u8()], [0x0F, 0x05]);
        let core = assert_assembly(modifier, 0);

        let callback = index.expect("Test", "Callback");
        let attribute = callback
            .find_attribute("UnmanagedFunctionPointerAttribute")
            .unwrap();
        let reader::MemberRefParent::TypeRef(reference) = attribute.ctor().parent() else {
            panic!("callback attribute must reference its external type");
        };
        assert_eq!(assert_assembly(reference, 0), core);
        let reader::AttributeType::MemberRef(ctor) = attribute.ctor() else {
            panic!("callback attribute must use an external constructor");
        };
        let mut blob = ctor.blob(2);
        assert_eq!(
            [
                blob.read_u8(),
                blob.read_u8(),
                blob.read_u8(),
                blob.read_u8()
            ],
            [0x20, 1, 1, 0x11]
        );
        let convention = type_ref(&mut blob);
        assert_eq!(
            (convention.namespace(), convention.name()),
            (INTEROP, "CallingConvention")
        );
        assert_eq!(assert_assembly(convention, 0), core);
    }
}
