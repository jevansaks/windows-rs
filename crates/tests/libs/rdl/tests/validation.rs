fn out_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("OUT_DIR")).join(format!("test_rdl_validation_{name}.winmd"))
}

fn error(name: &str, source: &str) -> windows_rdl::Error {
    windows_rdl::reader()
        .input_text_named("src/test.rdl", source)
        .output(out_path(name))
        .write()
        .unwrap_err()
}

fn error_with_default(name: &str, source: &str) -> windows_rdl::Error {
    windows_rdl::reader()
        .input_text_named("src/test.rdl", source)
        .reference_default()
        .output(out_path(name))
        .write()
        .unwrap_err()
}

#[test]
fn duplicate_symbols_are_rejected() {
    let cases = [
        (
            "type",
            "#[win32] mod Test { struct Value {} struct Value {} }",
            "duplicate type `Value`",
        ),
        (
            "overlapping_architecture",
            "#[win32] mod Test {
                #[arch(X86 | X64)]
                struct Value {}
                #[arch(X64)]
                struct Value {}
            }",
            "duplicate type `Value`",
        ),
        (
            "function",
            "#[win32] mod Test {
                #[library(\"test.dll\")]
                extern fn Open();
                #[library(\"test.dll\")]
                extern fn Open();
            }",
            "duplicate function `Open`",
        ),
        (
            "constant",
            "#[win32] mod Test { const Value: i32 = 1; const Value: i32 = 2; }",
            "duplicate constant `Value`",
        ),
        (
            "top_level_kind",
            "#[win32] mod Test { struct Value {} const Value: i32 = 1; }",
            "duplicate symbol `Value`",
        ),
        (
            "field",
            "#[win32] mod Test { struct Value { item: i32, item: i32 } }",
            "duplicate field `item`",
        ),
        (
            "nested_field",
            "#[win32] mod Test { struct Value { Anonymous: struct { item: i32, item: i32 } } }",
            "duplicate field `item`",
        ),
        (
            "bit_field",
            "#[win32] mod Test { struct Value { bits: u32 { item: 1, item: 1 } } }",
            "duplicate bit-field member `item`",
        ),
        (
            "enum_variant",
            "#[win32] mod Test { #[repr(i32)] enum Value { Item = 0, Item = 1 } }",
            "duplicate enum variant `Item`",
        ),
        (
            "method",
            "#[winrt] mod Test { interface IValue { fn Get(&self, value: i32); fn Get(&self, value: i32); } }",
            "duplicate method `Get` on `Test.IValue`",
        ),
        (
            "return_type",
            "#[winrt] mod Test { interface IValue { fn Get(&self) -> i32; fn Get(&self) -> u32; } }",
            "duplicate method `Get` on `Test.IValue`",
        ),
        (
            "property",
            "#[winrt] mod Test { interface IValue { Name: String; Name: String; } }",
            "duplicate property `Name` on `Test.IValue`",
        ),
        (
            "property_type",
            "#[winrt] mod Test { interface IValue { #[get] Value: i32; #[set] Value: u32; } }",
            "duplicate property `Value` on `Test.IValue`",
        ),
        (
            "event",
            "#[winrt] mod Test { delegate fn Handler(); interface IValue { event Changed: Handler; event Changed: Handler; } }",
            "duplicate event `Changed` on `Test.IValue`",
        ),
        (
            "interface_member_kind",
            "#[winrt] mod Test { interface IValue { fn Name(&self); Name: String; } }",
            "duplicate interface member `Name`",
        ),
        (
            "attribute_property",
            "#[win32] mod Test { attribute Value { Item: i32, Item: i32, } }",
            "duplicate attribute property `Item`",
        ),
        (
            "attribute_constructor",
            "#[win32] mod Test { attribute Value { fn(item: i32); fn(other: i32); } }",
            "duplicate attribute constructor `Value`",
        ),
        (
            "class_interface",
            "#[winrt] mod Test { interface IValue {} class Value { IValue, IValue, } }",
            "duplicate interface `Test.IValue` on `Test.Value`",
        ),
        (
            "generic_parameter",
            "#[winrt] mod Test { interface IValue<T, T> {} }",
            "duplicate generic parameter `T`",
        ),
        (
            "parameter",
            "#[winrt] mod Test { interface IValue { fn Get(&self, value: i32, value: i32); } }",
            "duplicate parameter `value`",
        ),
    ];

    for (name, source, message) in cases {
        let error = error(name, source);
        assert_eq!(error.code.as_deref(), Some("RDL0001"), "{name}");
        assert_eq!(error.message, message, "{name}");
        assert_eq!(error.file_name, "src/test.rdl", "{name}");
        assert_eq!(error.labels.len(), 2, "{name}");
        assert_eq!(
            error.labels[0].style,
            windows_rdl::LabelStyle::Primary,
            "{name}"
        );
        assert_eq!(
            error.labels[1].style,
            windows_rdl::LabelStyle::Secondary,
            "{name}"
        );
        assert_eq!(error.labels[1].message, "first declared here", "{name}");
    }
}

