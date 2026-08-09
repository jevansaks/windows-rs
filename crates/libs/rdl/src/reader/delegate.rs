use super::guid;
use super::*;

syn::custom_keyword!(delegate);

#[derive(Debug)]
pub struct Delegate {
    pub attrs: Vec<syn::Attribute>,
    pub sig: syn::Signature,
    pub return_attrs: Vec<syn::Attribute>,
}

impl syn::parse::Parse for Delegate {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;
        input.parse::<delegate>()?;

        let fn_token: syn::Token![fn] = input.parse()?;
        let ident: syn::Ident = input.parse()?;
        let generics: syn::Generics = input.parse()?;

        let content;
        let paren_token = syn::parenthesized!(content in input);
        let (inputs, variadic) = parse_fn_inputs(&content)?;

        let (output, return_attrs) = parse_return_type_with_attrs(input)?;

        input.parse::<syn::Token![;]>()?;

        let sig = make_sig(
            fn_token,
            ident,
            generics,
            paren_token,
            inputs,
            variadic,
            output,
        );

        Ok(Self {
            attrs,
            sig,
            return_attrs,
        })
    }
}

impl Encoder<'_> {
    pub fn encode_delegate(&mut self, item: &Delegate) -> Result<(), Error> {
        let extends = self.output.TypeRef("System", "MulticastDelegate");

        let flags = metadata::TypeAttributes::Public
            | metadata::TypeAttributes::Sealed
            | metadata::TypeAttributes::WindowsRuntime;

        self.generics = item
            .sig
            .generics
            .params
            .iter()
            .map(|generic| {
                if let syn::GenericParam::Type(ty) = generic {
                    Ok(ty.ident.to_string())
                } else {
                    Err(self.error(generic, "only type generic parameters are supported"))
                }
            })
            .collect::<Result<Vec<_>, Error>>()?;

        let mut name = self.name.to_string();

        if !self.generics.is_empty() {
            name = format!("{name}`{}", self.generics.len());
        }

        let delegate = self.output.TypeDef(
            self.namespace,
            &name,
            metadata::writer::TypeDefOrRef::TypeRef(extends),
            flags,
        );
        self.origin(delegate, &item.sig.ident);

        self.encode_attrs(
            metadata::writer::HasAttribute::TypeDef(delegate),
            &item.attrs,
            &["guid", "no_guid", "invoke", "invoke_no_new_slot"],
        )?;

        if let Some(arch_bits) = self.read_arch(&item.attrs)? {
            self.emit_arch_attribute(metadata::writer::HasAttribute::TypeDef(delegate), arch_bits);
        }

        let already_has_guid = self.encode_guid_pseudo_attrs(
            metadata::writer::HasAttribute::TypeDef(delegate),
            &item.attrs,
        )?;

        for (number, name) in self.generics.iter().enumerate() {
            self.output.GenericParam(
                name,
                metadata::writer::TypeOrMethodDef::TypeDef(delegate),
                number.try_into().unwrap(),
                metadata::GenericParamAttributes::None,
            );
        }

        let mut flags = metadata::MethodAttributes::Public
            | metadata::MethodAttributes::HideBySig
            | metadata::MethodAttributes::SpecialName
            | metadata::MethodAttributes::Virtual;
        let mut invoke_no_new_slot = None;
        for attr in &item.attrs {
            if attr.path().is_ident("invoke_no_new_slot") {
                if !matches!(attr.meta, syn::Meta::Path(_)) {
                    return self.err(
                        attr,
                        "`invoke_no_new_slot` attribute does not accept arguments",
                    );
                }
                if invoke_no_new_slot.replace(attr).is_some() {
                    return self.err(attr, "duplicate `invoke_no_new_slot` attribute");
                }
            }
        }
        if invoke_no_new_slot.is_none() {
            flags |= metadata::MethodAttributes::NewSlot;
        }

        let params = self.collect_params(&item.sig)?;

        // Delegates are always WinRT - validate that all parameter and return types are WinRT.
        for arg in &item.sig.inputs {
            if let syn::FnArg::Typed(pt) = arg {
                let ty = self.encode_type(&pt.ty)?;
                self.validate_type_is_winrt(&pt.ty, &ty)?;
            }
        }

        let types: Vec<metadata::Type> = params.iter().map(|param| param.ty.clone()).collect();
        let return_type = self.encode_return_type(&item.sig.output)?;

        if let syn::ReturnType::Type(_, return_syn_ty) = &item.sig.output {
            self.validate_type_is_winrt(return_syn_ty.as_ref(), &return_type)?;
        }

        if !already_has_guid {
            guid::derive_and_emit_guid(
                self.output,
                metadata::writer::HasAttribute::TypeDef(delegate),
                self.namespace,
                self.name,
                &[("Invoke", types.as_slice(), &return_type)],
            );
        }

        let constructor_types = [metadata::Type::Object, metadata::Type::ISize];
        let constructor = self.output.MethodDef(
            ".ctor",
            &metadata::Signature {
                flags: metadata::MethodCallAttributes::HASTHIS,
                return_type: metadata::Type::Void,
                types: constructor_types.to_vec(),
            },
            metadata::MethodAttributes::Private
                | metadata::MethodAttributes::HideBySig
                | metadata::MethodAttributes::SpecialName
                | metadata::MethodAttributes::RTSpecialName,
            metadata::MethodImplAttributes::Runtime,
        );
        self.origin(constructor, &item.sig.ident);
        self.encode_simple_params(&[
            ("object", &constructor_types[0]),
            ("method", &constructor_types[1]),
        ])?;

        let signature = metadata::Signature {
            flags: metadata::MethodCallAttributes::HASTHIS,
            return_type,
            types,
        };

        // Delegate methods are runtime-implemented, matching real WinRT delegate metadata
        // so that strict consumers (e.g. CsWinRT) recognize the `Invoke` method.
        let invoke = self.output.MethodDef(
            "Invoke",
            &signature,
            flags,
            metadata::MethodImplAttributes::Runtime,
        );
        self.origin(invoke, &item.sig.ident);
        self.encode_wrapped_attrs(
            metadata::writer::HasAttribute::MethodDef(invoke),
            &item.attrs,
            "invoke",
        )?;

        self.encode_return_attrs(&item.return_attrs)?;
        self.encode_params(&params)?;

        Ok(())
    }
}
