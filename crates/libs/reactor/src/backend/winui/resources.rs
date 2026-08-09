use rustc_hash::{FxHashMap, FxHashSet};

use super::*;

#[derive(Default)]
pub(super) struct ResourceState {
    theme_bindings: FxHashMap<ControlId, Vec<(Prop, ThemeRef)>>,
    local_keys: FxHashMap<ControlId, FxHashSet<String>>,
}

impl ResourceState {
    pub(super) fn set_local(
        &mut self,
        id: ControlId,
        handle: &Handle,
        resources: &HashMap<String, ResourceValue>,
    ) -> Result<()> {
        let dictionary = handle.as_framework_element().Resources()?;
        let map = dictionary.cast::<windows_collections::IMap<
            windows_core::IInspectable,
            windows_core::IInspectable,
        >>()?;

        let previous = self.local_keys.get(&id).cloned().unwrap_or_default();
        for key in previous {
            if resources.contains_key(&key) {
                continue;
            }
            let key = windows_reference::IReference::from(key.as_str());
            if map.HasKey(&key)? {
                map.Remove(&key)?;
            }
        }

        for (key, value) in resources {
            let key = windows_reference::IReference::from(key.as_str());
            let value: windows_core::IInspectable = match value {
                ResourceValue::String(value) => {
                    windows_reference::IReference::from(value.as_str()).cast()?
                }
                ResourceValue::SolidColorBrush(color) => solid_brush(*color)?.cast()?,
                ResourceValue::F64(value) => windows_reference::IReference::from(*value).cast()?,
                ResourceValue::Thickness(value) => {
                    windows_reference::IReference::from(*value).cast()?
                }
                ResourceValue::CornerRadius(value) => {
                    windows_reference::IReference::from(bindings::CornerRadius {
                        top_left: value.top_left,
                        top_right: value.top_right,
                        bottom_right: value.bottom_right,
                        bottom_left: value.bottom_left,
                    })
                    .cast()?
                }
            };
            map.Insert(&key, &value)?;
        }

        if resources.is_empty() {
            self.local_keys.remove(&id);
        } else {
            self.local_keys
                .insert(id, resources.keys().cloned().collect());
        }
        Ok(())
    }

    pub(super) fn set_theme_bindings(
        &mut self,
        id: ControlId,
        handle: Option<&Handle>,
        bindings: &[(Prop, ThemeRef)],
    ) {
        if bindings.is_empty() {
            self.theme_bindings.remove(&id);
            if let Some(handle) = handle
                && let Some((_, element)) = style_target_for_handle(handle)
            {
                diag::dropped(element.SetStyle(None));
            }
            return;
        }

        self.theme_bindings.insert(id, bindings.to_vec());
        if let Some(handle) = handle {
            apply_theme_resource_style(handle, bindings);
        }
    }

    pub(super) fn refresh_theme(&self, controls: &FxHashMap<ControlId, Handle>) {
        for (id, bindings) in &self.theme_bindings {
            let Some(handle) = controls.get(id) else {
                continue;
            };
            apply_theme_resource_style(handle, bindings);
        }
    }

    pub(super) fn remove(&mut self, id: ControlId) {
        self.theme_bindings.remove(&id);
        self.local_keys.remove(&id);
    }
}

fn apply_theme_resource_style(handle: &Handle, bindings: &[(Prop, ThemeRef)]) {
    let Some((target_type, element)) = style_target_for_handle(handle) else {
        return;
    };

    let mut setters = String::new();
    for (prop, theme_ref) in bindings {
        let property = match prop {
            Prop::Background => "Background",
            Prop::Foreground => "Foreground",
            Prop::BorderBrush => "BorderBrush",
            _ => continue,
        };
        let resource_key = theme_ref.resource_key();
        setters.push_str(&format!(
            "<Setter Property='{property}' Value='{{ThemeResource {resource_key}}}'/>"
        ));
    }

    if setters.is_empty() {
        diag::dropped(element.SetStyle(None));
        return;
    }

    let xaml = format!(
        "<Style xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation' TargetType='{target_type}'>{setters}</Style>"
    );

    match bindings::XamlReader::Load(&xaml) {
        Ok(object) => {
            if let Ok(style) = object.cast::<bindings::Style>() {
                // Force WinUI to re-resolve {ThemeResource} values.
                diag::dropped(element.SetStyle(None));
                diag::dropped(element.SetStyle(&style));
            }
        }
        Err(error) => {
            diag::warn(format_args!(
                "ThemeStyle: XamlReader::Load failed: {error:?} xaml={xaml}"
            ));
        }
    }
}

fn style_target_for_handle(handle: &Handle) -> Option<(&'static str, bindings::IFrameworkElement)> {
    match handle {
        Handle::Border(value) => value.cast().ok().map(|element| ("Border", element)),
        Handle::StackPanel(value) => value.cast().ok().map(|element| ("StackPanel", element)),
        Handle::Grid(value) => value.cast().ok().map(|element| ("Grid", element)),
        Handle::Button(value) => value.cast().ok().map(|element| ("Button", element)),
        Handle::TextBox(value) => value.cast().ok().map(|element| ("TextBox", element)),
        Handle::TextBlock(value) => value.cast().ok().map(|element| ("TextBlock", element)),
        Handle::Canvas(value) => value.cast().ok().map(|element| ("Canvas", element)),
        _ => None,
    }
}