#[test]
fn duplicate_labels_preserve_both_source_names() {
    let error = windows_rdl::reader()
        .input_texts_named([
            ("src/first.rdl", "#[win32] mod Test { struct Value {} }"),
            ("src/second.rdl", "#[win32] mod Test { struct Value {} }"),
        ])
        .output(out_path("source_names"))
        .write()
        .unwrap_err();

    assert_eq!(error.file_name, "src/second.rdl");
    assert_eq!(error.labels[0].source, "src/second.rdl");
    assert_eq!(error.labels[1].source, "src/first.rdl");
}

#[test]
fn duplicate_signatures_use_resolved_type_identity() {
    for (name, source, message) in [
        (
            "resolved_method",
            r#"
use Other::Value as Alias;

#[winrt]
mod Test {
    interface IValue {
        fn Get(&self, value: Alias);
        fn Get(&self, value: Other::Value);
    }
}

#[winrt]
mod Other {
    struct Value {}
}
"#,
            "duplicate method `Get` on `Test.IValue`",
        ),
        (
            "resolved_attribute",
            r#"
use Other::Value as Alias;

#[win32]
mod Test {
    attribute Marker {
        fn(value: Alias);
        fn(value: Other::Value);
    }
}

#[win32]
mod Other {
    struct Value {}
}
"#,
            "duplicate attribute constructor `Marker`",
        ),
    ] {
        let error = error(name, source);
        assert_eq!(error.code.as_deref(), Some("RDL0001"));
        assert_eq!(error.message, message);
    }
}

#[test]
fn shared_validator_reports_authorable_invalid_types() {
    let cases = [
        (
            "void_field",
            "#[win32] mod Test { struct Value { item: void } }",
            "field `Test.Value.item` has invalid type `Void`",
        ),
        (
            "void_parameter",
            "#[winrt] mod Test { interface IValue { fn Get(&self, value: void); } }",
            "method `Test.IValue.Get` parameter 1 has invalid type `Void`",
        ),
        (
            "void_property",
            "#[winrt] mod Test { interface IValue { Value: void; } }",
            "property `Test.IValue.Value` has invalid value type `Void`",
        ),
    ];

    for (name, source, message) in cases {
        let error = error(name, source);
        assert_eq!(error.message, message, "{name}");
        assert_eq!(error.file_name, "src/test.rdl", "{name}");
        assert_eq!(error.labels.len(), 2, "{name}");
        assert_eq!(
            error.labels[1].style,
            windows_rdl::LabelStyle::Secondary,
            "{name}"
        );
    }
}

#[test]
fn shared_validator_reports_authorable_attribute_errors() {
    let cases = [(
        "attribute_duplicate_named_argument",
        r#"
#[win32]
mod Test {
    attribute MarkerAttribute {
        fn();
        Item: i32,
    }
    #[Marker(Item = 1, Item = 2)]
    struct Value {}
}
"#,
        "attribute `Test.MarkerAttribute` has duplicate named field argument `Item`",
    )];

    for (name, source, message) in cases {
        assert_eq!(error(name, source).message, message, "{name}");
    }
}

