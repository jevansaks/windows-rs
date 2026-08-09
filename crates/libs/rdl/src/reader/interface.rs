use super::guid;
use super::*;

syn::custom_keyword!(interface);
syn::custom_keyword!(event);

#[derive(Debug)]
pub struct Interface {
    pub attrs: Vec<syn::Attribute>,
    pub token: interface,
    pub name: syn::Ident,
    pub generics: syn::Generics,
    pub requires: Vec<syn::Path>,
    pub members: Vec<InterfaceMember>,
    pub winrt: bool,
}

#[derive(Debug)]
pub struct Property {
    pub attrs: Vec<syn::Attribute>,
    pub name: syn::Ident,
    pub ty: syn::Type,
}

#[derive(Debug)]
pub struct Event {
    pub attrs: Vec<syn::Attribute>,
    pub name: syn::Ident,
    pub handler_ty: syn::Type,
}

#[derive(Debug)]
pub enum InterfaceMember {
    Method(Method),
    Property(Property),
    Event(Event),
}

struct InterfaceEventAssociation {
    name: syn::Ident,
    _colon: syn::Token![:],
    ty: syn::Type,
    _comma: syn::Token![,],
    accessors: syn::punctuated::Punctuated<InterfaceEventAssociationAccessor, syn::Token![,]>,
}

struct InterfaceEventAssociationAccessor {
    kind: syn::Ident,
    _eq: syn::Token![=],
    method: syn::Ident,
}

impl syn::parse::Parse for InterfaceEventAssociation {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self {
            name: input.parse()?,
            _colon: input.parse()?,
            ty: input.parse()?,
            _comma: input.parse()?,
            accessors: syn::punctuated::Punctuated::parse_terminated(input)?,
        })
    }
}

impl syn::parse::Parse for InterfaceEventAssociationAccessor {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        Ok(Self {
            kind: input.parse()?,
            _eq: input.parse()?,
            method: input.parse()?,
        })
    }
}

impl syn::parse::Parse for InterfaceMember {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        // Peek past any outer attributes to determine which member kind follows.
        let fork = input.fork();
        fork.call(syn::Attribute::parse_outer)?;

        if fork.peek(syn::Token![fn]) {
            return input.parse().map(Self::Method);
        }

        if fork.peek(event) {
            let attrs = input.call(syn::Attribute::parse_outer)?;
            let _event_token: event = input.parse()?;
            let name: syn::Ident = input.parse()?;
            input.parse::<syn::Token![:]>()?;
            let handler_ty: syn::Type = input.parse()?;
            input.parse::<syn::Token![;]>()?;
            return Ok(Self::Event(Event {
                attrs,
                name,
                handler_ty,
            }));
        }

        // Property shorthand: `[#[get] | #[set]] Name: Type;`
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let name: syn::Ident = input.parse()?;
        input.parse::<syn::Token![:]>()?;
        let ty: syn::Type = input.parse()?;
        input.parse::<syn::Token![;]>()?;
        Ok(Self::Property(Property { attrs, name, ty }))
    }
}

impl syn::parse::Parse for Interface {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let token = input.parse()?;
        let name = input.parse()?;
        let generics = input.parse()?;

        let requires = if input.parse::<syn::Token![:]>().is_ok() {
            let mut requires = vec![input.parse::<syn::Path>()?];
            while input.parse::<syn::Token![+]>().is_ok() {
                requires.push(input.parse::<syn::Path>()?);
            }
            requires
        } else {
            vec![]
        };

        let content;
        syn::braced!(content in input);
        let mut members = vec![];

        while !content.is_empty() {
            members.push(content.parse()?);
        }

        Ok(Self {
            attrs,
            token,
            name,
            generics,
            requires,
            members,
            winrt: false,
        })
    }
}

