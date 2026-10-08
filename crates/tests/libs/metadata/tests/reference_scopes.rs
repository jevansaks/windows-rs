use windows_metadata::*;

const COMPILER: &str = "System.Runtime.CompilerServices";
const INTEROP: &str = "System.Runtime.InteropServices";

fn core_reference_bytes() -> Vec<u8> {
    let mut file = writer::File::new("Core.Owner");
    for (namespace, name) in [
        (COMPILER, "IsConst"),
        (INTEROP, "CallingConvention"),
        (INTEROP, "UnmanagedFunctionPointerAttribute"),
    ] {
        file.TypeDef(namespace, name, Default::default(), TypeAttributes::Public);
    }
    file.into_stream()
}

fn type_ref<'a>(index: &'a reader::Index, namespace: &str, name: &str) -> reader::TypeRef<'a> {
    let matches: Vec<_> = index
        .type_refs()
        .filter(|reference| {
            let qualified = reference.qualified_name();
            qualified.namespace == namespace && qualified.name == name
        })
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected one TypeRef for {namespace}.{name}, found {matches:?}"
    );
    matches[0]
}

#[test]
fn unresolved_non_core_names_remain_local() {
    let mut file = writer::File::new("Consumer");
    file.TypeDef("Test", "Holder", Default::default(), TypeAttributes::Public);
    for (field, namespace, name) in [
        ("compiler", COMPILER, "LocalCompilerType"),
        ("interop", INTEROP, "LocalInteropType"),
    ] {
        file.Field(
            field,
            &Type::value_named(namespace, name),
            FieldAttributes::Public,
        );
    }

    let index = reader::Index::new(vec![reader::File::new(file.into_stream()).unwrap()]);
    for (namespace, name) in [
        (COMPILER, "LocalCompilerType"),
        (INTEROP, "LocalInteropType"),
    ] {
        assert!(matches!(
            type_ref(&index, namespace, name).scope(),
            reader::ResolutionScope::Module(_)
        ));
    }
}

#[test]
fn core_references_prefer_exact_supplied_definitions() {
    let reference = reader::Index::new(vec![reader::File::new(core_reference_bytes()).unwrap()]);
    let mut file = writer::File::new("Consumer");
    file.set_reference(reference);
    file.CoreTypeRef(INTEROP, "UnmanagedFunctionPointerAttribute");
    file.CoreTypeRef(INTEROP, "CallingConvention");
    file.TypeDef("Test", "Holder", Default::default(), TypeAttributes::Public);
    file.Field(
        "pointer",
        &Type::PtrConst(Box::new(Type::I32), 1),
        FieldAttributes::Public,
    );

    let index = reader::Index::new(vec![reader::File::new(file.into_stream()).unwrap()]);
    for (namespace, name) in [
        (COMPILER, "IsConst"),
        (INTEROP, "CallingConvention"),
        (INTEROP, "UnmanagedFunctionPointerAttribute"),
    ] {
        let assembly = type_ref(&index, namespace, name).assembly().unwrap();
        assert_eq!(assembly.name(), "Core.Owner");
        assert_eq!(assembly.version(), (0xFF, 0xFF, 0xFF, 0xFF));
        assert_eq!(assembly.flags(), AssemblyFlags::WindowsRuntime);
    }
}

fn assert_local_and_core_is_const(core_first: bool) {
    let mut file = writer::File::new("Consumer");
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef(
        COMPILER,
        "IsConst",
        value_type,
        TypeAttributes::Public | TypeAttributes::SequentialLayout | TypeAttributes::Sealed,
    );
    file.TypeDef(
        "Test",
        "Holder",
        value_type,
        TypeAttributes::Public | TypeAttributes::SequentialLayout | TypeAttributes::Sealed,
    );

    let write_local = |file: &mut writer::File| {
        file.Field(
            "local",
            &Type::value_named(COMPILER, "IsConst"),
            FieldAttributes::Public,
        );
    };
    let write_core = |file: &mut writer::File| {
        file.Field(
            "ptr",
            &Type::PtrConst(Box::new(Type::I32), 1),
            FieldAttributes::Public,
        );
    };

    if core_first {
        write_core(&mut file);
        write_local(&mut file);
    } else {
        write_local(&mut file);
        write_core(&mut file);
    }

    let index = reader::Index::new(vec![reader::File::new(file.into_stream()).unwrap()]);
    let holder = index.expect("Test", "Holder");

    let local = holder
        .fields()
        .find(|field| field.name() == "local")
        .unwrap();
    let mut blob = local.blob(2);
    assert_eq!([blob.read_u8(), blob.read_u8()], [0x06, 0x11]);
    let reader::TypeDefOrRef::TypeRef(local_ref) = blob.decode() else {
        panic!("local field must use a TypeRef");
    };
    assert!(
        matches!(local_ref.scope(), reader::ResolutionScope::Module(_)),
        "local IsConst must remain module-scoped: {:?}",
        local_ref.scope()
    );

    let ptr = holder.fields().find(|field| field.name() == "ptr").unwrap();
    let mut blob = ptr.blob(2);
    assert_eq!([blob.read_u8(), blob.read_u8()], [0x06, 0x1F]);
    let reader::TypeDefOrRef::TypeRef(core_ref) = blob.decode() else {
        panic!("const modifier must use a TypeRef");
    };
    let assembly = core_ref
        .assembly()
        .unwrap_or_else(|| panic!("const modifier must use the core assembly: {core_ref:?}"));
    assert_eq!(assembly.name(), "mscorlib");
    assert_eq!([blob.read_u8(), blob.read_u8()], [0x0F, 0x08]);
    assert_ne!(local_ref, core_ref);

    let references: Vec<_> = index
        .type_refs()
        .filter(|reference| reference.namespace() == COMPILER && reference.name() == "IsConst")
        .collect();
    assert_eq!(
        references.len(),
        2,
        "local and core identities must be distinct"
    );
}

#[test]
fn local_then_core_type_refs_remain_distinct() {
    assert_local_and_core_is_const(false);
}

#[test]
fn core_then_local_type_refs_remain_distinct() {
    assert_local_and_core_is_const(true);
}
