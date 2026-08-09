use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MemberOwner {
    Interface,
    InterfaceRuntime,
    ClassInstance,
    ClassOverridable,
    ClassProtected,
    ClassStatic,
}

#[derive(Default)]
pub(super) struct MemberMaps {
    pub(super) property: bool,
    pub(super) event: bool,
    pub(super) properties: Vec<ProjectedProperty>,
    pub(super) methods: Vec<(String, metadata::writer::MethodDef)>,
}

#[derive(Default)]
pub(super) struct MemberProjection<'a> {
    pub(super) excluded_properties: &'a [String],
    pub(super) excluded_events: &'a [String],
    pub(super) owner: Option<&'a metadata::TypeName>,
    pub(super) target: Option<metadata::writer::TypeDef>,
    pub(super) declaration: Option<&'a metadata::TypeName>,
}

pub(super) struct ProjectedProperty {
    pub(super) name: String,
    pub(super) ty: metadata::Type,
    pub(super) call_flags: metadata::MethodCallAttributes,
    pub(super) row: metadata::writer::Property,
    pub(super) getter: bool,
    pub(super) setter: bool,
}

pub(super) struct MemberMethod<'a> {
    pub(super) name: &'a str,
    pub(super) signature: &'a metadata::Signature,
    pub(super) flags: metadata::MethodAttributes,
    pub(super) impl_flags: metadata::MethodImplAttributes,
}

pub(super) struct ClassProjection<'a> {
    file: &'a File,
    interface: &'a Interface,
    name: metadata::TypeName,
    member_owner: MemberOwner,
    excluded_properties: Vec<String>,
    excluded_events: Vec<String>,
    owner: metadata::TypeName,
}

impl<'a> ClassProjection<'a> {
    pub(super) fn instance(
        file: &'a File,
        interface: &'a Interface,
        name: metadata::TypeName,
        member_owner: MemberOwner,
        excluded_properties: Vec<String>,
        excluded_events: Vec<String>,
        owner: metadata::TypeName,
    ) -> Self {
        debug_assert!(matches!(
            member_owner,
            MemberOwner::ClassInstance
                | MemberOwner::ClassOverridable
                | MemberOwner::ClassProtected
        ));
        Self {
            file,
            interface,
            name,
            member_owner,
            excluded_properties,
            excluded_events,
            owner,
        }
    }

    pub(super) fn statics(
        file: &'a File,
        interface: &'a Interface,
        name: metadata::TypeName,
        excluded_properties: Vec<String>,
        excluded_events: Vec<String>,
        owner: metadata::TypeName,
    ) -> Self {
        Self {
            file,
            interface,
            name,
            member_owner: MemberOwner::ClassStatic,
            excluded_properties,
            excluded_events,
            owner,
        }
    }
}

impl MemberMaps {
    pub(super) fn contains_property(&self, name: &str) -> bool {
        self.properties.iter().any(|property| property.name == name)
    }

    pub(super) fn method(&self, name: &str) -> Option<metadata::writer::MethodDef> {
        self.methods
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, method)| *method)
    }
}

impl MemberOwner {
    pub(super) fn method_flags(self) -> metadata::MethodAttributes {
        match self {
            Self::Interface | Self::InterfaceRuntime => {
                metadata::MethodAttributes::Public
                    | metadata::MethodAttributes::HideBySig
                    | metadata::MethodAttributes::Abstract
                    | metadata::MethodAttributes::NewSlot
                    | metadata::MethodAttributes::Virtual
            }
            Self::ClassInstance => {
                metadata::MethodAttributes::Public
                    | metadata::MethodAttributes::HideBySig
                    | metadata::MethodAttributes::Final
                    | metadata::MethodAttributes::NewSlot
                    | metadata::MethodAttributes::Virtual
            }
            Self::ClassOverridable => {
                metadata::MethodAttributes::Family
                    | metadata::MethodAttributes::HideBySig
                    | metadata::MethodAttributes::NewSlot
                    | metadata::MethodAttributes::Virtual
            }
            Self::ClassProtected => {
                metadata::MethodAttributes::Family
                    | metadata::MethodAttributes::HideBySig
                    | metadata::MethodAttributes::Final
                    | metadata::MethodAttributes::NewSlot
                    | metadata::MethodAttributes::Virtual
            }
            Self::ClassStatic => {
                metadata::MethodAttributes::Public
                    | metadata::MethodAttributes::HideBySig
                    | metadata::MethodAttributes::Static
            }
        }
    }