impl Encoder<'_> {
    pub fn encode_interface(&mut self, item: &Interface) -> Result<(), Error> {
        let mut has_exclusive_to = false;
        for attr in &item.attrs {
            has_exclusive_to |= self.is_exclusive_to_attribute(attr)?;
        }

        let mut flags = metadata::TypeAttributes::Abstract | metadata::TypeAttributes::Interface;

        if !has_exclusive_to {
            flags |= metadata::TypeAttributes::Public;
        }

        if item.winrt {
            flags |= metadata::TypeAttributes::WindowsRuntime;
        }

        let mut generics = Vec::with_capacity(item.generics.params.len());
        for generic in &item.generics.params {
            let syn::GenericParam::Type(generic) = generic else {
                return self.err(generic, "only type generic parameters are supported");
            };
            generics.push(generic.ident.to_string());
        }
        self.generics = generics;

        let mut name = self.name.to_string();

        if !self.generics.is_empty() {
            name = format!("{name}`{}", self.generics.len());
        }

        let interface = self.output.TypeDef(
            self.namespace,
            &name,
            metadata::writer::TypeDefOrRef::default(),
            flags,
        );
        self.origin(interface, &item.name);

        for (number, name) in self.generics.iter().enumerate() {
            self.output.GenericParam(
                name,
                metadata::writer::TypeOrMethodDef::TypeDef(interface),
                number.try_into().unwrap(),
                metadata::GenericParamAttributes::None,
            );
        }

        let mut runtime = None;
        for attr in &item.attrs {
            if attr.path().is_ident("runtime") {
                if !matches!(attr.meta, syn::Meta::Path(_)) {
                    return self.err(attr, "`runtime` attribute does not accept arguments");
                }
                if runtime.replace(attr).is_some() {
                    return self.err(attr, "duplicate `runtime` attribute");
                }
            }
        }

        self.encode_attrs(
            metadata::writer::HasAttribute::TypeDef(interface),
            &item.attrs,
            &["guid", "no_guid", "interface_event", "runtime"],
        )?;

        if let Some(arch_bits) = self.read_arch(&item.attrs)? {
            self.emit_arch_attribute(
                metadata::writer::HasAttribute::TypeDef(interface),
                arch_bits,
            );
        }

        let already_has_guid = self.encode_guid_pseudo_attrs(
            metadata::writer::HasAttribute::TypeDef(interface),
            &item.attrs,
        )?;

        if !item.winrt && item.requires.len() > 1 {
            return self.err(
                &item.requires[1],
                "non-WinRT interface can only inherit from one interface",
            );
        }

        for require in &item.requires {
            let ty = self.encode_path(require)?;
            let implementation = self.output.InterfaceImpl(interface, &ty);
            self.origin(implementation, require);
        }

        let mut maps = MemberMaps::default();
        let method_signatures = self.encode_interface_members(
            item,
            interface,
            if runtime.is_some() {
                MemberOwner::InterfaceRuntime
            } else {
                MemberOwner::Interface
            },
            &mut maps,
            MemberProjection::default(),
        )?;
        self.encode_interface_event_associations(interface, item, &mut maps)?;

        if !already_has_guid {
            let methods: Vec<(&str, &[metadata::Type], &metadata::Type)> = method_signatures
                .iter()
                .map(|(name, types, ret)| (name.as_str(), types.as_slice(), ret))
                .collect();

            guid::derive_and_emit_guid(
                self.output,
                metadata::writer::HasAttribute::TypeDef(interface),
                self.namespace,
                self.name,
                &methods,
            );
        }

        Ok(())
    }

    pub(super) fn encode_interface_members(
        &mut self,
        item: &Interface,
        owner: metadata::writer::TypeDef,
        member_owner: MemberOwner,
        maps: &mut MemberMaps,
        projection: MemberProjection<'_>,
    ) -> Result<Vec<(String, Vec<metadata::Type>, metadata::Type)>, Error> {
        let base_flags = member_owner.method_flags();
        let call_flags = member_owner.call_flags();
        let impl_flags = member_owner.impl_flags();

        let mut method_signatures: Vec<(String, Vec<metadata::Type>, metadata::Type)> = Vec::new();
        for member in &item.members {
            match member {
                InterfaceMember::Method(method) => {
                    let public_name = method.sig.ident.unraw_to_string();
                    let mut metadata_name = public_name.clone();
                    let mut overload_attr = None;
                    let mut default_overload_attr = None;

                    for attr in &method.attrs {
                        if attr.path().is_ident("overload") {
                            if overload_attr.replace(attr).is_some() {
                                return self.err(attr, "duplicate `overload` attribute");
                            }
                            if !item.winrt {
                                return self
                                    .err(attr, "`overload` is only supported on WinRT interfaces");
                            }
                            let name: syn::Ident = attr.parse_args().map_err(|_| {
                                self.error(attr, "`overload` requires one metadata method name")
                            })?;
                            metadata_name = name.unraw_to_string();
                        } else if attr.path().is_ident("default_overload") {
                            if default_overload_attr.replace(attr).is_some() {
                                return self.err(attr, "duplicate `default_overload` attribute");
                            }
                            if !matches!(attr.meta, syn::Meta::Path(_)) {
                                return self.err(
                                    attr,
                                    "`default_overload` attribute does not accept arguments",
                                );
                            }
                        }
                    }

                    if let Some(default_overload_attr) = default_overload_attr
                        && overload_attr.is_none()
                    {
                        return self.err(
                            default_overload_attr,
                            "`default_overload` requires an `overload` attribute",
                        );
                    }

                    let mut params = vec![];

                    if method.sig.inputs.is_empty() {
                        return self.err(&method.sig.ident, "`&self` parameter not found");
                    }

                    for (sequence, arg) in method.sig.inputs.iter().enumerate() {
                        match arg {
                            syn::FnArg::Receiver(receiver) => {
                                if *receiver != syn::parse_quote! { &self } {
                                    return self.err(receiver, "`&self` parameter not found");
                                }
                            }
                            syn::FnArg::Typed(pt) => {
                                if sequence == 0 {
                                    return self.err(arg, "`&self` parameter not found");
                                }
                                let p = self.param(pt)?;
                                if item.winrt {
                                    self.validate_type_is_winrt(&pt.ty, &p.ty)?;
                                }
                                params.push(p);
                            }
                        }
                    }

                    let types: Vec<metadata::Type> =
                        params.iter().map(|param| param.ty.clone()).collect();
                    let return_type = self.encode_return_type(&method.sig.output)?;

                    if item.winrt
                        && let syn::ReturnType::Type(_, return_syn_ty) = &method.sig.output
                    {
                        self.validate_type_is_winrt(return_syn_ty.as_ref(), &return_type)?;
                    }

                    if matches!(
                        member_owner,
                        MemberOwner::Interface | MemberOwner::InterfaceRuntime
                    ) {
                        method_signatures.push((
                            metadata_name.clone(),
                            types.clone(),
                            return_type.clone(),
                        ));
                    }

                    let signature = metadata::Signature {
                        flags: call_flags,
                        return_type,
                        types,
                    };

                    let mut is_special = false;
                    for attr in &method.attrs {
                        if attr.path().is_ident("special") {
                            if !matches!(attr.meta, syn::Meta::Path(_)) {
                                return self
                                    .err(attr, "`special` attribute does not accept arguments");
                            }
                            is_special = true;
                        }
                    }

                    let mut flags = base_flags;
                    if is_special {
                        flags |= metadata::MethodAttributes::SpecialName;
                    }

                    let method_def = self.emit_member_method(
                        MemberMethod {
                            name: &metadata_name,
                            signature: &signature,
                            flags,
                            impl_flags,
                        },
                        maps,
                        &projection,
                        &method.sig.ident,
                    );

                    self.encode_attrs(
                        metadata::writer::HasAttribute::MethodDef(method_def),
                        &method.attrs,
                        &["special", "overload", "default_overload"],
                    )?;
                    if overload_attr.is_some() {
                        self.emit_overload_attribute(
                            metadata::writer::HasAttribute::MethodDef(method_def),
                            &public_name,
                        );
                    }
                    if default_overload_attr.is_some() {
                        self.emit_default_overload_attribute(
                            metadata::writer::HasAttribute::MethodDef(method_def),
                        );
                    }

                    self.encode_return_attrs(&method.return_attrs)?;
                    self.encode_params(&params)?;
                }

                InterfaceMember::Property(prop) => {
                    let is_get_only = prop.attrs.iter().any(|attr| {
                        attr.path().is_ident("get") && matches!(attr.meta, syn::Meta::Path(_))
                    });
                    let is_set_only = prop.attrs.iter().any(|attr| {
                        attr.path().is_ident("set") && matches!(attr.meta, syn::Meta::Path(_))
                    });

                    if is_get_only && is_set_only {
                        return self.err(
                            &prop.name,
                            "property cannot have both `#[get]` and `#[set]` attributes",
                        );
                    }

                    for attr in &prop.attrs {
                        if !attr.path().is_ident("get")
                            && !attr.path().is_ident("set")
                            && !attr.path().is_ident("property")
                            && !attr.path().is_ident("set_name")
                        {
                            return self.err(
                                attr,
                                "only accessor and property wrappers are supported on properties",
                            );
                        }
                        if attr.path().is_ident("property")
                            && !matches!(attr.meta, syn::Meta::List(_))
                        {
                            return self.err(attr, "property attribute requires an argument");
                        }
                        if matches!(attr.meta, syn::Meta::NameValue(_)) {
                            return self
                                .err(attr, "`get`/`set` attribute must be a marker or wrapper");
                        }
                    }

                    let ty = self.encode_type(&prop.ty)?;
                    let method_flags = base_flags | metadata::MethodAttributes::SpecialName;
                    let set_name = self.read_accessor_name(&prop.attrs, "set_name", "value")?;

                    let mut get_method = None;
                    let mut put_method = None;

                    if !is_set_only {
                        let get_name = format!("get_{}", prop.name);
                        let signature = metadata::Signature {
                            flags: call_flags,
                            return_type: ty.clone(),
                            types: vec![],
                        };
                        if matches!(
                            member_owner,
                            MemberOwner::Interface | MemberOwner::InterfaceRuntime
                        ) {
                            method_signatures.push((get_name.clone(), vec![], ty.clone()));
                        }
                        let method = self.emit_member_method(
                            MemberMethod {
                                name: &get_name,
                                signature: &signature,
                                flags: method_flags,
                                impl_flags,
                            },
                            maps,
                            &projection,
                            &prop.name,
                        );
                        get_method = Some(method);
                        self.encode_wrapped_attrs(
                            metadata::writer::HasAttribute::MethodDef(method),
                            &prop.attrs,
                            "get",
                        )?;
                        self.encode_simple_params(&[])?;
                    }

                    if !is_get_only {
                        let put_name = format!("put_{}", prop.name);
                        let signature = metadata::Signature {
                            flags: call_flags,
                            return_type: metadata::Type::Void,
                            types: vec![ty.clone()],
                        };
                        if matches!(
                            member_owner,
                            MemberOwner::Interface | MemberOwner::InterfaceRuntime
                        ) {
                            method_signatures.push((
                                put_name.clone(),
                                vec![ty.clone()],
                                metadata::Type::Void,
                            ));
                        }
                        let method = self.emit_member_method(
                            MemberMethod {
                                name: &put_name,
                                signature: &signature,
                                flags: method_flags,
                                impl_flags,
                            },
                            maps,
                            &projection,
                            &prop.name,
                        );
                        put_method = Some(method);
                        self.encode_wrapped_attrs(
                            metadata::writer::HasAttribute::MethodDef(method),
                            &prop.attrs,
                            "set",
                        )?;
                        self.encode_simple_params(&[(&set_name, &ty)])?;
                    }

                    if projection
                        .excluded_properties
                        .iter()
                        .any(|name| name == &prop.name.to_string())
                    {
                        continue;
                    }

                    let name = prop.name.to_string();
                    let getter = get_method.is_some();
                    let setter = put_method.is_some();
                    let existing = maps.properties.iter().position(|property| {
                        property.name == name
                            && property.ty == ty
                            && property.call_flags == call_flags
                            && (!getter || !property.getter)
                            && (!setter || !property.setter)
                    });
                    let property = if let Some(existing) = existing {
                        let property = &mut maps.properties[existing];
                        property.getter |= getter;
                        property.setter |= setter;
                        property.row
                    } else {
                        let property = self.output.PropertyWithSignature(
                            &name,
                            &metadata::Signature {
                                flags: call_flags,
                                return_type: ty.clone(),
                                types: vec![],
                            },
                            0,
                        );
                        self.origin(property, &prop.name);
                        self.encode_wrapped_attrs(
                            metadata::writer::HasAttribute::Property(property),
                            &prop.attrs,
                            "property",
                        )?;
                        maps.properties.push(ProjectedProperty {
                            name,
                            ty: ty.clone(),
                            call_flags,
                            row: property,
                            getter,
                            setter,
                        });

                        if !maps.property {
                            let map = self.output.PropertyMap(owner, property);
                            self.origin(map, &prop.name);
                            maps.property = true;
                        }
                        property
                    };

                    if let Some(method) = get_method {
                        self.output.MethodSemantics(
                            0x0002, // Getter
                            method,
                            metadata::writer::HasSemantics::Property(property),
                        );
                    }

                    if let Some(method) = put_method {
                        self.output.MethodSemantics(
                            0x0001, // Setter
                            method,
                            metadata::writer::HasSemantics::Property(property),
                        );
                    }
                }

                InterfaceMember::Event(evt) => {
                    for attr in &evt.attrs {
                        if !attr.path().is_ident("add")
                            && !attr.path().is_ident("remove")
                            && !attr.path().is_ident("event")
                            && !attr.path().is_ident("add_name")
                            && !attr.path().is_ident("remove_name")
                        {
                            return self.err(
                                attr,
                                "only accessor and event wrappers are supported on events",
                            );
                        }
                        if !matches!(attr.meta, syn::Meta::List(_)) {
                            return self.err(attr, "event accessor attribute requires an argument");
                        }
                    }

                    let handler_ty = self.encode_type(&evt.handler_ty)?;
                    if !matches!(handler_ty, metadata::Type::ClassName(_)) {
                        return self.err(
                            &evt.handler_ty,
                            "event handler must be a delegate or class type",
                        );
                    }
                    let token_ty =
                        metadata::Type::value_named("Windows.Foundation", "EventRegistrationToken");
                    let method_flags = base_flags | metadata::MethodAttributes::SpecialName;
                    let add_param_name =
                        self.read_accessor_name(&evt.attrs, "add_name", "handler")?;
                    let remove_param_name =
                        self.read_accessor_name(&evt.attrs, "remove_name", "token")?;

                    let add_name = format!("add_{}", evt.name);
                    let add_signature = metadata::Signature {
                        flags: call_flags,
                        return_type: token_ty.clone(),
                        types: vec![handler_ty.clone()],
                    };
                    if matches!(
                        member_owner,
                        MemberOwner::Interface | MemberOwner::InterfaceRuntime
                    ) {
                        method_signatures.push((
                            add_name.clone(),
                            vec![handler_ty.clone()],
                            token_ty.clone(),
                        ));
                    }
                    let add_method = self.emit_member_method(
                        MemberMethod {
                            name: &add_name,
                            signature: &add_signature,
                            flags: method_flags,
                            impl_flags,
                        },
                        maps,
                        &projection,
                        &evt.name,
                    );
                    self.encode_wrapped_attrs(
                        metadata::writer::HasAttribute::MethodDef(add_method),
                        &evt.attrs,
                        "add",
                    )?;
                    self.encode_simple_params(&[(&add_param_name, &handler_ty)])?;

                    let remove_name = format!("remove_{}", evt.name);
                    let remove_signature = metadata::Signature {
                        flags: call_flags,
                        return_type: metadata::Type::Void,
                        types: vec![token_ty.clone()],
                    };
                    if matches!(
                        member_owner,
                        MemberOwner::Interface | MemberOwner::InterfaceRuntime
                    ) {
                        method_signatures.push((
                            remove_name.clone(),
                            vec![token_ty.clone()],
                            metadata::Type::Void,
                        ));
                    }
                    let remove_method = self.emit_member_method(
                        MemberMethod {
                            name: &remove_name,
                            signature: &remove_signature,
                            flags: method_flags,
                            impl_flags,
                        },
                        maps,
                        &projection,
                        &evt.name,
                    );
                    self.encode_wrapped_attrs(
                        metadata::writer::HasAttribute::MethodDef(remove_method),
                        &evt.attrs,
                        "remove",
                    )?;
                    self.encode_simple_params(&[(&remove_param_name, &token_ty)])?;

                    if projection
                        .excluded_events
                        .iter()
                        .any(|name| name == &evt.name.to_string())
                    {
                        continue;
                    }

                    let event = self.output.Event(&evt.name.to_string(), &handler_ty);
                    self.origin(event, &evt.name);
                    self.encode_wrapped_attrs(
                        metadata::writer::HasAttribute::Event(event),
                        &evt.attrs,
                        "event",
                    )?;

                    if !maps.event {
                        let map = self.output.EventMap(owner, event);
                        self.origin(map, &evt.name);
                        maps.event = true;
                    }

                    self.output.MethodSemantics(
                        0x0008, // AddOn
                        add_method,
                        metadata::writer::HasSemantics::Event(event),
                    );
                    self.output.MethodSemantics(
                        0x0010, // RemoveOn
                        remove_method,
                        metadata::writer::HasSemantics::Event(event),
                    );
                }
            }
        }

        Ok(method_signatures)
    }

    fn encode_interface_event_associations(
        &mut self,
        interface: metadata::writer::TypeDef,
        item: &Interface,
        maps: &mut MemberMaps,
    ) -> Result<(), Error> {
        for attr in item
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("interface_event"))
        {
            let association: InterfaceEventAssociation = attr.parse_args().map_err(|_| {
                self.error(
                    attr,
                    "`interface_event` requires `Name: Type, add = method, remove = method`",
                )
            })?;
            let ty = self.encode_type(&association.ty)?;
            let event = self.output.Event(&association.name.unraw_to_string(), &ty);
            self.origin(event, attr);
            if !maps.event {
                let map = self.output.EventMap(interface, event);
                self.origin(map, attr);
                maps.event = true;
            }
            for accessor in &association.accessors {
                let semantics = match accessor.kind.to_string().as_str() {
                    "add" => 0x0008,
                    "remove" => 0x0010,
                    _ => return self.err(&accessor.kind, "expected `add` or `remove`"),
                };
                let name = accessor.method.unraw_to_string();
                let Some(method) = maps.method(&name) else {
                    return self.err(
                        &accessor.method,
                        &format!("interface association method `{name}` was not declared"),
                    );
                };
                self.output.MethodSemantics(
                    semantics,
                    method,
                    metadata::writer::HasSemantics::Event(event),
                );
            }
        }
        Ok(())
    }

    fn read_accessor_name(
        &self,
        attrs: &[syn::Attribute],
        name: &str,
        default: &str,
    ) -> Result<String, Error> {
        let mut result = None;
        for attr in attrs.iter().filter(|attr| attr.path().is_ident(name)) {
            let ident: syn::Ident = attr
                .parse_args()
                .map_err(|_| self.error(attr, &format!("`{name}` requires one identifier")))?;
            if result.replace(ident.unraw_to_string()).is_some() {
                return self.err(attr, &format!("duplicate `{name}` attribute"));
            }
        }
        Ok(result.unwrap_or_else(|| default.to_string()))
    }
}
