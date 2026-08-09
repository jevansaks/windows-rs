use super::*;

pub(super) trait CopyPolicy {
    fn namespace(&self, def: reader::TypeDef) -> String {
        def.namespace().to_string()
    }

    fn ty(&self, ty: &Type) -> Type {
        ty.clone()
    }

    fn signature(&self, signature: &Signature) -> Signature {
        Signature {
            flags: signature.flags,
            return_type: self.ty(&signature.return_type),
            types: signature.types.iter().map(|ty| self.ty(ty)).collect(),
        }
    }
}

pub(super) struct Identity;

impl CopyPolicy for Identity {}

pub(super) struct TypeOptions<'a> {
    pub(super) arch_override: Option<i32>,
    pub(super) invoke_signature: Option<&'a Signature>,
}

pub(super) fn write_type<P: CopyPolicy>(
    policy: &P,
    file: &mut writer::File,
    context: &mut CopyContext,
    index: &reader::Index,
    def: reader::TypeDef,
    outer: Option<writer::TypeDef>,
    options: TypeOptions<'_>,
) {
    let extends = def
        .extends()
        .map(|extends| {
            let extends = policy.ty(&Type::ClassName(TypeName::named(
                extends.namespace(),
                extends.name(),
            )));
            let Type::ClassName(extends) = extends else {
                unreachable!("base type transform must preserve class type")
            };
            writer::TypeDefOrRef::TypeRef(file.TypeRef(&extends.namespace, &extends.name))
        })
        .unwrap_or_default();

    debug_assert!(
        !def.flags().is_nested() || def.namespace().is_empty(),
        "nested type should have empty namespace"
    );
    debug_assert!(
        def.flags().is_nested() || !def.namespace().is_empty(),
        "non-nested type should have non-empty namespace"
    );

    let namespace = if def.flags().is_nested() {
        String::new()
    } else {
        policy.namespace(def)
    };
    let type_def = file.TypeDef(&namespace, def.name(), extends, def.flags());

    if let Some(outer) = outer {
        file.NestedClass(type_def, outer);
    }

    for field in def.fields() {
        write_field(policy, file, field, None);
    }

    let generics: Vec<_> = def
        .generic_params()
        .map(|param| Type::Generic(param.name().to_string(), param.sequence()))
        .collect();

    write_attributes_with_arch(
        file,
        writer::HasAttribute::TypeDef(type_def),
        def,
        options.arch_override,
    );

    for map in def.interface_impls() {
        let interface = policy.ty(&map.interface(&generics));
        let interface_impl = file.InterfaceImpl(type_def, &interface);
        write_attributes(
            file,
            writer::HasAttribute::InterfaceImpl(interface_impl),
            map,
        );
    }

    for generic in def.generic_params() {
        file.GenericParam(
            generic.name(),
            writer::TypeOrMethodDef::TypeDef(type_def),
            generic.sequence(),
            generic.flags(),
        );
    }

    for method in def.methods() {
        write_method(
            policy,
            file,
            context,
            method,
            &generics,
            None,
            options
                .invoke_signature
                .filter(|_| method.name() == "Invoke"),
        );
    }
    write_method_impls(policy, file, context, type_def, def, &generics);
    write_semantics(policy, file, context, type_def, def, &generics);

    if let Some(class_layout) = def.class_layout() {
        file.ClassLayout(
            type_def,
            class_layout.packing_size(),
            class_layout.class_size(),
        );
    }

    for inner_def in index.nested(def) {
        write_type(
            policy,
            file,
            context,
            index,
            inner_def,
            Some(type_def),
            TypeOptions {
                arch_override: options.arch_override,
                invoke_signature: None,
            },
        );
    }
}

pub(super) fn write_field<P: CopyPolicy>(
    policy: &P,
    file: &mut writer::File,
    field: reader::Field,
    arch_override: Option<i32>,
) {
    let field_def = file.Field(field.name(), &policy.ty(&field.ty()), field.flags());
    if let Some(layout) = field.layout() {
        file.FieldLayout(field_def, layout.offset());
    }
    if let Some(constant) = field.constant() {
        file.Constant(writer::HasConstant::Field(field_def), &constant.value());
    }
    write_attributes_with_arch(
        file,
        writer::HasAttribute::Field(field_def),
        field,
        arch_override,
    );
}