#[test]
fn winrt_contract_policy_reports_source_errors() {
    let definitions = r#"
#[winrt]
mod Windows {
    mod Foundation {
        mod Metadata {
            attribute ApiContractAttribute {
                fn();
            }
            attribute ContractVersionAttribute {
                fn(version: u32);
                fn(contract: String, version: u32);
                fn(contract: Type, version: u32);
            }
            attribute ActivatableAttribute {
                fn(factory: Type, version: u32);
            }
        }
    }
}
"#;
    let cases = [
        (
            "api_contract_shape",
            r#"
#[winrt]
mod Test {
    #[Windows::Foundation::Metadata::ApiContract]
    interface IBad {}
}
"#,
            "API contract `Test.IBad` must be a struct",
        ),
        (
            "api_contract_version_missing",
            r#"
#[winrt]
mod Test {
    #[Windows::Foundation::Metadata::ApiContract]
    struct Contract {}
}
"#,
            "API contract `Test.Contract` must have a contract version",
        ),
        (
            "api_contract_multiple_versions",
            r#"
#[winrt]
mod Test {
    #[Windows::Foundation::Metadata::ApiContract]
    #[Windows::Foundation::Metadata::ContractVersion(1)]
    #[Windows::Foundation::Metadata::ContractVersion(2)]
    struct Contract {}
}
"#,
            "API contract `Test.Contract` has multiple contract versions",
        ),
        (
            "contract_version_zero",
            r#"
#[winrt]
mod Test {
    #[Windows::Foundation::Metadata::ApiContract]
    #[Windows::Foundation::Metadata::ContractVersion(0)]
    struct Contract {}
}
"#,
            "contract version must not be zero",
        ),
        (
            "contract_version_requires_contract",
            r#"
#[winrt]
mod Test {
    #[Windows::Foundation::Metadata::ContractVersion(1)]
    struct Value {}
}
"#,
            "contract version must name an API contract",
        ),
        (
            "contract_version_missing_contract",
            r#"
#[winrt]
mod Test {
    #[Windows::Foundation::Metadata::ContractVersion("Missing.Contract", 1)]
    struct Value {}
}
"#,
            "contract version references missing API contract `Missing.Contract`",
        ),
        (
            "contract_version_wrong_target",
            r#"
#[winrt]
mod Test {
    struct NotAContract {}
    #[Windows::Foundation::Metadata::ContractVersion(NotAContract, 1)]
    struct Value {}
}
"#,
            "contract version target `Test.NotAContract` is not an API contract",
        ),
    ];

    for (name, source, message) in cases {
        let source = format!("{source}\n{definitions}");
        assert_eq!(error(name, &source).message, message, "{name}");
    }
}

#[test]
fn winrt_attribute_target_policy_reports_source_errors() {
    let error = error_with_default(
        "attribute_target",
        r#"
#[winrt]
mod Test {
    struct Value {
        #[Windows::Foundation::Metadata::ApiContract]
        item: i32,
    }
}
"#,
    );
    assert_eq!(
        error.message,
        "attribute `Windows.Foundation.Metadata.ApiContractAttribute` cannot target a field"
    );
}

#[test]
fn repeated_non_multiple_attributes_report_source_errors() {
    let error = error_with_default(
        "duplicate_attribute",
        r#"
#[winrt]
mod Test {
    #[Windows::Foundation::Metadata::ApiContract]
    #[Windows::Foundation::Metadata::ApiContract]
    struct Contract {}
}
"#,
    );
    assert_eq!(
        error.message,
        "duplicate attribute `Windows.Foundation.Metadata.ApiContractAttribute`"
    );
}

#[test]
fn contract_versions_cannot_precede_their_owning_type() {
    let error = error(
        "contract_version_order",
        r#"
#[winrt]
mod Test {
    #[Windows::Foundation::Metadata::ApiContract]
    #[Windows::Foundation::Metadata::ContractVersion(1)]
    struct Contract {}

    #[Windows::Foundation::Metadata::ContractVersion(Contract, 2)]
    interface IValue {
        #[Windows::Foundation::Metadata::ContractVersion(Contract, 1)]
        fn Get(&self);
    }
}

#[winrt]
mod Windows {
    mod Foundation {
        mod Metadata {
            attribute ApiContractAttribute {
                fn();
            }
            attribute ContractVersionAttribute {
                fn(version: u32);
                fn(contract: Type, version: u32);
            }
        }
    }
}
"#,
    );
    assert_eq!(
        error.message,
        "contract version 1 precedes owning type `Test.IValue` version 2"
    );
}

