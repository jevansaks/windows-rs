use windows_metadata::HasAttributes;

fn temp_path(name: &str, extension: &str) -> String {
    std::env::temp_dir()
        .join(format!("windows_rdl_{name}.{extension}"))
        .to_string_lossy()
        .into_owned()
}

fn compile(source: &str) -> windows_metadata::reader::Index {
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    windows_metadata::reader::Index::new(vec![windows_metadata::reader::File::new(bytes).unwrap()])
}

#[test]
fn default_input_resolves_default_metadata() {
    windows_rdl::reader()
        .input_text(
            r#"
use Windows::Foundation::*;

#[winrt]
mod Test {
    struct Wrapper {
        value: Point,
    }
}
"#,
        )
        .reference_default()
        .output(temp_path("default_input", "winmd"))
        .write()
        .unwrap();
}

#[test]
fn null_attribute_values_roundtrip() {
    let source = r#"
#[win32]
mod Test {
    attribute MaybeAttribute {
        fn(value: String, ty: Type);
    }

    #[Maybe(null, null)]
    struct Value {}
}
"#;
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    let index = windows_metadata::reader::Index::new(vec![
        windows_metadata::reader::File::new(bytes.clone()).unwrap(),
    ]);
    assert_eq!(
        index
            .expect("Test", "Value")
            .attributes()
            .next()
            .unwrap()
            .try_value()
            .unwrap(),
        [
            (
                String::new(),
                windows_metadata::Value::Null(windows_metadata::Type::String),
            ),
            (
                String::new(),
                windows_metadata::Value::Null(windows_metadata::Type::ClassName(
                    windows_metadata::TypeName::named("System", "Type"),
                )),
            ),
        ]
    );

    let output = temp_path("null_attribute_values", "rdl");
    windows_rdl::writer()
        .input_bytes(&bytes)
        .output(&output)
        .write()
        .unwrap();
    let source = std::fs::read_to_string(output).unwrap();
    assert!(source.contains("#[Maybe(null, null)]"));
    windows_rdl::reader()
        .input_text(&source)
        .bytes("roundtrip")
        .unwrap();
}

#[test]
fn boxed_attribute_values_roundtrip() {
    let source = r#"
#[win32]
mod Test {
    attribute BoxedAttribute {
        fn(number: Object, text: Object, missing: Object);
    }

    #[Boxed(boxed(i32, 42), boxed(String, "hello"), null)]
    struct Value {}
}
"#;
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    let index = windows_metadata::reader::Index::new(vec![
        windows_metadata::reader::File::new(bytes.clone()).unwrap(),
    ]);
    assert_eq!(
        index
            .expect("Test", "Value")
            .attributes()
            .next()
            .unwrap()
            .try_value()
            .unwrap(),
        [
            (
                String::new(),
                windows_metadata::Value::Boxed(Box::new(windows_metadata::Value::I32(42))),
            ),
            (
                String::new(),
                windows_metadata::Value::Boxed(Box::new(windows_metadata::Value::Utf8(
                    "hello".to_string(),
                ))),
            ),
            (
                String::new(),
                windows_metadata::Value::Null(windows_metadata::Type::Object),
            ),
        ]
    );

    let output = temp_path("boxed_attribute_values", "rdl");
    windows_rdl::writer()
        .input_bytes(&bytes)
        .output(&output)
        .write()
        .unwrap();
    let source = std::fs::read_to_string(output).unwrap();
    assert!(source.contains("boxed(i32, 42)"));
    assert!(source.contains("boxed(String, \"hello\")"));
    windows_rdl::reader()
        .input_text(&source)
        .bytes("roundtrip")
        .unwrap();
}

