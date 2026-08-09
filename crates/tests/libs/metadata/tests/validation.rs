use windows_metadata::writer::RowHandle;
use windows_metadata::*;

fn index(file: writer::File) -> reader::Index {
    file.into_index()
}

fn attribute_ctor(file: &mut writer::File, name: &str, signature: &Signature) -> writer::MemberRef {
    attribute_ctor_in(file, "Test", name, signature)
}

fn attribute_ctor_in(
    file: &mut writer::File,
    namespace: &str,
    name: &str,
    signature: &Signature,
) -> writer::MemberRef {
    let ty = file.TypeRef(namespace, name);
    file.MemberRef(".ctor", signature, writer::MemberRefParent::TypeRef(ty))
}

#[test]
fn writer_handles_match_finalized_row_ids() {
    let mut file = writer::File::new("test");
    let handle = file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::ExplicitLayout,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let class_layout = file.ClassLayout(handle, 4, 4);
    let field_layout = file.FieldLayout(field, 0);
    let expected = handle.row_id(0);
    let expected_class_layout = class_layout.row_id(0);
    let expected_field_layout = field_layout.row_id(0);
    let index = index(file);
    let actual = index.expect("Test", "Value").row_id();
    let actual_class_layout = index
        .expect("Test", "Value")
        .class_layout()
        .unwrap()
        .row_id();
    let actual_field_layout = index
        .expect("Test", "Value")
        .fields()
        .next()
        .unwrap()
        .layout()
        .unwrap()
        .row_id();

    assert_eq!(actual, expected);
    assert_eq!(actual_class_layout, expected_class_layout);
    assert_eq!(actual_field_layout, expected_field_layout);
    assert_eq!(actual.table(), reader::TableId::TypeDef);
    assert_eq!(actual.table() as u8, 0x02);
}

#[test]
fn finalized_image_validates_before_packaging() {
    let mut file = writer::File::new("test");
    let extends = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef("Test", "Value", extends, TypeAttributes::Public);
    file.TypeDef("Test", "Value", extends, TypeAttributes::Public);

    let finalized = file.finalize();
    let errors = finalized.validate(validator::ValidationProfile::Common);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message(), "duplicate type `Test.Value`");

    let bytes = finalized.into_stream();
    assert!(reader::File::new(bytes).is_some());
}

#[test]
fn generic_attribute_constructor_signatures_use_declaring_type_arity() {
    let mut file = writer::File::new("test");
    let attribute_type = file.TypeRef("Test", "GenericAttribute`2");
    let ctor = file.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![Type::Generic(String::new(), 1)],
        },
        writer::MemberRefParent::TypeRef(attribute_type),
    );
    let target = file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(target),
        writer::AttributeType::MemberRef(ctor),
        &[(String::new(), Value::I32(1))],
    );

    let index = index(file);
    let attribute = index
        .expect("Test", "Value")
        .attributes()
        .next()
        .unwrap();
    assert_eq!(
        attribute.ctor().instantiated_signature().types,
        [Type::Generic(String::new(), 1)]
    );
    assert_eq!(
        validator::validate(&index)[0].message(),
        "attribute `Test.GenericAttribute`2` constructor parameter 1 has invalid type `Generic(\"\", 1)`"
    );
}

#[test]
fn duplicate_type_identity_is_rejected() {
    let mut file = writer::File::new("test");
    let extends = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));

    file.TypeDef(
        "Test",
        "Value",
        extends,
        TypeAttributes::Public | TypeAttributes::Sealed,
    );
    file.TypeDef(
        "Test",
        "Value",
        extends,
        TypeAttributes::Public | TypeAttributes::Sealed,
    );

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message(), "duplicate type `Test.Value`");
    assert!(errors[0].related().is_some());
}

#[test]
fn duplicate_field_and_method_identities_are_rejected() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "IValue",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
    );

    file.Field("Value", &Type::I32, FieldAttributes::Public);
    file.Field("Value", &Type::U32, FieldAttributes::Public);

    let signature = Signature {
        return_type: Type::Void,
        types: vec![Type::I32],
        ..Default::default()
    };
    file.MethodDef(
        "Get",
        &signature,
        MethodAttributes::Public,
        Default::default(),
    );
    file.Param("value", 1, ParamAttributes::In);
    file.MethodDef(
        "Get",
        &signature,
        MethodAttributes::Public,
        Default::default(),
    );
    file.Param("value", 1, ParamAttributes::In);

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].message(), "duplicate field `Value`");
    assert_eq!(
        errors[1].message(),
        "duplicate method `Get` on `Test.IValue`"
    );
}

#[test]
fn invalid_overload_metadata_is_rejected() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "IValue",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public
            | TypeAttributes::Interface
            | TypeAttributes::Abstract
            | TypeAttributes::WindowsRuntime,
    );

    let first_signature = Signature {
        types: vec![Type::I32],
        ..Default::default()
    };
    let first = file.MethodDef(
        "GetFirst",
        &first_signature,
        MethodAttributes::Public,
        Default::default(),
    );
    let overload_ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "OverloadAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![Type::String],
        },
    );
    let default_ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "DefaultOverloadAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![],
        },
    );
    file.Attribute(
        writer::HasAttribute::MethodDef(first),
        writer::AttributeType::MemberRef(overload_ctor),
        &[(String::new(), Value::Utf8("Get".to_string()))],
    );
    file.Attribute(
        writer::HasAttribute::MethodDef(first),
        writer::AttributeType::MemberRef(default_ctor),
        &[],
    );

    let second = file.MethodDef(
        "GetSecond",
        &first_signature,
        MethodAttributes::Public,
        Default::default(),
    );
    file.Attribute(
        writer::HasAttribute::MethodDef(second),
        writer::AttributeType::MemberRef(overload_ctor),
        &[(String::new(), Value::Utf8("Get".to_string()))],
    );
    file.Attribute(
        writer::HasAttribute::MethodDef(second),
        writer::AttributeType::MemberRef(default_ctor),
        &[],
    );

    let plain = file.MethodDef(
        "Plain",
        &Signature::default(),
        MethodAttributes::Public,
        Default::default(),
    );
    file.Attribute(
        writer::HasAttribute::MethodDef(plain),
        writer::AttributeType::MemberRef(default_ctor),
        &[],
    );

    let output = index(file);
    assert!(validator::validate(&output).is_empty());
    let errors = validator::Validator::new(&output)
        .profile(validator::ValidationProfile::WinRT)
        .validate();
    assert_eq!(errors.len(), 3);
    assert_eq!(
        errors[0].message(),
        "duplicate overload signature `Get` on `Test.IValue`"
    );
    assert_eq!(
        errors[1].message(),
        "duplicate default overload `Get` on `Test.IValue`"
    );
    assert_eq!(
        errors[2].message(),
        "method `Test.IValue.Plain` has `DefaultOverloadAttribute` without `OverloadAttribute`"
    );
}

