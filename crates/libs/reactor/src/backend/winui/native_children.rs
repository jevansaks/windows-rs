use rustc_hash::FxHashMap;

use super::*;

#[derive(Default)]
pub(super) struct NativeChildren {
    logical: FxHashMap<ControlId, Vec<ControlId>>,
}

impl NativeChildren {
    pub(super) fn append(
        &mut self,
        controls: &FxHashMap<ControlId, Handle>,
        parent: ControlId,
        child: ControlId,
    ) {
        self.logical.entry(parent).or_default().push(child);
        if is_phantom_child(controls, child) {
            return;
        }

        let parent_handle = controls
            .get(&parent)
            .unwrap_or_else(|| panic!("WinUIBackend::append_child: unknown parent {parent}"));
        let child_handle = controls
            .get(&child)
            .unwrap_or_else(|| panic!("WinUIBackend::append_child: unknown child {child}"));
        let child = child_handle.as_ui_element();
        let container = classify_container(parent_handle).unwrap_or_else(|| {
            panic!(
                "WinUIBackend::append_child: {} ({parent}) is not a container",
                parent_handle.kind_name()
            )
        });
        container_append(&container, &child);
    }

    pub(super) fn remove(
        &mut self,
        controls: &FxHashMap<ControlId, Handle>,
        parent: ControlId,
        index: usize,
    ) {
        let phantom = self
            .logical
            .get(&parent)
            .and_then(|children| children.get(index).copied())
            .is_some_and(|child| is_phantom_child(controls, child));
        let visual_index = self.visual_index(controls, parent, index);
        if let Some(children) = self.logical.get_mut(&parent)
            && index < children.len()
        {
            children.remove(index);
        }
        if phantom {
            return;
        }

        let parent_handle = controls
            .get(&parent)
            .unwrap_or_else(|| panic!("WinUIBackend::remove_child: unknown parent {parent}"));
        let container = classify_container(parent_handle)
            .unwrap_or_else(|| panic!("WinUIBackend::remove_child: {parent} is not a container"));
        container_remove(&container, visual_index);
    }

    pub(super) fn replace(
        &mut self,
        controls: &FxHashMap<ControlId, Handle>,
        parent: ControlId,
        index: usize,
        new: ControlId,
    ) {
        let old = self
            .logical
            .get(&parent)
            .and_then(|children| children.get(index).copied());
        let old_phantom = old.is_some_and(|child| is_phantom_child(controls, child));
        let new_phantom = is_phantom_child(controls, new);
        let visual_index = self.visual_index(controls, parent, index);
        if let Some(children) = self.logical.get_mut(&parent)
            && index < children.len()
        {
            children[index] = new;
        }

        match (old_phantom, new_phantom) {
            (true, true) => {}
            (false, true) => visual_remove_at(controls, parent, visual_index),
            (true, false) => visual_insert_at(controls, parent, visual_index, new),
            (false, false) => visual_set_at(controls, parent, visual_index, new),
        }
    }

    pub(super) fn move_child(
        &mut self,
        controls: &FxHashMap<ControlId, Handle>,
        parent: ControlId,
        from: usize,
        to: usize,
    ) {
        if from == to {
            return;
        }

        let visual_from = self.visual_index(controls, parent, from);
        let visual_to = self.visual_index(controls, parent, to);
        let moved_phantom = self
            .logical
            .get(&parent)
            .and_then(|children| children.get(from).copied())
            .is_some_and(|child| is_phantom_child(controls, child));
        if let Some(children) = self.logical.get_mut(&parent)
            && from < children.len()
            && to < children.len()
        {
            let child = children.remove(from);
            children.insert(to, child);
        }
        if moved_phantom || visual_from == visual_to {
            return;
        }

        let parent_handle = controls
            .get(&parent)
            .unwrap_or_else(|| panic!("WinUIBackend::move_child: unknown parent {parent}"));
        let container = classify_container(parent_handle)
            .unwrap_or_else(|| panic!("WinUIBackend::move_child: {parent} is not a container"));
        container_move(&container, visual_from, visual_to);
    }

    pub(super) fn insert(
        &mut self,
        controls: &FxHashMap<ControlId, Handle>,
        parent: ControlId,
        index: usize,
        child: ControlId,
    ) {
        let visual_index = self.visual_index(controls, parent, index);
        let children = self.logical.entry(parent).or_default();
        let index = index.min(children.len());
        children.insert(index, child);
        if is_phantom_child(controls, child) {
            return;
        }

        visual_insert_at(controls, parent, visual_index, child);
    }