#[test]
fn array_attribute_values_roundtrip() {
    let source = r#"
#[win32]
mod Test {
    attribute ArrayAttribute {
        fn(numbers: [i32], names: [String], missing: [u8], boxed_values: Object);
    }

    #[Array([1, 2], ["hello", null], null, boxed([i32], [3, 4]))]
    struct Value {}
}
"#;
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    let index = windows_metadata::reader::Index::new(vec![
        windows_metadata::reader::File::new(bytes.clone()).unwrap(),
    ]);
    assert_eq!(
        index
            .expect("Test", "Value")
            .attributes()
            .next()
            .unwrap()
            .try_value()
            .unwrap(),
        [
            (
                String::new(),
                windows_metadata::Value::Array(
                    windows_metadata::Type::I32,
                    vec![
                        windows_metadata::Value::I32(1),
                        windows_metadata::Value::I32(2),
                    ],
                ),
            ),
            (
                String::new(),
                windows_metadata::Value::Array(
                    windows_metadata::Type::String,
                    vec![
                        windows_metadata::Value::Utf8("hello".to_string()),
                        windows_metadata::Value::Null(windows_metadata::Type::String),
                    ],
                ),
            ),
            (
                String::new(),
                windows_metadata::Value::Null(windows_metadata::Type::Array(Box::new(
                    windows_metadata::Type::U8,
                ))),
            ),
            (
                String::new(),
                windows_metadata::Value::Boxed(Box::new(windows_metadata::Value::Array(
                    windows_metadata::Type::I32,
                    vec![
                        windows_metadata::Value::I32(3),
                        windows_metadata::Value::I32(4),
                    ],
                ))),
            ),
        ]
    );

    let output = temp_path("array_attribute_values", "rdl");
    windows_rdl::writer()
        .input_bytes(&bytes)
        .output(&output)
        .write()
        .unwrap();
    let source = std::fs::read_to_string(output).unwrap();
    assert!(source.contains("[\"hello\", null]"));
    assert!(source.contains("boxed([i32], [3, 4])"));
    windows_rdl::reader()
        .input_text(&source)
        .bytes("roundtrip")
        .unwrap();
}

#[test]
fn reference_bytes_resolve_metadata() {
    let reference = temp_path("reference_bytes_reference", "winmd");

    windows_rdl::reader()
        .input_text(
            r#"
#[winrt]
mod Other {
    struct Point {
        x: i32,
        y: i32,
    }
}
"#,
        )
        .output(&reference)
        .write()
        .unwrap();

    let bytes = std::fs::read(reference).unwrap();
    windows_rdl::reader()
        .input_text(
            r#"
use Other::*;

#[winrt]
mod Test {
    struct Wrapper {
        value: Point,
    }
}
"#,
        )
        .reference_byte_sets([bytes])
        .output(temp_path("reference_bytes", "winmd"))
        .write()
        .unwrap();
}

#[test]
fn reference_path_resolves_metadata() {
    let reference = temp_path("reference_path_reference", "winmd");

    windows_rdl::reader()
        .input_text(
            r#"
#[winrt]
mod Other {
    struct Point {
        x: i32,
        y: i32,
    }
}
"#,
        )
        .output(&reference)
        .write()
        .unwrap();

    windows_rdl::reader()
        .input_text(
            r#"
use Other::*;

#[winrt]
mod Test {
    struct Wrapper {
        value: Point,
    }
}
"#,
        )
        .reference(&reference)
        .output(temp_path("reference_path", "winmd"))
        .write()
        .unwrap();
}

#[test]
fn writer_accepts_metadata_bytes() {
    let winmd = temp_path("writer_bytes_input", "winmd");
    let rdl = temp_path("writer_bytes_output", "rdl");

    windows_rdl::reader()
        .input_text(
            r#"
#[win32]
mod Test {
    struct Value {
        value: u32,
    }
}
"#,
        )
        .output(&winmd)
        .write()
        .unwrap();

    let bytes = std::fs::read(&winmd).unwrap();
    windows_rdl::writer()
        .input_byte_sets([bytes])
        .output(&rdl)
        .write()
        .unwrap();

    assert!(
        std::fs::read_to_string(rdl)
            .unwrap()
            .contains("struct Value")
    );
}

#[test]
fn delegates_preserve_runtime_method_shape() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    delegate fn Handler(value: i32);
    #[invoke_no_new_slot]
    delegate fn GenericHandler<T>(value: T);
}
"#,
    );

    let methods: Vec<_> = index.expect("Test", "Handler").methods().collect();
    assert_eq!(methods.len(), 2);
    assert_eq!(methods[0].name(), ".ctor");
    assert_eq!(methods[0].flags().0, 0x1881);
    assert_eq!(methods[0].impl_flags().0, 0x0003);
    assert_eq!(methods[0].signature(&[]).flags.0, 0x20);
    assert_eq!(methods[1].name(), "Invoke");
    assert_eq!(methods[1].flags().0, 0x09c6);
    assert_eq!(methods[1].impl_flags().0, 0x0003);
    assert_eq!(methods[1].signature(&[]).flags.0, 0x20);
    let invoke = index
        .expect("Test", "GenericHandler")
        .methods()
        .find(|method| method.name() == "Invoke")
        .unwrap();
    assert_eq!(invoke.flags().0, 0x08c6);
}

