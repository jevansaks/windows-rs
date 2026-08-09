use super::*;
use std::collections::BTreeSet;

#[derive(Default)]
struct ProjectionShape {
    properties: Vec<ProjectedProperty>,
    events: Vec<ProjectedEvent>,
}

struct ProjectedProperty {
    name: String,
    signature: metadata::Signature,
    accessors: BTreeSet<(u16, String)>,
}

struct ProjectedEvent {
    name: String,
    ty: metadata::Type,
    accessors: BTreeSet<(u16, String)>,
}

pub fn write_class(item: &metadata::reader::TypeDef) -> Result<TokenStream, Error> {
    let namespace = item.namespace();
    let name = write_ident(item.name());
    let flags = item.flags();
    let supported = metadata::TypeAttributes::Public
        | metadata::TypeAttributes::Abstract
        | metadata::TypeAttributes::Sealed
        | metadata::TypeAttributes::WindowsRuntime;
    if flags.0 & !supported.0 != 0 {
        return Err(writer_err!(
            "class `{}` has unsupported flags {:#x}",
            item.name(),
            flags.0
        ));
    }
    let class_shape =
        flags & (metadata::TypeAttributes::Abstract | metadata::TypeAttributes::Sealed);
    let shape_attr = if class_shape == metadata::TypeAttributes::Sealed {
        quote! {}
    } else if class_shape == metadata::TypeAttributes::default() {
        quote! { #[unsealed] }
    } else if class_shape == (metadata::TypeAttributes::Abstract | metadata::TypeAttributes::Sealed)
    {
        quote! { #[static_only] }
    } else {
        return Err(writer_err!(
            "class `{}` has unsupported abstract/sealed flags {:#x}",
            item.name(),
            class_shape.0
        ));
    };
    let extends = item
        .extends()
        .ok_or_else(|| writer_err!("class `{}` has no base type", item.name()))?;

    let extends = if extends == ("System", "Object") {
        quote! {}
    } else {
        let ty = write_type_ref(namespace, &extends);
        quote! { : #ty }
    };

    let custom_attrs = write_custom_attributes(item.attributes(), namespace, item.index())?;
    let association_attrs = write_static_association_attrs(item, namespace);
    let class_association_attrs = write_class_association_attrs(item, namespace)?;

    let interfaces = item
        .interface_impls()
        .map(|implementation| write_interface(item, namespace, &implementation))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(quote! {
        #shape_attr
        #(#custom_attrs)*
        #(#association_attrs)*
        #(#class_association_attrs)*
        class #name #extends {
            #(#interfaces)*
        }
    })
}

fn write_class_association_attrs(
    class: &metadata::reader::TypeDef,
    namespace: &str,
) -> Result<Vec<TokenStream>, Error> {
    let projections = projection_shape(class);

    let mut result = Vec::new();
    for property in class
        .properties()
        .filter(|property| !property_is_projected(class, property, &projections.properties))
    {
        let row_attrs =
            write_custom_attributes(property.attributes(), namespace, property.index())?;
        let signature = property.signature(&[]);
        let static_token = if signature
            .flags
            .contains(metadata::MethodCallAttributes::HASTHIS)
        {
            quote! {}
        } else {
            quote! { static }
        };
        let name = write_ident(property.name());
        let ty = write_type(namespace, &signature.return_type);
        let accessors = property
            .semantics()
            .map(|semantics| {
                let kind = match semantics.semantics() {
                    0x0002 => quote! { get },
                    0x0001 => quote! { set },
                    value => {
                        return Err(writer_err!(
                            "property `{}` has unsupported semantics {value:#x}",
                            property.name()
                        ));
                    }
                };
                let method = write_class_association_method(namespace, class, &semantics.method());
                Ok(quote! { #kind = #method })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        result.push(quote! {
            #[class_property(#(#row_attrs)* #static_token #name: #ty, #(#accessors),*)]
        });
    }

    for event in class
        .events()
        .filter(|event| !event_is_projected(class, event, &projections.events))
    {
        let row_attrs = write_custom_attributes(event.attributes(), namespace, event.index())?;
        let name = write_ident(event.name());
        let ty = write_type(namespace, &event.ty(&[]));
        let accessors = event
            .semantics()
            .map(|semantics| {
                let kind = match semantics.semantics() {
                    0x0008 => quote! { add },
                    0x0010 => quote! { remove },
                    value => {
                        return Err(writer_err!(
                            "event `{}` has unsupported semantics {value:#x}",
                            event.name()
                        ));
                    }
                };
                let method = write_class_association_method(namespace, class, &semantics.method());
                Ok(quote! { #kind = #method })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        result.push(quote! {
            #[class_event(#(#row_attrs)* #name: #ty, #(#accessors),*)]
        });
    }

    Ok(result)
}

pub(super) fn event_projects_from_shorthand(event: &metadata::reader::Event) -> bool {
    let mut add = None;
    let mut remove = None;
    for semantics in event.semantics() {
        match semantics.semantics() {
            0x0008 => add = Some(semantics.method().pos()),
            0x0010 => remove = Some(semantics.method().pos()),
            _ => {}
        }
    }
    matches!((add, remove), (Some(add), Some(remove)) if add < remove)
}

fn write_class_association_method(
    namespace: &str,
    class: &metadata::reader::TypeDef,
    method: &metadata::reader::MethodDef,
) -> TokenStream {
    let name = write_ident(method.name());
    let metadata::reader::MemberRefParent::TypeDef(owner) = method.parent() else {
        unreachable!("MethodDef parent is always a TypeDef")
    };
    if owner == *class {
        quote! { #name }
    } else {
        let owner = write_type(
            namespace,
            &metadata::Type::class_named(owner.namespace(), owner.name()),
        );
        quote! { #owner::#name }
    }
}

fn write_static_association_attrs(
    class: &metadata::reader::TypeDef,
    namespace: &str,
) -> Vec<TokenStream> {
    let mut result = Vec::new();
    for attribute in class.attributes().filter(|attribute| {
        attribute.namespace() == "Windows.Foundation.Metadata"
            && attribute.name() == "StaticAttribute"
    }) {
        let Some(metadata::Value::TypeName(name)) = attribute
            .value()
            .into_iter()
            .find_map(|(_, value)| matches!(value, metadata::Value::TypeName(_)).then_some(value))
        else {
            continue;
        };
        let Some(definition) = find_type(class.index(), &name) else {
            continue;
        };
        let interface = write_value(namespace, &metadata::Value::TypeName(name.clone()));
        let projection = interface_projection(definition, &name.generics, true);
        for property in &projection.properties {
            if !class.properties().any(|candidate| {
                property_is_inferable(&candidate)
                    && property_contributes(class, &candidate, property)
            }) {
                let member = write_ident(&property.name);
                result.push(quote! { #[no_static_property(#interface, #member)] });
            }
        }
        for event in &projection.events {
            if !class.events().any(|candidate| {
                event_is_inferable(&candidate) && event_contributes(class, &candidate, event)
            }) {
                let member = write_ident(&event.name);
                result.push(quote! { #[no_static_event(#interface, #member)] });
            }
        }
    }
    result.sort_by_key(TokenStream::to_string);
    result
}

fn write_interface(
    class: &metadata::reader::TypeDef,
    namespace: &str,
    imp: &metadata::reader::InterfaceImpl,
) -> Result<TokenStream, Error> {
    let interface_type = imp.interface(&[]);
    let interface = write_type(namespace, &interface_type);
    let default = imp.attributes().any(|attribute| {
        attribute.namespace() == "Windows.Foundation.Metadata"
            && attribute.name() == "DefaultAttribute"
    });
    let default_attr = if default {
        quote! { #[default] }
    } else {
        quote! {}
    };
    let custom_attrs = write_custom_attributes(
        imp.attributes().filter(|attribute| {
            attribute.namespace() != "Windows.Foundation.Metadata"
                || attribute.name() != "DefaultAttribute"
        }),
        namespace,
        imp.index(),
    )?;
    let mut association_attrs = Vec::new();
    if let metadata::Type::ClassName(name) = &interface_type
        && let Some(definition) = find_type(imp.index(), name)
    {
        let projection = interface_projection(definition, &name.generics, false);
        for property in &projection.properties {
            if !class.properties().any(|candidate| {
                property_is_inferable(&candidate)
                    && property_contributes(class, &candidate, property)
            }) {
                let member = write_ident(&property.name);
                association_attrs.push(quote! { #[no_property(#member)] });
            }
        }
        for event in &projection.events {
            if !class.events().any(|candidate| {
                event_is_inferable(&candidate) && event_contributes(class, &candidate, event)
            }) {
                let member = write_ident(&event.name);
                association_attrs.push(quote! { #[no_event(#member)] });
            }
        }
    }
    association_attrs.sort_by_key(TokenStream::to_string);

    Ok(quote! {
        #default_attr
        #(#custom_attrs)*
        #(#association_attrs)*
        #interface,
    })
}

fn projection_shape(class: &metadata::reader::TypeDef) -> ProjectionShape {
    let mut result = ProjectionShape::default();

    for implementation in class.interface_impls() {
        if let metadata::Type::ClassName(name) = implementation.interface(&[])
            && let Some(definition) = find_type(class.index(), &name)
        {
            result.extend(interface_projection(definition, &name.generics, false));
        }
    }
    for attribute in class.attributes().filter(|attribute| {
        attribute.namespace() == "Windows.Foundation.Metadata"
            && attribute.name() == "StaticAttribute"
    }) {
        let Some(metadata::Value::TypeName(name)) = attribute
            .value()
            .into_iter()
            .find_map(|(_, value)| matches!(value, metadata::Value::TypeName(_)).then_some(value))
        else {
            continue;
        };
        if let Some(definition) = find_type(class.index(), &name) {
            result.extend(interface_projection(definition, &name.generics, true));
        }
    }

    result
}

impl ProjectionShape {
    fn extend(&mut self, other: Self) {
        self.properties.extend(other.properties);
        self.events.extend(other.events);
    }
}

fn interface_projection(
    interface: metadata::reader::TypeDef,
    generics: &[metadata::Type],
    is_static: bool,
) -> ProjectionShape {
    let properties = interface
        .properties()
        .map(|property| {
            let mut signature = property.signature(generics);
            if is_static {
                signature.flags = metadata::MethodCallAttributes(
                    signature.flags.0 & !metadata::MethodCallAttributes::HASTHIS.0,
                );
            }
            ProjectedProperty {
                name: property.name().to_string(),
                signature,
                accessors: property
                    .semantics()
                    .map(|semantics| (semantics.semantics(), semantics.method().name().to_string()))
                    .collect(),
            }
        })
        .collect();
    let events = interface
        .events()
        .filter(event_projects_from_shorthand)
        .map(|event| ProjectedEvent {
            name: event.name().to_string(),
            ty: event.ty(generics),
            accessors: event
                .semantics()
                .map(|semantics| (semantics.semantics(), semantics.method().name().to_string()))
                .collect(),
        })
        .collect();

    ProjectionShape { properties, events }
}

fn property_contributes(
    class: &metadata::reader::TypeDef,
    property: &metadata::reader::Property,
    projected: &ProjectedProperty,
) -> bool {
    property.name() == projected.name
        && property.signature(&[]) == projected.signature
        && projected
            .accessors
            .iter()
            .all(|accessor| class_property_accessors(class, property).contains(accessor))
}

fn property_is_projected(
    class: &metadata::reader::TypeDef,
    property: &metadata::reader::Property,
    projections: &[ProjectedProperty],
) -> bool {
    if !property_is_inferable(property) {
        return false;
    }
    let actual = class_property_accessors(class, property);
    if actual.is_empty() {
        return false;
    }
    let mut projected = BTreeSet::new();
    for candidate in projections {
        if property.name() == candidate.name && property.signature(&[]) == candidate.signature {
            projected.extend(candidate.accessors.iter().cloned());
        }
    }
    actual == projected
}

fn property_is_inferable(property: &metadata::reader::Property) -> bool {
    property.attributes().next().is_none()
}

fn class_property_accessors(
    class: &metadata::reader::TypeDef,
    property: &metadata::reader::Property,
) -> BTreeSet<(u16, String)> {
    property
        .semantics()
        .filter_map(|semantics| {
            let metadata::reader::MemberRefParent::TypeDef(owner) = semantics.method().parent()
            else {
                return None;
            };
            (owner == *class)
                .then(|| (semantics.semantics(), semantics.method().name().to_string()))
        })
        .collect()
}

fn event_contributes(
    class: &metadata::reader::TypeDef,
    event: &metadata::reader::Event,
    projected: &ProjectedEvent,
) -> bool {
    event.name() == projected.name
        && event.ty(&[]) == projected.ty
        && projected
            .accessors
            .iter()
            .all(|accessor| class_event_accessors(class, event).contains(accessor))
}

fn event_is_projected(
    class: &metadata::reader::TypeDef,
    event: &metadata::reader::Event,
    projections: &[ProjectedEvent],
) -> bool {
    if !event_is_inferable(event) {
        return false;
    }
    let actual = class_event_accessors(class, event);
    if actual.is_empty() {
        return false;
    }
    let mut projected = BTreeSet::new();
    for candidate in projections {
        if event.name() == candidate.name && event.ty(&[]) == candidate.ty {
            projected.extend(candidate.accessors.iter().cloned());
        }
    }
    actual == projected
}

fn event_is_inferable(event: &metadata::reader::Event) -> bool {
    event.attributes().next().is_none()
}

fn class_event_accessors(
    class: &metadata::reader::TypeDef,
    event: &metadata::reader::Event,
) -> BTreeSet<(u16, String)> {
    event
        .semantics()
        .filter_map(|semantics| {
            let metadata::reader::MemberRefParent::TypeDef(owner) = semantics.method().parent()
            else {
                return None;
            };
            (owner == *class)
                .then(|| (semantics.semantics(), semantics.method().name().to_string()))
        })
        .collect()
}

fn find_type<'a>(
    index: &'a metadata::reader::Index,
    name: &metadata::TypeName,
) -> Option<metadata::reader::TypeDef<'a>> {
    index.get(&name.namespace, &name.name).next().or_else(|| {
        if let Some((base_name, _)) = name.name.rsplit_once('`') {
            return index.get(&name.namespace, base_name).next();
        }
        if name.generics.is_empty() {
            None
        } else {
            let generic_name = format!("{}`{}", name.name, name.generics.len());
            index.get(&name.namespace, &generic_name).next()
        }
    })
}