    pub(super) fn remove_control(&mut self, id: ControlId) {
        self.logical.remove(&id);
        for children in self.logical.values_mut() {
            children.retain(|child| *child != id);
        }
    }

    fn visual_index(
        &self,
        controls: &FxHashMap<ControlId, Handle>,
        parent: ControlId,
        logical_index: usize,
    ) -> usize {
        let Some(children) = self.logical.get(&parent) else {
            return logical_index;
        };
        children
            .iter()
            .take(logical_index)
            .filter(|child| !is_phantom_child(controls, **child))
            .count()
    }
}

fn is_phantom_child(controls: &FxHashMap<ControlId, Handle>, id: ControlId) -> bool {
    matches!(controls.get(&id), Some(Handle::ContentDialog(_)))
}

fn visual_insert_at(
    controls: &FxHashMap<ControlId, Handle>,
    parent: ControlId,
    visual_index: usize,
    child: ControlId,
) {
    let parent_handle = controls
        .get(&parent)
        .unwrap_or_else(|| panic!("WinUIBackend::visual_insert_at: unknown parent {parent}"));
    let child_handle = controls
        .get(&child)
        .unwrap_or_else(|| panic!("WinUIBackend::visual_insert_at: unknown child {child}"));
    let child = child_handle.as_ui_element();
    let container = classify_container(parent_handle)
        .unwrap_or_else(|| panic!("WinUIBackend::visual_insert_at: {parent} is not a container"));
    container_insert(&container, visual_index, &child);
}

fn visual_remove_at(
    controls: &FxHashMap<ControlId, Handle>,
    parent: ControlId,
    visual_index: usize,
) {
    let parent_handle = controls
        .get(&parent)
        .unwrap_or_else(|| panic!("WinUIBackend::visual_remove_at: unknown parent {parent}"));
    let container = classify_container(parent_handle)
        .unwrap_or_else(|| panic!("WinUIBackend::visual_remove_at: {parent} is not a container"));
    container_remove(&container, visual_index);
}

fn visual_set_at(
    controls: &FxHashMap<ControlId, Handle>,
    parent: ControlId,
    visual_index: usize,
    new: ControlId,
) {
    let parent_handle = controls
        .get(&parent)
        .unwrap_or_else(|| panic!("WinUIBackend::visual_set_at: unknown parent {parent}"));
    let new_handle = controls
        .get(&new)
        .unwrap_or_else(|| panic!("WinUIBackend::visual_set_at: unknown new {new}"));
    let child = new_handle.as_ui_element();
    let container = classify_container(parent_handle)
        .unwrap_or_else(|| panic!("WinUIBackend::visual_set_at: {parent} is not a container"));
    container_set(&container, visual_index, &child);
}

enum ContainerChildren<'a> {
    Panel(bindings::UIElementCollection),
    SingleChild(&'a Handle),
    ContentControl(bindings::IContentControl),
    DirectContent(&'a Handle),
    InspectableVector(windows_collections::IVector<windows_core::IInspectable>),
}

fn classify_container(handle: &Handle) -> Option<ContainerChildren<'_>> {
    match handle {
        Handle::StackPanel(value) => Some(ContainerChildren::Panel(
            value.cast::<bindings::IPanel>().ok()?.Children().ok()?,
        )),
        Handle::Grid(value) => Some(ContainerChildren::Panel(
            value.cast::<bindings::IPanel>().ok()?.Children().ok()?,
        )),
        Handle::Canvas(value) => Some(ContainerChildren::Panel(
            value.cast::<bindings::IPanel>().ok()?.Children().ok()?,
        )),
        Handle::RelativePanel(value) => Some(ContainerChildren::Panel(
            value.cast::<bindings::IPanel>().ok()?.Children().ok()?,
        )),
        Handle::Border(_) | Handle::Viewbox(_) => Some(ContainerChildren::SingleChild(handle)),
        Handle::ScrollViewer(value) => Some(ContainerChildren::ContentControl(value.cast().ok()?)),
        Handle::Expander(value) => Some(ContainerChildren::ContentControl(value.cast().ok()?)),
        Handle::TabViewItem(value) => Some(ContainerChildren::ContentControl(value.cast().ok()?)),
        Handle::NavigationView(value) => {
            Some(ContainerChildren::ContentControl(value.cast().ok()?))
        }
        Handle::PivotItem(value) => Some(ContainerChildren::ContentControl(value.cast().ok()?)),
        Handle::ScrollView(_) | Handle::SplitView(_) => {
            Some(ContainerChildren::DirectContent(handle))
        }
        Handle::TabView(value) => {
            Some(ContainerChildren::InspectableVector(value.TabItems().ok()?))
        }
        Handle::Pivot(value) => Some(ContainerChildren::InspectableVector(
            value
                .cast::<bindings::IItemsControl>()
                .ok()?
                .Items()
                .ok()?
                .cast()
                .ok()?,
        )),
        _ => None,
    }
}

