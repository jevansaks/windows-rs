use super::*;

pub(super) fn validate(context: &mut Context<'_>) {
    for ty in context.types() {
        let flags = ty.flags();
        if !flags.contains(crate::TypeAttributes::WindowsRuntime) {
            if context.profile == ValidationProfile::WinRT {
                context.invalid(
                    ty.row_id(),
                    None,
                    format!(
                        "WinRT type `{}.{}` must have the WindowsRuntime flag",
                        ty.namespace(),
                        ty.name()
                    ),
                );
            }
            continue;
        }

        if flags.contains(crate::TypeAttributes::Interface)
            && !flags.contains(crate::TypeAttributes::Abstract)
        {
            context.invalid(
                ty.row_id(),
                None,
                format!(
                    "WinRT interface `{}.{}` must have the Abstract flag",
                    ty.namespace(),
                    ty.name()
                ),
            );
        }

        validate_overloads(context, ty);

        if ty.category() == reader::TypeCategory::Class {
            validate_default_interface(context, ty);
            validate_class_factories(context, ty);
        }

        validate_api_contract(context, ty);
    }

    validate_attribute_targets(context);
    let versions = validate_contract_versions(context);
    validate_contract_version_order(context, &versions);
}

fn validate_overloads(context: &mut Context<'_>, ty: reader::TypeDef<'_>) {
    let mut overloads = HashMap::<String, Vec<(reader::MethodDef<'_>, crate::Signature)>>::new();
    let mut default_overloads = HashMap::<String, Vec<reader::MethodDef<'_>>>::new();
    let generics = generics(ty);

    for method in ty.methods() {
        let signature = method.signature(&generics);
        let overload_name = method
            .attributes()
            .find(|attribute| {
                attribute.namespace() == "Windows.Foundation.Metadata"
                    && attribute.name() == "OverloadAttribute"
            })
            .and_then(|attribute| {
                attribute
                    .value()
                    .into_iter()
                    .find_map(|(_, value)| match value {
                        crate::Value::Utf8(name) => Some(name),
                        _ => None,
                    })
            });
        let is_default_overload = method.attributes().any(|attribute| {
            attribute.namespace() == "Windows.Foundation.Metadata"
                && attribute.name() == "DefaultOverloadAttribute"
        });

        if is_default_overload && overload_name.is_none() {
            context.invalid(
                method.row_id(),
                Some(ty.row_id()),
                format!(
                    "method `{}.{}.{}` has `DefaultOverloadAttribute` without \
                     `OverloadAttribute`",
                    ty.namespace(),
                    ty.name(),
                    method.name()
                ),
            );
        }

        if let Some(overload_name) = overload_name {
            let previous = overloads
                .entry(overload_name.clone())
                .or_default()
                .iter()
                .find(|(previous, previous_signature)| {
                    same_identity(previous_signature, &signature)
                        && arches_overlap(previous.arches(), method.arches())
                });
            if let Some((previous, _)) = previous {
                context.duplicate(
                    method.row_id(),
                    previous.row_id(),
                    format!(
                        "duplicate overload signature `{overload_name}` on `{}.{}`",
                        ty.namespace(),
                        ty.name()
                    ),
                );
            }
            overloads
                .entry(overload_name.clone())
                .or_default()
                .push((method, signature.clone()));

            if is_default_overload {
                let previous = default_overloads
                    .entry(overload_name.clone())
                    .or_default()
                    .iter()
                    .find(|previous| arches_overlap(previous.arches(), method.arches()));
                if let Some(previous) = previous {
                    context.duplicate(
                        method.row_id(),
                        previous.row_id(),
                        format!(
                            "duplicate default overload `{overload_name}` on `{}.{}`",
                            ty.namespace(),
                            ty.name()
                        ),
                    );
                }
                default_overloads
                    .entry(overload_name)
                    .or_default()
                    .push(method);
            }
        }
    }
}

fn validate_api_contract(context: &mut Context<'_>, ty: reader::TypeDef<'_>) {
    if !ty.has_attribute("ApiContractAttribute") {
        return;
    }

    if ty.category() != reader::TypeCategory::Struct {
        context.invalid(
            ty.row_id(),
            None,
            format!(
                "API contract `{}.{}` must be a struct",
                ty.namespace(),
                ty.name()
            ),
        );
    }

    let mut versions = ty.attributes().filter(|attribute| {
        attribute.namespace() == "Windows.Foundation.Metadata"
            && attribute.name() == "ContractVersionAttribute"
    });
    let Some(first) = versions.next() else {
        context.invalid(
            ty.row_id(),
            None,
            format!(
                "API contract `{}.{}` must have a contract version",
                ty.namespace(),
                ty.name()
            ),
        );
        return;
    };

    for duplicate in versions {
        context.duplicate(
            duplicate.row_id(),
            first.row_id(),
            format!(
                "API contract `{}.{}` has multiple contract versions",
                ty.namespace(),
                ty.name()
            ),
        );
    }
}

struct ContractVersion {
    contract: Option<crate::TypeName>,
    version: u64,
}

fn validate_contract_versions<'a>(
    context: &mut Context<'a>,
) -> Vec<(reader::Attribute<'a>, ContractVersion)> {
    let mut versions = Vec::new();

    for attribute in context.index.attributes().filter(|attribute| {
        attribute.namespace() == "Windows.Foundation.Metadata"
            && attribute.name() == "ContractVersionAttribute"
    }) {
        let Some(version) = contract_version(context, attribute) else {
            continue;
        };

        if version.version == 0 {
            context.invalid(
                attribute.row_id(),
                attribute_target(attribute.parent()).map(|(_, _, row)| row),
                "contract version must not be zero".to_string(),
            );
        }

        if version.contract.is_none() {
            let contract = matches!(
                attribute.parent(),
                reader::HasAttribute::TypeDef(ty)
                    if ty.has_attribute("ApiContractAttribute")
            );
            if !contract {
                context.invalid(
                    attribute.row_id(),
                    attribute_target(attribute.parent()).map(|(_, _, row)| row),
                    "contract version must name an API contract".to_string(),
                );
            }
        } else if let Some(name) = &version.contract {
            let local = context.index.get(&name.namespace, &name.name).next();
            let target = local.or_else(|| {
                context
                    .references
                    .and_then(|references| references.get(&name.namespace, &name.name).next())
            });

            match target {
                None => context.invalid(
                    attribute.row_id(),
                    attribute_target(attribute.parent()).map(|(_, _, row)| row),
                    format!(
                        "contract version references missing API contract `{}.{}`",
                        name.namespace, name.name
                    ),
                ),
                Some(target) if !target.has_attribute("ApiContractAttribute") => context.invalid(
                    attribute.row_id(),
                    local.map(|target| target.row_id()),
                    format!(
                        "contract version target `{}.{}` is not an API contract",
                        name.namespace, name.name
                    ),
                ),
                Some(_) => {}
            }
        }

        versions.push((attribute, version));
    }

    versions
}