#[test]
fn runtime_interfaces_preserve_method_implementation_flags() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    #[runtime]
    interface IValue<T> {
        fn Get(&self) -> T;
        Value: T;
    }
}
"#,
    );

    for method in index.expect("Test", "IValue").methods() {
        assert_eq!(method.impl_flags().0, 0x0003);
    }
}

#[test]
fn winrt_attribute_constructors_are_runtime_instance_methods() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    attribute MarkerAttribute {
        fn(value: i32);
    }
}
"#,
    );

    let method = index
        .expect("Test", "MarkerAttribute")
        .methods()
        .next()
        .unwrap();
    assert_eq!(method.signature(&[]).flags.0, 0x20);
    assert_eq!(method.impl_flags().0, 0x0003);
}

#[test]
fn exclusive_interface_members_are_projected_onto_class() {
    use windows_metadata::HasAttributes;

    let index = compile(
        r#"
#[winrt]
mod Test {
    #[Windows::Foundation::Metadata::ExclusiveTo(Widget)]
    interface IWidget {
        #[overload(RenameWithString)]
        fn Rename(&self, value: String);
        Value: i32;
    }
    class Widget {
        IWidget,
    }
}
#[winrt]
mod Windows {
    mod Foundation {
        mod Metadata {
            attribute ExclusiveToAttribute {
                fn(r#type: Type);
            }
            attribute OverloadAttribute {
                fn(name: String);
            }
        }
    }
}
"#,
    );

    let class = index.expect("Test", "Widget");
    let methods: Vec<_> = class.methods().collect();
    assert_eq!(methods.len(), 3);
    assert_eq!(methods[0].name(), "RenameWithString");
    assert_eq!(methods[0].flags().0, 0x01e6);
    assert_eq!(methods[0].impl_flags().0, 0x0003);
    assert_eq!(methods[0].signature(&[]).flags.0, 0x20);
    assert!(methods[0].has_attribute("OverloadAttribute"));
    assert_eq!(methods[1].name(), "get_Value");
    assert_eq!(methods[1].flags().0, 0x09e6);
    assert_eq!(methods[1].impl_flags().0, 0x0003);
    assert_eq!(methods[2].name(), "put_Value");
    assert_eq!(methods[2].flags().0, 0x09e6);
    assert_eq!(methods[2].impl_flags().0, 0x0003);

    let properties: Vec<_> = class.properties().collect();
    assert_eq!(properties.len(), 1);
    assert_eq!(properties[0].name(), "Value");
    assert_eq!(properties[0].signature(&[]).flags.0, 0x28);
    assert_eq!(properties[0].semantics().count(), 2);
}

#[test]
fn static_interface_members_are_projected_onto_class() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    interface IWidgetStatics {
        fn Create(&self) -> Widget;
        Value: i32;
    }
    #[Windows::Foundation::Metadata::Static(IWidgetStatics, 1)]
    class Widget {}
}
#[winrt]
mod Windows {
    mod Foundation {
        mod Metadata {
            attribute StaticAttribute {
                fn(r#type: Type, version: u32);
            }
        }
    }
}
"#,
    );

    let class = index.expect("Test", "Widget");
    let methods: Vec<_> = class.methods().collect();
    assert_eq!(methods.len(), 3);
    assert_eq!(methods[0].name(), "Create");
    assert_eq!(methods[0].flags().0, 0x0096);
    assert_eq!(methods[0].impl_flags().0, 0x0003);
    assert_eq!(methods[0].signature(&[]).flags.0, 0x00);
    assert_eq!(methods[1].name(), "get_Value");
    assert_eq!(methods[1].flags().0, 0x0896);
    assert_eq!(methods[1].impl_flags().0, 0x0003);
    assert_eq!(methods[2].name(), "put_Value");
    assert_eq!(methods[2].flags().0, 0x0896);
    assert_eq!(methods[2].impl_flags().0, 0x0003);

    let properties: Vec<_> = class.properties().collect();
    assert_eq!(properties.len(), 1);
    assert_eq!(properties[0].name(), "Value");
    assert_eq!(properties[0].signature(&[]).flags.0, 0x08);
    assert_eq!(properties[0].semantics().count(), 2);
}

