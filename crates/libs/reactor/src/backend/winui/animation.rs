use super::*;

fn easing_for(
    compositor: &windows_composition::Compositor,
    easing: Easing,
) -> windows_composition::CompositionEasingFunction {
    let (p1, p2) = match easing {
        Easing::Linear => return compositor.create_linear_easing_function(),
        Easing::EaseOut => (
            windows_numerics::Vector2 { x: 0.0, y: 0.0 },
            windows_numerics::Vector2 { x: 0.58, y: 1.0 },
        ),
        Easing::EaseIn => (
            windows_numerics::Vector2 { x: 0.42, y: 0.0 },
            windows_numerics::Vector2 { x: 1.0, y: 1.0 },
        ),
        Easing::EaseInOut => (
            windows_numerics::Vector2 { x: 0.42, y: 0.0 },
            windows_numerics::Vector2 { x: 0.58, y: 1.0 },
        ),
    };
    compositor.create_cubic_bezier_easing_function(p1, p2)
}

fn element_visual(ui: &bindings::UIElement) -> Result<windows_composition::Visual> {
    let raw = bindings::ElementCompositionPreview::GetElementVisual(ui)?;
    windows_composition::Visual::from_host(raw.into())
}

pub(super) fn apply_implicit_transitions(
    ui: &bindings::UIElement,
    transitions: Option<ImplicitTransitions>,
) -> Result<()> {
    let visual = element_visual(ui)?;
    let Some(t) = transitions.filter(|t| !t.is_empty()) else {
        visual.set_implicit_animations(None);
        return Ok(());
    };
    let compositor = visual.compositor();
    let collection = compositor.create_implicit_animation_collection();

    // The DSL exposes duration only, so implicit transitions use XAML's EaseOut curve.
    let insert = |target: &str, duration: std::time::Duration, is_scalar: bool| {
        let easing = easing_for(&compositor, Easing::EaseOut);
        if is_scalar {
            let a = compositor.create_scalar_key_frame_animation();
            a.set_duration(duration);
            a.insert_expression_key_frame_with_easing(1.0, "this.FinalValue", &easing);
            a.set_target(target);
            collection.insert(target, &a);
        } else {
            let a = compositor.create_vector3_key_frame_animation();
            a.set_duration(duration);
            a.insert_expression_key_frame_with_easing(1.0, "this.FinalValue", &easing);
            a.set_target(target);
            collection.insert(target, &a);
        }
    };

    if let Some(s) = t.opacity {
        insert("Opacity", s.duration, true);
    }
    if let Some(s) = t.rotation {
        insert("RotationAngleInDegrees", s.duration, true);
    }
    if let Some(v) = t.scale {
        insert("Scale", v.duration, false);
    }
    if let Some(v) = t.translation {
        // `Offset` collides with XAML layout; this should target `Translation`.
        insert("Offset", v.duration, false);
    }
    visual.set_implicit_animations(Some(&collection));
    Ok(())
}

pub(super) fn run_property_animation(
    ui: &bindings::UIElement,
    config: AnimationConfig,
) -> Result<()> {
    let visual = element_visual(ui)?;
    let compositor = visual.compositor();

    if let Some(opacity) = config.opacity {
        let animation = compositor.create_scalar_key_frame_animation();
        animation.set_duration(config.duration);
        let easing = easing_for(&compositor, config.easing);
        animation.insert_key_frame_with_easing(1.0, opacity as f32, &easing);
        visual.start_animation("Opacity", &animation);
    }
    if let Some(scale) = config.scale {
        let current = visual.scale();
        let scale = scale as f32;
        if current.x == scale && current.y == scale {
            return Ok(());
        }
        // Before first layout, ActualWidth/Height are 0 and CenterPoint is reused.
        if let Ok(fe) = ui.cast::<bindings::IFrameworkElement>() {
            let width = fe.ActualWidth().unwrap_or(0.0) as f32;
            let height = fe.ActualHeight().unwrap_or(0.0) as f32;
            if width > 0.0 && height > 0.0 {
                visual.set_center_point(windows_numerics::Vector3 {
                    x: width / 2.0,
                    y: height / 2.0,
                    z: 0.0,
                });
            } else {
                diag::warn(format_args!(
                    "animation: skipping CenterPoint - element not yet laid out"
                ));
            }
        }
        let animation = compositor.create_vector3_key_frame_animation();
        animation.set_duration(config.duration);
        let easing = easing_for(&compositor, config.easing);
        animation.insert_key_frame_with_easing(
            1.0,
            windows_numerics::Vector3 {
                x: scale,
                y: scale,
                z: current.z,
            },
            &easing,
        );
        visual.start_animation("Scale", &animation);
    }
    Ok(())
}

fn build_element_transition_animation(
    ui: &bindings::UIElement,
    config: AnimationConfig,
    is_enter: bool,
) -> Result<Option<bindings::ICompositionAnimationBase>> {
    if config.opacity.is_none() && config.scale.is_none() {
        return Ok(None);
    }

    let visual = element_visual(ui)?;
    let compositor = visual.compositor();
    let easing = easing_for(&compositor, config.easing);
    let group = compositor.create_animation_group();

    if let Some(opacity) = config.opacity {
        let animation = compositor.create_scalar_key_frame_animation();
        animation.set_duration(config.duration);
        animation.set_target("Opacity");
        if is_enter {
            animation.insert_key_frame_with_easing(0.0, 0.0, &easing);
        }
        animation.insert_key_frame_with_easing(1.0, opacity as f32, &easing);
        group.add(&animation);
    }

    if let Some(scale) = config.scale {
        let z = visual.scale().z;
        let animation = compositor.create_vector3_key_frame_animation();
        animation.set_duration(config.duration);
        animation.set_target("Scale");
        if is_enter {
            animation.insert_key_frame_with_easing(
                0.0,
                windows_numerics::Vector3 { x: 0.0, y: 0.0, z },
                &easing,
            );
        }
        let scale = scale as f32;
        animation.insert_key_frame_with_easing(
            1.0,
            windows_numerics::Vector3 {
                x: scale,
                y: scale,
                z,
            },
            &easing,
        );
        group.add(&animation);
    }

    Ok(Some(group.as_host().cast()?))
}

pub(super) fn apply_element_transitions(
    ui: &bindings::UIElement,
    enter: Option<AnimationConfig>,
    exit: Option<AnimationConfig>,
) -> Result<()> {
    let enter = enter
        .map(|config| build_element_transition_animation(ui, config, true))
        .transpose()?
        .flatten();
    let exit = exit
        .map(|config| build_element_transition_animation(ui, config, false))
        .transpose()?
        .flatten();

    bindings::ElementCompositionPreview::SetImplicitShowAnimation(ui, enter.as_ref())?;
    bindings::ElementCompositionPreview::SetImplicitHideAnimation(ui, exit.as_ref())
}
