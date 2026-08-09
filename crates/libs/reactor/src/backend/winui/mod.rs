use std::cell::RefCell;

use rustc_hash::FxHashMap;

use super::*;

mod animation;
mod convert;
mod diag;
mod events;
mod generated_attach_event;
mod generated_set_prop;
mod native_children;
mod properties;
mod resources;
use animation::*;
use convert::*;
use events::{DragRevokerSet, PointerRevokerSet, RevokerOwner};
use native_children::NativeChildren;
use resources::ResourceState;

/// Keeps `Handle`, `ControlKind` construction, and diagnostics in one table.
macro_rules! define_handles {
    ( $( $variant:ident ),* $(,)? ) => {
        enum Handle {
            $( $variant(bindings::$variant), )*
        }

        impl Handle {
            fn cast_inner<T: windows_core::Interface>(&self) -> windows_core::Result<T> {
                match self {
                    $( Handle::$variant(v) => v.cast::<T>(), )*
                }
            }
            fn as_framework_element(&self) -> bindings::FrameworkElement {
                self.cast_inner().unwrap()
            }
            fn as_ui_element(&self) -> bindings::UIElement {
                self.cast_inner().unwrap()
            }
            fn kind_name(&self) -> &'static str {
                match self {
                    $( Handle::$variant(_) => stringify!($variant), )*
                }
            }
        }

        impl WinUIBackend {
            fn make_handle_for_kind(kind: ControlKind) -> Handle {
                match kind {
                    $(
                        ControlKind::$variant => Handle::$variant(
                            <bindings::$variant>::new().unwrap(),
                        ),
                    )*
                }
            }
        }

        fn describe_kind(h: &Handle) -> &'static str {
            match h {
                $( Handle::$variant(_) => stringify!($variant), )*
            }
        }
    };
}

define_handles! {
    AutoSuggestBox,
    Border,
    BreadcrumbBar,
    Button,
    CalendarDatePicker,
    CalendarView,
    Canvas,
    CheckBox,
    ColorPicker,
    ComboBox,
    CommandBar,
    ContentDialog,
    DatePicker,
    DropDownButton,
    Ellipse,
    Expander,
    FlipView,
    Grid,
    GridView,
    HyperlinkButton,
    Image,
    InfoBadge,
    InfoBar,
    Line,
    ListBox,
    ListView,
    MenuBar,
    NavigationView,
    NumberBox,
    PasswordBox,
    PersonPicture,
    Pivot,
    PivotItem,
    ProgressBar,
    ProgressRing,
    RadioButton,
    RadioButtons,
    RatingControl,
    Rectangle,
    RelativePanel,
    RepeatButton,
    RichEditBox,
    RichTextBlock,
    ScrollView,
    ScrollViewer,
    SelectorBar,
    Slider,
    SplitButton,
    SplitView,
    StackPanel,
    SwapChainPanel,
    TabView,
    TabViewItem,
    TeachingTip,
    TextBlock,
    TextBox,
    TimePicker,
    TitleBar,
    ToggleButton,
    ToggleSwitch,
    TreeView,
    Viewbox,
    WebView2,
}

/// [`Backend`] implementation that creates real `Microsoft.UI.Xaml`
/// controls and drives them on the WinUI thread.
pub struct WinUIBackend {
    controls: RefCell<FxHashMap<ControlId, Handle>>,
    events: RefCell<events::EventState>,
    /// Per-list virtualization state for templated ListView/GridView/FlipView.
    templated: RefCell<FxHashMap<ControlId, TemplatedList>>,
    /// Shared ListView/GridView template; its root `ContentControl` can host
    /// reactor elements where `ListViewItemPresenter` would render strings.
    content_template: RefCell<Option<bindings::DataTemplate>>,
    native_children: RefCell<NativeChildren>,
    resources: RefCell<ResourceState>,
    /// Per-host window state for window-level props.
    window_state: RefCell<Option<Rc<HostWindowState>>>,
    next_id: RefCell<u32>,
}

/// Shared templated-list state touched from WinUI event handlers.
#[derive(Clone, Default)]
struct TemplatedShared {
    source: Rc<RefCell<Option<windows_collections::IObservableVector<windows_core::IInspectable>>>>,
    /// Logical row index -> template-root content host.
    containers: Rc<RefCell<FxHashMap<usize, bindings::IContentControl>>>,
}

/// Per-list backend bookkeeping for templated (virtualized) lists.
struct TemplatedList {
    shared: TemplatedShared,
    realize_revoker: Option<windows_core::EventRevoker>,
    reorder_revoker: Option<windows_core::EventRevoker>,
}

impl TemplatedList {
    fn new() -> Self {
        Self {
            shared: TemplatedShared::default(),
            realize_revoker: None,
            reorder_revoker: None,
        }
    }
}

impl Default for WinUIBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WinUIBackend {
    pub fn new() -> Self {
        Self {
            controls: RefCell::new(FxHashMap::default()),
            events: RefCell::new(events::EventState::default()),
            templated: RefCell::new(FxHashMap::default()),
            content_template: RefCell::new(None),
            native_children: RefCell::new(NativeChildren::default()),
            resources: RefCell::new(ResourceState::default()),
            window_state: RefCell::new(None),
            next_id: RefCell::new(0),
        }
    }

    pub(crate) fn set_window_state(&self, state: Rc<HostWindowState>) {
        *self.window_state.borrow_mut() = Some(state);
    }
    pub fn get_ui_element(&self, id: ControlId) -> Option<windows_core::IInspectable> {
        self.controls
            .borrow()
            .get(&id)
            .map(|h| h.as_ui_element().cast().unwrap())
    }
    /// Parses the shared virtualization item template on first use.
    fn content_template(&self) -> bindings::DataTemplate {
        if let Some(t) = self.content_template.borrow().as_ref() {
            return t.clone();
        }
        let template = bindings::XamlReader::Load(CONTENT_TEMPLATE_XAML)
            .unwrap()
            .cast::<bindings::DataTemplate>()
            .unwrap();
        *self.content_template.borrow_mut() = Some(template.clone());
        template
    }
    pub fn find_titlebar(&self) -> Option<bindings::TitleBar> {
        self.controls.borrow().values().find_map(|h| match h {
            Handle::TitleBar(tb) => Some(tb.clone()),
            _ => None,
        })
    }
    fn alloc_id(&self) -> ControlId {
        let mut counter = self.next_id.borrow_mut();
        *counter += 1;
        ControlId::new(*counter)
    }
}

/// Boxed indices let drag-reorder be read back as a permutation.
fn box_index(i: usize) -> windows_core::IInspectable {
    windows_reference::IReference::<i32>::from(i as i32).into()
}

fn unbox_index(value: &windows_core::IInspectable) -> Option<usize> {
    let r = value.cast::<windows_reference::IReference<i32>>().ok()?;
    usize::try_from(r.Value().ok()?).ok()
}

const CONTENT_TEMPLATE_XAML: &str = "<DataTemplate xmlns='http://schemas.microsoft.com/winfx/2006/xaml/presentation'><ContentControl HorizontalContentAlignment='Stretch' VerticalContentAlignment='Stretch'/></DataTemplate>";