pub(super) fn write_method<P: CopyPolicy>(
    policy: &P,
    file: &mut writer::File,
    context: &mut CopyContext,
    method: reader::MethodDef,
    generics: &[Type],
    arch_override: Option<i32>,
    signature_override: Option<&Signature>,
) -> writer::MethodDef {
    let signature = signature_override
        .cloned()
        .unwrap_or_else(|| policy.signature(&method.signature(generics)));
    let method_def = file.MethodDef(
        method.name(),
        &signature,
        method.flags(),
        method.impl_flags(),
    );
    context.method(method, method_def);
    for param_def in method.params() {
        let param = file.Param(param_def.name(), param_def.sequence(), param_def.flags());
        write_attributes(file, writer::HasAttribute::Param(param), param_def);
    }
    write_attributes_with_arch(
        file,
        writer::HasAttribute::MethodDef(method_def),
        method,
        arch_override,
    );
    if let Some(impl_map) = method.impl_map() {
        file.ImplMap(
            method_def,
            impl_map.flags(),
            impl_map.import_name(),
            impl_map.import_scope().name(),
        );
    }
    for generic in method.generic_params() {
        file.GenericParam(
            generic.name(),
            writer::TypeOrMethodDef::MethodDef(method_def),
            generic.sequence(),
            generic.flags(),
        );
    }
    method_def
}

fn write_method_impls<P: CopyPolicy>(
    policy: &P,
    file: &mut writer::File,
    context: &mut CopyContext,
    type_def: writer::TypeDef,
    def: reader::TypeDef,
    generics: &[Type],
) {
    for method_impl in def.method_impls() {
        let body = write_method_target(policy, file, method_impl.body(), generics);
        let declaration = write_method_target(policy, file, method_impl.declaration(), generics);
        context.method_impl(
            type_def,
            body,
            declaration,
            format!("MethodImpl on `{}.{}`", def.namespace(), def.name()),
        );
    }
}

fn write_method_target<P: CopyPolicy>(
    policy: &P,
    file: &mut writer::File,
    target: reader::MethodDefOrRef,
    generics: &[Type],
) -> MethodTarget {
    match target {
        reader::MethodDefOrRef::MethodDef(method) => CopyContext::method_target(method),
        reader::MethodDefOrRef::MemberRef(member) => {
            let parent = match member.parent() {
                reader::MemberRefParent::TypeDef(parent) => {
                    Type::ClassName(TypeName::named(parent.namespace(), parent.name()))
                }
                reader::MemberRefParent::TypeRef(parent) => {
                    Type::ClassName(TypeName::named(parent.namespace(), parent.name()))
                }
                reader::MemberRefParent::TypeSpec(parent) => parent.ty(generics),
                reader::MemberRefParent::ModuleRef(_) => {
                    return MethodTarget::Unsupported("ModuleRef");
                }
                reader::MemberRefParent::MethodDef(_) => {
                    return MethodTarget::Unsupported("MethodDef");
                }
            };
            let parent = file.MemberRefType(&policy.ty(&parent));
            let signature = policy.signature(&member.instantiated_signature(generics));
            let member = file.MemberRef(member.name(), &signature, parent);
            MethodTarget::MemberRef(member)
        }
    }
}

pub(super) fn write_semantics<P: CopyPolicy>(
    policy: &P,
    file: &mut writer::File,
    context: &mut CopyContext,
    type_def: writer::TypeDef,
    def: reader::TypeDef,
    generics: &[Type],
) {
    let mut first_property = None;
    for property in def.properties() {
        let signature = policy.signature(&property.signature(generics));
        let property_def =
            file.PropertyWithSignature(property.name(), &signature, property.flags());
        first_property.get_or_insert(property_def);
        write_attributes(file, writer::HasAttribute::Property(property_def), property);
        if let Some(constant) = property.constant() {
            file.Constant(
                writer::HasConstant::Property(property_def),
                &constant.value(),
            );
        }
        for semantics in property.semantics() {
            context.semantics(
                semantics.semantics(),
                semantics.method(),
                writer::HasSemantics::Property(property_def),
                format!(
                    "property {}.{}.{}",
                    def.namespace(),
                    def.name(),
                    property.name()
                ),
            );
        }
    }
    if let Some(first_property) = first_property {
        file.PropertyMap(type_def, first_property);
    }

    let mut first_event = None;
    for event in def.events() {
        let event_def =
            file.EventWithFlags(event.name(), &policy.ty(&event.ty(generics)), event.flags());
        first_event.get_or_insert(event_def);
        write_attributes(file, writer::HasAttribute::Event(event_def), event);
        for semantics in event.semantics() {
            context.semantics(
                semantics.semantics(),
                semantics.method(),
                writer::HasSemantics::Event(event_def),
                format!("event {}.{}.{}", def.namespace(), def.name(), event.name()),
            );
        }
    }
    if let Some(first_event) = first_event {
        file.EventMap(type_def, first_event);
    }
}