#[test]
fn generic_static_interface_members_roundtrip() {
    let source = r#"
#[winrt]
mod Test {
    interface IBox<T> {}
    interface IWidgetStatics<T, U> {
        fn Create(&self, value: T) -> T;
        Value: U;
    }
    #[Windows::Foundation::Metadata::Static(IWidgetStatics::<i32, IBox<String>>, 1)]
    class Widget {}
}
#[winrt]
mod Windows {
    mod Foundation {
        mod Metadata {
            attribute StaticAttribute {
                fn(r#type: Type, version: u32);
            }
        }
    }
}
"#;
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    let verify = |bytes: Vec<u8>| {
        let index = windows_metadata::reader::Index::new(vec![
            windows_metadata::reader::File::new(bytes).unwrap(),
        ]);
        let class = index.expect("Test", "Widget");
        let methods: Vec<_> = class.methods().collect();
        assert_eq!(methods.len(), 3);
        assert_eq!(methods[0].name(), "Create");
        assert_eq!(
            methods[0].signature(&[]).return_type,
            windows_metadata::Type::I32
        );
        assert_eq!(
            methods[0].signature(&[]).types,
            [windows_metadata::Type::I32]
        );
        assert_eq!(
            methods[1].signature(&[]).return_type,
            windows_metadata::Type::ClassName(windows_metadata::TypeName {
                namespace: "Test".to_string(),
                name: "IBox`1".to_string(),
                generics: vec![windows_metadata::Type::String],
            })
        );
        assert_eq!(
            methods[2].signature(&[]).types,
            [windows_metadata::Type::ClassName(
                windows_metadata::TypeName {
                    namespace: "Test".to_string(),
                    name: "IBox`1".to_string(),
                    generics: vec![windows_metadata::Type::String],
                }
            )]
        );
    };
    verify(bytes.clone());

    let output = temp_path("generic_static_interface", "rdl");
    windows_rdl::writer()
        .input_bytes(&bytes)
        .output(&output)
        .write()
        .unwrap();
    let source = std::fs::read_to_string(output).unwrap();
    assert!(source.contains("Static(IWidgetStatics::<i32, IBox<String>>, 1)"));
    let bytes = windows_rdl::reader()
        .input_text(&source)
        .bytes("roundtrip")
        .unwrap();
    verify(bytes);
}

#[test]
fn static_association_rows_can_be_excluded() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    interface IWidgetStatics {
        Value: i32;
    }
    #[Windows::Foundation::Metadata::Static(IWidgetStatics, 1)]
    #[no_static_property(IWidgetStatics, Value)]
    class Widget {}
}
#[winrt]
mod Windows {
    mod Foundation {
        mod Metadata {
            attribute StaticAttribute {
                fn(r#type: Type, version: u32);
            }
        }
    }
}
"#,
    );

    let class = index.expect("Test", "Widget");
    assert_eq!(class.methods().count(), 2);
    assert_eq!(class.properties().count(), 0);
}

#[test]
fn accessor_parameter_names_are_preserved() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    delegate fn Handler();
    interface IWidget {
        #[set_name(new_value)]
        Value: i32;
        #[add_name(p_handler)]
        #[remove_name(cookie)]
        event Changed: Handler;
    }
    class Widget {
        #[no_property(Value)]
        #[no_event(Changed)]
        IWidget,
    }
}
"#,
    );

    for type_name in ["IWidget", "Widget"] {
        let ty = index.expect("Test", type_name);
        for (method_name, expected) in [
            ("put_Value", "new_value"),
            ("add_Changed", "p_handler"),
            ("remove_Changed", "cookie"),
        ] {
            let method = ty
                .methods()
                .find(|method| method.name() == method_name)
                .unwrap();
            let params = method.params_by_sequence(1).unwrap();
            assert_eq!(params.params()[0].unwrap().name(), expected);
        }
    }
    let class = index.expect("Test", "Widget");
    assert_eq!(class.properties().count(), 0);
    assert_eq!(class.events().count(), 0);
}