#[test]
fn native_typedef_void_field_is_a_common_encoding_exception() {
    let mut file = writer::File::new("test");
    let extends = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    let ty = file.TypeDef(
        "Test",
        "Handle",
        extends,
        TypeAttributes::Public | TypeAttributes::SequentialLayout,
    );
    file.Field("Value", &Type::Void, FieldAttributes::Public);
    let ctor = attribute_ctor_in(
        &mut file,
        "Windows.Win32.Foundation.Metadata",
        "NativeTypedefAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            ..Default::default()
        },
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(ty),
        writer::AttributeType::MemberRef(ctor),
        &[],
    );

    let output = index(file);
    assert!(validator::validate(&output).is_empty());
    assert!(
        validator::Validator::new(&output)
            .profile(validator::ValidationProfile::Win32)
            .validate()
            .is_empty()
    );
}

#[test]
fn malformed_parameter_associations_are_rejected() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "IValue",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
    );

    let signature = Signature {
        return_type: Type::Void,
        types: vec![Type::I32],
        ..Default::default()
    };
    file.MethodDef(
        "Get",
        &signature,
        MethodAttributes::Public,
        Default::default(),
    );
    file.Param("first", 1, ParamAttributes::In);
    file.Param("duplicate", 1, ParamAttributes::In);

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].message(),
        "invalid parameters for `Test.IValue` method `Get`: duplicate Param.Sequence 1"
    );
    assert!(errors[0].related().is_none());
}

#[test]
fn duplicate_properties_events_and_semantics_are_rejected() {
    let mut file = writer::File::new("test");
    let ty = file.TypeDef(
        "Test",
        "IValue",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
    );

    let method_signature = Signature {
        return_type: Type::I32,
        ..Default::default()
    };
    let getter = file.MethodDef(
        "get_Value",
        &method_signature,
        MethodAttributes::Public | MethodAttributes::SpecialName,
        Default::default(),
    );

    let first_property = file.Property("Value", &Type::I32);
    file.PropertyMap(ty, first_property);
    file.MethodSemantics(
        0x0002,
        getter,
        writer::HasSemantics::Property(first_property),
    );
    file.MethodSemantics(
        0x0002,
        getter,
        writer::HasSemantics::Property(first_property),
    );
    file.Property("Value", &Type::I32);

    let event_type = Type::ClassName(TypeName::named("Test", "Handler"));
    let first_event = file.Event("Changed", &event_type);
    file.EventMap(ty, first_event);
    file.Event("Changed", &event_type);

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 3);
    assert_eq!(
        errors[0].message(),
        "property `Value` has duplicate method semantics 0x0002"
    );
    assert_eq!(
        errors[1].message(),
        "duplicate property `Value` on `Test.IValue`"
    );
    assert_eq!(
        errors[2].message(),
        "duplicate event `Changed` on `Test.IValue`"
    );
}

#[test]
fn property_overloads_and_split_accessors_are_accepted() {
    let mut file = writer::File::new("test");
    let ty = file.TypeDef(
        "Test",
        "IValue",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
    );

    let getter = file.MethodDef(
        "get_Value",
        &Signature {
            return_type: Type::I32,
            ..Default::default()
        },
        MethodAttributes::Public | MethodAttributes::SpecialName,
        Default::default(),
    );
    let setter = file.MethodDef(
        "put_Value",
        &Signature {
            return_type: Type::Void,
            types: vec![Type::I32],
            ..Default::default()
        },
        MethodAttributes::Public | MethodAttributes::SpecialName,
        Default::default(),
    );

    let getter_property = file.Property("Value", &Type::I32);
    file.PropertyMap(ty, getter_property);
    file.MethodSemantics(
        0x0002,
        getter,
        writer::HasSemantics::Property(getter_property),
    );
    let setter_property = file.Property("Value", &Type::I32);
    file.MethodSemantics(
        0x0001,
        setter,
        writer::HasSemantics::Property(setter_property),
    );
    file.PropertyWithSignature(
        "Value",
        &Signature {
            return_type: Type::I32,
            types: vec![Type::U32],
            ..Default::default()
        },
        Default::default(),
    );

    assert!(validator::validate(&index(file)).is_empty());
}

#[test]
fn return_types_do_not_distinguish_member_identities() {
    let mut file = writer::File::new("test");
    let ty = file.TypeDef(
        "Test",
        "IValue",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
    );
    file.MethodDef(
        "Get",
        &Signature {
            return_type: Type::I32,
            ..Default::default()
        },
        MethodAttributes::Public,
        Default::default(),
    );
    file.MethodDef(
        "Get",
        &Signature {
            return_type: Type::U32,
            ..Default::default()
        },
        MethodAttributes::Public,
        Default::default(),
    );

    let property = file.Property("Value", &Type::I32);
    file.PropertyMap(ty, property);
    file.Property("Value", &Type::U32);

    let event = file.Event(
        "Changed",
        &Type::ClassName(TypeName::named("Test", "FirstHandler")),
    );
    file.EventMap(ty, event);
    file.Event(
        "Changed",
        &Type::ClassName(TypeName::named("Test", "SecondHandler")),
    );

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 3);
    assert_eq!(
        errors[0].message(),
        "duplicate property `Value` on `Test.IValue`"
    );
    assert_eq!(
        errors[1].message(),
        "duplicate event `Changed` on `Test.IValue`"
    );
    assert_eq!(
        errors[2].message(),
        "duplicate method `Get` on `Test.IValue`"
    );
    assert!(
        errors
            .iter()
            .all(|error| error.category() == validator::ValidationCategory::Duplicate)
    );
}

#[test]
fn duplicate_interface_implementations_are_rejected() {
    let mut file = writer::File::new("test");
    let object = file.TypeRef("System", "Object");
    let class = file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::TypeRef(object),
        TypeAttributes::Public,
    );
    let interface = Type::ClassName(TypeName::named("Test", "IValue"));
    file.InterfaceImpl(class, &interface);
    file.InterfaceImpl(class, &interface);

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].message(),
        "duplicate interface `Test.IValue` on `Test.Value`"
    );
    assert_eq!(
        errors[0].category(),
        validator::ValidationCategory::Duplicate
    );
}