    pub(super) fn call_flags(self) -> metadata::MethodCallAttributes {
        match self {
            Self::ClassStatic => metadata::MethodCallAttributes::default(),
            Self::Interface
            | Self::InterfaceRuntime
            | Self::ClassInstance
            | Self::ClassOverridable
            | Self::ClassProtected => metadata::MethodCallAttributes::HASTHIS,
        }
    }

    pub(super) fn impl_flags(self) -> metadata::MethodImplAttributes {
        match self {
            Self::Interface => metadata::MethodImplAttributes::default(),
            Self::InterfaceRuntime => metadata::MethodImplAttributes::Runtime,
            Self::ClassInstance
            | Self::ClassOverridable
            | Self::ClassProtected
            | Self::ClassStatic => metadata::MethodImplAttributes::Runtime,
        }
    }
}

impl Encoder<'_> {
    pub(super) fn emit_member_method<S: Spanned>(
        &mut self,
        method: MemberMethod<'_>,
        maps: &mut MemberMaps,
        projection: &MemberProjection<'_>,
        source: &S,
    ) -> metadata::writer::MethodDef {
        let row = self.output.MethodDef(
            method.name,
            method.signature,
            method.flags,
            method.impl_flags,
        );
        if let Some(owner) = projection.owner {
            self.projections.insert(owner, method.name, row);
        }
        if let (Some(target), Some(declaration)) = (projection.target, projection.declaration) {
            let parent = self
                .output
                .MemberRefType(&metadata::Type::ClassName(declaration.clone()));
            let mut signature = method.signature.clone();
            signature.flags |= metadata::MethodCallAttributes::HASTHIS;
            let declaration = self.output.MemberRef(method.name, &signature, parent);
            self.output.MethodImpl(
                target,
                metadata::writer::MethodDefOrRef::MethodDef(row),
                metadata::writer::MethodDefOrRef::MemberRef(declaration),
            );
        }
        maps.methods.push((method.name.to_string(), row));
        self.origin(row, source);
        row
    }

    pub(super) fn encode_class_projection(
        &mut self,
        target: metadata::writer::TypeDef,
        maps: &mut MemberMaps,
        projection: ClassProjection<'_>,
    ) -> Result<(), Error> {
        let generics: Vec<_> = projection
            .interface
            .generics
            .type_params()
            .map(|param| param.ident.to_string())
            .collect();
        if generics.len() != projection.name.generics.len() {
            return self.err(
                &projection.interface.name,
                "generic interface argument count does not match its declaration",
            );
        }

        let mut encoder = Encoder {
            output: self.output,
            origins: self.origins,
            index: self.index,
            file: projection.file,
            namespace: &projection.name.namespace,
            name: &projection.name.name,
            generics,
            generic_args: projection.name.generics.clone(),
            projections: &mut *self.projections,
        };

        if projection.member_owner != MemberOwner::ClassStatic {
            let exclusive_to = encoder.exclusive_to(projection.interface)?;
            if exclusive_to.is_some() && exclusive_to != Some(projection.owner.clone()) {
                return Ok(());
            }
        }

        encoder
            .encode_interface_members(
                projection.interface,
                target,
                projection.member_owner,
                maps,
                MemberProjection {
                    excluded_properties: &projection.excluded_properties,
                    excluded_events: &projection.excluded_events,
                    owner: Some(&projection.owner),
                    target: (projection.member_owner != MemberOwner::ClassStatic).then_some(target),
                    declaration: (projection.member_owner != MemberOwner::ClassStatic)
                        .then_some(&projection.name),
                },
            )
            .map(|_| ())
    }

    pub(super) fn local_interface<'a>(
        index: &'a Index<'a>,
        name: &metadata::TypeName,
    ) -> Option<(&'a File, &'a Interface)> {
        let entries = index
            .namespaces
            .get(&name.namespace)?
            .types
            .get(&name.name)?;
        let (file, Item::Interface(interface)) = *entries.first()? else {
            return None;
        };
        Some((file, interface))
    }

    fn exclusive_to(&self, interface: &Interface) -> Result<Option<metadata::TypeName>, Error> {
        for attr in &interface.attrs {
            if !self.is_exclusive_to_attribute(attr)? {
                continue;
            }
            let attr_ref = self.resolve_emitted_attribute_ref(attr)?;

            let Some(metadata::Value::TypeName(class_name)) =
                attr_ref.args.first().map(|(_, value)| value)
            else {
                return self.err(attr, "`ExclusiveToAttribute` requires a class type");
            };
            return Ok(Some(class_name.clone()));
        }

        Ok(None)
    }
}