#[test]
fn raw_accessors_do_not_gain_association_rows() {
    let source = r#"
#[winrt]
mod Test {
    delegate fn Handler();
    interface IWidget {
        #[special]
        fn get_Size(&self) -> u32;
        #[special]
        fn add_Changed(&self, handler: Handler) -> Windows::Foundation::EventRegistrationToken;
        #[special]
        fn remove_Changed(&self, token: Windows::Foundation::EventRegistrationToken);
    }
    class Widget {
        IWidget,
    }
}
#[winrt]
mod Windows {
    mod Foundation {
        struct EventRegistrationToken {
            value: i64,
        }
    }
}
"#;
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    let output = temp_path("raw_accessors", "rdl");
    windows_rdl::writer()
        .input_bytes(&bytes)
        .output(&output)
        .write()
        .unwrap();
    let output = std::fs::read_to_string(output).unwrap();
    assert!(output.contains("fn get_Size"));
    assert!(output.contains("fn add_Changed"));

    let index = compile(&output);
    for type_name in ["IWidget", "Widget"] {
        let ty = index.expect("Test", type_name);
        assert_eq!(ty.properties().count(), 0);
        assert_eq!(ty.events().count(), 0);
    }
}

#[test]
fn interface_event_preserves_reverse_accessor_order() {
    let source = r#"
#[winrt]
mod Test {
    delegate fn Handler();
    #[interface_event(Changed: Handler, remove = remove_Changed, add = add_Changed)]
    interface IWidget {
        #[special]
        fn remove_Changed(&self, token: Windows::Foundation::EventRegistrationToken);
        #[special]
        fn add_Changed(&self, handler: Handler) -> Windows::Foundation::EventRegistrationToken;
    }
}
#[winrt]
mod Windows {
    mod Foundation {
        struct EventRegistrationToken {
            value: i64,
        }
    }
}
"#;
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    let output = temp_path("interface_event", "rdl");
    windows_rdl::writer()
        .input_bytes(&bytes)
        .output(&output)
        .write()
        .unwrap();
    let output = std::fs::read_to_string(output).unwrap();
    assert!(output.contains("#[interface_event("));

    let index = compile(&output);
    let interface = index.expect("Test", "IWidget");
    assert_eq!(
        interface
            .methods()
            .map(|method| method.name())
            .collect::<Vec<_>>(),
        ["remove_Changed", "add_Changed"]
    );
    assert_eq!(interface.events().count(), 1);
}

#[test]
fn explicit_class_associations_can_reference_base_methods() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    delegate fn Handler();
    interface IBase {
        Value: i32;
    }
    #[unsealed]
    class Base {
        IBase,
    }
    interface IWidget {
        #[special]
        fn add_Changed(&self, handler: Handler) -> Windows::Foundation::EventRegistrationToken;
        #[special]
        fn remove_Changed(&self, token: Windows::Foundation::EventRegistrationToken);
    }
    #[class_property(Value: i32, get = Base::get_Value, set = Base::put_Value)]
    #[class_event(Changed: Handler, add = add_Changed, remove = remove_Changed)]
    class Widget: Base {
        IWidget,
    }
}
#[winrt]
mod Windows {
    mod Foundation {
        struct EventRegistrationToken {
            value: i64,
        }
    }
}
"#,
    );

    let class = index.expect("Test", "Widget");
    let property = class.properties().next().unwrap();
    assert_eq!(property.name(), "Value");
    assert_eq!(property.semantics().count(), 2);
    let event = class.events().next().unwrap();
    assert_eq!(event.name(), "Changed");
    assert_eq!(event.semantics().count(), 2);
}

