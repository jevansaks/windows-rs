use std::path::{Path, PathBuf};
use windows_metadata::*;

const COLLECTIONS: &str = "Windows.Foundation.Collections";
const COMPILER: &str = "System.Runtime.CompilerServices";
const INTEROP: &str = "System.Runtime.InteropServices";

fn scratch(name: &str) -> PathBuf {
    let path = Path::new(env!("OUT_DIR"))
        .join("reference_scopes")
        .join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn reference_bytes() -> Vec<u8> {
    let mut file = writer::File::new("Windows");

    let map = file.TypeDef(
        COLLECTIONS,
        "IMapView`2",
        Default::default(),
        TypeAttributes::Public
            | TypeAttributes::Interface
            | TypeAttributes::Abstract
            | TypeAttributes::WindowsRuntime,
    );
    file.GenericParam(
        "K",
        writer::TypeOrMethodDef::TypeDef(map),
        0,
        GenericParamAttributes::None,
    );
    file.GenericParam(
        "V",
        writer::TypeOrMethodDef::TypeDef(map),
        1,
        GenericParamAttributes::None,
    );

    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    let outer = file.TypeDef(
        "Collision",
        "Outer",
        value_type,
        TypeAttributes::Public | TypeAttributes::SequentialLayout | TypeAttributes::Sealed,
    );
    let inner = file.TypeDef(
        "",
        "Outer_0",
        value_type,
        TypeAttributes::NestedPublic | TypeAttributes::SequentialLayout | TypeAttributes::Sealed,
    );
    file.NestedClass(inner, outer);

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

fn assert_core(reference: reader::TypeRef<'_>) {
    let assembly = reference
        .assembly()
        .unwrap_or_else(|| panic!("{reference:?} must be external"));
    assert_eq!(assembly.name(), "mscorlib");
    assert_eq!(assembly.version(), (4, 0, 0, 0));
    assert_eq!(assembly.flags(), AssemblyFlags(0));
    assert_eq!(
        assembly.public_key_or_token(),
        [0xB7, 0x7A, 0x5C, 0x56, 0x19, 0x34, 0xE0, 0x89]
    );
    assert_eq!(assembly.culture(), "");
    assert!(assembly.hash_value().is_empty());
}

fn assert_windows(reference: reader::TypeRef<'_>) {
    let assembly = reference
        .assembly()
        .unwrap_or_else(|| panic!("{reference:?} must be external"));
    assert_eq!(assembly.name(), "Windows");
    assert_eq!(assembly.version(), (0xFF, 0xFF, 0xFF, 0xFF));
    assert_eq!(assembly.flags(), AssemblyFlags::WindowsRuntime);
    assert!(assembly.public_key_or_token().is_empty());
    assert_eq!(assembly.culture(), "");
    assert!(assembly.hash_value().is_empty());
}

fn assert_external_scopes(path: &Path) {
    let index = reader::Index::read(path).unwrap();
    let map = type_ref(&index, COLLECTIONS, "IMapView`2");
    let attribute = type_ref(&index, INTEROP, "UnmanagedFunctionPointerAttribute");
    let convention = type_ref(&index, INTEROP, "CallingConvention");
    let is_const = type_ref(&index, COMPILER, "IsConst");
    assert_windows(map);
    assert_core(attribute);
    assert_core(convention);
    assert_core(is_const);

    let consumer = index.expect("Test", "IConsumer");
    let get_view = consumer
        .methods()
        .find(|method| method.name() == "GetView")
        .unwrap();
    assert!(matches!(
        get_view.signature(&[]).return_type,
        Type::ClassName(TypeName {
            namespace,
            name,
            generics,
        }) if namespace == COLLECTIONS && name == "IMapView`2" && generics.len() == 2
    ));

    let callback = index.expect("Native", "Callback");
    let invoke = callback
        .methods()
        .find(|method| method.name() == "Invoke")
        .unwrap();
    assert_eq!(
        invoke.signature(&[]).types,
        [Type::PtrConst(Box::new(Type::I32), 1)]
    );

    let unmanaged = callback
        .find_attribute("UnmanagedFunctionPointerAttribute")
        .unwrap();
    let reader::AttributeType::MemberRef(ctor) = unmanaged.ctor() else {
        panic!("callback attribute must use a MemberRef constructor");
    };
    let reader::MemberRefParent::TypeRef(parent) = ctor.parent() else {
        panic!("callback attribute constructor must use a TypeRef parent");
    };
    assert_eq!(parent, attribute);
    assert_eq!(
        ctor.signature(&[]).types,
        [Type::value_named(INTEROP, "CallingConvention")]
    );
}

fn roundtrip(source: &str, reference: &[u8], dir: &Path, assert_scopes: fn(&Path)) -> PathBuf {
    let first = dir.join("first.winmd");
    windows_rdl::reader()
        .input_text(source)
        .reference_bytes(reference)
        .output(&first)
        .write()
        .unwrap();
    assert_scopes(&first);

    let rdl = dir.join("readback.rdl");
    windows_rdl::writer()
        .input(&first)
        .output(&rdl)
        .write()
        .unwrap();

    let second = dir.join("second.winmd");
    windows_rdl::reader()
        .input(&rdl)
        .reference_bytes(reference)
        .output(&second)
        .write()
        .unwrap();
    assert_scopes(&second);
    second
}

#[test]
fn exact_reference_shapes_survive_compile_write_readback() {
    let dir = scratch("external");
    let reference = reference_bytes();
    let source = r#"
#[winrt]
mod Test {
    interface IConsumer {
        fn GetView(&self) -> Windows::Foundation::Collections::IMapView<String, Object>;
    }
}

#[win32]
mod Native {
    extern "C" fn Callback(value: *const i32);
}
"#;

    roundtrip(source, &reference, &dir, assert_external_scopes);
}

fn assert_local_scopes(path: &Path) {
    let index = reader::Index::read(path).unwrap();
    let map = type_ref(&index, COLLECTIONS, "IMapView`2");
    assert!(
        matches!(map.scope(), reader::ResolutionScope::Module(_)),
        "local generic definition must beat the external homonym: {:?}",
        map.scope()
    );

    let outer = type_ref(&index, "Collision", "Outer");
    assert!(
        matches!(outer.scope(), reader::ResolutionScope::Module(_)),
        "local enclosing definition must beat the external homonym: {:?}",
        outer.scope()
    );
    let nested = type_ref(&index, "Collision", "Outer/Outer_0");
    let reader::ResolutionScope::TypeRef(enclosing) = nested.scope() else {
        panic!("nested local reference must use its enclosing TypeRef");
    };
    assert_eq!(enclosing, outer);
}

#[test]
fn local_generic_and_nested_homonyms_remain_module_scoped() {
    let dir = scratch("local");
    let reference = reference_bytes();
    let source = r#"
#[winrt]
mod Windows {
    mod Foundation {
        mod Collections {
            interface IConsumer {
                fn GetView(&self) -> IMapView<String, Object>;
            }

            interface IMapView<K, V> {
                fn Lookup(&self, key: K) -> V;
            }
        }
    }
}

#[win32]
mod Collision {
    struct Outer {
        child: struct {
            value: i32,
        },
    }
}
"#;

    roundtrip(source, &reference, &dir, assert_local_scopes);
}