#[test]
fn invalid_attribute_constructors_are_rejected() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let expected_parent = field.row_id(0);
    let attribute = writer::MemberRefParent::TypeRef(file.TypeRef("Test", "MarkerAttribute"));

    let wrong_name = file.MemberRef("Create", &Signature::default(), attribute);
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(wrong_name),
        &[],
    );

    let static_ctor = file.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes(0),
            ..Default::default()
        },
        attribute,
    );
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(static_ctor),
        &[],
    );

    let returning_ctor = file.MemberRef(
        ".ctor",
        &Signature {
            return_type: Type::I32,
            ..Default::default()
        },
        attribute,
    );
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(returning_ctor),
        &[],
    );

    let vararg_ctor = file.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS | MethodCallAttributes::VARARG,
            ..Default::default()
        },
        attribute,
    );
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(vararg_ctor),
        &[],
    );

    let valid_ctor = file.MemberRef(".ctor", &Signature::default(), attribute);
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(valid_ctor),
        &[0, 0],
    );

    let invalid_parameters = file.MemberRef(
        ".ctor",
        &Signature {
            types: vec![
                Type::ISize,
                Type::PtrMut(Box::new(Type::Void), 1),
                Type::ClassName(TypeName::named("Test", "Value")),
            ],
            ..Default::default()
        },
        attribute,
    );
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(invalid_parameters),
        &[1, 0],
    );

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 8);
    assert_eq!(
        errors[0].message(),
        "attribute `Test.MarkerAttribute` constructor is named `Create` instead of `.ctor`"
    );
    assert_eq!(
        errors[1].message(),
        "attribute `Test.MarkerAttribute` constructor must be an instance method"
    );
    assert_eq!(
        errors[2].message(),
        "attribute `Test.MarkerAttribute` constructor must return void"
    );
    assert_eq!(
        errors[3].message(),
        "attribute `Test.MarkerAttribute` constructor must use the default calling convention"
    );
    assert_eq!(
        errors[4].message(),
        "attribute `Test.MarkerAttribute` value is invalid at byte 0: invalid custom-attribute prolog"
    );
    assert_eq!(
        errors[5].message(),
        "attribute `Test.MarkerAttribute` constructor parameter 1 has invalid type `ISize`"
    );
    assert_eq!(
        errors[6].message(),
        "attribute `Test.MarkerAttribute` constructor parameter 2 has invalid type `PtrMut(Void, 1)`"
    );
    assert_eq!(
        errors[7].message(),
        "attribute `Test.MarkerAttribute` constructor parameter 3 has invalid type `Test.Value`"
    );
    assert!(errors.iter().all(|error| {
        error.category() == validator::ValidationCategory::Invalid
            && error.related() == Some(expected_parent)
    }));
}

#[test]
fn invalid_attribute_values_are_rejected() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let expected_parent = field.row_id(0);

    let truncated = attribute_ctor(&mut file, "TruncatedAttribute", &Signature::default());
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(truncated),
        &[1, 0],
    );

    let boolean = attribute_ctor(
        &mut file,
        "BooleanAttribute",
        &Signature {
            types: vec![Type::Bool],
            ..Default::default()
        },
    );
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(boolean),
        &[1, 0, 2],
    );

    let tag = attribute_ctor(&mut file, "TagAttribute", &Signature::default());
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(tag),
        &[1, 0, 1, 0, 0x52],
    );

    let trailing = attribute_ctor(&mut file, "TrailingAttribute", &Signature::default());
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(trailing),
        &[1, 0, 0, 0, 0],
    );

    let character = attribute_ctor(
        &mut file,
        "CharacterAttribute",
        &Signature {
            types: vec![Type::Char],
            ..Default::default()
        },
    );
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(character),
        &[1, 0, 65, 0, 0, 0],
    );

    let array = attribute_ctor(
        &mut file,
        "ArrayAttribute",
        &Signature {
            types: vec![Type::Array(Box::new(Type::I32))],
            ..Default::default()
        },
    );
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(array),
        &[1, 0, 3, 0, 0, 0, 0, 0],
    );

    let index = index(file);
    let errors = validator::validate(&index);
    assert_eq!(errors.len(), 5);
    assert_eq!(
        errors[0].message(),
        "attribute `Test.TruncatedAttribute` value is invalid at byte 2: truncated custom-attribute value"
    );
    assert_eq!(
        errors[1].message(),
        "attribute `Test.BooleanAttribute` value is invalid at byte 2: invalid Boolean value"
    );
    assert_eq!(
        errors[2].message(),
        "attribute `Test.TagAttribute` value is invalid at byte 4: invalid named-argument tag"
    );
    assert_eq!(
        errors[3].message(),
        "attribute `Test.TrailingAttribute` value is invalid at byte 4: trailing custom-attribute data"
    );
    assert_eq!(
        errors[4].message(),
        "attribute `Test.ArrayAttribute` value is invalid at byte 6: array element count exceeds remaining data"
    );
    assert!(errors.iter().all(|error| {
        error.category() == validator::ValidationCategory::Invalid
            && error.related() == Some(expected_parent)
    }));

    let character = index
        .attributes()
        .find(|attribute| attribute.name() == "CharacterAttribute")
        .unwrap();
    assert_eq!(
        character.try_value().unwrap(),
        [(String::new(), Value::Char(65))]
    );
}

#[test]
fn attribute_enum_values_use_reference_backing_types() {
    let mut reference = writer::File::new("reference");
    let system_enum = reference.TypeRef("System", "Enum");
    reference.TypeDef(
        "Test",
        "SmallEnum",
        writer::TypeDefOrRef::TypeRef(system_enum),
        TypeAttributes::Public | TypeAttributes::Sealed,
    );
    reference.Field("value__", &Type::U8, FieldAttributes::Public);
    let reference = index(reference);

    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let enum_name = TypeName::named("Test", "SmallEnum");
    let ctor = attribute_ctor(
        &mut file,
        "EnumAttribute",
        &Signature {
            types: vec![Type::ValueName(enum_name.clone())],
            ..Default::default()
        },
    );
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(ctor),
        &[1, 0, 7, 0, 0],
    );
    let output = index(file);
    let attribute = output.attributes().next().unwrap();

    assert!(attribute.try_value().unwrap_err().is_unsupported());
    assert_eq!(
        attribute.try_value_with_references(&reference).unwrap(),
        [(
            String::new(),
            Value::EnumValue(enum_name, Box::new(Value::U8(7)))
        )]
    );
    assert!(
        validator::Validator::new(&output)
            .references(&reference)
            .validate()
            .is_empty()
    );
}

#[test]
fn attribute_char_values_preserve_utf16_code_units() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let ctor = attribute_ctor(
        &mut file,
        "CharAttribute",
        &Signature {
            types: vec![Type::Char],
            ..Default::default()
        },
    );
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(ctor),
        &[(String::new(), Value::Char(0xd800))],
    );

    let index = index(file);
    let attribute = index.attributes().next().unwrap();
    assert_eq!(
        attribute.try_value().unwrap(),
        [(String::new(), Value::Char(0xd800))]
    );
    assert!(validator::validate(&index).is_empty());
}

