use rustc_hash::FxHashMap;

use super::*;

/// Owns event callback and revoker lifetimes. Event dispatch remains in the backend and generated
/// attachment code.
#[derive(Default)]
pub(super) struct EventState {
    revokers: FxHashMap<(ControlId, RevokerOwner), Vec<windows_core::EventRevoker>>,
    property_observers: FxHashMap<(ControlId, Event), PropertyObserver>,
    selection_revokers: FxHashMap<ControlId, windows_core::EventRevoker>,
    pointer_revokers: FxHashMap<ControlId, PointerRevokerSet>,
    drag_revokers: FxHashMap<ControlId, DragRevokerSet>,
    menu_handlers: FxHashMap<ControlId, EventHandler>,
    command_bar_handlers: FxHashMap<ControlId, EventHandler>,
    command_bar_flyout_handlers: FxHashMap<ControlId, EventHandler>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum RevokerOwner {
    Event(Event),
    MenuItems,
    CommandBarPrimary,
    CommandBarSecondary,
    CommandBarFlyout,
}

impl EventState {
    pub(super) fn insert_revokers(
        &mut self,
        id: ControlId,
        event: Event,
        revokers: Vec<windows_core::EventRevoker>,
    ) {
        self.insert_owned_revokers(id, RevokerOwner::Event(event), revokers);
    }

    pub(super) fn insert_owned_revokers(
        &mut self,
        id: ControlId,
        owner: RevokerOwner,
        revokers: Vec<windows_core::EventRevoker>,
    ) {
        if revokers.is_empty() {
            self.revokers.remove(&(id, owner));
        } else {
            self.revokers.insert((id, owner), revokers);
        }
    }

    pub(super) fn observe_navigation_state(
        &mut self,
        id: ControlId,
        event: Event,
        navigation: &bindings::NavigationView,
        handler: EventHandler,
    ) -> Result<()> {
        let property = match event {
            Event::NavigationPaneOpenChanged => bindings::NavigationView::IsPaneOpenProperty()?,
            Event::NavigationDisplayModeChanged => bindings::NavigationView::DisplayModeProperty()?,
            _ => unreachable!(),
        };
        let object = navigation.cast::<bindings::DependencyObject>()?;
        let navigation = navigation.clone();
        let callback = bindings::DependencyPropertyChangedCallback::new(
            move |_sender, _property| match event {
                Event::NavigationPaneOpenChanged => match navigation.IsPaneOpen() {
                    Ok(open) => handler.invoke_bool(open),
                    Err(error) => diag::warn(format_args!(
                        "failed to read NavigationView.IsPaneOpen for {id}: {error:?}"
                    )),
                },
                Event::NavigationDisplayModeChanged => match navigation.DisplayMode() {
                    Ok(mode) => handler.invoke_navigation_display_mode(mode),
                    Err(error) => diag::warn(format_args!(
                        "failed to read NavigationView.DisplayMode for {id}: {error:?}"
                    )),
                },
                _ => unreachable!(),
            },
        );
        let token = object.RegisterPropertyChangedCallback(&property, &callback)?;
        self.property_observers.insert(
            (id, event),
            PropertyObserver {
                object,
                property,
                token,
            },
        );
        Ok(())
    }

    pub(super) fn detach(&mut self, id: ControlId, event: Event) {
        self.revokers.remove(&(id, RevokerOwner::Event(event)));
        self.property_observers.remove(&(id, event));
    }

    pub(super) fn replace_selection(&mut self, id: ControlId, revoker: windows_core::EventRevoker) {
        self.selection_revokers.insert(id, revoker);
    }

    pub(super) fn clear_selection(&mut self, id: ControlId) {
        self.selection_revokers.remove(&id);
    }

    pub(super) fn take_pointer(&mut self, id: ControlId) -> Option<PointerRevokerSet> {
        self.pointer_revokers.remove(&id)
    }

    pub(super) fn set_pointer(&mut self, id: ControlId, revokers: PointerRevokerSet) {
        self.pointer_revokers.insert(id, revokers);
    }

    pub(super) fn take_drag(&mut self, id: ControlId) -> Option<DragRevokerSet> {
        self.drag_revokers.remove(&id)
    }

    pub(super) fn set_drag(&mut self, id: ControlId, revokers: DragRevokerSet) {
        self.drag_revokers.insert(id, revokers);
    }

    pub(super) fn set_menu_handler(&mut self, id: ControlId, handler: EventHandler) {
        self.menu_handlers.insert(id, handler);
    }

    pub(super) fn menu_handler(&self, id: ControlId) -> Option<EventHandler> {
        self.menu_handlers.get(&id).cloned()
    }

    pub(super) fn clear_menu_handler(&mut self, id: ControlId) {
        self.menu_handlers.remove(&id);
        self.revokers.remove(&(id, RevokerOwner::MenuItems));
    }

    pub(super) fn set_command_bar_handler(&mut self, id: ControlId, handler: EventHandler) {
        self.command_bar_handlers.insert(id, handler);
    }

    pub(super) fn command_bar_handler(&self, id: ControlId) -> Option<EventHandler> {
        self.command_bar_handlers.get(&id).cloned()
    }