impl Backend for WinUIBackend {
    fn create(&mut self, kind: ControlKind) -> ControlId {
        let id = self.alloc_id();
        let handle = Self::make_handle_for_kind(kind);
        self.controls.borrow_mut().insert(id, handle);
        id
    }
    fn set_prop(&mut self, id: ControlId, prop: Prop, value: &PropValue) {
        let map = self.controls.borrow();
        let handle = map
            .get(&id)
            .unwrap_or_else(|| panic!("WinUIBackend::set_prop: unknown control {id}"));
        let result: Result<()> = (|| -> Result<()> {
            if generated_set_prop::dispatch(handle, prop, value)? {
                return Ok(());
            }
            if let (Prop::Resources, PropValue::Resources(resources)) = (prop, value) {
                self.resources
                    .borrow_mut()
                    .set_local(id, handle, resources)?;
                return Ok(());
            }
            properties::apply(
                properties::PropertyContext {
                    controls: &map,
                    events: &self.events,
                    window_state: &self.window_state,
                },
                id,
                handle,
                prop,
                value,
            )
        })();
        if let Err(e) = result {
            diag::warn(format_args!("set_prop on {id}: {e:?}"));
        }
    }
    fn append_child(&mut self, parent: ControlId, child: ControlId) {
        let controls = self.controls.borrow();
        self.native_children
            .borrow_mut()
            .append(&controls, parent, child);
    }
    fn remove_child(&mut self, parent: ControlId, index: usize) {
        let controls = self.controls.borrow();
        self.native_children
            .borrow_mut()
            .remove(&controls, parent, index);
    }
    fn replace_child(&mut self, parent: ControlId, index: usize, new: ControlId) {
        let controls = self.controls.borrow();
        self.native_children
            .borrow_mut()
            .replace(&controls, parent, index, new);
    }
    fn move_child(&mut self, parent: ControlId, from: usize, to: usize) {
        let controls = self.controls.borrow();
        self.native_children
            .borrow_mut()
            .move_child(&controls, parent, from, to);
    }
    fn insert_child(&mut self, parent: ControlId, index: usize, child: ControlId) {
        let controls = self.controls.borrow();
        self.native_children
            .borrow_mut()
            .insert(&controls, parent, index, child);
    }
    fn set_templated_item_count(&mut self, id: ControlId, count: usize) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        let items_control: bindings::IItemsControl = match handle {
            Handle::ListView(lv) => lv.cast().unwrap(),
            Handle::GridView(gv) => gv.cast().unwrap(),
            _ => return,
        };

        let mut lists = self.templated.borrow_mut();
        let entry = lists.entry(id).or_insert_with(TemplatedList::new);
        let source_slot = &entry.shared.source;