fn container_append(container: &ContainerChildren<'_>, child: &bindings::UIElement) {
    match container {
        ContainerChildren::Panel(children) => children.Append(child).unwrap(),
        ContainerChildren::SingleChild(handle) => put_single_child(handle, Some(child)),
        ContainerChildren::ContentControl(control) => control.SetContent(child).unwrap(),
        ContainerChildren::DirectContent(handle) => put_direct_content(handle, Some(child)),
        ContainerChildren::InspectableVector(children) => {
            let child: windows_core::IInspectable = child.cast().unwrap();
            children.Append(&child).unwrap();
        }
    }
}

fn container_insert(container: &ContainerChildren<'_>, index: usize, child: &bindings::UIElement) {
    match container {
        ContainerChildren::Panel(children) => children.InsertAt(index as u32, child).unwrap(),
        ContainerChildren::SingleChild(handle) => put_single_child(handle, Some(child)),
        ContainerChildren::ContentControl(control) => control.SetContent(child).unwrap(),
        ContainerChildren::DirectContent(handle) => put_direct_content(handle, Some(child)),
        ContainerChildren::InspectableVector(children) => {
            let child: windows_core::IInspectable = child.cast().unwrap();
            children.InsertAt(index as u32, &child).unwrap();
        }
    }
}

fn container_set(container: &ContainerChildren<'_>, index: usize, child: &bindings::UIElement) {
    match container {
        ContainerChildren::Panel(children) => children.SetAt(index as u32, child).unwrap(),
        ContainerChildren::SingleChild(handle) => put_single_child(handle, Some(child)),
        ContainerChildren::ContentControl(control) => control.SetContent(child).unwrap(),
        ContainerChildren::DirectContent(handle) => put_direct_content(handle, Some(child)),
        ContainerChildren::InspectableVector(children) => {
            let child: windows_core::IInspectable = child.cast().unwrap();
            children.SetAt(index as u32, &child).unwrap();
        }
    }
}

fn container_remove(container: &ContainerChildren<'_>, index: usize) {
    match container {
        ContainerChildren::Panel(children) => children.RemoveAt(index as u32).unwrap(),
        ContainerChildren::SingleChild(handle) => {
            debug_assert_eq!(index, 0);
            put_single_child(handle, None);
        }
        ContainerChildren::ContentControl(control) => {
            debug_assert_eq!(index, 0);
            control
                .SetContent(None::<&windows_core::IInspectable>)
                .unwrap();
        }
        ContainerChildren::DirectContent(handle) => {
            debug_assert_eq!(index, 0);
            put_direct_content(handle, None);
        }
        ContainerChildren::InspectableVector(children) => children.RemoveAt(index as u32).unwrap(),
    }
}

fn container_move(container: &ContainerChildren<'_>, from: usize, to: usize) {
    match container {
        ContainerChildren::Panel(children) => {
            let child = children.GetAt(from as u32).unwrap();
            children.RemoveAt(from as u32).unwrap();
            children.InsertAt(to as u32, &child).unwrap();
        }
        ContainerChildren::SingleChild(_)
        | ContainerChildren::ContentControl(_)
        | ContainerChildren::DirectContent(_) => {}
        ContainerChildren::InspectableVector(children) => {
            let child = children.GetAt(from as u32).unwrap();
            children.RemoveAt(from as u32).unwrap();
            children.InsertAt(to as u32, &child).unwrap();
        }
    }
}

fn put_single_child(handle: &Handle, child: Option<&bindings::UIElement>) {
    match handle {
        Handle::Border(value) => value.SetChild(child).unwrap(),
        Handle::Viewbox(value) => value.SetChild(child).unwrap(),
        _ => unreachable!(),
    }
}

fn put_direct_content(handle: &Handle, child: Option<&bindings::UIElement>) {
    match handle {
        Handle::ScrollView(value) => value.SetContent(child).unwrap(),
        Handle::SplitView(value) => value.SetContent(child).unwrap(),
        _ => unreachable!(),
    }
}