#[test]
fn explicit_class_associations_preserve_row_attributes() {
    use windows_metadata::HasAttributes;

    let source = r#"
#[winrt]
mod Test {
    attribute MarkerAttribute {
        fn();
    }
    delegate fn Handler();
    interface IWidget {
        Value: i32;
        event Changed: Handler;
    }
    #[class_property(#[Marker] Value: i32, get = get_Value, set = put_Value)]
    #[class_event(#[Marker] Changed: Handler, add = add_Changed, remove = remove_Changed)]
    class Widget {
        #[no_property(Value)]
        #[no_event(Changed)]
        IWidget,
    }
}
"#;
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    let output = temp_path("class_association_attributes", "rdl");
    windows_rdl::writer()
        .input_bytes(&bytes)
        .output(&output)
        .write()
        .unwrap();
    let output = std::fs::read_to_string(output).unwrap();
    assert!(output.contains("#[class_property(#[Marker] Value: i32"));
    assert!(output.contains("#[class_event(#[Marker] Changed: Handler"));

    let index = compile(&output);
    let class = index.expect("Test", "Widget");
    let property = class.properties().next().unwrap();
    assert_eq!(
        property.attributes().next().unwrap().name(),
        "MarkerAttribute"
    );
    let event = class.events().next().unwrap();
    assert_eq!(event.attributes().next().unwrap().name(), "MarkerAttribute");
}

#[test]
fn association_suppression_matches_property_signatures() {
    let source = r#"
#[winrt]
mod Test {
    interface IStringValue {
        #[set]
        Value: String;
    }
    interface IIntValue {
        #[set]
        Value: i32;
    }
    class Value {
        IStringValue,
        #[no_property(Value)]
        IIntValue,
    }
}
"#;
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    let output = temp_path("property_signature_suppression", "rdl");
    windows_rdl::writer()
        .input_bytes(&bytes)
        .output(&output)
        .write()
        .unwrap();
    let output = std::fs::read_to_string(output).unwrap();
    assert_eq!(output.matches("#[no_property(Value)]").count(), 1);

    let index = compile(&output);
    let properties: Vec<_> = index.expect("Test", "Value").properties().collect();
    assert_eq!(properties.len(), 1);
    assert_eq!(
        properties[0].signature(&[]).return_type,
        windows_metadata::Type::String
    );
}

#[test]
fn activation_factories_are_projected_as_constructors() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    interface IWidgetFactory {
        fn WithValue(&self, value: i32) -> Widget;
    }
    #[Windows::Foundation::Metadata::Activatable(IWidgetFactory, 1)]
    #[Windows::Foundation::Metadata::Activatable(1)]
    class Widget {}

    interface IComposableFactory {
        fn CreateInstance(&self, base: Object, inner: &mut Object) -> Composable;
        fn WithValue(&self, value: i32, base: Object, inner: &mut Object) -> Composable;
    }
    #[Windows::Foundation::Metadata::Composable(IComposableFactory, Protected, 1)]
    class Composable {}
}
#[winrt]
mod Windows {
    mod Foundation {
        mod Metadata {
            attribute ActivatableAttribute {
                fn(version: u32);
                fn(r#type: Type, version: u32);
            }
            attribute ComposableAttribute {
                fn(r#type: Type, compositionType: CompositionType, version: u32);
            }
            #[repr(i32)]
            enum CompositionType {
                Protected = 1,
                Public = 2,
            }
        }
    }
}
"#,
    );

    let widget: Vec<_> = index.expect("Test", "Widget").methods().collect();
    assert_eq!(widget.len(), 2);
    assert_eq!(widget[0].name(), ".ctor");
    assert_eq!(
        widget[0].signature(&[]).types,
        [windows_metadata::Type::I32]
    );
    assert_eq!(widget[0].flags().0, 0x1886);
    assert_eq!(widget[0].impl_flags().0, 0x0003);
    assert_eq!(widget[1].name(), ".ctor");
    assert!(widget[1].signature(&[]).types.is_empty());

    let composable: Vec<_> = index.expect("Test", "Composable").methods().collect();
    assert_eq!(composable.len(), 2);
    assert_eq!(composable[0].name(), ".ctor");
    assert!(composable[0].signature(&[]).types.is_empty());
    assert_eq!(composable[0].flags().0, 0x1884);
    assert_eq!(composable[1].name(), ".ctor");
    assert_eq!(
        composable[1].signature(&[]).types,
        [windows_metadata::Type::I32]
    );
}

#[test]
fn class_shape_attributes_preserve_type_flags() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    class Sealed {}
    #[unsealed]
    class Unsealed {}
    #[static_only]
    class StaticOnly {}
}
"#,
    );

    assert_eq!(index.expect("Test", "Sealed").flags().0, 0x4101);
    assert_eq!(index.expect("Test", "Unsealed").flags().0, 0x4001);
    assert_eq!(index.expect("Test", "StaticOnly").flags().0, 0x4181);
}