#[test]
fn attribute_null_values_preserve_their_types() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let system_type = Type::ClassName(TypeName::named("System", "Type"));
    let ctor = attribute_ctor(
        &mut file,
        "NullAttribute",
        &Signature {
            types: vec![Type::String, system_type.clone()],
            ..Default::default()
        },
    );
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(ctor),
        &[
            (String::new(), Value::Null(Type::String)),
            (String::new(), Value::Null(system_type.clone())),
        ],
    );

    let index = index(file);
    let attribute = index.attributes().next().unwrap();
    assert_eq!(
        attribute.try_value().unwrap(),
        [
            (String::new(), Value::Null(Type::String)),
            (String::new(), Value::Null(system_type)),
        ]
    );
    assert!(validator::validate(&index).is_empty());
}

#[test]
fn attribute_boxed_values_preserve_their_types() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let ctor = attribute_ctor(
        &mut file,
        "BoxedAttribute",
        &Signature {
            types: vec![Type::Object, Type::Object, Type::Object],
            ..Default::default()
        },
    );
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(ctor),
        &[
            (String::new(), Value::Boxed(Box::new(Value::I32(42)))),
            (
                String::new(),
                Value::Boxed(Box::new(Value::Utf8("hello".to_string()))),
            ),
            (String::new(), Value::Null(Type::Object)),
        ],
    );

    let index = index(file);
    let attribute = index.attributes().next().unwrap();
    assert_eq!(
        attribute.try_value().unwrap(),
        [
            (String::new(), Value::Boxed(Box::new(Value::I32(42)))),
            (
                String::new(),
                Value::Boxed(Box::new(Value::Utf8("hello".to_string()))),
            ),
            (String::new(), Value::Null(Type::Object)),
        ]
    );
    assert!(validator::validate(&index).is_empty());
}

#[test]
fn attribute_array_values_preserve_their_element_types() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let ctor = attribute_ctor(
        &mut file,
        "ArrayAttribute",
        &Signature {
            types: vec![
                Type::Array(Box::new(Type::I32)),
                Type::Array(Box::new(Type::String)),
                Type::Array(Box::new(Type::U8)),
            ],
            ..Default::default()
        },
    );
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(ctor),
        &[
            (
                String::new(),
                Value::Array(Type::I32, vec![Value::I32(1), Value::I32(2)]),
            ),
            (
                String::new(),
                Value::Array(
                    Type::String,
                    vec![
                        Value::Utf8("hello".to_string()),
                        Value::Null(Type::String),
                    ],
                ),
            ),
            (
                String::new(),
                Value::Null(Type::Array(Box::new(Type::U8))),
            ),
            (
                "Items".to_string(),
                Value::Array(Type::U16, vec![Value::U16(3), Value::U16(4)]),
            ),
        ],
    );

    let index = index(file);
    let attribute = index.attributes().next().unwrap();
    assert_eq!(
        attribute.try_value().unwrap(),
        [
            (
                String::new(),
                Value::Array(Type::I32, vec![Value::I32(1), Value::I32(2)]),
            ),
            (
                String::new(),
                Value::Array(
                    Type::String,
                    vec![
                        Value::Utf8("hello".to_string()),
                        Value::Null(Type::String),
                    ],
                ),
            ),
            (
                String::new(),
                Value::Null(Type::Array(Box::new(Type::U8))),
            ),
            (
                "Items".to_string(),
                Value::Array(Type::U16, vec![Value::U16(3), Value::U16(4)]),
            ),
        ]
    );
    assert!(validator::validate(&index).is_empty());
}

#[test]
fn attribute_named_arguments_are_validated() {
    let mut reference = writer::File::new("reference");
    let system_attribute = reference.TypeRef("System", "Attribute");
    let definition = reference.TypeDef(
        "Test",
        "NamedAttribute",
        writer::TypeDefOrRef::TypeRef(system_attribute),
        TypeAttributes::Public,
    );
    reference.Field("Field", &Type::I32, FieldAttributes::Public);
    let property = reference.PropertyWithSignature(
        "Property",
        &Signature {
            return_type: Type::U32,
            ..Default::default()
        },
        0,
    );
    let setter = reference.MethodDef(
        "set_Property",
        &Signature {
            return_type: Type::Void,
            types: vec![Type::U32],
            ..Default::default()
        },
        MethodAttributes::Public | MethodAttributes::SpecialName,
        MethodImplAttributes::default(),
    );
    reference.MethodSemantics(0x0001, setter, writer::HasSemantics::Property(property));
    reference.Field(
        "StaticField",
        &Type::I32,
        FieldAttributes::Public | FieldAttributes::Static,
    );
    reference.PropertyWithSignature(
        "ReadOnly",
        &Signature {
            return_type: Type::U32,
            ..Default::default()
        },
        0,
    );
    reference.PropertyMap(definition, property);
    let reference = index(reference);

    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let expected_parent = field.row_id(0);
    let ctor = attribute_ctor(&mut file, "NamedAttribute", &Signature::default());
    let mut blob = vec![1, 0, 7, 0];
    blob.extend([0x53, 0x08, 5, b'F', b'i', b'e', b'l', b'd', 1, 0, 0, 0]);
    blob.extend([
        0x54, 0x09, 8, b'P', b'r', b'o', b'p', b'e', b'r', b't', b'y', 2, 0, 0, 0,
    ]);
    blob.extend([
        0x53, 0x08, 7, b'M', b'i', b's', b's', b'i', b'n', b'g', 3, 0, 0, 0,
    ]);
    blob.extend([0x53, 0x09, 5, b'F', b'i', b'e', b'l', b'd', 4, 0, 0, 0]);
    blob.extend([
        0x54, 0x09, 8, b'P', b'r', b'o', b'p', b'e', b'r', b't', b'y', 5, 0, 0, 0,
    ]);
    blob.extend([
        0x53, 0x08, 11, b'S', b't', b'a', b't', b'i', b'c', b'F', b'i', b'e', b'l', b'd', 6, 0, 0,
        0,
    ]);
    blob.extend([
        0x54, 0x09, 8, b'R', b'e', b'a', b'd', b'O', b'n', b'l', b'y', 7, 0, 0, 0,
    ]);
    file.AttributeBlob(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(ctor),
        &blob,
    );
    let output = index(file);

    let errors = validator::Validator::new(&output)
        .references(&reference)
        .validate();
    assert_eq!(errors.len(), 6);
    assert_eq!(
        errors[0].message(),
        "attribute `Test.NamedAttribute` has no named field `Missing`"
    );
    assert_eq!(
        errors[1].message(),
        "attribute `Test.NamedAttribute` has duplicate named field argument `Field`"
    );
    assert_eq!(
        errors[2].message(),
        "attribute `Test.NamedAttribute` named field `Field` expects `I32` but found `U32`"
    );
    assert_eq!(
        errors[3].message(),
        "attribute `Test.NamedAttribute` has duplicate named property argument `Property`"
    );
    assert_eq!(
        errors[4].message(),
        "attribute `Test.NamedAttribute` named field `StaticField` is not a public writable instance member"
    );
    assert_eq!(
        errors[5].message(),
        "attribute `Test.NamedAttribute` named property `ReadOnly` is not a public writable instance member"
    );
    assert!(errors.iter().all(|error| {
        error.related() == Some(expected_parent)
            && matches!(
                error.category(),
                validator::ValidationCategory::Invalid | validator::ValidationCategory::Duplicate
            )
    }));
}