    pub(super) fn clear_command_bar_handler(&mut self, id: ControlId) {
        self.command_bar_handlers.remove(&id);
        self.revokers.remove(&(id, RevokerOwner::CommandBarPrimary));
        self.revokers
            .remove(&(id, RevokerOwner::CommandBarSecondary));
    }

    pub(super) fn set_command_bar_flyout_handler(&mut self, id: ControlId, handler: EventHandler) {
        self.command_bar_flyout_handlers.insert(id, handler);
    }

    pub(super) fn command_bar_flyout_handler(&self, id: ControlId) -> Option<EventHandler> {
        self.command_bar_flyout_handlers.get(&id).cloned()
    }

    pub(super) fn clear_command_bar_flyout_handler(&mut self, id: ControlId) {
        self.command_bar_flyout_handlers.remove(&id);
        self.revokers.remove(&(id, RevokerOwner::CommandBarFlyout));
    }

    pub(super) fn remove(&mut self, id: ControlId) -> bool {
        let captured = self
            .pointer_revokers
            .remove(&id)
            .is_some_and(|revokers| revokers.capture_on_press);
        self.drag_revokers.remove(&id);
        self.selection_revokers.remove(&id);
        self.revokers.retain(|(control, _), _| *control != id);
        self.property_observers
            .retain(|(control, _), _| *control != id);
        self.menu_handlers.remove(&id);
        self.command_bar_handlers.remove(&id);
        self.command_bar_flyout_handlers.remove(&id);
        captured
    }
}

pub(super) fn wire_menu_bar_clicks(
    menu_bar: &bindings::MenuBar,
    handler: &EventHandler,
) -> Vec<windows_core::EventRevoker> {
    let mut revokers = Vec::new();
    let Ok(items) = menu_bar.Items() else {
        return revokers;
    };
    for item in &items {
        if let Ok(flyout_items) = item.Items() {
            wire_flyout_items_click(&flyout_items, handler, &mut revokers);
        }
    }
    revokers
}

pub(super) fn wire_flyout_clicks(
    flyout: &bindings::MenuFlyout,
    handler: &EventHandler,
) -> Vec<windows_core::EventRevoker> {
    let mut revokers = Vec::new();
    if let Ok(items) = flyout.Items() {
        wire_flyout_items_click(&items, handler, &mut revokers);
    }
    revokers
}

fn wire_flyout_items_click(
    items: &windows_collections::IVector<bindings::MenuFlyoutItemBase>,
    handler: &EventHandler,
    revokers: &mut Vec<windows_core::EventRevoker>,
) {
    for base in items {
        if let Ok(item) = base.cast::<bindings::MenuFlyoutItem>() {
            let text = item.Text().unwrap_or_default().clone();
            let handler = handler.clone();
            if let Ok(revoker) = item.Click(move |_sender, _args| {
                handler.invoke_string(text.clone());
            }) {
                revokers.push(revoker);
            }
        } else if let Ok(submenu) = base.cast::<bindings::MenuFlyoutSubItem>()
            && let Ok(submenu_items) = submenu.Items()
        {
            wire_flyout_items_click(&submenu_items, handler, revokers);
        }
    }
}

pub(super) fn wire_command_bar_clicks(
    commands: &windows_collections::IObservableVector<bindings::ICommandBarElement>,
    handler: &EventHandler,
) -> Vec<windows_core::EventRevoker> {
    let mut revokers = Vec::new();
    for element in commands {
        if let Ok(button) = element.cast::<bindings::AppBarButton>() {
            let label = button.Label().unwrap_or_default().clone();
            let handler = handler.clone();
            if let Ok(revoker) = button.cast::<bindings::ButtonBase>().and_then(|button| {
                button.Click(move |_sender, _args| {
                    handler.invoke_string(label.clone());
                })
            }) {
                revokers.push(revoker);
            }
        }
    }
    revokers
}

pub(super) struct PropertyObserver {
    object: bindings::DependencyObject,
    property: bindings::DependencyProperty,
    token: i64,
}

impl Drop for PropertyObserver {
    fn drop(&mut self) {
        diag::dropped(
            self.object
                .UnregisterPropertyChangedCallback(&self.property, self.token),
        );
    }
}

#[derive(Default)]
pub(super) struct PointerRevokerSet {
    pub(super) tapped: Option<windows_core::EventRevoker>,
    pub(super) right_tapped: Option<windows_core::EventRevoker>,
    pub(super) pressed: Option<windows_core::EventRevoker>,
    pub(super) released: Option<windows_core::EventRevoker>,
    pub(super) moved: Option<windows_core::EventRevoker>,
    pub(super) entered: Option<windows_core::EventRevoker>,
    pub(super) exited: Option<windows_core::EventRevoker>,
    pub(super) capture_lost: Option<windows_core::EventRevoker>,
    pub(super) canceled: Option<windows_core::EventRevoker>,
    pub(super) capture_on_press: bool,
}

#[derive(Default)]
pub(super) struct DragRevokerSet {
    pub(super) enter: Option<windows_core::EventRevoker>,
    pub(super) leave: Option<windows_core::EventRevoker>,
    pub(super) over: Option<windows_core::EventRevoker>,
    pub(super) drop: Option<windows_core::EventRevoker>,
}