#[test]
fn default_and_custom_interface_attributes_are_explicit() {
    use windows_metadata::HasAttributes;

    let source = r#"
#[winrt]
mod Test {
    attribute MarkerAttribute {
        fn();
    }
    interface IFirst {}
    interface IDefault {}
    class Widget {
        #[Marker]
        IFirst,
        #[default]
        IDefault,
    }
}
"#;
    let bytes = windows_rdl::reader()
        .input_text(source)
        .bytes("test")
        .unwrap();
    let index = windows_metadata::reader::Index::new(vec![
        windows_metadata::reader::File::new(bytes.clone()).unwrap(),
    ]);
    let implementations: Vec<_> = index.expect("Test", "Widget").interface_impls().collect();
    assert_eq!(implementations.len(), 2);
    assert_eq!(
        implementations[0].interface(&[]),
        windows_metadata::Type::class_named("Test", "IFirst")
    );
    assert!(implementations[0].has_attribute("MarkerAttribute"));
    assert!(!implementations[0].has_attribute("DefaultAttribute"));
    assert_eq!(
        implementations[1].interface(&[]),
        windows_metadata::Type::class_named("Test", "IDefault")
    );
    assert!(implementations[1].has_attribute("DefaultAttribute"));

    let output = temp_path("interface_attributes", "rdl");
    windows_rdl::writer()
        .input_bytes(&bytes)
        .output(&output)
        .write()
        .unwrap();
    let output = std::fs::read_to_string(output).unwrap();
    let class = &output[output.find("class Widget").unwrap()..];
    assert!(class.find("IFirst").unwrap() < class.find("IDefault").unwrap());
    assert!(output.contains("#[Marker]"));
    assert!(output.contains("#[default]"));
}

#[test]
fn overridable_interfaces_project_protected_virtual_methods() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    interface IWidgetOverrides {
        fn OnChanged(&self, value: i32);
    }
    interface IWidgetProtected {
        Value: i32;
    }
    #[unsealed]
    class Widget {
        #[Windows::Foundation::Metadata::Overridable]
        IWidgetOverrides,
        #[Windows::Foundation::Metadata::Protected]
        IWidgetProtected,
    }
}
#[winrt]
mod Windows {
    mod Foundation {
        mod Metadata {
            attribute OverridableAttribute {
                fn();
            }
            attribute ProtectedAttribute {
                fn();
            }
        }
    }
}
"#,
    );

    let method = index
        .expect("Test", "Widget")
        .methods()
        .find(|method| method.name() == "OnChanged")
        .unwrap();
    assert_eq!(method.flags().0, 0x01c4);
    assert_eq!(method.impl_flags().0, 0x0003);
    assert_eq!(method.signature(&[]).flags.0, 0x20);

    for method_name in ["get_Value", "put_Value"] {
        let method = index
            .expect("Test", "Widget")
            .methods()
            .find(|method| method.name() == method_name)
            .unwrap();
        assert_eq!(method.flags().0, 0x09e4);
    }
}

#[test]
fn generic_interface_members_are_projected_with_concrete_types() {
    let index = compile(
        r#"
#[winrt]
mod Test {
    interface IValue<T> {
        fn Get(&self, fallback: T) -> T;
        Value: T;
    }
    class StringValue {
        IValue<String>,
    }
}
"#,
    );

    let methods: Vec<_> = index.expect("Test", "StringValue").methods().collect();
    assert_eq!(methods.len(), 3);
    let get = methods[0].signature(&[]);
    assert_eq!(get.types, [windows_metadata::Type::String]);
    assert_eq!(get.return_type, windows_metadata::Type::String);
    assert_eq!(
        methods[1].signature(&[]).return_type,
        windows_metadata::Type::String
    );
    assert_eq!(
        methods[2].signature(&[]).types,
        [windows_metadata::Type::String]
    );
    assert_eq!(
        index
            .expect("Test", "StringValue")
            .properties()
            .next()
            .unwrap()
            .signature(&[])
            .return_type,
        windows_metadata::Type::String
    );
}