#[test]
fn attribute_multiplicity_is_validated() {
    let mut reference = writer::File::new("reference");
    let system_attribute = reference.TypeRef("System", "Attribute");
    let attribute_targets = TypeName::named("Windows.Foundation.Metadata", "AttributeTargets");

    reference.TypeDef(
        "Windows.Foundation.Metadata",
        "AttributeUsageAttribute",
        writer::TypeDefOrRef::TypeRef(system_attribute),
        TypeAttributes::Public,
    );
    let usage_ref = reference.TypeRef("Windows.Foundation.Metadata", "AttributeUsageAttribute");
    let usage_ctor = reference.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![Type::ValueName(attribute_targets.clone())],
        },
        writer::MemberRefParent::TypeRef(usage_ref),
    );
    reference.TypeDef(
        "Windows.Foundation.Metadata",
        "AllowMultipleAttribute",
        writer::TypeDefOrRef::TypeRef(system_attribute),
        TypeAttributes::Public,
    );
    let allow_multiple_ref =
        reference.TypeRef("Windows.Foundation.Metadata", "AllowMultipleAttribute");
    let allow_multiple_ctor = reference.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            ..Default::default()
        },
        writer::MemberRefParent::TypeRef(allow_multiple_ref),
    );

    let method_only = reference.TypeDef(
        "Test",
        "MethodOnlyAttribute",
        writer::TypeDefOrRef::TypeRef(system_attribute),
        TypeAttributes::Public,
    );
    reference.Attribute(
        writer::HasAttribute::TypeDef(method_only),
        writer::AttributeType::MemberRef(usage_ctor),
        &[(
            String::new(),
            Value::EnumValue(attribute_targets, Box::new(Value::I32(64))),
        )],
    );
    let repeatable = reference.TypeDef(
        "Test",
        "RepeatableAttribute",
        writer::TypeDefOrRef::TypeRef(system_attribute),
        TypeAttributes::Public,
    );
    reference.Attribute(
        writer::HasAttribute::TypeDef(repeatable),
        writer::AttributeType::MemberRef(usage_ctor),
        &[(
            String::new(),
            Value::EnumValue(
                TypeName::named("Windows.Foundation.Metadata", "AttributeTargets"),
                Box::new(Value::I32(64)),
            ),
        )],
    );
    reference.Attribute(
        writer::HasAttribute::TypeDef(repeatable),
        writer::AttributeType::MemberRef(allow_multiple_ctor),
        &[],
    );
    reference.TypeDef(
        "Test",
        "UnspecifiedAttribute",
        writer::TypeDefOrRef::TypeRef(system_attribute),
        TypeAttributes::Public,
    );
    let reference = index(reference);

    let mut file = writer::File::new("test");
    let method_only_ref = file.TypeRef("Test", "MethodOnlyAttribute");
    let method_only_ctor = file.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            ..Default::default()
        },
        writer::MemberRefParent::TypeRef(method_only_ref),
    );
    let repeatable_ref = file.TypeRef("Test", "RepeatableAttribute");
    let repeatable_ctor = file.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            ..Default::default()
        },
        writer::MemberRefParent::TypeRef(repeatable_ref),
    );
    let unspecified_ref = file.TypeRef("Test", "UnspecifiedAttribute");
    let unspecified_ctor = file.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            ..Default::default()
        },
        writer::MemberRefParent::TypeRef(unspecified_ref),
    );
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    let expected_parent = field.row_id(0);
    for _ in 0..2 {
        file.Attribute(
            writer::HasAttribute::Field(field),
            writer::AttributeType::MemberRef(method_only_ctor),
            &[],
        );
        file.Attribute(
            writer::HasAttribute::Field(field),
            writer::AttributeType::MemberRef(repeatable_ctor),
            &[],
        );
        file.Attribute(
            writer::HasAttribute::Field(field),
            writer::AttributeType::MemberRef(unspecified_ctor),
            &[],
        );
    }

    let output = index(file);
    assert!(validator::validate(&output).is_empty());

    let errors = validator::Validator::new(&output)
        .references(&reference)
        .validate();
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].message(),
        "duplicate attribute `Test.MethodOnlyAttribute`"
    );
    assert_eq!(
        errors[0].category(),
        validator::ValidationCategory::Duplicate
    );
    assert_eq!(errors[0].related(), Some(expected_parent));
}

#[test]
fn winrt_attribute_targets_are_validated() {
    let mut reference = writer::File::new("reference");
    let system_attribute = reference.TypeRef("System", "Attribute");
    reference.TypeDef(
        "Windows.Foundation.Metadata",
        "AttributeUsageAttribute",
        writer::TypeDefOrRef::TypeRef(system_attribute),
        TypeAttributes::Public,
    );
    let usage_ref = reference.TypeRef("Windows.Foundation.Metadata", "AttributeUsageAttribute");
    let usage_ctor = reference.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![Type::I32],
        },
        writer::MemberRefParent::TypeRef(usage_ref),
    );
    let method_only = reference.TypeDef(
        "Test",
        "MethodOnlyAttribute",
        writer::TypeDefOrRef::TypeRef(system_attribute),
        TypeAttributes::Public,
    );
    reference.Attribute(
        writer::HasAttribute::TypeDef(method_only),
        writer::AttributeType::MemberRef(usage_ctor),
        &[(String::new(), Value::I32(64))],
    );
    let reference = index(reference);

    let mut file = writer::File::new("test");
    let attribute_ref = file.TypeRef("Test", "MethodOnlyAttribute");
    let attribute_ctor = file.MemberRef(
        ".ctor",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            ..Default::default()
        },
        writer::MemberRefParent::TypeRef(attribute_ref),
    );
    file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::WindowsRuntime,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    file.Attribute(
        writer::HasAttribute::Field(field),
        writer::AttributeType::MemberRef(attribute_ctor),
        &[],
    );
    let second_field = file.Field("SecondValue", &Type::I32, FieldAttributes::Public);
    file.Attribute(
        writer::HasAttribute::Field(second_field),
        writer::AttributeType::MemberRef(attribute_ctor),
        &[],
    );

    let output = index(file);
    assert!(validator::Validator::new(&output)
        .references(&reference)
        .validate()
        .is_empty());

    let errors = validator::Validator::new(&output)
        .references(&reference)
        .profile(validator::ValidationProfile::Windows)
        .validate();
    assert_eq!(errors.len(), 2);
    for error in &errors {
        assert_eq!(
            error.message(),
            "attribute `Test.MethodOnlyAttribute` cannot target a field"
        );
    }
    assert_eq!(
        errors
            .iter()
            .map(|error| error.related())
            .collect::<Vec<_>>(),
        [Some(field.row_id(0)), Some(second_field.row_id(0))]
    );
}