#[test]
fn variadic_win32_functions_require_the_c_calling_convention() {
    let error = error(
        "variadic_calling_convention",
        r#"
#[win32]
mod Test {
    #[library("test.dll")]
    extern "system" fn Print(format: *const i8, ...);
}
"#,
    );
    assert_eq!(
        error.message,
        "variadic Win32 P/Invoke method `Test.Apis.Print` must use the C calling convention"
    );
}

#[test]
fn unresolved_types_are_collected_before_encoding() {
    let report = windows_rdl::reader()
        .input_text_named(
            "src/test.rdl",
            r#"
#[winrt]
mod Test {
    interface IValue {
        fn First(&self, value: MissingFirst);
        fn Second(&self, value: MissingSecond);
    }
}
"#,
        )
        .check_all();

    assert_eq!(report.diagnostics().len(), 2);
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.message == "type not found")
    );
}

#[test]
fn generic_arity_is_validated_on_resolved_types() {
    let report = windows_rdl::reader()
        .input_text_named(
            "src/test.rdl",
            r#"
#[winrt]
mod Test {
    interface IVector<T> {
        fn Append(&self, value: T);
    }
    interface IUses {
        fn Missing(&self, value: IVector);
        fn Extra(&self, value: IVector<i32, u32>);
    }
}
"#,
        )
        .check_all();

    assert_eq!(report.diagnostics().len(), 2);
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.code.as_deref() == Some("RDL0005"))
    );
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("but 0 were provided"))
    );
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("but 2 were provided"))
    );
}

