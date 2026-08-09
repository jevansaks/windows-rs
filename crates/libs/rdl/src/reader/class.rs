use super::*;

syn::custom_keyword!(class);

#[derive(Debug)]
pub struct Class {
    pub attrs: Vec<syn::Attribute>,
    pub name: syn::Ident,
    pub extends: Option<syn::Path>,
    pub interfaces: Vec<ClassInterface>,
}

impl syn::parse::Parse for Class {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;
        input.parse::<class>()?;
        let name = input.parse()?;

        let extends = if input.parse::<syn::Token![:]>().is_ok() {
            Some(input.parse()?)
        } else {
            None
        };

        let content;
        syn::braced!(content in input);

        let interfaces = content
            .parse_terminated(ClassInterface::parse, syn::Token![,])?
            .into_iter()
            .collect();

        Ok(Self {
            attrs,
            name,
            extends,
            interfaces,
        })
    }
}

#[derive(Debug)]
pub struct ClassInterface {
    pub attrs: Vec<syn::Attribute>,
    pub ty: syn::Path,
}

struct StaticAssociationExclusion {
    interface: syn::Path,
    _comma: syn::Token![,],
    member: syn::Ident,
}

struct ClassAssociation {
    attrs: Vec<syn::Attribute>,
    is_static: bool,
    name: syn::Ident,
    _colon: syn::Token![:],
    ty: syn::Type,
    _comma: syn::Token![,],
    accessors: syn::punctuated::Punctuated<ClassAssociationAccessor, syn::Token![,]>,
}

struct ClassAssociationAccessor {
    kind: syn::Ident,
    _eq: syn::Token![=],
    method: syn::Path,
}

impl syn::parse::Parse for ClassAssociation {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self {
            attrs: input.call(syn::Attribute::parse_outer)?,
            is_static: input.parse::<syn::Token![static]>().is_ok(),
            name: input.parse()?,
            _colon: input.parse()?,
            ty: input.parse()?,
            _comma: input.parse()?,
            accessors: syn::punctuated::Punctuated::parse_terminated(input)?,
        })
    }
}

impl syn::parse::Parse for ClassAssociationAccessor {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self {
            kind: input.parse()?,
            _eq: input.parse()?,
            method: input.parse()?,
        })
    }
}

impl syn::parse::Parse for StaticAssociationExclusion {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self {
            interface: input.parse()?,
            _comma: input.parse()?,
            member: input.parse()?,
        })
    }
}

impl syn::parse::Parse for ClassInterface {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let ty = input.parse()?;

        Ok(Self { attrs, ty })
    }
}