#[test]
fn winrt_api_contract_shape_is_validated() {
    let mut file = writer::File::new("test");
    let contract = file.TypeDef(
        "Test",
        "BadContract",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::WindowsRuntime,
    );
    let api_contract_ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "ApiContractAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            ..Default::default()
        },
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(contract),
        writer::AttributeType::MemberRef(api_contract_ctor),
        &[],
    );

    let index = index(file);
    assert!(validator::validate(&index).is_empty());
    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::WinRT)
        .validate();
    assert_eq!(errors.len(), 2);
    assert_eq!(
        errors[0].message(),
        "API contract `Test.BadContract` must be a struct"
    );
    assert_eq!(
        errors[1].message(),
        "API contract `Test.BadContract` must have a contract version"
    );
}

#[test]
fn winrt_contract_version_target_is_validated() {
    let mut file = writer::File::new("test");
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef(
        "Test",
        "NotAContract",
        value_type,
        TypeAttributes::Public
            | TypeAttributes::WindowsRuntime
            | TypeAttributes::SequentialLayout,
    );
    let value = file.TypeDef(
        "Test",
        "Value",
        value_type,
        TypeAttributes::Public
            | TypeAttributes::WindowsRuntime
            | TypeAttributes::SequentialLayout,
    );
    let contract_version_ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "ContractVersionAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![
                Type::ClassName(TypeName::named("System", "Type")),
                Type::U32,
            ],
        },
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(value),
        writer::AttributeType::MemberRef(contract_version_ctor),
        &[
            (
                String::new(),
                Value::TypeName(TypeName::named("Test", "NotAContract")),
            ),
            (String::new(), Value::U32(65536)),
        ],
    );

    let index = index(file);
    assert!(validator::validate(&index).is_empty());
    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::WinRT)
        .validate();
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].message(),
        "contract version target `Test.NotAContract` is not an API contract"
    );
}

#[test]
fn winrt_contract_version_must_not_be_zero() {
    let mut file = writer::File::new("test");
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    let contract = file.TypeDef(
        "Test",
        "Contract",
        value_type,
        TypeAttributes::Public
            | TypeAttributes::WindowsRuntime
            | TypeAttributes::SequentialLayout,
    );
    let api_contract_ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "ApiContractAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            ..Default::default()
        },
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(contract),
        writer::AttributeType::MemberRef(api_contract_ctor),
        &[],
    );
    let contract_version_ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "ContractVersionAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![Type::U32],
        },
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(contract),
        writer::AttributeType::MemberRef(contract_version_ctor),
        &[(String::new(), Value::U32(0))],
    );

    let index = index(file);
    assert!(validator::validate(&index).is_empty());
    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::WinRT)
        .validate();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].message(), "contract version must not be zero");
}

#[test]
fn winrt_member_version_must_not_precede_owning_type() {
    let mut file = writer::File::new("test");
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    let contract = file.TypeDef(
        "Test",
        "Contract",
        value_type,
        TypeAttributes::Public
            | TypeAttributes::WindowsRuntime
            | TypeAttributes::SequentialLayout,
    );
    let api_contract_ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "ApiContractAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            ..Default::default()
        },
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(contract),
        writer::AttributeType::MemberRef(api_contract_ctor),
        &[],
    );
    let self_version_ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "ContractVersionAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![Type::U32],
        },
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(contract),
        writer::AttributeType::MemberRef(self_version_ctor),
        &[(String::new(), Value::U32(131072))],
    );

    let object = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "Object"));
    let class = file.TypeDef(
        "Test",
        "Value",
        object,
        TypeAttributes::Public | TypeAttributes::WindowsRuntime,
    );
    let method = file.MethodDef(
        "Old",
        &Signature {
            flags: MethodCallAttributes(0),
            return_type: Type::Void,
            ..Default::default()
        },
        MethodAttributes::Public | MethodAttributes::Static,
        MethodImplAttributes::default(),
    );
    let named_version_ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "ContractVersionAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![
                Type::ClassName(TypeName::named("System", "Type")),
                Type::U32,
            ],
        },
    );
    let contract_args = |version| {
        vec![
            (
                String::new(),
                Value::TypeName(TypeName::named("Test", "Contract")),
            ),
            (String::new(), Value::U32(version)),
        ]
    };
    file.Attribute(
        writer::HasAttribute::TypeDef(class),
        writer::AttributeType::MemberRef(named_version_ctor),
        &contract_args(131072),
    );
    file.Attribute(
        writer::HasAttribute::MethodDef(method),
        writer::AttributeType::MemberRef(named_version_ctor),
        &contract_args(65536),
    );

    let index = index(file);
    assert!(validator::validate(&index).is_empty());
    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::WinRT)
        .validate();
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].message(),
        "contract version 65536 precedes owning type `Test.Value` version 131072"
    );
}

#[test]
fn invalid_member_signatures_are_rejected() {
    let mut file = writer::File::new("test");
    let ty = file.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    file.Field("Bad", &Type::Void, FieldAttributes::Public);
    file.MethodDef(
        "StaticWithThis",
        &Signature::default(),
        MethodAttributes::Public | MethodAttributes::Static,
        MethodImplAttributes::default(),
    );
    file.MethodDef(
        "InstanceWithoutThis",
        &Signature {
            flags: MethodCallAttributes(0),
            ..Default::default()
        },
        MethodAttributes::Public,
        MethodImplAttributes::default(),
    );
    file.MethodDef(
        "BadParameter",
        &Signature {
            types: vec![Type::Void],
            ..Default::default()
        },
        MethodAttributes::Public,
        MethodImplAttributes::default(),
    );
    let bad_value = file.PropertyWithSignature(
        "BadValue",
        &Signature {
            return_type: Type::Void,
            ..Default::default()
        },
        0,
    );
    file.PropertyWithSignature(
        "BadIndex",
        &Signature {
            return_type: Type::I32,
            types: vec![Type::Array(Box::new(Type::Void))],
            ..Default::default()
        },
        0,
    );
    file.PropertyMap(ty, bad_value);

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 5);
    assert_eq!(
        errors[0].message(),
        "field `Test.Value.Bad` has invalid type `Void`"
    );
    assert_eq!(
        errors[1].message(),
        "property `Test.Value.BadValue` has invalid value type `Void`"
    );
    assert_eq!(
        errors[2].message(),
        "property `Test.Value.BadIndex` index parameter 1 has invalid type `Void[]`"
    );
    assert_eq!(
        errors[3].message(),
        "static method `Test.Value.StaticWithThis` has an instance calling convention"
    );
    assert_eq!(
        errors[4].message(),
        "method `Test.Value.BadParameter` parameter 1 has invalid type `Void`"
    );
}