#[test]
fn generic_static_interface_arguments_are_validated() {
    let mismatch = windows_rdl::reader()
        .input_text_named(
            "src/test.rdl",
            r#"
#[winrt]
mod Test {
    interface IStatics<T> {}
    #[Windows::Foundation::Metadata::Static(IStatics::<i32, u32>, 1)]
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
        )
        .check_all();
    assert!(mismatch.diagnostics().iter().any(|diagnostic| {
        diagnostic.code.as_deref() == Some("RDL0005")
            && diagnostic.message.contains("but 2 were provided")
    }));

    let open = windows_rdl::reader()
        .input_text_named(
            "src/test.rdl",
            r#"
#[winrt]
mod Test {
    interface IStatics<T> {}
    #[Windows::Foundation::Metadata::Static(IStatics::<T>, 1)]
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
        )
        .check_all();
    assert!(
        open.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message == "type not found")
    );
}

#[test]
fn unrepresentable_syntax_is_rejected() {
    let cases = [
        (
            "function_generics",
            "#[win32] mod Test { #[library(\"test.dll\")] extern fn Open<T>(); }",
            "generic parameters are not supported on functions",
        ),
        (
            "callback_generics",
            "#[win32] mod Test { extern fn Callback<T>(); }",
            "generic parameters are not supported on callbacks",
        ),
        (
            "method_generics",
            "#[winrt] mod Test { interface IValue { fn Get<T>(&self); } }",
            "generic parameters are not supported on interface methods",
        ),
        (
            "callback_variadic",
            "#[win32] mod Test { extern \"C\" fn Callback(...); }",
            "variadic parameters are not supported on callbacks",
        ),
        (
            "delegate_variadic",
            "#[winrt] mod Test { delegate fn Handler(...); }",
            "variadic parameters are not supported on delegates",
        ),
        (
            "method_variadic",
            "#[winrt] mod Test { interface IValue { fn Get(&self, ...); } }",
            "variadic parameters are not supported on interface methods",
        ),
        (
            "attribute_constructor_variadic",
            "#[win32] mod Test { attribute Value { fn(...); } }",
            "variadic attribute constructors are not supported",
        ),
        (
            "attribute_constructor_return",
            "#[win32] mod Test { attribute Value { fn() -> i32; } }",
            "attribute constructors cannot return a value",
        ),
        (
            "enum_variant_fields",
            "#[win32] mod Test { #[repr(i32)] enum Value { Item(i32) = 0 } }",
            "enum variants with fields are not supported",
        ),
        (
            "generic_attributes",
            "#[winrt] mod Test { interface IValue<#[Marker] T> {} }",
            "attributes on generic parameters are not represented",
        ),
        (
            "generic_bounds",
            "#[winrt] mod Test { interface IValue<T: Marker> {} }",
            "generic parameter bounds are not represented",
        ),
        (
            "generic_defaults",
            "#[winrt] mod Test { interface IValue<T = i32> {} }",
            "generic parameter defaults are not represented",
        ),
        (
            "const_generics",
            "#[winrt] mod Test { interface IValue<const N: usize> {} }",
            "only type generic parameters are supported on interfaces",
        ),
    ];

    for (name, source, message) in cases {
        let error = error(name, source);
        assert_eq!(error.code.as_deref(), Some("RDL0002"), "{name}");
        assert_eq!(error.message, message, "{name}");
        assert_eq!(error.file_name, "src/test.rdl", "{name}");
        assert_eq!(error.labels.len(), 1, "{name}");
        assert_eq!(
            error.labels[0].message, "not represented in metadata",
            "{name}"
        );
    }
}

#[test]
fn invalid_event_handler_is_rejected() {
    let error = error(
        "invalid_event_handler",
        "#[winrt] mod Test { interface IValue { event Changed: Object; } }",
    );
    assert_eq!(
        error.message,
        "event handler must be a delegate or class type"
    );
    assert_eq!(error.file_name, "src/test.rdl");
    assert_eq!(error.labels.len(), 1);
}

#[test]
fn invalid_explicit_associations_are_rejected() {
    let cases = [
        (
            "missing_class_accessor",
            "#[winrt] mod Test {
                interface IValue {}
                #[class_property(Value: i32, get = get_Value)]
                class Value { IValue, }
            }",
            "class association method `Test.Value.get_Value` was not projected",
        ),
        (
            "static_class_event",
            "#[winrt] mod Test {
                delegate fn Handler();
                interface IValue {}
                #[class_event(static Changed: Handler, add = add_Changed, remove = remove_Changed)]
                class Value { IValue, }
            }",
            "class events do not accept `static`",
        ),
        (
            "missing_interface_accessor",
            "#[winrt] mod Test {
                delegate fn Handler();
                #[interface_event(Changed: Handler, add = add_Changed, remove = remove_Changed)]
                interface IValue {}
            }",
            "interface association method `add_Changed` was not declared",
        ),
        (
            "duplicate_property_suppression",
            "#[winrt] mod Test {
                interface IValue { Value: i32; }
                class Value {
                    #[no_property(Value)]
                    #[no_property(Value)]
                    IValue,
                }
            }",
            "duplicate `no_property` for `Value`",
        ),
    ];

    for (name, source, message) in cases {
        assert_eq!(error(name, source).message, message, "{name}");
    }
}

#[test]
fn method_overloads_are_allowed() {
    windows_rdl::reader()
        .input_text(
            "#[winrt] mod Test {
                interface IValue {
                    fn Get(&self, value: i32);
                    fn Get(&self, value: String);
                }
            }",
        )
        .output(out_path("method_overloads"))
        .write()
        .unwrap();
}

