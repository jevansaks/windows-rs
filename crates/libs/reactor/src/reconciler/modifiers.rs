use super::*;

impl<B: Backend + 'static> Reconciler<B> {
    pub(super) fn apply_modifiers(&mut self, id: ControlId, mods: &Modifiers) {
        if mods.is_empty() {
            return;
        }
        if let Some(v) = mods.margin {
            self.backend
                .set_prop(id, Prop::Margin, &PropValue::Thickness(v));
        }
        if let Some(v) = mods.padding {
            self.backend
                .set_prop(id, Prop::Padding, &PropValue::Thickness(v));
        }
        if let Some(v) = mods.width {
            self.backend.set_prop(id, Prop::Width, &PropValue::F64(v));
        }
        if let Some(v) = mods.height {
            self.backend.set_prop(id, Prop::Height, &PropValue::F64(v));
        }
        if let Some(v) = mods.min_width {
            self.backend
                .set_prop(id, Prop::MinWidth, &PropValue::F64(v));
        }
        if let Some(v) = mods.max_width {
            self.backend
                .set_prop(id, Prop::MaxWidth, &PropValue::F64(v));
        }
        if let Some(v) = mods.min_height {
            self.backend
                .set_prop(id, Prop::MinHeight, &PropValue::F64(v));
        }
        if let Some(v) = mods.max_height {
            self.backend
                .set_prop(id, Prop::MaxHeight, &PropValue::F64(v));
        }
        if let Some(v) = mods.horizontal_alignment {
            self.backend
                .set_prop(id, Prop::HorizontalAlignment, &PropValue::I32(v.0));
        }
        if let Some(v) = mods.vertical_alignment {
            self.backend
                .set_prop(id, Prop::VerticalAlignment, &PropValue::I32(v.0));
        }
        if let Some(v) = mods.opacity {
            self.backend.set_prop(id, Prop::Opacity, &PropValue::F64(v));
        }
        if let Some(v) = &mods.background {
            self.backend
                .set_prop(id, Prop::Background, &PropValue::Color(*v));
        }
        if let Some(v) = &mods.foreground {
            self.backend
                .set_prop(id, Prop::Foreground, &PropValue::Color(*v));
        }
        if let Some(v) = &mods.font_family {
            self.backend
                .set_prop(id, Prop::FontFamily, &PropValue::Str(v.clone()));
        }
        if let Some(v) = mods.font_size {
            self.backend
                .set_prop(id, Prop::FontSize, &PropValue::F64(v));
        }

        if let Some(v) = mods.allow_drop {
            self.backend
                .set_prop(id, Prop::AllowDrop, &PropValue::Bool(v));
        }

        self.apply_theme_bindings_for(id, mods);
        self.apply_animations_for(id, mods);
        self.apply_accessibility_for(id, mods);
        self.apply_keyboard_accelerators_for(id, mods);
        self.apply_tooltip_for(id, mods);
        self.apply_pointer_handlers_for(id, mods);
        self.apply_drag_handlers_for(id, mods);

        if let Some(p) = mods.grid {
            self.apply_grid_placement(id, p);
        }

        if !mods.resources.is_empty() {
            self.backend.set_prop(
                id,
                Prop::Resources,
                &PropValue::Resources(mods.resources.clone()),
            );
        }
    }

    fn apply_tooltip_for(&mut self, id: ControlId, mods: &Modifiers) {
        let Some(tt) = mods.tooltip.as_deref() else {
            return;
        };
        self.backend.set_tooltip(id, Some(tt));
    }

    fn apply_pointer_handlers_for(&mut self, id: ControlId, mods: &Modifiers) {
        let Some(ph) = mods.pointer_handlers.as_deref() else {
            return;
        };
        if ph.is_empty() {
            return;
        }
        self.backend.set_pointer_handlers(id, Some(ph));
    }

    fn apply_drag_handlers_for(&mut self, id: ControlId, mods: &Modifiers) {
        let Some(dh) = mods.drag_handlers.as_deref() else {
            return;
        };
        if dh.is_empty() {
            return;
        }
        self.backend.set_drag_handlers(id, Some(dh));
    }

    fn apply_accessibility_for(&mut self, id: ControlId, mods: &Modifiers) {
        let Some(acc) = mods.accessibility.as_deref() else {
            return;
        };
        if acc.is_empty() {
            return;
        }
        self.backend.set_accessibility(id, acc);
    }

    fn apply_keyboard_accelerators_for(&mut self, id: ControlId, mods: &Modifiers) {
        if mods.keyboard_accelerators.is_empty() {
            return;
        }
        self.backend
            .set_keyboard_accelerators(id, &mods.keyboard_accelerators);
    }

    fn apply_animations_for(&mut self, id: ControlId, mods: &Modifiers) {
        let Some(anim) = mods.animations.as_deref() else {
            return;
        };
        if anim.is_empty() {
            return;
        }
        if let Some(it) = anim.implicit_transitions
            && !it.is_empty()
        {
            self.backend.set_implicit_transitions(id, Some(it));
        }
        if let Some(la) = anim.layout_animation {
            self.backend.set_layout_animation(id, Some(la));
        }

        let enter = match anim.property_animation {
            None => anim.enter_transition,
            Some(_) => None,
        };
        if enter.is_some() || anim.exit_transition.is_some() {
            self.backend
                .set_element_transitions(id, enter, anim.exit_transition);
        }

        if let Some(p) = anim.property_animation {
            self.backend.run_property_animation(id, Some(p));
        }
    }

    fn diff_animations_for(
        &mut self,
        id: ControlId,
        old: Option<&AnimationModifiers>,
        new: Option<&AnimationModifiers>,
    ) {
        let old_it = old.and_then(|a| a.implicit_transitions);
        let new_it = new.and_then(|a| a.implicit_transitions);
        if old_it != new_it {
            self.backend
                .set_implicit_transitions(id, new_it.filter(|t| !t.is_empty()));
        }
        let old_la = old.and_then(|a| a.layout_animation);
        let new_la = new.and_then(|a| a.layout_animation);
        if old_la != new_la {
            self.backend.set_layout_animation(id, new_la);
        }

        let old_pa = old.and_then(|a| a.property_animation);
        let new_pa = new.and_then(|a| a.property_animation);
        if old_pa != new_pa {
            self.backend.run_property_animation(id, new_pa);
        }

        let old_enter = old.and_then(|a| match a.property_animation {
            None => a.enter_transition,
            Some(_) => None,
        });
        let new_enter = new.and_then(|a| match a.property_animation {
            None => a.enter_transition,
            Some(_) => None,
        });
        let old_exit = old.and_then(|a| a.exit_transition);
        let new_exit = new.and_then(|a| a.exit_transition);
        if old_enter != new_enter || old_exit != new_exit {
            self.backend
                .set_element_transitions(id, new_enter, new_exit);
        }
    }

    fn apply_theme_bindings_for(&mut self, id: ControlId, mods: &Modifiers) {
        let Some(map) = mods.theme_bindings.as_deref() else {
            return;
        };
        if map.is_empty() {
            return;
        }
        let Some(kind) = self.tree.kind(id) else {
            return;
        };
        let bindings: Vec<(Prop, ThemeRef)> = map.iter().map(|(p, t)| (*p, t.clone())).collect();
        self.backend.set_theme_bindings(id, kind, &bindings);
    }

    pub(super) fn diff_modifiers(&mut self, id: ControlId, old: &Modifiers, new: &Modifiers) {
        self.diff_opt_copy(
            id,
            Prop::Margin,
            old.margin,
            new.margin,
            PropValue::Thickness,
        );
        self.diff_opt_copy(
            id,
            Prop::Padding,
            old.padding,
            new.padding,
            PropValue::Thickness,
        );
        self.diff_opt_f64(id, Prop::Width, old.width, new.width);
        self.diff_opt_f64(id, Prop::Height, old.height, new.height);
        self.diff_opt_f64(id, Prop::MinWidth, old.min_width, new.min_width);
        self.diff_opt_f64(id, Prop::MaxWidth, old.max_width, new.max_width);
        self.diff_opt_f64(id, Prop::MinHeight, old.min_height, new.min_height);
        self.diff_opt_f64(id, Prop::MaxHeight, old.max_height, new.max_height);
        self.diff_opt_copy(
            id,
            Prop::HorizontalAlignment,
            old.horizontal_alignment,
            new.horizontal_alignment,
            |v: HorizontalAlignment| PropValue::I32(v.0),
        );
        self.diff_opt_copy(
            id,
            Prop::VerticalAlignment,
            old.vertical_alignment,
            new.vertical_alignment,
            |v: VerticalAlignment| PropValue::I32(v.0),
        );
        self.diff_opt_f64(id, Prop::Opacity, old.opacity, new.opacity);
        self.diff_opt_clone(
            id,
            Prop::Background,
            &old.background,
            &new.background,
            PropValue::Color,
        );
        self.diff_opt_clone(
            id,
            Prop::Foreground,
            &old.foreground,
            &new.foreground,
            PropValue::Color,
        );
        self.diff_opt_clone(
            id,
            Prop::FontFamily,
            &old.font_family,
            &new.font_family,
            PropValue::Str,
        );
        self.diff_opt_f64(id, Prop::FontSize, old.font_size, new.font_size);

        if old.theme_bindings != new.theme_bindings {
            let kind = self.tree.kind(id);
            if let Some(kind) = kind {
                let bindings: Vec<(Prop, ThemeRef)> = new
                    .theme_bindings
                    .as_deref()
                    .map(|m| m.iter().map(|(p, t)| (*p, t.clone())).collect())
                    .unwrap_or_default();
                self.backend.set_theme_bindings(id, kind, &bindings);
            }
        }

        let old_anim = old.animations.as_deref();
        let new_anim = new.animations.as_deref();
        if old_anim != new_anim {
            self.diff_animations_for(id, old_anim, new_anim);
        }

        let old_acc = old.accessibility.as_deref();
        let new_acc = new.accessibility.as_deref();
        if old_acc != new_acc {
            static EMPTY: AccessibilityModifiers = AccessibilityModifiers {
                automation_name: None,
                automation_id: None,
                help_text: None,
                live_setting: None,
                heading_level: None,
            };
            let new_acc = new_acc.unwrap_or(&EMPTY);
            self.backend.set_accessibility(id, new_acc);
        }

        let old_ka = &old.keyboard_accelerators;
        let new_ka = &new.keyboard_accelerators;
        if old_ka != new_ka {
            self.backend.set_keyboard_accelerators(id, new_ka);
        }

        // ToolTipService survives re-renders, so clear Some->None explicitly.
        let old_tt = old.tooltip.as_deref();
        let new_tt = new.tooltip.as_deref();
        if old_tt != new_tt {
            self.backend.set_tooltip(id, new_tt);
        }

        // Clear Some->None so event tokens are dropped.
        let old_ph = old.pointer_handlers.as_deref();
        let new_ph = new.pointer_handlers.as_deref();
        if old_ph != new_ph {
            let new_ph = new_ph.filter(|p| !p.is_empty());
            self.backend.set_pointer_handlers(id, new_ph);
        }

        if old.allow_drop != new.allow_drop {
            self.backend.set_prop(
                id,
                Prop::AllowDrop,
                &PropValue::Bool(new.allow_drop.unwrap_or(false)),
            );
        }

        let old_dh = old.drag_handlers.as_deref();
        let new_dh = new.drag_handlers.as_deref();
        if old_dh != new_dh {
            let new_dh = new_dh.filter(|d| !d.is_empty());
            self.backend.set_drag_handlers(id, new_dh);
        }

        // Emit all grid props on change so stale values are cleared.
        if old.grid != new.grid {
            self.apply_grid_placement_full(id, new.grid.unwrap_or_default());
        }

        if old.resources != new.resources {
            self.backend.set_prop(
                id,
                Prop::Resources,
                &PropValue::Resources(new.resources.clone()),
            );
        }
    }
}