#[test]
fn invalid_method_semantics_are_rejected() {
    let mut file = writer::File::new("test");
    let ty = file.TypeDef(
        "Test",
        "IValue",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
    );
    let method = file.MethodDef(
        "Value",
        &Signature::default(),
        MethodAttributes::Public,
        Default::default(),
    );
    let property = file.Property("Value", &Type::I32);
    file.PropertyMap(ty, property);
    file.MethodSemantics(0x0040, method, writer::HasSemantics::Property(property));

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].message(),
        "property `Value` has invalid method semantics 0x0040"
    );
    assert!(errors[0].related().is_some());
}

#[test]
fn malformed_property_and_event_ownership_is_rejected() {
    let mut file = writer::File::new("test");
    let ty = file.TypeDef(
        "Test",
        "IValue",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::Interface | TypeAttributes::Abstract,
    );

    file.Property("Orphaned", &Type::I32);
    let property = file.Property("Value", &Type::I32);
    file.PropertyMap(ty, property);
    file.PropertyMap(ty, property);

    let event_type = Type::ClassName(TypeName::named("Test", "Handler"));
    file.Event("Orphaned", &event_type);
    let event = file.Event("Changed", &event_type);
    file.EventMap(ty, event);
    file.EventMap(ty, event);

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 4);
    assert_eq!(
        errors[0].message(),
        "duplicate property map for `Test.IValue`"
    );
    assert_eq!(errors[1].message(), "property `Orphaned` has no owner");
    assert_eq!(errors[2].message(), "duplicate event map for `Test.IValue`");
    assert_eq!(errors[3].message(), "event `Orphaned` has no owner");
}

#[test]
fn malformed_layouts_are_rejected() {
    let mut file = writer::File::new("test");
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    let ty = file.TypeDef(
        "Test",
        "Value",
        value_type,
        TypeAttributes::Public | TypeAttributes::SequentialLayout,
    );
    let field = file.Field("Value", &Type::I32, FieldAttributes::Public);
    file.ClassLayout(ty, 3, 4);
    file.ClassLayout(ty, 4, 4);
    file.FieldLayout(field, 0);
    file.FieldLayout(field, 4);

    let errors = validator::validate(&index(file));
    assert_eq!(errors.len(), 5);
    assert_eq!(
        errors[0].message(),
        "class layout for `Test.Value` has invalid packing size 3"
    );
    assert_eq!(
        errors[1].message(),
        "duplicate class layout for `Test.Value`"
    );
    assert_eq!(
        errors[2].message(),
        "field layout for `Test.Value.Value` requires explicit layout"
    );
    assert_eq!(errors[3].message(), "duplicate field layout for `Value`");
    assert_eq!(
        errors[4].message(),
        "field layout for `Test.Value.Value` requires explicit layout"
    );
}

#[test]
fn committed_windows_metadata_is_valid() {
    let index = reader::Index::new(
        [windows_default::WINRT, windows_default::WIN32]
            .into_iter()
            .map(|bytes| reader::File::new(bytes.to_vec()).unwrap())
            .collect(),
    );

    let errors = validator::validate(&index);
    assert!(
        errors.is_empty(),
        "committed metadata validation failed:\n{}",
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn winrt_profile_validates_factory_interfaces() {
    let mut file = writer::File::new("test");
    let object = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "Object"));
    let class = file.TypeDef(
        "Test",
        "Value",
        object,
        TypeAttributes::Public | TypeAttributes::WindowsRuntime,
    );
    file.TypeDef(
        "Test",
        "FactoryClass",
        object,
        TypeAttributes::Public | TypeAttributes::WindowsRuntime,
    );

    let ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "StaticAttribute",
        &Signature {
            flags: MethodCallAttributes::HASTHIS,
            return_type: Type::Void,
            types: vec![Type::ClassName(TypeName::named("System", "Type"))],
        },
    );
    file.Attribute(
        writer::HasAttribute::TypeDef(class),
        writer::AttributeType::MemberRef(ctor),
        &[(
            String::new(),
            Value::TypeName(TypeName::named("Test", "FactoryClass")),
        )],
    );

    let index = index(file);
    assert!(validator::validate(&index).is_empty());
    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::WinRT)
        .validate();
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].message(),
        "`Test.Value` factory `Test.FactoryClass` is not an interface"
    );
}

#[test]
fn committed_windows_metadata_is_valid_for_windows_profiles() {
    let index = reader::Index::new(
        [windows_default::WINRT, windows_default::WIN32]
            .into_iter()
            .map(|bytes| reader::File::new(bytes.to_vec()).unwrap())
            .collect(),
    );

    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::Windows)
        .validate();
    assert!(
        errors.is_empty(),
        "committed metadata profile validation failed:\n{}",
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn profile_type_flags_are_validated() {
    let mut winrt = writer::File::new("winrt");
    winrt.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let winrt = index(winrt);
    assert!(validator::validate(&winrt).is_empty());
    let errors = validator::Validator::new(&winrt)
        .profile(validator::ValidationProfile::WinRT)
        .validate();
    assert_eq!(
        errors[0].message(),
        "WinRT type `Test.Value` must have the WindowsRuntime flag"
    );

    let mut win32 = writer::File::new("win32");
    win32.TypeDef(
        "Test",
        "Value",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public | TypeAttributes::WindowsRuntime,
    );
    let win32 = index(win32);
    let errors = validator::Validator::new(&win32)
        .profile(validator::ValidationProfile::Win32)
        .validate();
    assert_eq!(
        errors[0].message(),
        "Win32 type `Test.Value` must not have the WindowsRuntime flag"
    );
}

#[test]
fn winrt_interface_and_default_flags_are_validated() {
    let mut file = writer::File::new("test");
    for name in ["IFirst", "ISecond"] {
        file.TypeDef(
            "Test",
            name,
            writer::TypeDefOrRef::default(),
            TypeAttributes::Public
                | TypeAttributes::WindowsRuntime
                | TypeAttributes::Interface,
        );
    }
    let object = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "Object"));
    let class = file.TypeDef(
        "Test",
        "Value",
        object,
        TypeAttributes::Public | TypeAttributes::WindowsRuntime,
    );
    let first = file.InterfaceImpl(
        class,
        &Type::ClassName(TypeName::named("Test", "IFirst")),
    );
    let second = file.InterfaceImpl(
        class,
        &Type::ClassName(TypeName::named("Test", "ISecond")),
    );
    let ctor = attribute_ctor_in(
        &mut file,
        "Windows.Foundation.Metadata",
        "DefaultAttribute",
        &Signature::default(),
    );
    file.Attribute(
        writer::HasAttribute::InterfaceImpl(first),
        writer::AttributeType::MemberRef(ctor),
        &[],
    );
    file.Attribute(
        writer::HasAttribute::InterfaceImpl(second),
        writer::AttributeType::MemberRef(ctor),
        &[],
    );

    let index = index(file);
    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::WinRT)
        .validate();
    assert_eq!(errors.len(), 3);
    assert_eq!(
        errors[0].message(),
        "WinRT interface `Test.IFirst` must have the Abstract flag"
    );
    assert_eq!(
        errors[1].message(),
        "WinRT interface `Test.ISecond` must have the Abstract flag"
    );
    assert_eq!(
        errors[2].message(),
        "runtime class `Test.Value` has multiple default interfaces"
    );
    assert!(errors[2].related().is_some());
}