#[test]
fn explicit_overloads_emit_projected_and_metadata_names() {
    use windows_metadata::HasAttributes;

    let path = out_path("explicit_overloads");
    windows_rdl::reader()
        .input_text(
            "#[winrt] mod Test {
                interface IValue {
                    #[overload(Get)]
                    #[default_overload]
                    fn Get(&self, value: i32);
                    #[overload(GetWithString)]
                    fn Get(&self, value: String);
                }
            }",
        )
        .output(&path)
        .write()
        .unwrap();

    let index = windows_metadata::reader::Index::read(&path).unwrap();
    let methods: Vec<_> = index.expect("Test", "IValue").methods().collect();
    assert_eq!(methods.len(), 2);
    assert_eq!(methods[0].name(), "Get");
    assert_eq!(
        methods[0]
            .find_attribute("OverloadAttribute")
            .unwrap()
            .value()[0]
            .1,
        windows_metadata::Value::Utf8("Get".to_string())
    );
    assert!(methods[0].has_attribute("DefaultOverloadAttribute"));
    assert_eq!(methods[1].name(), "GetWithString");
    assert_eq!(
        methods[1]
            .find_attribute("OverloadAttribute")
            .unwrap()
            .value()[0]
            .1,
        windows_metadata::Value::Utf8("Get".to_string())
    );

    let rdl = path.with_extension("rdl");
    windows_rdl::writer()
        .input(&path)
        .output(&rdl)
        .write()
        .unwrap();
    let source = std::fs::read_to_string(rdl).unwrap();
    assert!(source.contains("#[overload(Get)]"));
    assert!(source.contains("#[default_overload]"));
    assert!(source.contains("#[overload(GetWithString)]"));
    assert_eq!(source.matches("fn Get(").count(), 2);
}

#[test]
fn overload_validation_reports_source_errors() {
    let cases = [
        (
            "default_without_overload",
            "#[winrt] mod Test {
                interface IValue {
                    #[default_overload]
                    fn Get(&self);
                }
            }",
            "`default_overload` requires an `overload` attribute",
        ),
        (
            "duplicate_overload_signature",
            "#[winrt] mod Test {
                interface IValue {
                    #[overload(GetFirst)]
                    fn Get(&self, value: i32);
                    #[overload(GetSecond)]
                    fn Get(&self, value: i32);
                }
            }",
            "duplicate overload signature `Get` on `Test.IValue`",
        ),
        (
            "duplicate_default_overload",
            "#[winrt] mod Test {
                interface IValue {
                    #[overload(GetFirst)]
                    #[default_overload]
                    fn Get(&self, value: i32);
                    #[overload(GetSecond)]
                    #[default_overload]
                    fn Get(&self, value: String);
                }
            }",
            "duplicate default overload `Get` on `Test.IValue`",
        ),
    ];

    for (name, source, message) in cases {
        assert_eq!(error(name, source).message, message, "{name}");
    }
}

#[test]
fn overload_metadata_names_may_repeat_for_distinct_signatures() {
    windows_rdl::reader()
        .input_text(
            "#[winrt] mod Test {
                interface IValue {
                    #[overload(Get)]
                    fn Get(&self, value: i32);
                    #[overload(Get)]
                    fn Get(&self, value: String);
                }
            }",
        )
        .output(out_path("repeated_overload_metadata_name"))
        .write()
        .unwrap();
}

#[test]
fn unrelated_overload_attribute_is_preserved() {
    let path = out_path("custom_overload_attribute");
    windows_rdl::reader()
        .input_text(
            "#[winrt] mod Test {
                attribute OverloadAttribute { fn(value: String); }
                interface IValue {
                    #[Test::Overload(\"custom\")]
                    fn Get(&self);
                }
            }",
        )
        .output(&path)
        .write()
        .unwrap();

    let rdl = path.with_extension("rdl");
    windows_rdl::writer()
        .input(&path)
        .output(&rdl)
        .write()
        .unwrap();
    let source = std::fs::read_to_string(rdl).unwrap();
    assert!(source.contains("#[Overload(\"custom\")]"));
    assert!(!source.contains("#[overload("));
}

#[test]
fn disjoint_architecture_variants_are_allowed() {
    windows_rdl::reader()
        .input_text(
            "#[win32] mod Test {
                #[arch(X86)]
                struct Value { item: i32 }
                #[arch(X64)]
                struct Value { item: i64 }
            }",
        )
        .output(out_path("architecture_variants"))
        .write()
        .unwrap();
}

#[test]
fn split_property_accessors_are_allowed() {
    windows_rdl::reader()
        .input_text(
            "#[winrt] mod Test {
                interface IValue {
                    #[get]
                    Name: String;
                    #[set]
                    Name: String;
                }
            }",
        )
        .output(out_path("split_property"))
        .write()
        .unwrap();
}
