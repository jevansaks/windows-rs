use super::*;

pub(super) fn validate(context: &mut Context<'_>) {
    for ty in context.types() {
        if ty.flags().contains(crate::TypeAttributes::WindowsRuntime) {
            if context.profile == ValidationProfile::Win32 {
                context.invalid(
                    ty.row_id(),
                    None,
                    format!(
                        "Win32 type `{}.{}` must not have the WindowsRuntime flag",
                        ty.namespace(),
                        ty.name()
                    ),
                );
            }
            continue;
        }

        validate_layout(context, ty);

        for method in ty.methods() {
            let pinvoke = method
                .flags()
                .contains(crate::MethodAttributes::PInvokeImpl);
            let impl_map = method.impl_map();
            if pinvoke && !method.flags().contains(crate::MethodAttributes::Static) {
                context.invalid(
                    method.row_id(),
                    Some(ty.row_id()),
                    format!(
                        "Win32 P/Invoke method `{}.{}.{}` must be static",
                        ty.namespace(),
                        ty.name(),
                        method.name()
                    ),
                );
            }
            if pinvoke && impl_map.is_none() {
                context.invalid(
                    method.row_id(),
                    Some(ty.row_id()),
                    format!(
                        "Win32 method `{}.{}.{}` has PInvokeImpl without an ImplMap",
                        ty.namespace(),
                        ty.name(),
                        method.name()
                    ),
                );
            } else if pinvoke && let Some(impl_map) = impl_map {
                validate_calling_convention(context, ty, method, impl_map);
            } else if let Some(impl_map) = impl_map {
                context.invalid(
                    impl_map.row_id(),
                    Some(method.row_id()),
                    format!(
                        "Win32 method `{}.{}.{}` has an ImplMap without PInvokeImpl",
                        ty.namespace(),
                        ty.name(),
                        method.name()
                    ),
                );
            }
        }
    }
}

fn validate_layout(context: &mut Context<'_>, ty: reader::TypeDef<'_>) {
    if ty.category() != reader::TypeCategory::Struct {
        return;
    }

    let flags = ty.flags();
    let sequential = flags.contains(crate::TypeAttributes::SequentialLayout);
    let explicit = flags.contains(crate::TypeAttributes::ExplicitLayout);
    match (sequential, explicit) {
        (false, false) => context.invalid(
            ty.row_id(),
            None,
            format!(
                "Win32 struct `{}.{}` requires sequential or explicit layout",
                ty.namespace(),
                ty.name()
            ),
        ),
        (true, true) => context.invalid(
            ty.row_id(),
            None,
            format!(
                "Win32 struct `{}.{}` cannot use both sequential and explicit layout",
                ty.namespace(),
                ty.name()
            ),
        ),
        (false, true) => {
            let fields: Vec<_> = ty
                .fields()
                .filter(|field| {
                    let flags = field.flags();
                    !flags.contains(crate::FieldAttributes::Static)
                        && !flags.contains(crate::FieldAttributes::Literal)
                })
                .collect();
            if fields.iter().any(|field| field.layout().is_some()) {
                for field in fields.into_iter().filter(|field| field.layout().is_none()) {
                    context.invalid(
                        field.row_id(),
                        Some(ty.row_id()),
                        format!(
                            "explicit-layout Win32 struct `{}.{}` field `{}` has no field layout",
                            ty.namespace(),
                            ty.name(),
                            field.name()
                        ),
                    );
                }
            }
        }
        (true, false) => {}
    }
}

fn validate_calling_convention(
    context: &mut Context<'_>,
    ty: reader::TypeDef<'_>,
    method: reader::MethodDef<'_>,
    impl_map: reader::ImplMap<'_>,
) {
    let convention = (impl_map.flags() & crate::PInvokeAttributes::CallConvMask).0;
    if convention == 0 {
        context.invalid(
            impl_map.row_id(),
            Some(method.row_id()),
            format!(
                "Win32 P/Invoke method `{}.{}.{}` has no calling convention",
                ty.namespace(),
                ty.name(),
                method.name()
            ),
        );
        return;
    }

    if !matches!(
        convention,
        value if value == crate::PInvokeAttributes::CallConvPlatformapi.0
            || value == crate::PInvokeAttributes::CallConvCdecl.0
            || value == crate::PInvokeAttributes::CallConvFastcall.0
    ) {
        context.invalid(
            impl_map.row_id(),
            Some(method.row_id()),
            format!(
                "Win32 P/Invoke method `{}.{}.{}` has unsupported calling convention 0x{convention:x}",
                ty.namespace(),
                ty.name(),
                method.name()
            ),
        );
        return;
    }

    let signature = method.signature(&[]);
    if signature.flags.0 & 0x0f == crate::MethodCallAttributes::VARARG.0
        && convention != crate::PInvokeAttributes::CallConvCdecl.0
    {
        context.invalid(
            impl_map.row_id(),
            Some(method.row_id()),
            format!(
                "variadic Win32 P/Invoke method `{}.{}.{}` must use the C calling convention",
                ty.namespace(),
                ty.name(),
                method.name()
            ),
        );
    }
}
