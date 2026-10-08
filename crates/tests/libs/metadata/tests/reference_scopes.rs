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