#[test]
fn win32_pinvoke_mapping_is_validated() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Apis",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let signature = Signature {
        flags: MethodCallAttributes::default(),
        return_type: Type::Void,
        types: vec![],
    };
    file.MethodDef(
        "MissingMap",
        &signature,
        MethodAttributes::Public | MethodAttributes::Static | MethodAttributes::PInvokeImpl,
        Default::default(),
    );
    let extra_map = file.MethodDef(
        "ExtraMap",
        &signature,
        MethodAttributes::Public | MethodAttributes::Static,
        Default::default(),
    );
    file.ImplMap(
        extra_map,
        PInvokeAttributes::CallConvPlatformapi,
        "ExtraMap",
        "test.dll",
    );

    let index = index(file);
    assert!(validator::validate(&index).is_empty());
    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::Win32)
        .validate();
    assert_eq!(errors.len(), 2);
    assert_eq!(
        errors[0].message(),
        "Win32 method `Test.Apis.MissingMap` has PInvokeImpl without an ImplMap"
    );
    assert_eq!(
        errors[1].message(),
        "Win32 method `Test.Apis.ExtraMap` has an ImplMap without PInvokeImpl"
    );
}

#[test]
fn win32_pinvoke_calling_conventions_are_validated() {
    let mut file = writer::File::new("test");
    file.TypeDef(
        "Test",
        "Apis",
        writer::TypeDefOrRef::default(),
        TypeAttributes::Public,
    );
    let instance = file.MethodDef(
        "Instance",
        &Signature::default(),
        MethodAttributes::Public | MethodAttributes::PInvokeImpl,
        Default::default(),
    );
    file.ImplMap(
        instance,
        PInvokeAttributes::CallConvPlatformapi,
        "Instance",
        "test.dll",
    );

    let static_signature = Signature {
        flags: MethodCallAttributes(0),
        return_type: Type::Void,
        types: vec![],
    };
    let missing = file.MethodDef(
        "MissingConvention",
        &static_signature,
        MethodAttributes::Public | MethodAttributes::Static | MethodAttributes::PInvokeImpl,
        Default::default(),
    );
    file.ImplMap(
        missing,
        PInvokeAttributes::default(),
        "MissingConvention",
        "test.dll",
    );

    let unsupported = file.MethodDef(
        "UnsupportedConvention",
        &static_signature,
        MethodAttributes::Public | MethodAttributes::Static | MethodAttributes::PInvokeImpl,
        Default::default(),
    );
    file.ImplMap(
        unsupported,
        PInvokeAttributes(0x300),
        "UnsupportedConvention",
        "test.dll",
    );

    let variadic = file.MethodDef(
        "Variadic",
        &Signature {
            flags: MethodCallAttributes::VARARG,
            return_type: Type::Void,
            types: vec![],
        },
        MethodAttributes::Public | MethodAttributes::Static | MethodAttributes::PInvokeImpl,
        Default::default(),
    );
    file.ImplMap(
        variadic,
        PInvokeAttributes::CallConvPlatformapi,
        "Variadic",
        "test.dll",
    );
    let fastcall = file.MethodDef(
        "Fastcall",
        &static_signature,
        MethodAttributes::Public | MethodAttributes::Static | MethodAttributes::PInvokeImpl,
        Default::default(),
    );
    file.ImplMap(
        fastcall,
        PInvokeAttributes::CallConvFastcall,
        "Fastcall",
        "test.dll",
    );

    let index = index(file);
    assert!(validator::validate(&index).is_empty());
    assert_eq!(
        index
            .expect("Test", "Apis")
            .methods()
            .find(|method| method.name() == "Fastcall")
            .unwrap()
            .calling_convention(),
        "fastcall"
    );
    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::Win32)
        .validate();
    assert_eq!(errors.len(), 4);
    assert_eq!(
        errors[0].message(),
        "Win32 P/Invoke method `Test.Apis.Instance` must be static"
    );
    assert_eq!(
        errors[1].message(),
        "Win32 P/Invoke method `Test.Apis.MissingConvention` has no calling convention"
    );
    assert_eq!(
        errors[2].message(),
        "Win32 P/Invoke method `Test.Apis.UnsupportedConvention` has unsupported calling convention 0x300"
    );
    assert_eq!(
        errors[3].message(),
        "variadic Win32 P/Invoke method `Test.Apis.Variadic` must use the C calling convention"
    );
}

#[test]
fn win32_native_struct_layout_is_validated() {
    let mut file = writer::File::new("test");
    let value_type = writer::TypeDefOrRef::TypeRef(file.TypeRef("System", "ValueType"));
    file.TypeDef(
        "Test",
        "Missing",
        value_type,
        TypeAttributes::Public,
    );
    file.Field("Value", &Type::I32, FieldAttributes::Public);

    file.TypeDef(
        "Test",
        "Explicit",
        value_type,
        TypeAttributes::Public | TypeAttributes::ExplicitLayout,
    );
    let first = file.Field("First", &Type::I32, FieldAttributes::Public);
    file.Field("Second", &Type::I32, FieldAttributes::Public);
    file.FieldLayout(first, 0);

    file.TypeDef(
        "Test",
        "Both",
        value_type,
        TypeAttributes::Public
            | TypeAttributes::SequentialLayout
            | TypeAttributes::ExplicitLayout,
    );

    let index = index(file);
    assert!(validator::validate(&index).is_empty());
    let errors = validator::Validator::new(&index)
        .profile(validator::ValidationProfile::Win32)
        .validate();
    assert_eq!(errors.len(), 3);
    assert_eq!(
        errors[0].message(),
        "Win32 struct `Test.Both` cannot use both sequential and explicit layout"
    );
    assert_eq!(
        errors[1].message(),
        "explicit-layout Win32 struct `Test.Explicit` field `Second` has no field layout"
    );
    assert_eq!(
        errors[2].message(),
        "Win32 struct `Test.Missing` requires sequential or explicit layout"
    );
}