impl Encoder<'_> {
    pub fn encode_class(&mut self, item: &Class) -> Result<(), Error> {
        let extends = if let Some(path) = &item.extends {
            let extends = self.encode_path(path)?;
            if let metadata::Type::ClassName(ref tn) = extends {
                // Classes are always WinRT - the base class must also be WinRT.
                self.validate_type_is_winrt(path, &extends)?;
                self.output.TypeRef(&tn.namespace, &tn.name)
            } else {
                return self.err(&item.extends, "invalid base type");
            }
        } else {
            self.output.TypeRef("System", "Object")
        };

        let mut unsealed = None;
        let mut static_only = None;
        for attr in &item.attrs {
            if attr.path().is_ident("unsealed") {
                if !matches!(attr.meta, syn::Meta::Path(_)) {
                    return self.err(attr, "`unsealed` attribute does not accept arguments");
                }
                if unsealed.replace(attr).is_some() {
                    return self.err(attr, "duplicate `unsealed` attribute");
                }
            } else if attr.path().is_ident("static_only") {
                if !matches!(attr.meta, syn::Meta::Path(_)) {
                    return self.err(attr, "`static_only` attribute does not accept arguments");
                }
                if static_only.replace(attr).is_some() {
                    return self.err(attr, "duplicate `static_only` attribute");
                }
            }
        }
        if let (Some(_), Some(static_only)) = (unsealed, static_only) {
            return self.err(
                static_only,
                "class cannot be both `unsealed` and `static_only`",
            );
        }

        let mut flags = metadata::TypeAttributes::Public | metadata::TypeAttributes::WindowsRuntime;
        if static_only.is_some() {
            flags |= metadata::TypeAttributes::Abstract | metadata::TypeAttributes::Sealed;
        } else if unsealed.is_none() {
            flags |= metadata::TypeAttributes::Sealed;
        }

        let class = self.output.TypeDef(
            self.namespace,
            self.name,
            metadata::writer::TypeDefOrRef::TypeRef(extends),
            flags,
        );
        self.origin(class, &item.name);

        self.encode_attrs(
            metadata::writer::HasAttribute::TypeDef(class),
            &item.attrs,
            &[
                "unsealed",
                "static_only",
                "no_static_property",
                "no_static_event",
                "class_property",
                "class_event",
            ],
        )?;

        self.encode_constructors(item)?;

        let mut explicit_default = None;
        for (index, interface) in item.interfaces.iter().enumerate() {
            for attr in &interface.attrs {
                if !attr.path().is_ident("default") {
                    continue;
                }
                if !matches!(attr.meta, syn::Meta::Path(_)) {
                    return self.err(attr, "`default` attribute does not accept arguments");
                }
                if explicit_default.replace(index).is_some() {
                    return self.err(attr, "class has more than one default interface");
                }
            }
        }

        let mut maps = MemberMaps::default();
        for (index, interface) in item.interfaces.iter().enumerate() {
            let default = explicit_default.map_or(index == 0, |default| default == index);
            self.encode_implement(class, interface, default)?;
            self.encode_instance_projection(class, interface, &mut maps)?;
        }

        self.encode_static_projections(class, &item.attrs, &mut maps)?;
        self.encode_class_associations(class, item, &mut maps)?;

        Ok(())
    }

    fn encode_constructors(&mut self, item: &Class) -> Result<(), Error> {
        let class_name = metadata::TypeName::named(self.namespace, self.name);
        let mut signatures = Vec::<Vec<metadata::Type>>::new();

        for attr in &item.attrs {
            let composable =
                self.is_attribute_type(attr, "Windows.Foundation.Metadata", "ComposableAttribute")?;
            let activatable = self.is_attribute_type(
                attr,
                "Windows.Foundation.Metadata",
                "ActivatableAttribute",
            )?;
            if !composable && !activatable {
                continue;
            }

            let attr_ref = self.resolve_emitted_attribute_ref(attr)?;
            let factory = attr_ref.args.iter().find_map(|(_, value)| match value {
                metadata::Value::TypeName(name) => Some(name),
                _ => None,
            });
            let visibility = if composable
                && attr_ref.args.iter().any(|(_, value)| {
                    matches!(
                        value,
                        metadata::Value::EnumValue(name, value)
                            if name
                                == &metadata::TypeName::named(
                                    "Windows.Foundation.Metadata",
                                    "CompositionType",
                                )
                                && value.integer_bits() == Some(1)
                    )
                }) {
                metadata::MethodAttributes::Family
            } else {
                metadata::MethodAttributes::Public
            };

            if let Some(factory) = factory {
                self.encode_factory_constructors(
                    &class_name,
                    factory,
                    composable,
                    visibility,
                    attr,
                    &mut signatures,
                )?;
            } else if activatable {
                self.encode_constructor(
                    &[],
                    metadata::MethodAttributes::Public,
                    attr,
                    &mut signatures,
                )?;
            } else {
                return self.err(attr, "`ComposableAttribute` requires a factory interface");
            }
        }

        Ok(())
    }

    fn encode_factory_constructors(
        &mut self,
        class_name: &metadata::TypeName,
        factory_name: &metadata::TypeName,
        composable: bool,
        visibility: metadata::MethodAttributes,
        attr: &syn::Attribute,
        signatures: &mut Vec<Vec<metadata::Type>>,
    ) -> Result<(), Error> {
        let Some((file, factory)) = Self::local_interface(self.index, factory_name) else {
            return self.err(
                attr,
                "activation factory interface must be declared in the RDL input",
            );
        };
        if !factory.generics.params.is_empty() {
            return self.err(attr, "generic activation factory is not supported");
        }

        let mut encoder = Encoder {
            output: self.output,
            origins: self.origins,
            index: self.index,
            file,
            namespace: &factory_name.namespace,
            name: &factory_name.name,
            generics: vec![],
            generic_args: vec![],
            projections: &mut *self.projections,
        };

        for member in &factory.members {
            let InterfaceMember::Method(method) = member else {
                return encoder.err(
                    &factory.name,
                    "activation factory interfaces can only contain methods",
                );
            };

            let mut params = Vec::new();
            for (sequence, input) in method.sig.inputs.iter().enumerate() {
                match input {
                    syn::FnArg::Receiver(receiver)
                        if sequence == 0 && *receiver == syn::parse_quote! { &self } => {}
                    syn::FnArg::Typed(param) if sequence != 0 => {
                        params.push(encoder.param(param)?);
                    }
                    _ => {
                        return encoder
                            .err(input, "activation factory method requires `&self` first");
                    }
                }
            }

            let return_type = encoder.encode_return_type(&method.sig.output)?;
            if return_type != metadata::Type::ClassName(class_name.clone()) {
                return encoder.err(
                    &method.sig.output,
                    "activation factory method must return the runtime class",
                );
            }

            if composable {
                if params.len() < 2
                    || params[params.len() - 2].ty != metadata::Type::Object
                    || params.last().unwrap().ty
                        != metadata::Type::RefMut(Box::new(metadata::Type::Object))
                {
                    return encoder.err(
                        &method.sig,
                        "composable factory method must end with `Object, &mut Object`",
                    );
                }
                params.truncate(params.len() - 2);
            }

            encoder.encode_constructor(&params, visibility, &method.sig.ident, signatures)?;
        }

        Ok(())
    }

    fn encode_constructor<S: Spanned>(
        &mut self,
        params: &[Param],
        visibility: metadata::MethodAttributes,
        source: &S,
        signatures: &mut Vec<Vec<metadata::Type>>,
    ) -> Result<(), Error> {
        let types: Vec<_> = params.iter().map(|param| param.ty.clone()).collect();
        if signatures.contains(&types) {
            return Ok(());
        }
        signatures.push(types.clone());

        let constructor = self.output.MethodDef(
            ".ctor",
            &metadata::Signature {
                flags: metadata::MethodCallAttributes::HASTHIS,
                return_type: metadata::Type::Void,
                types,
            },
            visibility
                | metadata::MethodAttributes::HideBySig
                | metadata::MethodAttributes::SpecialName
                | metadata::MethodAttributes::RTSpecialName,
            metadata::MethodImplAttributes::Runtime,
        );
        self.origin(constructor, source);
        self.encode_params(params)
    }

    fn encode_instance_projection(
        &mut self,
        class: metadata::writer::TypeDef,
        implementation: &ClassInterface,
        maps: &mut MemberMaps,
    ) -> Result<(), Error> {
        let metadata::Type::ClassName(interface_name) = self.encode_path(&implementation.ty)?
        else {
            return self.err(&implementation.ty, "invalid interface type");
        };
        let class_name = metadata::TypeName::named(self.namespace, self.name);
        let Some((file, interface)) = Self::local_interface(self.index, &interface_name) else {
            return Ok(());
        };
        let excluded_properties =
            self.read_excluded_associations(&implementation.attrs, "no_property")?;
        let excluded_events = self.read_excluded_associations(&implementation.attrs, "no_event")?;

        let mut member_owner = MemberOwner::ClassInstance;
        for attr in &implementation.attrs {
            if self.is_attribute_type(
                attr,
                "Windows.Foundation.Metadata",
                "OverridableAttribute",
            )? {
                member_owner = MemberOwner::ClassOverridable;
                break;
            }
            if self.is_attribute_type(attr, "Windows.Foundation.Metadata", "ProtectedAttribute")? {
                member_owner = MemberOwner::ClassProtected;
            }
        }

        self.encode_class_projection(
            class,
            maps,
            ClassProjection::instance(
                file,
                interface,
                interface_name,
                member_owner,
                excluded_properties,
                excluded_events,
                class_name,
            ),
        )
    }

    fn encode_static_projections(
        &mut self,
        class: metadata::writer::TypeDef,
        attrs: &[syn::Attribute],
        maps: &mut MemberMaps,
    ) -> Result<(), Error> {
        let mut projected = Vec::<metadata::TypeName>::new();
        let class_name = metadata::TypeName::named(self.namespace, self.name);

        for attr in attrs {
            if !self.is_attribute_type(attr, "Windows.Foundation.Metadata", "StaticAttribute")? {
                continue;
            }
            let attr_ref = self.resolve_emitted_attribute_ref(attr)?;

            let Some(metadata::Value::TypeName(interface_name)) =
                attr_ref.args.first().map(|(_, value)| value)
            else {
                return self.err(attr, "`StaticAttribute` requires an interface type");
            };

            if projected.contains(interface_name) {
                continue;
            }
            projected.push(interface_name.clone());

            let Some((file, interface)) = Self::local_interface(self.index, interface_name) else {
                return self.err(
                    attr,
                    "`StaticAttribute` interface must be declared in the RDL input",
                );
            };
            let excluded_properties =
                self.read_static_exclusions(attrs, "no_static_property", interface_name)?;
            let excluded_events =
                self.read_static_exclusions(attrs, "no_static_event", interface_name)?;

            self.encode_class_projection(
                class,
                maps,
                ClassProjection::statics(
                    file,
                    interface,
                    interface_name.clone(),
                    excluded_properties,
                    excluded_events,
                    class_name.clone(),
                ),
            )?;
        }

        Ok(())
    }

    fn encode_implement(
        &mut self,
        class: metadata::writer::TypeDef,
        interface: &ClassInterface,
        default: bool,
    ) -> Result<(), Error> {
        let ty = self.encode_path(&interface.ty)?;

        // Classes are always WinRT - every implemented interface must also be WinRT.
        self.validate_type_is_winrt(&interface.ty, &ty)?;

        let interface_impl = self.output.InterfaceImpl(class, &ty);
        self.origin(interface_impl, &interface.ty);

        if default {
            let default_attribute = metadata::writer::MemberRefParent::TypeRef(
                self.output
                    .TypeRef("Windows.Foundation.Metadata", "DefaultAttribute"),
            );

            let default_ctor = self.output.MemberRef(
                ".ctor",
                &metadata::Signature {
                    flags: metadata::MethodCallAttributes::HASTHIS,
                    ..Default::default()
                },
                default_attribute,
            );

            self.output.Attribute(
                metadata::writer::HasAttribute::InterfaceImpl(interface_impl),
                metadata::writer::AttributeType::MemberRef(default_ctor),
                &[],
            );
        }

        self.encode_attrs(
            metadata::writer::HasAttribute::InterfaceImpl(interface_impl),
            &interface.attrs,
            &["default", "no_property", "no_event"],
        )?;

        Ok(())
    }

    fn read_excluded_associations(
        &self,
        attrs: &[syn::Attribute],
        name: &str,
    ) -> Result<Vec<String>, Error> {
        let mut result = Vec::new();
        for attr in attrs.iter().filter(|attr| attr.path().is_ident(name)) {
            let ident: syn::Ident = attr
                .parse_args()
                .map_err(|_| self.error(attr, &format!("`{name}` requires one identifier")))?;
            let ident = ident.unraw_to_string();
            if result.contains(&ident) {
                return self.err(attr, &format!("duplicate `{name}` for `{ident}`"));
            }
            result.push(ident);
        }
        Ok(result)
    }

    fn read_static_exclusions(
        &self,
        attrs: &[syn::Attribute],
        name: &str,
        interface_name: &metadata::TypeName,
    ) -> Result<Vec<String>, Error> {
        let mut result = Vec::new();
        for attr in attrs.iter().filter(|attr| attr.path().is_ident(name)) {
            let exclusion: StaticAssociationExclusion = attr.parse_args().map_err(|_| {
                self.error(
                    attr,
                    &format!("`{name}` requires an interface path and member name"),
                )
            })?;
            let metadata::Type::ClassName(excluded_interface) =
                self.encode_path(&exclusion.interface)?
            else {
                return self.err(&exclusion.interface, "invalid static interface type");
            };
            if &excluded_interface != interface_name {
                continue;
            }
            let member = exclusion.member.unraw_to_string();
            if result.contains(&member) {
                return self.err(attr, &format!("duplicate `{name}` for `{member}`"));
            }
            result.push(member);
        }
        Ok(result)
    }

    fn encode_class_associations(
        &mut self,
        class: metadata::writer::TypeDef,
        item: &Class,
        maps: &mut MemberMaps,
    ) -> Result<(), Error> {
        let class_name = metadata::TypeName::named(self.namespace, self.name);

        for attr in &item.attrs {
            let is_property = attr.path().is_ident("class_property");
            let is_event = attr.path().is_ident("class_event");
            if !is_property && !is_event {
                continue;
            }

            let association: ClassAssociation = attr.parse_args().map_err(|_| {
                self.error(
                    attr,
                    "class association requires `Name: Type, accessor = Type::method`",
                )
            })?;
            let name = association.name.unraw_to_string();
            let ty = self.encode_type(&association.ty)?;

            if is_property {
                if maps.contains_property(&name) {
                    return self.err(attr, &format!("duplicate class property `{name}`"));
                }
                let flags = if association.is_static {
                    metadata::MethodCallAttributes::default()
                } else {
                    metadata::MethodCallAttributes::HASTHIS
                };
                let property = self.output.PropertyWithSignature(
                    &name,
                    &metadata::Signature {
                        flags,
                        return_type: ty,
                        types: vec![],
                    },
                    0,
                );
                self.origin(property, attr);
                self.encode_attrs(
                    metadata::writer::HasAttribute::Property(property),
                    &association.attrs,
                    &[],
                )?;
                if !maps.property {
                    let map = self.output.PropertyMap(class, property);
                    self.origin(map, attr);
                    maps.property = true;
                }
                for accessor in &association.accessors {
                    let semantics = match accessor.kind.to_string().as_str() {
                        "get" => 0x0002,
                        "set" => 0x0001,
                        _ => return self.err(&accessor.kind, "expected `get` or `set`"),
                    };
                    let method = self.class_association_method(&class_name, &accessor.method)?;
                    self.output.MethodSemantics(
                        semantics,
                        method,
                        metadata::writer::HasSemantics::Property(property),
                    );
                }
            } else {
                if association.is_static {
                    return self.err(attr, "class events do not accept `static`");
                }
                let event = self.output.Event(&name, &ty);
                self.origin(event, attr);
                self.encode_attrs(
                    metadata::writer::HasAttribute::Event(event),
                    &association.attrs,
                    &[],
                )?;
                if !maps.event {
                    let map = self.output.EventMap(class, event);
                    self.origin(map, attr);
                    maps.event = true;
                }
                for accessor in &association.accessors {
                    let semantics = match accessor.kind.to_string().as_str() {
                        "add" => 0x0008,
                        "remove" => 0x0010,
                        _ => return self.err(&accessor.kind, "expected `add` or `remove`"),
                    };
                    let method = self.class_association_method(&class_name, &accessor.method)?;
                    self.output.MethodSemantics(
                        semantics,
                        method,
                        metadata::writer::HasSemantics::Event(event),
                    );
                }
            }
        }

        Ok(())
    }

    fn class_association_method(
        &self,
        class_name: &metadata::TypeName,
        path: &syn::Path,
    ) -> Result<metadata::writer::MethodDef, Error> {
        let mut owner_path = path.clone();
        let method = owner_path.segments.pop().unwrap().into_value();
        let owner = if owner_path.segments.is_empty() {
            class_name.clone()
        } else {
            let metadata::Type::ClassName(owner) = self.encode_path(&owner_path)? else {
                return self.err(path, "class association method owner must be a class");
            };
            owner
        };
        self.projections
            .get(&owner, &method.ident.unraw_to_string())
            .ok_or_else(|| {
                self.error(
                    path,
                    &format!(
                        "class association method `{}.{}` was not projected",
                        if owner.namespace.is_empty() {
                            owner.name.clone()
                        } else {
                            format!("{}.{}", owner.namespace, owner.name)
                        },
                        method.ident.unraw_to_string()
                    ),
                )
            })
    }
}