fn validate_contract_version_order(
    context: &mut Context<'_>,
    versions: &[(reader::Attribute<'_>, ContractVersion)],
) {
    let mut owners = HashMap::new();
    for ty in context
        .types()
        .into_iter()
        .filter(|ty| ty.flags().contains(crate::TypeAttributes::WindowsRuntime))
    {
        owners.extend(ty.fields().map(|row| (row.row_id(), ty)));
        for method in ty.methods() {
            owners.insert(method.row_id(), ty);
            owners.extend(method.params().map(|row| (row.row_id(), ty)));
        }
        owners.extend(ty.properties().map(|row| (row.row_id(), ty)));
        owners.extend(ty.events().map(|row| (row.row_id(), ty)));
        owners.extend(ty.interface_impls().map(|row| (row.row_id(), ty)));
    }

    let mut type_versions = HashMap::<RowId, Vec<(&ContractVersion, RowId)>>::new();
    for (attribute, version) in versions {
        if let reader::HasAttribute::TypeDef(ty) = attribute.parent() {
            type_versions
                .entry(ty.row_id())
                .or_default()
                .push((version, attribute.row_id()));
        }
    }

    for (attribute, version) in versions {
        let Some(contract) = &version.contract else {
            continue;
        };
        let Some((_, _, row)) = attribute_target(attribute.parent()) else {
            continue;
        };
        let Some(owner) = owners.get(&row) else {
            continue;
        };
        let Some(owner_versions) = type_versions.get(&owner.row_id()) else {
            continue;
        };
        let Some((owner_version, owner_attribute)) = owner_versions
            .iter()
            .filter(|(candidate, _)| {
                candidate.contract.as_ref().is_some_and(|candidate| {
                    candidate.namespace == contract.namespace && candidate.name == contract.name
                })
            })
            .min_by_key(|(candidate, _)| candidate.version)
        else {
            continue;
        };

        if version.version < owner_version.version {
            context.invalid(
                attribute.row_id(),
                Some(*owner_attribute),
                format!(
                    "contract version {} precedes owning type `{}.{}` version {}",
                    version.version,
                    owner.namespace(),
                    owner.name(),
                    owner_version.version
                ),
            );
        }
    }
}

fn contract_version(
    context: &Context<'_>,
    attribute: reader::Attribute<'_>,
) -> Option<ContractVersion> {
    let args = attribute_args(context, attribute)?;
    let fixed: Vec<_> = args
        .iter()
        .filter_map(|arg| match arg {
            reader::AttributeArg::Fixed(value) => Some(value),
            _ => None,
        })
        .collect();
    let version = fixed.last()?.integer_bits()?;
    let contract = if fixed.len() > 1 {
        Some(contract_name(fixed[0])?)
    } else {
        None
    };

    Some(ContractVersion { contract, version })
}

fn contract_name(value: &crate::Value) -> Option<crate::TypeName> {
    match value {
        crate::Value::TypeName(name) => Some(name.clone()),
        crate::Value::Utf8(name) | crate::Value::Utf16(name) => {
            let (namespace, name) = name.rsplit_once('.')?;
            Some(crate::TypeName::named(namespace, name))
        }
        _ => None,
    }
}

fn attribute_args(
    context: &Context<'_>,
    attribute: reader::Attribute<'_>,
) -> Option<Vec<reader::AttributeArg>> {
    match context.references {
        Some(references) => attribute.try_args_with_references(references).ok(),
        None => attribute.try_args().ok(),
    }
}

fn validate_attribute_targets(context: &mut Context<'_>) {
    let mut owners = HashMap::new();
    for ty in context
        .types()
        .into_iter()
        .filter(|ty| ty.flags().contains(crate::TypeAttributes::WindowsRuntime))
    {
        let (type_target, _) = type_def_target(ty);
        owners.insert(ty.row_id(), type_target);
        owners.extend(ty.fields().map(|row| (row.row_id(), type_target)));
        for method in ty.methods() {
            owners.insert(method.row_id(), type_target);
            owners.extend(method.params().map(|row| (row.row_id(), type_target)));
        }
        owners.extend(ty.properties().map(|row| (row.row_id(), type_target)));
        owners.extend(ty.events().map(|row| (row.row_id(), type_target)));
        owners.extend(ty.interface_impls().map(|row| (row.row_id(), type_target)));
    }

    let mut usage_masks = HashMap::<RowId, Option<u32>>::new();
    for attribute in context.index.attributes() {
        let Some(definition) = attributes::definition(context, attribute) else {
            continue;
        };
        let Some(mask) = *usage_masks
            .entry(definition.row_id())
            .or_insert_with(|| attribute_usage_mask(context, definition))
        else {
            continue;
        };
        let Some((target, name, row)) = attribute_target(attribute.parent()) else {
            continue;
        };
        let Some(owner) = owners.get(&row) else {
            continue;
        };

        if mask & target == 0 && mask & owner == 0 {
            context.invalid(
                attribute.row_id(),
                Some(row),
                format!(
                    "attribute `{}.{}` cannot target {name}",
                    definition.namespace(),
                    definition.name()
                ),
            );
        }
    }
}

fn attribute_usage_mask(context: &Context<'_>, definition: reader::TypeDef<'_>) -> Option<u32> {
    let usage = definition.find_attribute("AttributeUsageAttribute")?;
    attribute_args(context, usage)?
        .iter()
        .find_map(|arg| match arg {
            reader::AttributeArg::Fixed(crate::Value::EnumValue(_, value)) => {
                match value.as_ref() {
                    crate::Value::I32(value) => Some(*value as u32),
                    crate::Value::U32(value) => Some(*value),
                    _ => None,
                }
            }
            reader::AttributeArg::Fixed(crate::Value::I32(value)) => Some(*value as u32),
            reader::AttributeArg::Fixed(crate::Value::U32(value)) => Some(*value),
            _ => None,
        })
}

fn attribute_target(parent: reader::HasAttribute<'_>) -> Option<(u32, &'static str, RowId)> {
    Some(match parent {
        reader::HasAttribute::TypeDef(row) => {
            let (target, name) = type_def_target(row);
            (target, name, row.row_id())
        }
        reader::HasAttribute::Event(row) => (4 | 64, "an event", row.row_id()),
        reader::HasAttribute::Field(row) => (8, "a field", row.row_id()),
        reader::HasAttribute::MethodDef(row) => {
            let mut target = 64;
            if row.name().starts_with("get_") || row.name().starts_with("put_") {
                target |= 256;
            } else if row.name().starts_with("add_") || row.name().starts_with("remove_") {
                target |= 4;
            }
            (target, "a method", row.row_id())
        }
        reader::HasAttribute::MethodParam(row) => (128, "a parameter", row.row_id()),
        reader::HasAttribute::Property(row) => (256 | 64, "a property", row.row_id()),
        reader::HasAttribute::InterfaceImpl(row) => {
            (2048, "an interface implementation", row.row_id())
        }
        reader::HasAttribute::TypeRef(_)
        | reader::HasAttribute::MemberRef(_)
        | reader::HasAttribute::TypeSpec(_)
        | reader::HasAttribute::GenericParam(_) => return None,
    })
}

fn type_def_target(row: reader::TypeDef<'_>) -> (u32, &'static str) {
    if row.has_attribute("ApiContractAttribute") {
        (2 | 8192, "an API contract")
    } else {
        match row.category() {
            reader::TypeCategory::Delegate => (1, "a delegate"),
            reader::TypeCategory::Enum => (2, "an enum"),
            reader::TypeCategory::Interface => (16, "an interface"),
            reader::TypeCategory::Class | reader::TypeCategory::Attribute => {
                (512, "a runtime class")
            }
            reader::TypeCategory::Struct => (1024, "a struct"),
        }
    }
}

fn validate_default_interface(context: &mut Context<'_>, ty: reader::TypeDef<'_>) {
    let mut default = None;
    for implementation in ty.interface_impls() {
        let Some(attribute) = implementation.attributes().find(|attribute| {
            attribute.namespace() == "Windows.Foundation.Metadata"
                && attribute.name() == "DefaultAttribute"
        }) else {
            continue;
        };

        if let Some(previous) = default {
            context.duplicate(
                attribute.row_id(),
                previous,
                format!(
                    "runtime class `{}.{}` has multiple default interfaces",
                    ty.namespace(),
                    ty.name()
                ),
            );
        } else {
            default = Some(attribute.row_id());
        }
    }
}

fn validate_class_factories(context: &mut Context<'_>, ty: reader::TypeDef<'_>) {
    for attribute in ty.attributes().filter(|attribute| {
        attribute.namespace() == "Windows.Foundation.Metadata"
            && matches!(
                attribute.name(),
                "ActivatableAttribute" | "ComposableAttribute" | "StaticAttribute"
            )
    }) {
        let Some(args) = attribute_args(context, attribute) else {
            continue;
        };
        let Some(factory) = args.into_iter().find_map(|arg| match arg {
            reader::AttributeArg::Fixed(crate::Value::TypeName(name)) => Some(name),
            _ => None,
        }) else {
            continue;
        };

        let local = context.index.get(&factory.namespace, &factory.name).next();
        let target = local.or_else(|| {
            context
                .references
                .and_then(|references| references.get(&factory.namespace, &factory.name).next())
        });

        match target {
            None => context.invalid(
                attribute.row_id(),
                Some(ty.row_id()),
                format!(
                    "`{}.{}` references missing factory interface `{}.{}`",
                    ty.namespace(),
                    ty.name(),
                    factory.namespace,
                    factory.name
                ),
            ),
            Some(target) if target.category() != reader::TypeCategory::Interface => {
                context.invalid(
                    attribute.row_id(),
                    local.map(|target| target.row_id()),
                    format!(
                        "`{}.{}` factory `{}.{}` is not an interface",
                        ty.namespace(),
                        ty.name(),
                        factory.namespace,
                        factory.name
                    ),
                );
            }
            Some(_) => {}
        }
    }
}