        let mut slot = source_slot.borrow_mut();
        match slot.as_ref() {
            None => {
                // Install the template before WinUI realizes row containers.
                diag::dropped(items_control.SetItemTemplate(&self.content_template()));
                let values: Vec<Option<windows_core::IInspectable>> =
                    (0..count).map(|i| Some(box_index(i))).collect();
                let source: windows_collections::IObservableVector<windows_core::IInspectable> =
                    values.into();
                diag::dropped(items_control.SetItemsSource(&source));
                *slot = Some(source);
            }
            Some(source) => {
                let current = source.Size().unwrap_or(0) as usize;
                if count > current {
                    for i in current..count {
                        diag::dropped(source.Append(&box_index(i)));
                    }
                } else {
                    for _ in count..current {
                        diag::dropped(source.RemoveAtEnd());
                    }
                }
            }
        }
    }
    fn set_templated_row_content(
        &mut self,
        list_id: ControlId,
        row_idx: usize,
        content: Option<ControlId>,
    ) {
        let map = self.controls.borrow();
        let list_h = map
            .get(&list_id)
            .unwrap_or_else(|| panic!("set_templated_row_content: unknown list {list_id}"));
        let content_ui = content.and_then(|c| map.get(&c).map(Handle::as_ui_element));

        // ListView/GridView rows are filled through realized template containers.
        match list_h {
            Handle::ListView(_) | Handle::GridView(_) => {
                let container = self
                    .templated
                    .borrow()
                    .get(&list_id)
                    .and_then(|t| t.shared.containers.borrow().get(&row_idx).cloned());
                let Some(container) = container else { return };
                match content_ui {
                    Some(ui) => diag::dropped(container.SetContent(&ui)),
                    None => {
                        diag::dropped(container.SetContent(None::<&windows_core::IInspectable>));
                    }
                }
                return;
            }
            Handle::FlipView(_) => {}
            other => panic!(
                "set_templated_row_content: {} is not a templated list",
                describe_kind(other)
            ),
        }

        let items_control: bindings::IItemsControl = match list_h {
            Handle::FlipView(fv) => fv.cast().unwrap(),
            _ => unreachable!(),
        };
        let items = items_control
            .Items()
            .unwrap()
            .cast::<windows_collections::IVector<windows_core::IInspectable>>()
            .unwrap();
        let current_len = items.Size().unwrap() as usize;
        match content_ui {
            Some(ui) => {
                let insp: windows_core::IInspectable = ui.cast().unwrap();
                if row_idx < current_len {
                    items.SetAt(row_idx as u32, &insp).unwrap();
                } else {
                    while (items.Size().unwrap() as usize) < row_idx {
                        let pad: windows_core::IInspectable =
                            bindings::TextBlock::new().unwrap().cast().unwrap();
                        items.Append(&pad).unwrap();
                    }
                    items.Append(&insp).unwrap();
                }
            }
            None => {
                if row_idx < current_len {
                    items.RemoveAt(row_idx as u32).unwrap();
                }
            }
        }
    }
    fn set_templated_selected_index(&mut self, id: ControlId, index: i32) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        let selector: bindings::ISelector = match handle {
            Handle::ListView(lv) => lv.cast().unwrap(),
            Handle::GridView(gv) => gv.cast().unwrap(),
            Handle::FlipView(fv) => fv.cast().unwrap(),
            _ => return,
        };
        diag::dropped(selector.SetSelectedIndex(index));
    }

    fn set_templated_selection_mode(&mut self, id: ControlId, mode: SelectionMode) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        let lvb: bindings::IListViewBase = match handle {
            Handle::ListView(lv) => lv.cast().unwrap(),
            Handle::GridView(gv) => gv.cast().unwrap(),
            // FlipView doesn't support SelectionMode.
            _ => return,
        };
        use SelectionMode;
        let winui_mode = match mode {
            SelectionMode::None => bindings::ListViewSelectionMode::None,
            SelectionMode::Single => bindings::ListViewSelectionMode::Single,
            SelectionMode::Multiple => bindings::ListViewSelectionMode::Multiple,
            SelectionMode::Extended => bindings::ListViewSelectionMode::Extended,
        };
        diag::dropped(lvb.SetSelectionMode(winui_mode));
    }

    fn set_templated_can_drag_items(&mut self, id: ControlId, value: bool) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        let lvb: bindings::IListViewBase = match handle {
            Handle::ListView(lv) => lv.cast().unwrap(),
            Handle::GridView(gv) => gv.cast().unwrap(),
            _ => return,
        };
        diag::dropped(lvb.SetCanDragItems(value));
    }

    fn set_templated_can_reorder_items(&mut self, id: ControlId, value: bool) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        let lvb: bindings::IListViewBase = match handle {
            Handle::ListView(lv) => lv.cast().unwrap(),
            Handle::GridView(gv) => gv.cast().unwrap(),
            _ => return,
        };
        diag::dropped(lvb.SetCanReorderItems(value));
    }

    fn set_templated_allow_drop(&mut self, id: ControlId, value: bool) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        let ui: bindings::IUIElement = match handle {
            Handle::ListView(lv) => lv.cast().unwrap(),
            Handle::GridView(gv) => gv.cast().unwrap(),
            Handle::FlipView(fv) => fv.cast().unwrap(),
            _ => return,
        };
        diag::dropped(ui.SetAllowDrop(value));
    }

    fn set_header_element(&mut self, id: ControlId, header_id: Option<ControlId>) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        if let Handle::Expander(e) = handle {
            if let Some(hdr_id) = header_id {
                if let Some(hdr_handle) = map.get(&hdr_id) {
                    let ui_elem = hdr_handle.as_ui_element();
                    diag::dropped(e.SetHeader(&ui_elem));
                }
            } else {
                diag::dropped(e.SetHeader(None));
            }
        } else if let Handle::TitleBar(tb) = handle {
            if let Some(hdr_id) = header_id {
                if let Some(hdr_handle) = map.get(&hdr_id) {
                    let ui_elem = hdr_handle.as_ui_element();
                    diag::dropped(tb.SetContent(&ui_elem));
                }
            } else {
                diag::dropped(tb.SetContent(None));
            }
        }
    }

    fn set_pane_element(&mut self, id: ControlId, pane_id: Option<ControlId>) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        if let Handle::SplitView(sv) = handle {
            if let Some(pid) = pane_id {
                if let Some(pane_handle) = map.get(&pid) {
                    let ui_elem = pane_handle.as_ui_element();
                    diag::dropped(sv.SetPane(&ui_elem));
                }
            } else {
                diag::dropped(sv.SetPane(None));
            }
        } else if let Handle::TitleBar(tb) = handle {
            if let Some(pid) = pane_id {
                if let Some(pane_handle) = map.get(&pid) {
                    let ui_elem = pane_handle.as_ui_element();
                    diag::dropped(tb.SetRightHeader(&ui_elem));
                }
            } else {
                diag::dropped(tb.SetRightHeader(None));
            }
        } else if let Handle::NavigationView(nv) = handle {
            if let Some(pid) = pane_id {
                if let Some(pane_handle) = map.get(&pid) {
                    let ui_elem = pane_handle.as_ui_element();
                    diag::dropped(nv.SetPaneFooter(&ui_elem));
                }
            } else {
                diag::dropped(nv.SetPaneFooter(None));
            }
        }
    }

    fn scroll_templated_to_index(&mut self, id: ControlId, index: i32) {
        if index < 0 {
            return;
        }
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        let lvb: Option<bindings::IListViewBase> = match handle {
            Handle::ListView(lv) => lv.cast().ok(),
            Handle::GridView(gv) => gv.cast().ok(),
            Handle::FlipView(fv) => {
                diag::dropped(
                    fv.cast::<bindings::ISelector>()
                        .unwrap()
                        .SetSelectedIndex(index),
                );
                None
            }
            _ => return,
        };
        if let Some(lvb) = lvb {
            let items_control: bindings::IItemsControl = match handle {
                Handle::ListView(lv) => lv.cast().unwrap(),
                Handle::GridView(gv) => gv.cast().unwrap(),
                _ => return,
            };
            if let Ok(items) = items_control.Items()
                && let Ok(coll) =
                    items.cast::<windows_collections::IVector<windows_core::IInspectable>>()
            {
                let len = coll.Size().unwrap_or(0);
                if (index as u32) < len
                    && let Ok(item) = coll.GetAt(index as u32)
                {
                    diag::dropped(lvb.ScrollIntoView(&item));
                }
            }
        }
    }
    fn attach_templated_selection_changed(&mut self, id: ControlId, handler: Callback<i32>) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };

        let selector: bindings::ISelector = match handle {
            Handle::ListView(lv) => lv.cast().unwrap(),
            Handle::GridView(gv) => gv.cast().unwrap(),
            Handle::FlipView(fv) => fv.cast().unwrap(),
            _ => return,
        };
        self.events.borrow_mut().clear_selection(id);
        let control = selector.clone();
        let revoker = selector
            .SelectionChanged(move |_sender, _args| {
                let idx = control.SelectedIndex().unwrap_or(-1);
                handler.invoke(idx);
            })
            .unwrap_or_else(|e| {
                panic!(
                    "WinUIBackend::attach_templated_selection_changed: \
                 Selector.SelectionChanged registration failed for control {id}: {e}"
                )
            });
        self.events.borrow_mut().replace_selection(id, revoker);
    }
    fn attach_templated_realization(
        &mut self,
        id: ControlId,
        realize: Rc<dyn Fn(usize)>,
        recycle: Rc<dyn Fn(usize)>,
    ) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        let lvb: bindings::IListViewBase = match handle {
            Handle::ListView(lv) => lv.cast().unwrap(),
            Handle::GridView(gv) => gv.cast().unwrap(),
            _ => return,
        };

        let mut lists = self.templated.borrow_mut();
        let entry = lists.entry(id).or_insert_with(TemplatedList::new);
        let containers = Rc::clone(&entry.shared.containers);

        let revoker = lvb
            .ContainerContentChanging(move |_sender, args| {
                let Some(args) = args.as_ref() else { return };
                let Ok(item_container) = args.ItemContainer() else {
                    return;
                };
                // Populate the template root, not the item container.
                let Ok(root) = item_container.cast::<bindings::IContentControl>() else {
                    return;
                };
                let Some(cc) = root
                    .ContentTemplateRoot()
                    .ok()
                    .and_then(|r| r.cast::<bindings::IContentControl>().ok())
                else {
                    return;
                };
                let recycling = args.InRecycleQueue().unwrap_or(false);
                if recycling {
                    // Clear before the reconciler unmounts the row.
                    diag::dropped(cc.SetContent(None::<&windows_core::IInspectable>));
                    let mut map = containers.borrow_mut();
                    if let Some(row) = map.iter().find(|(_, c)| **c == cc).map(|(row, _)| *row) {
                        map.remove(&row);
                        drop(map);
                        recycle(row);
                    }
                } else {
                    // Record the content host and suppress WinUI's phased rendering.
                    let row = args.ItemIndex().unwrap_or(-1);
                    if row < 0 {
                        return;
                    }
                    let row = row as usize;
                    diag::dropped(args.SetHandled(true));
                    containers.borrow_mut().insert(row, cc);
                    realize(row);
                }
            })
            .unwrap_or_else(|e| {
                panic!(
                    "WinUIBackend::attach_templated_realization: \
                     ContainerContentChanging registration failed for control {id}: {e}"
                )
            });
        entry.realize_revoker = Some(revoker);
    }
    fn attach_templated_reorder(&mut self, id: ControlId, handler: Callback<Vec<usize>>) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else { return };
        let lvb: bindings::IListViewBase = match handle {
            Handle::ListView(lv) => lv.cast().unwrap(),
            Handle::GridView(gv) => gv.cast().unwrap(),
            _ => return,
        };

        let mut lists = self.templated.borrow_mut();
        let entry = lists.entry(id).or_insert_with(TemplatedList::new);
        let source = Rc::clone(&entry.shared.source);

        let revoker = lvb
            .DragItemsCompleted(move |_sender, _args| {
                // Read the permutation, then reset the source to identity.
                let slot = source.borrow();
                let Some(source) = slot.as_ref() else { return };
                let len = source.Size().unwrap_or(0) as usize;
                let mut order = Vec::with_capacity(len);
                for i in 0..len as u32 {
                    match source.GetAt(i).ok().as_ref().and_then(unbox_index) {
                        Some(idx) => order.push(idx),
                        None => return,
                    }
                }
                let changed = order.iter().enumerate().any(|(i, v)| *v != i);
                if !changed {
                    return;
                }
                for i in 0..len {
                    diag::dropped(source.SetAt(i as u32, &box_index(i)));
                }
                drop(slot);
                handler.invoke(order);
            })
            .unwrap_or_else(|e| {
                panic!(
                    "WinUIBackend::attach_templated_reorder: \
                     DragItemsCompleted registration failed for control {id}: {e}"
                )
            });
        entry.reorder_revoker = Some(revoker);
    }
    fn destroy(&mut self, id: ControlId) {
        self.templated.borrow_mut().remove(&id);
        let captured = self.events.borrow_mut().remove(id);
        if captured && let Some(handle) = self.controls.borrow().get(&id) {
            diag::dropped(handle.as_ui_element().ReleasePointerCaptures());
        }
        self.controls.borrow_mut().remove(&id);
        self.native_children.borrow_mut().remove_control(id);
        self.resources.borrow_mut().remove(id);
    }
    fn attach_event(&mut self, id: ControlId, event: Event, handler: EventHandler) {
        let map = self.controls.borrow();
        let handle = map
            .get(&id)
            .unwrap_or_else(|| panic!("WinUIBackend::attach_event: unknown control {id}"));

        if matches!(
            event,
            Event::NavigationPaneOpenChanged | Event::NavigationDisplayModeChanged
        ) && let Handle::NavigationView(navigation) = handle
        {
            self.events
                .borrow_mut()
                .observe_navigation_state(id, event, navigation, handler)
                .unwrap_or_else(|error| {
                    panic!(
                        "WinUIBackend::attach_event: failed to observe {event:?} \
                         for control {id}: {error}"
                    )
                });
            return;
        }

        if let Some(revs) = generated_attach_event::dispatch(handle, event, &handler) {
            self.events.borrow_mut().insert_revokers(id, event, revs);
            return;
        }

        let mut revokers: Vec<windows_core::EventRevoker> = Vec::new();
        match (event, handle) {
            (Event::Closed, Handle::ContentDialog(d)) => {
                revokers.push(
                    d.Closed(move |_sender, args| {
                        let result = args
                            .as_ref()
                            .and_then(|a| a.Result().ok())
                            .unwrap_or(bindings::ContentDialogResult(0));
                        handler.invoke_i32(result.0);
                    })
                    .unwrap(),
                );
            }
            (Event::SelectionChanged, Handle::TabView(tv)) => {
                let control = tv.clone();
                revokers.push(
                    tv.SelectionChanged(move |_sender, _args| {
                        let idx = control.SelectedIndex().unwrap_or(-1);
                        if idx >= 0 {
                            handler.invoke_i32(idx);
                        }
                    })
                    .unwrap(),
                );
            }
            (Event::CloseRequested, Handle::TabView(tv)) => {
                revokers.push(
                    tv.TabCloseRequested(move |_sender, args| {
                        let key = args
                            .as_ref()
                            .and_then(|a| a.Tab().ok())
                            .and_then(|tab| {
                                tab.cast::<bindings::IFrameworkElement>()
                                    .unwrap()
                                    .Tag()
                                    .ok()
                            })
                            .and_then(|tag_obj| {
                                tag_obj
                                    .cast::<windows_reference::IReference<windows_core::HSTRING>>()
                                    .ok()
                                    .and_then(|pv| pv.Value().ok())
                            })
                            .map(|h| h.to_string_lossy())
                            .unwrap_or_default();
                        handler.invoke_string(key);
                    })
                    .unwrap(),
                );
            }
            (Event::SelectionChanged, Handle::NavigationView(nv)) => {
                revokers.push(
                    nv.SelectionChanged(move |_sender, args| {
                        let tag = args
                            .as_ref()
                            .and_then(|a| a.SelectedItem().ok())
                            .and_then(|item| item.cast::<bindings::NavigationViewItem>().ok())
                            .and_then(|nvi| {
                                nvi.cast::<bindings::IFrameworkElement>()
                                    .unwrap()
                                    .Tag()
                                    .ok()
                            })
                            .and_then(|tag_obj| {
                                tag_obj
                                    .cast::<windows_reference::IReference<windows_core::HSTRING>>()
                                    .ok()
                                    .and_then(|pv| pv.Value().ok())
                            })
                            .map(|h| h.to_string_lossy())
                            .unwrap_or_default();
                        handler.invoke_string(tag);
                    })
                    .unwrap(),
                );
            }
            (Event::QuerySubmitted, Handle::NavigationView(nv)) => {
                if let Ok(asb) = nv.AutoSuggestBox() {
                    revokers.push(
                        asb.QuerySubmitted(move |_sender, args| {
                            let query = args
                                .as_ref()
                                .and_then(|a| a.QueryText().ok())
                                .unwrap_or_default();
                            handler.invoke_string(query);
                        })
                        .unwrap(),
                    );
                }
            }
            (Event::TextChanged, Handle::NavigationView(nv)) => {
                if let Ok(asb) = nv.AutoSuggestBox() {
                    revokers.push(
                        asb.TextChanged(move |sender, _args| {
                            let text = sender
                                .as_ref()
                                .and_then(|s| s.Text().ok())
                                .unwrap_or_default();
                            handler.invoke_string(text);
                        })
                        .unwrap(),
                    );
                }
            }
            (Event::SuggestionChosen, Handle::NavigationView(nv)) => {
                if let Ok(asb) = nv.AutoSuggestBox() {
                    revokers.push(
                        asb.SuggestionChosen(move |_sender, args| {
                            let item = args
                                .as_ref()
                                .and_then(|a| a.SelectedItem().ok())
                                .and_then(|insp| {
                                    insp.cast::<windows_reference::IReference<
                                        windows_core::HSTRING,
                                    >>()
                                    .ok()
                                    .and_then(|pv| pv.Value().ok())
                                })
                                .map(|h| h.to_string_lossy())
                                .unwrap_or_default();
                            handler.invoke_string(item);
                        })
                        .unwrap(),
                    );
                }
            }
            (Event::SelectionChanged, Handle::Pivot(p)) => {
                let control = p.clone();
                revokers.push(
                    p.SelectionChanged(move |_sender, _args| {
                        let idx = control.SelectedIndex().unwrap_or(-1);
                        if idx >= 0 {
                            handler.invoke_i32(idx);
                        }
                    })
                    .unwrap(),
                );
            }
            (Event::SelectionChanged, Handle::ComboBox(c)) => {
                let selector: bindings::ISelector = c.cast().unwrap();
                let control = selector.clone();
                revokers.push(
                    selector
                        .SelectionChanged(move |_sender, _args| {
                            let idx = control.SelectedIndex().unwrap_or(-1);
                            handler.invoke_i32(idx);
                        })
                        .unwrap(),
                );
            }
            (Event::ColorChanged, Handle::ColorPicker(cp)) => {
                revokers.push(
                    cp.ColorChanged(move |_sender, args| {
                        let color =
                            args.as_ref()
                                .and_then(|a| a.NewColor().ok())
                                .unwrap_or(Color {
                                    a: 255,
                                    r: 0,
                                    g: 0,
                                    b: 0,
                                });
                        handler.invoke_color((color.a, color.r, color.g, color.b));
                    })
                    .unwrap(),
                );
            }
            (Event::SelectedDateChanged, Handle::DatePicker(dp)) => {
                revokers.push(
                    dp.SelectedDateChanged(move |_sender, args| {
                        if let Some(a) = args.as_ref()
                            && let Ok(dt) = a.NewDate()
                        {
                            handler.invoke_datetime(dt);
                        }
                    })
                    .unwrap(),
                );
            }
            (Event::SelectedTimeChanged, Handle::TimePicker(tp)) => {
                revokers.push(
                    tp.SelectedTimeChanged(move |_sender, args| {
                        if let Some(a) = args.as_ref()
                            && let Ok(ts) = a.NewTime()
                        {
                            handler.invoke_timespan(TimeSpan::from_ticks(ts.duration));
                        }
                    })
                    .unwrap(),
                );
            }
            (Event::DateChanged, Handle::CalendarDatePicker(cdp)) => {
                revokers.push(
                    cdp.DateChanged(move |_sender, args| {
                        if let Some(a) = args.as_ref()
                            && let Ok(dt) = a.NewDate()
                        {
                            handler.invoke_datetime(dt);
                        }
                    })
                    .unwrap(),
                );
            }
            (Event::SelectionChanged, Handle::ListBox(lb)) => {
                let selector: bindings::ISelector = lb.cast().unwrap();
                let control = selector.clone();
                revokers.push(
                    selector
                        .SelectionChanged(move |_sender, _args| {
                            if let Ok(idx) = control.SelectedIndex() {
                                handler.invoke_i32(idx);
                            }
                        })
                        .unwrap(),
                );
            }
            (Event::TextChanged, Handle::AutoSuggestBox(asb)) => {
                revokers.push(
                    asb.TextChanged(move |sender, args| {
                        // Only fire for user input, not programmatic changes.
                        let is_user_input = args
                            .as_ref()
                            .and_then(|a| a.Reason().ok())
                            .is_some_and(|r| {
                                r == bindings::AutoSuggestionBoxTextChangeReason::UserInput
                            });
                        if is_user_input {
                            let text = sender
                                .as_ref()
                                .and_then(|s| s.Text().ok())
                                .unwrap_or_default();
                            handler.invoke_string(text);
                        }
                    })
                    .unwrap(),
                );
            }
            (Event::QuerySubmitted, Handle::AutoSuggestBox(asb)) => {
                revokers.push(
                    asb.QuerySubmitted(move |_sender, args| {
                        let text = args
                            .as_ref()
                            .and_then(|a| a.QueryText().ok())
                            .unwrap_or_default();
                        handler.invoke_string(text);
                    })
                    .unwrap(),
                );
            }
            (Event::SuggestionChosen, Handle::AutoSuggestBox(asb)) => {
                revokers.push(
                    asb.SuggestionChosen(move |_sender, args| {
                        let item = args
                            .as_ref()
                            .and_then(|a| a.SelectedItem().ok())
                            .and_then(|insp| {
                                insp.cast::<windows_reference::IReference<windows_core::HSTRING>>()
                                    .ok()
                                    .and_then(|pv| pv.Value().ok())
                            })
                            .map(|h| h.to_string_lossy())
                            .unwrap_or_default();
                        handler.invoke_string(item);
                    })
                    .unwrap(),
                );
            }
            (Event::ItemClicked, Handle::MenuBar(mb)) => {
                self.events
                    .borrow_mut()
                    .set_menu_handler(id, handler.clone());
                let revs = events::wire_menu_bar_clicks(mb, &handler);
                self.events
                    .borrow_mut()
                    .insert_owned_revokers(id, RevokerOwner::MenuItems, revs);
                return;
            }
            (Event::ItemClicked, Handle::DropDownButton(btn)) => {
                self.events
                    .borrow_mut()
                    .set_menu_handler(id, handler.clone());
                let revokers = btn
                    .cast::<bindings::IButton>()
                    .and_then(|button| button.Flyout())
                    .and_then(|flyout| flyout.cast::<bindings::MenuFlyout>())
                    .map_or_else(
                        |_| Vec::new(),
                        |flyout| events::wire_flyout_clicks(&flyout, &handler),
                    );
                self.events.borrow_mut().insert_owned_revokers(
                    id,
                    RevokerOwner::MenuItems,
                    revokers,
                );
                return;
            }
            (Event::ItemClicked, Handle::Button(btn)) => {
                self.events
                    .borrow_mut()
                    .set_menu_handler(id, handler.clone());
                let revokers = btn
                    .cast::<bindings::IButton>()
                    .and_then(|button| button.Flyout())
                    .and_then(|flyout| flyout.cast::<bindings::MenuFlyout>())
                    .map_or_else(
                        |_| Vec::new(),
                        |flyout| events::wire_flyout_clicks(&flyout, &handler),
                    );
                self.events.borrow_mut().insert_owned_revokers(
                    id,
                    RevokerOwner::MenuItems,
                    revokers,
                );
                return;
            }
            (Event::CommandBarFlyoutClick, Handle::Button(btn)) => {
                self.events
                    .borrow_mut()
                    .set_command_bar_flyout_handler(id, handler.clone());
                let revokers = btn
                    .cast::<bindings::IButton>()
                    .and_then(|button| button.Flyout())
                    .and_then(|flyout| flyout.cast::<bindings::CommandBarFlyout>())
                    .map_or_else(
                        |_| Vec::new(),
                        |flyout| {
                            let mut revokers = flyout.PrimaryCommands().map_or_else(
                                |_| Vec::new(),
                                |commands| events::wire_command_bar_clicks(&commands, &handler),
                            );
                            if let Ok(commands) = flyout.SecondaryCommands() {
                                revokers
                                    .extend(events::wire_command_bar_clicks(&commands, &handler));
                            }
                            revokers
                        },
                    );
                self.events.borrow_mut().insert_owned_revokers(
                    id,
                    RevokerOwner::CommandBarFlyout,
                    revokers,
                );
                return;
            }
            (Event::ItemInvoked, Handle::TreeView(tv)) => {
                revokers.push(
                    tv.ItemInvoked(move |_sender, args| {
                        let text = args
                            .as_ref()
                            .and_then(|a| a.InvokedItem().ok())
                            .and_then(|insp| {
                                insp.cast::<bindings::ITreeViewNode>()
                                    .ok()
                                    .and_then(|node| node.Content().ok())
                            })
                            .and_then(|content| {
                                content
                                    .cast::<windows_reference::IReference<windows_core::HSTRING>>()
                                    .ok()
                                    .and_then(|r| r.Value().ok())
                            })
                            .map(|h| h.to_string_lossy())
                            .unwrap_or_default();
                        handler.invoke_string(text);
                    })
                    .unwrap(),
                );
            }
            (Event::Click, Handle::CommandBar(cb)) => {
                self.events
                    .borrow_mut()
                    .set_command_bar_handler(id, handler.clone());
                if let Ok(primary) = cb.PrimaryCommands() {
                    let revs = events::wire_command_bar_clicks(&primary, &handler);
                    self.events.borrow_mut().insert_owned_revokers(
                        id,
                        RevokerOwner::CommandBarPrimary,
                        revs,
                    );
                }
                if let Ok(secondary) = cb.SecondaryCommands() {
                    let revs = events::wire_command_bar_clicks(&secondary, &handler);
                    self.events.borrow_mut().insert_owned_revokers(
                        id,
                        RevokerOwner::CommandBarSecondary,
                        revs,
                    );
                }
                return;
            }
            (Event::SelectionChanged, Handle::SelectorBar(sb)) => {
                let sb2 = sb.clone();
                revokers.push(
                    sb.SelectionChanged(move |_sender, _args| {
                        if let Ok(selected) = sb2.SelectedItem()
                            && let Ok(text) = selected.Text()
                        {
                            handler.invoke_string(text);
                        }
                    })
                    .unwrap(),
                );
            }
            (Event::TextChanged, Handle::RichEditBox(reb)) => {
                let control = reb.clone();
                revokers.push(
                    reb.TextChanged(move |_sender, _args| {
                        let text = control
                            .Document()
                            .ok()
                            .and_then(|doc| {
                                let mut buf = windows_core::HSTRING::default();
                                doc.GetText(bindings::TextGetOptions::None, &mut buf).ok()?;
                                Some(buf.to_string_lossy())
                            })
                            .unwrap_or_default();
                        handler.invoke_string(text);
                    })
                    .unwrap(),
                );
            }
            (Event::Closed, _) => {}
            (event, _) => {
                panic!("WinUIBackend::attach_event: {event:?} on unexpected control {id}")
            }
        }
        drop(map);
        self.events
            .borrow_mut()
            .insert_revokers(id, event, revokers);
    }
    fn detach_event(&mut self, id: ControlId, event: Event) {
        let controls = self.controls.borrow();
        let handle = controls
            .get(&id)
            .unwrap_or_else(|| panic!("WinUIBackend::detach_event: unknown control {id}"));
        let mut events = self.events.borrow_mut();
        match (event, handle) {
            (
                Event::ItemClicked,
                Handle::MenuBar(_) | Handle::DropDownButton(_) | Handle::Button(_),
            ) => events.clear_menu_handler(id),
            (Event::Click, Handle::CommandBar(_)) => events.clear_command_bar_handler(id),
            (Event::CommandBarFlyoutClick, Handle::Button(_)) => {
                events.clear_command_bar_flyout_handler(id);
            }
            _ => events.detach(id, event),
        }
    }
    fn set_theme_bindings(
        &mut self,
        id: ControlId,
        kind: ControlKind,
        bindings: &[(Prop, ThemeRef)],
    ) {
        let _ = kind;
        let controls = self.controls.borrow();
        self.resources
            .borrow_mut()
            .set_theme_bindings(id, controls.get(&id), bindings);
    }
    fn on_theme_changed(&mut self) {
        // Re-apply so WinUI re-resolves {ThemeResource}.
        let controls = self.controls.borrow();
        self.resources.borrow().refresh_theme(&controls);
    }
    fn set_accessibility(&mut self, id: ControlId, accessibility: &AccessibilityModifiers) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else {
            return;
        };
        let fe = handle.as_framework_element();
        let dep: bindings::DependencyObject = match fe.cast() {
            Ok(d) => d,
            Err(_) => return,
        };
        diag::dropped(bindings::AutomationProperties::SetName(
            &dep,
            accessibility.automation_name.as_deref().unwrap_or(""),
        ));
        diag::dropped(bindings::AutomationProperties::SetAutomationId(
            &dep,
            accessibility.automation_id.as_deref().unwrap_or(""),
        ));
        diag::dropped(bindings::AutomationProperties::SetHelpText(
            &dep,
            accessibility.help_text.as_deref().unwrap_or(""),
        ));
        let live = accessibility
            .live_setting
            .unwrap_or(AutomationLiveSetting::Off);
        diag::dropped(bindings::AutomationProperties::SetLiveSetting(&dep, live));
        let heading = accessibility
            .heading_level
            .unwrap_or(AutomationHeadingLevel::None);
        diag::dropped(bindings::AutomationProperties::SetHeadingLevel(
            &dep, heading,
        ));
    }
    fn set_keyboard_accelerators(&mut self, id: ControlId, accelerators: &[KeyboardAccelerator]) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else {
            return;
        };
        let fe = handle.as_framework_element();
        let iue: bindings::IUIElement = match fe.cast() {
            Ok(i) => i,
            Err(_) => return,
        };
        let vec: windows_collections::IVector<bindings::KeyboardAccelerator> =
            match iue.KeyboardAccelerators() {
                Ok(v) => v,
                Err(_) => return,
            };
        diag::dropped(vec.Clear());

        diag::dropped(iue.SetKeyboardAcceleratorPlacementMode(
            bindings::KeyboardAcceleratorPlacementMode::Hidden,
        ));

        for accel in accelerators {
            let Ok(ka) = bindings::KeyboardAccelerator::new() else {
                continue;
            };
            let Ok(ika) = ka.cast::<bindings::IKeyboardAccelerator>() else {
                continue;
            };
            diag::dropped(ika.SetKey(accel.key));
            diag::dropped(ika.SetModifiers(accel.modifiers));
            let cb = accel.on_invoked.clone();
            let _ = ika
                .Invoked(move |_sender, args| {
                    if let Some(a) = args.as_ref() {
                        diag::dropped(a.SetHandled(true));
                    }
                    cb.invoke(());
                })
                .ok()
                .map(|r| r.into_token());
            diag::dropped(vec.Append(&ka));
        }
    }
    fn set_implicit_transitions(
        &mut self,
        id: ControlId,
        transitions: Option<ImplicitTransitions>,
    ) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else {
            return;
        };
        let ui: bindings::UIElement = handle.as_ui_element();
        if let Err(e) = apply_implicit_transitions(&ui, transitions) {
            diag::warn(format_args!("set_implicit_transitions failed: {e:?}"));
        }
    }
    fn set_layout_animation(&mut self, _id: ControlId, _config: Option<LayoutAnimationConfig>) {}
    fn run_property_animation(&mut self, id: ControlId, config: Option<AnimationConfig>) {
        let Some(cfg) = config else {
            return;
        };
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else {
            return;
        };
        let ui: bindings::UIElement = handle.as_ui_element();
        if let Err(e) = run_property_animation(&ui, cfg) {
            diag::warn(format_args!("run_property_animation failed: {e:?}"));
        }
    }
    fn set_element_transitions(
        &mut self,
        id: ControlId,
        enter: Option<AnimationConfig>,
        exit: Option<AnimationConfig>,
    ) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else {
            return;
        };
        let ui = handle.as_ui_element();
        if let Err(error) = apply_element_transitions(&ui, enter, exit) {
            diag::warn(format_args!("set_element_transitions failed: {error:?}"));
        }
    }
    fn set_rich_text_paragraphs(&mut self, id: ControlId, paragraphs: &[RichTextParagraph]) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else {
            return;
        };
        let Handle::RichTextBlock(rtb) = handle else {
            return;
        };
        let Ok(blocks) = rtb.Blocks() else { return };
        diag::dropped(blocks.Clear());
        for para_def in paragraphs {
            let Ok(para) = bindings::Paragraph::new() else {
                continue;
            };
            let Ok(inlines) = para.Inlines() else {
                continue;
            };
            for inline in &para_def.inlines {
                match inline {
                    RichTextInline::Run(r) => {
                        let Ok(run) = bindings::Run::new() else {
                            continue;
                        };
                        diag::dropped(run.SetText(&r.text));
                        if r.is_bold {
                            diag::dropped(run.cast::<bindings::ITextElement>().and_then(|te| {
                                te.SetFontWeight(bindings::FontWeight { weight: 700 })
                            }));
                        }
                        diag::dropped(
                            run.cast::<bindings::Inline>()
                                .and_then(|i| inlines.Append(&i)),
                        );
                    }
                    RichTextInline::LineBreak => {
                        let Ok(run) = bindings::Run::new() else {
                            continue;
                        };
                        diag::dropped(run.SetText("\n"));
                        diag::dropped(
                            run.cast::<bindings::Inline>()
                                .and_then(|i| inlines.Append(&i)),
                        );
                    }
                    RichTextInline::Hyperlink(h) => {
                        let Ok(run) = bindings::Run::new() else {
                            continue;
                        };
                        diag::dropped(run.SetText(&h.text));
                        diag::dropped(
                            run.cast::<bindings::Inline>()
                                .and_then(|i| inlines.Append(&i)),
                        );
                    }
                }
            }
            diag::dropped(
                para.cast::<bindings::Block>()
                    .and_then(|b| blocks.Append(&b)),
            );
        }
    }

    fn set_tooltip(&mut self, id: ControlId, tooltip: Option<&Tooltip>) {
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else {
            return;
        };
        let fe = handle.as_framework_element();
        let dep: bindings::DependencyObject = match fe.cast() {
            Ok(d) => d,
            Err(_) => return,
        };

        let inspectable: Option<windows_core::IInspectable> = match tooltip {
            None => None,
            Some(t) => match &t.content {
                TooltipContent::Text(s) => {
                    let reference = windows_reference::IReference::from(s.as_str());
                    Some(reference.into())
                }
                TooltipContent::Rich(elem) => {
                    let tt = match bindings::ToolTip::new() {
                        Ok(t) => t,
                        Err(e) => {
                            diag::warn(format_args!("ToolTip::new failed: {e:?}"));
                            return;
                        }
                    };
                    if let Some(ui) = mount_static_tooltip_element(elem)
                        && let Ok(cc) = tt.cast::<bindings::IContentControl>()
                    {
                        diag::dropped(cc.SetContent(&ui));
                    }
                    Some(tt.into())
                }
            },
        };
        diag::dropped(bindings::ToolTipService::SetToolTip(
            &dep,
            inspectable.as_ref(),
        ));

        let placement = tooltip
            .and_then(|t| t.placement)
            .map_or(bindings::PlacementMode::Top, map_placement);
        diag::dropped(bindings::ToolTipService::SetPlacement(&dep, placement));
    }

    fn set_pointer_handlers(&mut self, id: ControlId, handlers: Option<&PointerHandlers>) {
        // Remove the old token set from backend ownership before replacing it.
        let prev = self.events.borrow_mut().take_pointer(id);
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else {
            return;
        };
        let ui = handle.as_ui_element();
        let previous_capture = prev.as_ref().is_some_and(|tokens| tokens.capture_on_press);
        let next_capture = handlers.is_some_and(|handlers| handlers.capture_pointer_on_press);
        if previous_capture && !next_capture {
            // Keep the previous capture-lost callback attached while ending
            // an active gesture.
            diag::dropped(ui.ReleasePointerCaptures());
        }
        drop(prev);

        let Some(handlers) = handlers else {
            return;
        };
        let mut tokens = PointerRevokerSet {
            capture_on_press: handlers.capture_pointer_on_press,
            ..PointerRevokerSet::default()
        };

        if let Some(cb) = handlers.on_tapped.clone() {
            tokens.tapped = ui
                .Tapped(move |_sender, _args| {
                    cb.invoke(());
                })
                .ok();
        }

        if let Some(cb) = handlers.on_right_tapped.clone() {
            tokens.right_tapped = ui
                .RightTapped(move |_sender, _args| {
                    cb.invoke(());
                })
                .ok();
        }

        if handlers.on_pointer_pressed.is_some() || handlers.capture_pointer_on_press {
            let element = ui.clone();
            let cb = handlers.on_pointer_pressed.clone();
            let capture = handlers.capture_pointer_on_press;
            tokens.pressed = ui
                .PointerPressed(move |_sender, args| {
                    let capture_succeeded = if capture {
                        args.as_ref()
                            .and_then(|args| args.Pointer().ok())
                            .is_some_and(|pointer| match element.CapturePointer(&pointer) {
                                Ok(true) => true,
                                Ok(false) => {
                                    diag::warn(format_args!("pointer capture was refused"));
                                    false
                                }
                                Err(error) => {
                                    diag::warn(format_args!("pointer capture failed: {error:?}"));
                                    false
                                }
                            })
                    } else {
                        false
                    };
                    if let Some(cb) = &cb {
                        let mut info = pointer_event_info(&element, args);
                        info.capture_succeeded = capture_succeeded;
                        cb.invoke(info);
                    }
                })
                .ok();
        }

        if handlers.on_pointer_released.is_some() || handlers.capture_pointer_on_press {
            let element = ui.clone();
            let cb = handlers.on_pointer_released.clone();
            let capture = handlers.capture_pointer_on_press;
            tokens.released = ui
                .PointerReleased(move |_sender, args| {
                    let pointer = capture
                        .then(|| args.as_ref().and_then(|args| args.Pointer().ok()))
                        .flatten();
                    let info = pointer_event_info(&element, args);
                    if let Some(pointer) = pointer {
                        diag::dropped(element.ReleasePointerCapture(&pointer));
                    }
                    if let Some(cb) = &cb {
                        cb.invoke(info);
                    }
                })
                .ok();
        }

        if let Some(cb) = handlers.on_pointer_moved.clone() {
            let element = ui.clone();
            tokens.moved = ui
                .PointerMoved(move |_sender, args| {
                    let info = pointer_event_info(&element, args);
                    cb.invoke(info);
                })
                .ok();
        }

        if let Some(cb) = handlers.on_pointer_entered.clone() {
            let element = ui.clone();
            tokens.entered = ui
                .PointerEntered(move |_sender, args| {
                    let info = pointer_event_info(&element, args);
                    cb.invoke(info);
                })
                .ok();
        }

        if let Some(cb) = handlers.on_pointer_exited.clone() {
            tokens.exited = ui
                .PointerExited(move |_sender, _args| {
                    cb.invoke(());
                })
                .ok();
        }

        if let Some(cb) = handlers.on_pointer_capture_lost.clone() {
            tokens.capture_lost = ui
                .PointerCaptureLost(move |_sender, _args| {
                    cb.invoke(());
                })
                .ok();
        }

        if let Some(cb) = handlers.on_pointer_canceled.clone() {
            tokens.canceled = ui
                .PointerCanceled(move |_sender, _args| {
                    cb.invoke(());
                })
                .ok();
        }

        self.events.borrow_mut().set_pointer(id, tokens);
    }

    fn set_drag_handlers(&mut self, id: ControlId, handlers: Option<&DragHandlers>) {
        let prev = self.events.borrow_mut().take_drag(id);
        let map = self.controls.borrow();
        let Some(handle) = map.get(&id) else {
            return;
        };
        let ui = handle.as_ui_element();
        drop(prev);

        let Some(handlers) = handlers else {
            return;
        };
        let mut tokens = DragRevokerSet::default();

        if let Some(callback) = handlers.on_drag_enter.clone() {
            let marshaller = WinUIDispatcher::for_current_thread()
                .map(|dispatcher| dispatcher.marshaller())
                .ok();

            tokens.enter = ui
                .DragEnter(move |_sender, args| {
                    let Some(drag_event_args) = args.as_ref() else {
                        return;
                    };

                    let formats = drag_event_args
                        .DataView()
                        .ok()
                        .map(|data_package_view| read_available_formats(&data_package_view))
                        .unwrap_or_default();

                    let agile_deferral = drag_event_args
                        .GetDeferral()
                        .ok()
                        .and_then(|deferral| windows_core::AgileReference::new(&deferral).ok());

                    let agile_args = windows_core::AgileReference::new(drag_event_args).ok();

                    let callback = callback.clone();
                    let marshaller = marshaller.clone();
                    windows_threading::submit(move || {
                        let Some(marshaller) = marshaller else {
                            if let Some(deferral) =
                                agile_deferral.and_then(|agile_ref| agile_ref.resolve().ok())
                            {
                                diag::dropped(deferral.Complete());
                            }
                            return;
                        };
                        dispatch_accept(
                            marshaller,
                            callback,
                            formats,
                            agile_args,
                            agile_deferral,
                            vec![],
                            None,
                        );
                    });
                })
                .ok();
        }

        if let Some(cb) = handlers.on_drag_leave.clone() {
            tokens.leave = ui
                .DragLeave(move |_sender, args| {
                    let ctx = build_drag_context(args.as_ref());
                    cb.call(&ctx);
                })
                .ok();
        }

        if let Some(cb) = handlers.on_drag_over.clone() {
            tokens.over = ui
                .DragOver(move |_sender, args| {
                    accept_or_reject(&cb, args.as_ref());
                })
                .ok();
        }

        if let Some(callback) = handlers.on_drag_drop.clone() {
            let marshaller = WinUIDispatcher::for_current_thread()
                .map(|dispatcher| dispatcher.marshaller())
                .ok();

            tokens.drop = ui
                .Drop(move |_sender, args| {
                    let Some(drag_event_args) = args.as_ref() else {
                        return;
                    };

                    let data_view = drag_event_args.DataView().ok();

                    let formats = data_view
                        .as_ref()
                        .map(read_available_formats)
                        .unwrap_or_default();

                    let agile_deferral = drag_event_args
                        .GetDeferral()
                        .ok()
                        .and_then(|deferral| windows_core::AgileReference::new(&deferral).ok());

                    let agile_data_view = data_view.and_then(|data_package_view| {
                        windows_core::AgileReference::new(&data_package_view).ok()
                    });

                    let agile_args = windows_core::AgileReference::new(drag_event_args).ok();

                    let callback = callback.clone();
                    let marshaller = marshaller.clone();

                    windows_threading::submit(move || {
                        use crate::drag::DroppedItem;

                        let resolved_data_view = agile_data_view
                            .and_then(|agile_reference| agile_reference.resolve().ok());

                        let items: Vec<DroppedItem> = if formats.storage_items {
                            resolved_data_view
                                .as_ref()
                                .and_then(|data_package_view| {
                                    data_package_view.GetStorageItemsAsync().ok()
                                })
                                .and_then(|async_operation| async_operation.join().ok())
                                .map(|v| {
                                    let size = v.Size().unwrap_or(0);
                                    (0..size)
                                        .filter_map(|i| v.GetAt(i).ok())
                                        .map(|item| DroppedItem {
                                            path: item.Path().unwrap_or_default(),
                                            name: item.Name().unwrap_or_default(),
                                            is_folder: item.Attributes().is_ok_and(|attrs| {
                                                attrs.contains(bindings::FileAttributes::Directory)
                                            }),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default()
                        } else {
                            vec![]
                        };

                        let text: Option<String> = if formats.text {
                            resolved_data_view
                                .as_ref()
                                .and_then(|data_package_view| data_package_view.GetTextAsync().ok())
                                .and_then(|async_operation| async_operation.join().ok())
                                .and_then(|h| String::try_from(&h).ok())
                        } else {
                            None
                        };

                        let Some(marshaller) = marshaller else {
                            if let Some(deferral) =
                                agile_deferral.and_then(|agile_ref| agile_ref.resolve().ok())
                            {
                                diag::dropped(deferral.Complete());
                            }
                            return;
                        };
                        dispatch_accept(
                            marshaller,
                            callback,
                            formats,
                            agile_args,
                            agile_deferral,
                            items,
                            text,
                        );
                    });
                })
                .ok();
        }

        self.events.borrow_mut().set_drag(id, tokens);
    }

    fn get_native_element(&self, id: ControlId) -> Option<windows_core::IInspectable> {
        self.get_ui_element(id)
    }
}

const FORMAT_TEXT: &str = "Text";
const FORMAT_HTML: &str = "HTML Format";
const FORMAT_RTF: &str = "Rich Text Format";
const FORMAT_BITMAP: &str = "Bitmap";
const FORMAT_STORAGE_ITEMS: &str = "Shell IDList Array";
const FORMAT_URI_AND_WEB_LINK: &str = "UniformResourceLocatorW";
const FORMAT_APPLICATION_LINK: &str = "ApplicationLink";

#[derive(Copy, Clone, Default)]
struct AvailableFormats {
    text: bool,
    html: bool,
    rtf: bool,
    bitmap: bool,
    storage_items: bool,
    uri: bool,
    web_link: bool,
    application_link: bool,
}

fn read_available_formats(data_package_view: &bindings::DataPackageView) -> AvailableFormats {
    let mut available_formats = AvailableFormats::default();
    let Ok(formats) = data_package_view.AvailableFormats() else {
        return available_formats;
    };

    for s in &formats {
        match s.to_string_lossy().as_str() {
            FORMAT_TEXT => available_formats.text = true,
            FORMAT_HTML => available_formats.html = true,
            FORMAT_RTF => available_formats.rtf = true,
            FORMAT_BITMAP => available_formats.bitmap = true,
            FORMAT_STORAGE_ITEMS => available_formats.storage_items = true,
            FORMAT_URI_AND_WEB_LINK => {
                available_formats.uri = true;
                available_formats.web_link = true;
            }
            FORMAT_APPLICATION_LINK => available_formats.application_link = true,
            _ => {}
        }
    }
    available_formats
}

fn build_drag_context(args: Option<&bindings::DragEventArgs>) -> DragContext {
    use crate::drag::{DragContext, DroppedItem};
    let mut ctx = DragContext {
        has_text: false,
        has_html: false,
        has_rtf: false,
        has_bitmap: false,
        has_storage_items: false,
        has_uri: false,
        has_web_link: false,
        has_application_link: false,
        caption: None,
        glyph_visible: None,
        content_visible: None,
        get_text_fn: None,
        get_storage_items_fn: None,
    };
    let Some(a) = args else { return ctx };
    let Ok(dv) = a.DataView() else {
        return ctx;
    };

    let formats = read_available_formats(&dv);
    ctx.has_text = formats.text;
    ctx.has_html = formats.html;
    ctx.has_rtf = formats.rtf;
    ctx.has_bitmap = formats.bitmap;
    ctx.has_storage_items = formats.storage_items;
    ctx.has_uri = formats.uri;
    ctx.has_web_link = formats.web_link;
    ctx.has_application_link = formats.application_link;

    let dv_text = dv.clone();
    ctx.get_text_fn = Some(Box::new(move || {
        let h = dv_text.GetTextAsync().ok()?.join().ok()?;
        String::try_from(&h).ok()
    }));

    ctx.get_storage_items_fn = Some(Box::new(move || {
        let items = dv.GetStorageItemsAsync().ok().and_then(|op| op.join().ok());
        let Some(items) = items else {
            return Vec::new();
        };
        items
            .into_iter()
            .map(|item| DroppedItem {
                path: item.Path().unwrap_or_default(),
                name: item.Name().unwrap_or_default(),
                is_folder: item
                    .Attributes()
                    .is_ok_and(|a| a.contains(bindings::FileAttributes::Directory)),
            })
            .collect()
    }));

    ctx
}

fn dispatch_accept(
    m: UiMarshaller,
    cb: DragAsyncCallback,
    formats: AvailableFormats,
    iargs_agile: Option<windows_core::AgileReference<bindings::DragEventArgs>>,
    deferral_agile: Option<windows_core::AgileReference<bindings::DragOperationDeferral>>,
    items: Vec<DroppedItem>,
    text: Option<String>,
) {
    use crate::drag::{DragContext, DragOperation};
    m.dispatch(move || {
        let get_storage_items_fn = if items.is_empty() {
            None
        } else {
            let v = items.clone();
            Some(Box::new(move || v.clone()) as Box<dyn Fn() -> Vec<DroppedItem>>)
        };
        let get_text_fn =
            text.map(|t| Box::new(move || Some(t.clone())) as Box<dyn Fn() -> Option<String>>);
        let mut ctx = DragContext {
            has_text: formats.text,
            has_html: formats.html,
            has_rtf: formats.rtf,
            has_bitmap: formats.bitmap,
            has_storage_items: formats.storage_items,
            has_uri: formats.uri,
            has_web_link: formats.web_link,
            has_application_link: formats.application_link,
            caption: None,
            glyph_visible: None,
            content_visible: None,
            get_text_fn,
            get_storage_items_fn,
        };
        let op = cb.call(&mut ctx);
        if let Some(iargs) = iargs_agile.and_then(|a| a.resolve().ok()) {
            let accepted = match op {
                DragOperation::None => bindings::DataPackageOperation::None,
                DragOperation::Copy => bindings::DataPackageOperation::Copy,
                DragOperation::Move => bindings::DataPackageOperation::Move,
                DragOperation::Link => bindings::DataPackageOperation::Link,
            };
            diag::dropped(iargs.SetAcceptedOperation(accepted));
            if (ctx.caption.is_some()
                || ctx.glyph_visible.is_some()
                || ctx.content_visible.is_some())
                && let Ok(ui) = iargs.DragUIOverride()
            {
                if let Some(v) = ctx.caption {
                    diag::dropped(ui.SetCaption(&v));
                }
                if let Some(v) = ctx.glyph_visible {
                    diag::dropped(ui.SetIsGlyphVisible(v));
                }
                if let Some(v) = ctx.content_visible {
                    diag::dropped(ui.SetIsContentVisible(v));
                }
            }
        }
        if let Some(d) = deferral_agile.and_then(|a| a.resolve().ok()) {
            diag::dropped(d.Complete());
        }
    });
}

trait CallAccept {
    fn call(&self, ctx: &mut DragContext) -> DragOperation;
}
impl CallAccept for DragCallback {
    fn call(&self, ctx: &mut DragContext) -> DragOperation {
        self.call(ctx)
    }
}
impl CallAccept for DragAsyncCallback {
    fn call(&self, ctx: &mut DragContext) -> DragOperation {
        self.call(ctx)
    }
}

fn accept_or_reject<C: CallAccept>(cb: &C, args: Option<&bindings::DragEventArgs>) {
    use crate::drag::DragOperation;
    let Some(a) = args else { return };

    let mut ctx = build_drag_context(Some(a));

    let result = cb.call(&mut ctx);

    let accepted = match result {
        DragOperation::None => bindings::DataPackageOperation::None,
        DragOperation::Copy => bindings::DataPackageOperation::Copy,
        DragOperation::Move => bindings::DataPackageOperation::Move,
        DragOperation::Link => bindings::DataPackageOperation::Link,
    };
    diag::dropped(a.SetAcceptedOperation(accepted));

    if (ctx.caption.is_some() || ctx.glyph_visible.is_some() || ctx.content_visible.is_some())
        && let Ok(ui) = a.DragUIOverride()
    {
        if let Some(v) = ctx.caption {
            diag::dropped(ui.SetCaption(&v));
        }
        if let Some(v) = ctx.glyph_visible {
            diag::dropped(ui.SetIsGlyphVisible(v));
        }
        if let Some(v) = ctx.content_visible {
            diag::dropped(ui.SetIsContentVisible(v));
        }
    }
}

/// Extract local/window pointer positions and button state for a pointer callback.
///
/// `element` is captured once at attach time (the handler's own element), so
/// there is no per-event `QueryInterface`: the arg/point/properties classes
/// each `Deref` to their default interface.
fn pointer_event_info(
    element: &bindings::UIElement,
    args: windows_core::InRef<'_, bindings::PointerRoutedEventArgs>,
) -> PointerEventInfo {
    let mut info = PointerEventInfo::default();
    let Some(args) = args.as_ref() else {
        return info;
    };

    if let Ok(point) = args.GetCurrentPoint(element) {
        if let Ok(pos) = point.Position() {
            info.x = pos.x as f64;
            info.y = pos.y as f64;
        }
        if let Ok(props) = point.Properties() {
            info.is_left_button_pressed = props.IsLeftButtonPressed().unwrap_or(false);
            info.is_right_button_pressed = props.IsRightButtonPressed().unwrap_or(false);
            info.is_middle_button_pressed = props.IsMiddleButtonPressed().unwrap_or(false);
        }
    }

    if let Ok(point) = args.GetCurrentPoint(None::<&bindings::UIElement>)
        && let Ok(pos) = point.Position()
    {
        info.window_x = pos.x as f64;
        info.window_y = pos.y as f64;
    }

    info
}

fn map_placement(p: TooltipPlacement) -> bindings::PlacementMode {
    use TooltipPlacement;
    match p {
        TooltipPlacement::Top => bindings::PlacementMode::Top,
        TooltipPlacement::Bottom => bindings::PlacementMode::Bottom,
        TooltipPlacement::Left => bindings::PlacementMode::Left,
        TooltipPlacement::Right => bindings::PlacementMode::Right,
        TooltipPlacement::Mouse => bindings::PlacementMode::Mouse,
    }
}

/// Best-effort static mount for tooltip content. Supports `TextBlock`,
/// linear `StackPanel`, and `Image`; unsupported kinds fall back to a
/// `TextBlock` showing `kind_name()`.
fn mount_static_tooltip_element(el: &Element) -> Option<bindings::UIElement> {
    match el {
        Element::TextBlock(t) => {
            let tb = bindings::TextBlock::new().ok()?;
            tb.SetText(t.text.as_str()).ok()?;
            tb.cast::<bindings::UIElement>().ok()
        }
        Element::StackPanel(s) => {
            let sp = bindings::StackPanel::new().ok()?;
            sp.SetOrientation(s.orientation).ok()?;
            sp.SetSpacing(s.spacing).ok()?;
            let children = sp.cast::<bindings::IPanel>().ok()?.Children().ok()?;
            for child in &s.children {
                if let Some(cui) = mount_static_tooltip_element(child) {
                    diag::dropped(children.Append(&cui));
                }
            }
            sp.cast::<bindings::UIElement>().ok()
        }
        Element::Image(img) => {
            let i = bindings::Image::new().ok()?;
            if let Ok(Some(source)) = build_image_source(&img.source) {
                diag::dropped(i.SetSource(&source));
            }
            i.cast::<bindings::UIElement>().ok()
        }
        _ => {
            // Fallback: surface the kind_name so the developer sees a
            // hint rather than an empty popup.
            let tb = bindings::TextBlock::new().ok()?;
            tb.SetText(el.kind_name()).ok()?;
            tb.cast::<bindings::UIElement>().ok()
        }
    }
}
