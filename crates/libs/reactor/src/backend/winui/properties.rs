use super::events::{
    EventState, RevokerOwner, wire_command_bar_clicks, wire_flyout_clicks, wire_menu_bar_clicks,
};
use super::*;

pub(super) struct PropertyContext<'a> {
    pub(super) controls: &'a FxHashMap<ControlId, Handle>,
    pub(super) events: &'a RefCell<EventState>,
    pub(super) window_state: &'a RefCell<Option<Rc<HostWindowState>>>,
}

pub(super) fn apply(
    context: PropertyContext<'_>,
    id: ControlId,
    handle: &Handle,
    prop: Prop,
    value: &PropValue,
) -> Result<()> {
    if try_universal_prop(handle, prop, value)? {
        return Ok(());
    }
    match (prop, value, handle) {
        (Prop::MaxLines, PropValue::I32(v), Handle::TextBlock(text)) => text.SetMaxLines(*v),
        (Prop::MaxLines, PropValue::Unset, Handle::TextBlock(text)) => text.SetMaxLines(0),
        (Prop::TextTrimming, PropValue::I32(v), Handle::TextBlock(text)) => {
            text.SetTextTrimming(TextTrimming(*v))
        }
        (Prop::TextTrimming, PropValue::Unset, Handle::TextBlock(text)) => {
            text.SetTextTrimming(TextTrimming::None)
        }
        (Prop::IsTextSelectionEnabled, PropValue::Bool(v), Handle::RichTextBlock(tb)) => {
            tb.SetIsTextSelectionEnabled(*v)
        }
        (Prop::IsTextSelectionEnabled, PropValue::Unset, Handle::RichTextBlock(tb)) => {
            tb.SetIsTextSelectionEnabled(false)
        }
        (Prop::TextWrappingWrap, PropValue::I32(v), Handle::RichTextBlock(tb)) => {
            tb.SetTextWrapping(TextWrapping(*v))
        }
        (Prop::Content, PropValue::Str(s), Handle::Button(b)) => {
            let cc = b.cast::<bindings::IContentControl>()?;
            // Preserve an existing icon+text layout when only text changes.
            if let Ok(existing) = cc.Content()
                && let Ok(panel) = existing.cast::<bindings::IPanel>()
            {
                let children = panel.Children()?;
                if children.Size()? >= 2
                    && let Ok(tb) = children.GetAt(1)?.cast::<bindings::ITextBlock>()
                {
                    return tb.SetText(s);
                }
            }
            let tb = string_as_textblock(s)?;
            cc.SetContent(&tb)
        }
        (Prop::Icon, PropValue::Icon(icon), Handle::Button(b)) => {
            let icon_elem = build_icon_element(icon)?;
            let cc = b.cast::<bindings::IContentControl>()?;
            // Preserve text when replacing an existing icon.
            if let Ok(existing) = cc.Content()
                && let Ok(panel) = existing.cast::<bindings::IPanel>()
            {
                let children = panel.Children()?;
                if children.Size()? >= 2 {
                    children.SetAt(0, &icon_elem.cast::<bindings::UIElement>()?)?;
                    return Ok(());
                }
            }
            let use_icon_only = if let Ok(existing) = cc.Content() {
                existing.cast::<bindings::IIconElement>().is_ok()
                    || existing
                        .cast::<bindings::ITextBlock>()
                        .ok()
                        .and_then(|tb| tb.Text().ok())
                        .is_some_and(|t| t.is_empty())
            } else {
                true
            };
            if use_icon_only {
                cc.SetContent(&icon_elem)
            } else {
                let panel = bindings::StackPanel::new()?;
                panel.SetOrientation(Orientation::Horizontal)?;
                panel.SetSpacing(8.0)?;
                let children = panel.cast::<bindings::IPanel>()?.Children()?;
                children.Append(&icon_elem.cast::<bindings::UIElement>()?)?;
                if let Ok(existing) = cc.Content()
                    && let Ok(ui) = existing.cast::<bindings::UIElement>()
                {
                    children.Append(&ui)?;
                }
                cc.SetContent(&panel)
            }
        }
        (Prop::Icon, PropValue::Unset, Handle::Button(b)) => {
            let cc = b.cast::<bindings::IContentControl>()?;
            let Ok(existing) = cc.Content() else {
                return Ok(());
            };
            // Unwrap icon+text layout back to text-only.
            if let Ok(panel) = existing.cast::<bindings::IPanel>() {
                let children = panel.Children()?;
                if children.Size()? >= 2 {
                    let text_child = children.GetAt(1)?;
                    children.Clear()?;
                    return cc.SetContent(&text_child);
                }
            }
            if existing.cast::<bindings::IIconElement>().is_ok() {
                return cc.SetContent(None::<&windows_core::IInspectable>);
            }
            Ok(())
        }
        (Prop::StyleVariant, PropValue::I32(v), Handle::Button(b)) => {
            let fe = b.cast::<bindings::IFrameworkElement>()?;
            let style_key = match *v {
                1 => Some("AccentButtonStyle"),
                2 => Some("SubtleButtonStyle"),
                3 => Some("TextBlockButtonStyle"),
                _ => None, // 0 = Default
            };
            if let Some(key_str) = style_key {
                let resources = bindings::Application::Current().and_then(|app| app.Resources())?;
                let key = windows_reference::IReference::from(windows_core::HSTRING::from(key_str));
                let map = resources.cast::<windows_collections::IMap<
                    windows_core::IInspectable,
                    windows_core::IInspectable,
                >>()?;
                if let Ok(style_obj) = map.Lookup(&key)
                    && let Ok(s) = style_obj.cast::<bindings::Style>()
                {
                    fe.SetStyle(&s)?;
                }
            } else {
                fe.SetStyle(None)?;
            }
            Ok(())
        }
        (Prop::Value, PropValue::Str(s), Handle::TextBox(t)) => {
            if t.Text().ok().as_deref() == Some(s.as_str()) {
                return Ok(());
            }
            t.SetText(s.as_str())
        }
        (Prop::GridRows, PropValue::GridLengths(rows), Handle::Grid(g)) => {
            let defs = g.RowDefinitions()?;
            defs.Clear()?;
            for r in rows {
                let rd = bindings::RowDefinition::new()?;
                rd.SetHeight(to_xaml_gridlength(*r)?)?;
                defs.Append(&rd)?;
            }
            Ok(())
        }
        (Prop::GridColumns, PropValue::GridLengths(cols), Handle::Grid(g)) => {
            let defs = g.ColumnDefinitions()?;
            defs.Clear()?;
            for c in cols {
                let cd = bindings::ColumnDefinition::new()?;
                cd.SetWidth(to_xaml_gridlength(*c)?)?;
                defs.Append(&cd)?;
            }
            Ok(())
        }
        (Prop::Step, PropValue::F64(v), Handle::Slider(s)) => {
            s.SetStepFrequency(*v)?;
            s.cast::<bindings::IRangeBase>()?.SetSmallChange(*v)
        }
        (Prop::Step, PropValue::Unset, Handle::Slider(s)) => {
            s.SetStepFrequency(1.0)?;
            s.cast::<bindings::IRangeBase>()?.SetSmallChange(1.0)
        }
        (Prop::NavigateUri, PropValue::Str(s), Handle::HyperlinkButton(h)) => {
            let uri = bindings::Uri::CreateUri(s.as_str())?;
            h.SetNavigateUri(&uri)
        }
        (Prop::NavigateUri, PropValue::Unset, Handle::HyperlinkButton(h)) => h.SetNavigateUri(None),
        (Prop::IsClosable, PropValue::Bool(v), Handle::TabViewItem(ti)) => ti.SetIsClosable(*v),
        (Prop::IsOpen, PropValue::Bool(v), Handle::ContentDialog(d)) => {
            if *v {
                // ContentDialog is not in the tree, so borrow another XamlRoot.
                let xroot = context
                    .controls
                    .values()
                    .filter_map(|h| match h {
                        Handle::ContentDialog(_) => None,
                        other => other
                            .as_ui_element()
                            .cast::<bindings::IUIElement>()
                            .ok()
                            .and_then(|u| u.XamlRoot().ok()),
                    })
                    .next();
                match xroot {
                    Some(root) => {
                        diag::dropped(d.cast::<bindings::IUIElement>()?.SetXamlRoot(&root));
                        diag::dropped(d.ShowAsync());
                    }
                    None => {
                        diag::warn(format_args!(
                            "ContentDialog.is_open ignored - no XamlRoot available"
                        ));
                    }
                }
                Ok(())
            } else {
                d.Hide()
            }
        }
        (Prop::Value, PropValue::I32(v), Handle::InfoBadge(ib)) => {
            if *v < 0 {
                ib.SetValue(-1)
            } else {
                ib.SetValue(*v)
            }
        }
        (Prop::DisplayName, PropValue::Unset, Handle::PersonPicture(p)) => p.SetDisplayName(""),
        (Prop::Initials, PropValue::Unset, Handle::PersonPicture(p)) => p.SetInitials(""),
        (Prop::CornerRadius, PropValue::F64(v), Handle::Rectangle(r)) => {
            r.SetRadiusX(*v).and_then(|_| r.SetRadiusY(*v))
        }
        (Prop::CornerRadius, PropValue::Unset, Handle::Rectangle(r)) => {
            r.SetRadiusX(0.0).and_then(|_| r.SetRadiusY(0.0))
        }
        (Prop::CornerRadius, PropValue::F64(v), Handle::Border(b)) => {
            b.SetCornerRadius(bindings::CornerRadius {
                top_left: *v,
                top_right: *v,
                bottom_right: *v,
                bottom_left: *v,
            })
        }
        (Prop::CornerRadius, PropValue::Unset, Handle::Border(b)) => {
            b.SetCornerRadius(bindings::CornerRadius::default())
        }
        (Prop::BorderBrush, PropValue::Color(br), h) => set_border_brush(h, &solid_brush(*br)?),
        (Prop::BorderBrush, PropValue::Unset, h) => set_border_brush(h, None::<&bindings::Brush>),
        (Prop::BorderThickness, PropValue::Thickness(t), h) => set_border_thickness(h, *t),
        (Prop::BorderThickness, PropValue::Unset, h) => {
            set_border_thickness(h, Thickness::default())
        }
        (Prop::LineEndpoints, PropValue::LineEndpoints(p), Handle::Line(l)) => l
            .SetX1(p.x1)
            .and_then(|_| l.SetY1(p.y1))
            .and_then(|_| l.SetX2(p.x2))
            .and_then(|_| l.SetY2(p.y2)),
        (Prop::ImageSource, PropValue::ImageSource(source), Handle::Image(img)) => {
            match build_image_source(source)? {
                Some(source) => img.SetSource(&source),
                None => img.SetSource(None),
            }
        }
        (Prop::ImageSource, PropValue::Unset, Handle::Image(img)) => img.SetSource(None),
        (Prop::Header, PropValue::Str(s), Handle::TabViewItem(ti)) => {
            let tb = string_as_textblock(s)?;
            ti.SetHeader(&tb)
        }
        (Prop::Header, PropValue::Str(s), Handle::Expander(e)) => {
            let tb = string_as_textblock(s)?;
            e.SetHeader(&tb)
        }
        (Prop::Header, PropValue::Unset, Handle::Expander(e)) => e.SetHeader(None),
        (Prop::ItemKey, PropValue::Str(s), Handle::TabViewItem(ti)) => {
            let tag = windows_reference::IReference::from(s.as_str());
            ti.cast::<bindings::IFrameworkElement>()?.SetTag(&tag)
        }
        (Prop::ItemKey, PropValue::Unset, Handle::TabViewItem(ti)) => {
            ti.cast::<bindings::IFrameworkElement>()?.SetTag(None)
        }
        (Prop::MenuItems, PropValue::NavMenuItems(items), Handle::NavigationView(nv)) => {
            let menu = nv.MenuItems()?;
            menu.Clear()?;
            for item in items {
                let nv_item = build_nav_view_item(item)?;
                menu.Append(&nv_item)?;
            }
            Ok(())
        }
        (Prop::SelectedTag, PropValue::Str(tag), Handle::NavigationView(nv)) => {
            select_nav_item_by_tag(nv, tag)
        }
        (Prop::SelectedTag, PropValue::Unset, Handle::NavigationView(nv)) => {
            nv.SetSelectedItem(None)
        }
        (Prop::AutoSuggestBox, PropValue::Bool(true), Handle::NavigationView(nv)) => {
            let asb = bindings::AutoSuggestBox::new()?;
            nv.SetAutoSuggestBox(&asb)
        }
        (Prop::AutoSuggestBox, PropValue::Bool(false), Handle::NavigationView(nv)) => {
            nv.SetAutoSuggestBox(None)
        }
        (Prop::AutoSuggestPlaceholder, PropValue::Str(s), Handle::NavigationView(nv)) => {
            if let Ok(asb) = nv.AutoSuggestBox() {
                asb.SetPlaceholderText(s.as_str())?;
            }
            Ok(())
        }
        (Prop::AutoSuggestItems, PropValue::StrList(items), Handle::NavigationView(nv)) => {
            if let Ok(asb) = nv.AutoSuggestBox() {
                asb.cast::<bindings::IItemsControl>()?
                    .SetItemsSource(&str_list_as_ivector(items))?;
            }
            Ok(())
        }
        (Prop::Tall, PropValue::Bool(v), Handle::TitleBar(_)) => {
            if let Some(state) = context.window_state.borrow().as_ref() {
                state.set_titlebar_height(*v);
            }
            Ok(())
        }
        (Prop::IsBackButtonVisible, PropValue::Bool(v), Handle::NavigationView(nv)) => {
            let val = if *v {
                bindings::NavigationViewBackButtonVisible::Auto
            } else {
                bindings::NavigationViewBackButtonVisible::Collapsed
            };
            nv.cast::<bindings::INavigationView2>()?
                .SetIsBackButtonVisible(val)
        }
        (Prop::ItemHeader, PropValue::Str(s), Handle::PivotItem(pi)) => {
            let tb = string_as_textblock(s)?;
            pi.SetHeader(&tb)
        }
        (Prop::Items, PropValue::StrList(items), Handle::BreadcrumbBar(bc)) => {
            bc.SetItemsSource(&str_list_as_ivector(items))
        }
        (Prop::Value, PropValue::Str(s), Handle::PasswordBox(p)) => {
            if p.Password().ok().as_deref() == Some(s.as_str()) {
                return Ok(());
            }
            p.SetPassword(s.as_str())
        }
        (Prop::Value, PropValue::Unset, Handle::PasswordBox(p)) => p.SetPassword(""),
        (Prop::Items, PropValue::StrList(items), Handle::RadioButtons(r)) => {
            set_str_items(&r.Items()?.cast()?, items)
        }
        (Prop::Items, PropValue::StrList(items), Handle::ComboBox(c)) => set_str_items(
            &c.cast::<bindings::IItemsControl>()?.Items()?.cast()?,
            items,
        ),
        (Prop::ColorValue, PropValue::Color(c), Handle::ColorPicker(cp)) => cp.SetColor(*c),
        (Prop::Items, PropValue::StrList(items), Handle::ListBox(lb)) => set_str_items(
            &lb.cast::<bindings::IItemsControl>()?.Items()?.cast()?,
            items,
        ),
        (Prop::Text, PropValue::Str(s), Handle::AutoSuggestBox(asb)) => {
            // Skip SetText when the control already has this value -
            // calling SetText during a user-initiated TextChanged
            // cycle steals focus from the input field.
            if asb.Text().ok().as_deref() == Some(s.as_str()) {
                return Ok(());
            }
            asb.SetText(s)
        }
        (Prop::Items, PropValue::StrList(items), Handle::AutoSuggestBox(asb)) => asb
            .cast::<bindings::IItemsControl>()?
            .SetItemsSource(&str_list_as_ivector(items)),
        (Prop::DisplayMode, PropValue::I32(m), Handle::SplitView(sv)) => {
            sv.SetDisplayMode(bindings::SplitViewDisplayMode(*m))
        }
        (Prop::Items, PropValue::MenuBarItems(items), Handle::MenuBar(mb)) => {
            let winui_items = mb.Items()?;
            winui_items.Clear()?;
            for bar_item_def in items {
                let mbi = bindings::MenuBarItem::new()?;
                mbi.SetTitle(&bar_item_def.title)?;
                let flyout_items = mbi.Items()?;
                for menu_def in &bar_item_def.items {
                    let fi = build_menu_flyout_item_base(menu_def)?;
                    flyout_items.Append(&fi)?;
                }
                winui_items.Append(&mbi)?;
            }
            let handler = context.events.borrow().menu_handler(id);
            if let Some(handler) = handler {
                let revokers = wire_menu_bar_clicks(mb, &handler);
                context.events.borrow_mut().insert_owned_revokers(
                    id,
                    RevokerOwner::MenuItems,
                    revokers,
                );
            }
            Ok(())
        }
        (Prop::MenuFlyoutItems, PropValue::MenuFlyoutItems(items), Handle::DropDownButton(btn)) => {
            let flyout = bindings::MenuFlyout::new()?;
            let flyout_items = flyout.Items()?;
            for def in items {
                let fi = build_menu_flyout_item_base(def)?;
                flyout_items.Append(&fi)?;
            }
            btn.cast::<bindings::IButton>()?.SetFlyout(&flyout)?;
            let handler = context.events.borrow().menu_handler(id);
            if let Some(handler) = handler {
                let revokers = wire_flyout_clicks(&flyout, &handler);
                context.events.borrow_mut().insert_owned_revokers(
                    id,
                    RevokerOwner::MenuItems,
                    revokers,
                );
            }
            Ok(())
        }
        (Prop::MenuFlyoutItems, PropValue::MenuFlyoutItems(items), Handle::Button(btn)) => {
            let flyout = bindings::MenuFlyout::new()?;
            let flyout_items = flyout.Items()?;
            for def in items {
                let fi = build_menu_flyout_item_base(def)?;
                flyout_items.Append(&fi)?;
            }
            btn.SetFlyout(&flyout)?;
            let handler = context.events.borrow().menu_handler(id);
            if let Some(handler) = handler {
                let revokers = wire_flyout_clicks(&flyout, &handler);
                context.events.borrow_mut().insert_owned_revokers(
                    id,
                    RevokerOwner::MenuItems,
                    revokers,
                );
            }
            Ok(())
        }
        (
            Prop::CommandBarFlyoutCommands,
            PropValue::CommandBarFlyoutDef { primary, secondary },
            Handle::Button(btn),
        ) => {
            let flyout = bindings::CommandBarFlyout::new()?;
            let primary_cmds = flyout.PrimaryCommands()?;
            let secondary_cmds = flyout.SecondaryCommands()?;
            for def in primary {
                let el = build_command_bar_element(def)?;
                primary_cmds.Append(&el)?;
            }
            for def in secondary {
                let el = build_command_bar_element(def)?;
                secondary_cmds.Append(&el)?;
            }
            btn.SetFlyout(&flyout)?;
            let handler = context.events.borrow().command_bar_flyout_handler(id);
            if let Some(handler) = handler {
                let mut revokers = wire_command_bar_clicks(&primary_cmds, &handler);
                revokers.extend(wire_command_bar_clicks(&secondary_cmds, &handler));
                context.events.borrow_mut().insert_owned_revokers(
                    id,
                    RevokerOwner::CommandBarFlyout,
                    revokers,
                );
            }
            Ok(())
        }
        (Prop::Nodes, PropValue::TreeViewNodes(nodes), Handle::TreeView(tv)) => {
            let root = tv.RootNodes()?;
            root.Clear()?;
            for node_def in nodes {
                let node = build_tree_view_node(node_def)?;
                root.Append(&node)?;
            }
            Ok(())
        }
        (Prop::PrimaryCommands, PropValue::CommandBarCommands(cmds), Handle::CommandBar(cb)) => {
            let primary = cb.PrimaryCommands()?;
            primary.Clear()?;
            for def in cmds {
                let el = build_command_bar_element(def)?;
                primary.Append(&el)?;
            }
            let handler = context.events.borrow().command_bar_handler(id);
            if let Some(handler) = handler {
                let revokers = wire_command_bar_clicks(&primary, &handler);
                context.events.borrow_mut().insert_owned_revokers(
                    id,
                    RevokerOwner::CommandBarPrimary,
                    revokers,
                );
            }
            Ok(())
        }
        (Prop::SecondaryCommands, PropValue::CommandBarCommands(cmds), Handle::CommandBar(cb)) => {
            let secondary = cb.SecondaryCommands()?;
            secondary.Clear()?;
            for def in cmds {
                let el = build_command_bar_element(def)?;
                secondary.Append(&el)?;
            }
            let handler = context.events.borrow().command_bar_handler(id);
            if let Some(handler) = handler {
                let revokers = wire_command_bar_clicks(&secondary, &handler);
                context.events.borrow_mut().insert_owned_revokers(
                    id,
                    RevokerOwner::CommandBarSecondary,
                    revokers,
                );
            }
            Ok(())
        }
        (Prop::ActionButton, PropValue::Str(s), Handle::TeachingTip(tt)) => {
            let boxed: windows_core::IInspectable =
                windows_reference::IReference::<windows_core::HSTRING>::from(
                    windows_core::HSTRING::from(s.as_str()),
                )
                .cast()?;
            tt.SetActionButtonContent(&boxed)
        }
        (Prop::CloseButton, PropValue::Str(s), Handle::TeachingTip(tt)) => {
            let boxed: windows_core::IInspectable =
                windows_reference::IReference::<windows_core::HSTRING>::from(
                    windows_core::HSTRING::from(s.as_str()),
                )
                .cast()?;
            tt.SetCloseButtonContent(&boxed)
        }
        (Prop::Items, PropValue::SelectorBarItems(items), Handle::SelectorBar(sb)) => {
            let vec = sb.Items()?;
            vec.Clear()?;
            for def in items {
                let item = bindings::SelectorBarItem::new()?;
                item.SetText(&def.text)?;
                if let Some(icon) = &def.icon {
                    let icon_elem = build_icon_element(icon)?;
                    item.SetIcon(&icon_elem)?;
                }
                vec.Append(&item)?;
            }
            Ok(())
        }
        (Prop::Text, PropValue::Str(s), Handle::RichEditBox(reb)) => {
            let doc = reb.Document()?;
            let mut current = windows_core::HSTRING::default();
            doc.GetText(bindings::TextGetOptions::None, &mut current)
                .ok();
            if current == s.as_str() {
                return Ok(());
            }
            let read_only = reb.IsReadOnly()?;
            if read_only {
                reb.SetIsReadOnly(false)?;
            }
            let set_result = doc.SetText(bindings::TextSetOptions::None, s.as_str());
            let restore_result = if read_only {
                reb.SetIsReadOnly(true)
            } else {
                Ok(())
            };
            set_result?;
            restore_result
        }
        (Prop::Header, PropValue::Str(s), Handle::RichEditBox(reb)) => {
            let tb = string_as_textblock(s)?;
            reb.SetHeader(&tb)
        }
        (Prop::Header, PropValue::Unset, Handle::RichEditBox(reb)) => reb.SetHeader(None),
        (Prop::FlyoutContent, PropValue::Str(s), Handle::Button(b)) => {
            let flyout = bindings::Flyout::new()?;
            let tb = string_as_textblock(s)?;
            flyout.SetContent(&tb)?;
            b.SetFlyout(&flyout)?;
            Ok(())
        }
        (Prop::FlyoutPlacement, PropValue::I32(v), Handle::Button(b)) => {
            if let Ok(fb) = b.Flyout() {
                diag::dropped(
                    fb.cast::<bindings::IFlyoutBase>()?
                        .SetPlacement(FlyoutPlacementMode(*v)),
                );
            }
            Ok(())
        }
        (_, PropValue::Unset, _) => Ok(()),
        (p, v, h) => {
            diag::unhandled_prop(id, p, v, h);
            Ok(())
        }
    }
}

/// Handles props shared by base-class interfaces.
fn try_universal_prop(handle: &Handle, prop: Prop, value: &PropValue) -> Result<bool> {
    match (prop, value) {
        (Prop::FontSize, PropValue::F64(v)) => set_font_f64(handle, *v),
        (Prop::FontSize, PropValue::Unset) => set_font_f64(handle, 14.0),
        (Prop::FontWeight, PropValue::U16(w)) => {
            set_font_weight(handle, bindings::FontWeight { weight: *w })
        }
        (Prop::FontWeight, PropValue::Unset) => {
            set_font_weight(handle, bindings::FontWeight { weight: 400 })
        }
        (Prop::FontFamily, PropValue::Str(s)) => {
            set_font_family(handle, &bindings::FontFamily::CreateInstanceWithName(s)?)
        }
        (Prop::FontFamily, PropValue::Unset) => set_font_family(
            handle,
            &bindings::FontFamily::CreateInstanceWithName("Segoe UI")?,
        ),
        (Prop::Margin, PropValue::Thickness(t)) => {
            handle.as_framework_element().SetMargin(*t)?;
            Ok(true)
        }
        (Prop::Margin, PropValue::Unset) => {
            handle
                .as_framework_element()
                .SetMargin(Thickness::default())?;
            Ok(true)
        }
        (Prop::Width, PropValue::F64(v)) => {
            handle.as_framework_element().SetWidth(*v)?;
            Ok(true)
        }
        (Prop::Width, PropValue::Unset) => {
            handle.as_framework_element().SetWidth(f64::NAN)?;
            Ok(true)
        }
        (Prop::Height, PropValue::F64(v)) => {
            handle.as_framework_element().SetHeight(*v)?;
            Ok(true)
        }
        (Prop::Height, PropValue::Unset) => {
            handle.as_framework_element().SetHeight(f64::NAN)?;
            Ok(true)
        }
        (Prop::MinWidth, PropValue::F64(v)) => {
            handle.as_framework_element().SetMinWidth(*v)?;
            Ok(true)
        }
        (Prop::MinWidth, PropValue::Unset) => {
            handle.as_framework_element().SetMinWidth(0.0)?;
            Ok(true)
        }
        (Prop::MaxWidth, PropValue::F64(v)) => {
            handle.as_framework_element().SetMaxWidth(*v)?;
            Ok(true)
        }
        (Prop::MaxWidth, PropValue::Unset) => {
            handle.as_framework_element().SetMaxWidth(f64::INFINITY)?;
            Ok(true)
        }
        (Prop::MinHeight, PropValue::F64(v)) => {
            handle.as_framework_element().SetMinHeight(*v)?;
            Ok(true)
        }
        (Prop::MinHeight, PropValue::Unset) => {
            handle.as_framework_element().SetMinHeight(0.0)?;
            Ok(true)
        }
        (Prop::MaxHeight, PropValue::F64(v)) => {
            handle.as_framework_element().SetMaxHeight(*v)?;
            Ok(true)
        }
        (Prop::MaxHeight, PropValue::Unset) => {
            handle.as_framework_element().SetMaxHeight(f64::INFINITY)?;
            Ok(true)
        }
        (Prop::HorizontalAlignment, PropValue::I32(v)) => {
            handle
                .as_framework_element()
                .SetHorizontalAlignment(HorizontalAlignment(*v))?;
            Ok(true)
        }
        (Prop::HorizontalAlignment, PropValue::Unset) => {
            handle
                .as_framework_element()
                .SetHorizontalAlignment(HorizontalAlignment::Stretch)?;
            Ok(true)
        }
        (Prop::VerticalAlignment, PropValue::I32(v)) => {
            handle
                .as_framework_element()
                .SetVerticalAlignment(VerticalAlignment(*v))?;
            Ok(true)
        }
        (Prop::VerticalAlignment, PropValue::Unset) => {
            handle
                .as_framework_element()
                .SetVerticalAlignment(VerticalAlignment::Stretch)?;
            Ok(true)
        }
        (Prop::Opacity, PropValue::F64(v)) => {
            handle.as_ui_element().SetOpacity(*v)?;
            Ok(true)
        }
        (Prop::Opacity, PropValue::Unset) => {
            handle.as_ui_element().SetOpacity(1.0)?;
            Ok(true)
        }
        (Prop::AllowDrop, PropValue::Bool(v)) => {
            handle.as_ui_element().SetAllowDrop(*v)?;
            Ok(true)
        }
        (Prop::AllowDrop, PropValue::Unset) => {
            handle.as_ui_element().SetAllowDrop(false)?;
            Ok(true)
        }
        (Prop::IsEnabled, PropValue::Unset) => {
            handle
                .as_ui_element()
                .cast::<bindings::IControl>()?
                .SetIsEnabled(true)?;
            Ok(true)
        }
        (Prop::AttachedGridRow, PropValue::I32(v)) => {
            bindings::Grid::SetRow(&handle.as_framework_element(), *v)?;
            Ok(true)
        }
        (Prop::AttachedGridColumn, PropValue::I32(v)) => {
            bindings::Grid::SetColumn(&handle.as_framework_element(), *v)?;
            Ok(true)
        }
        (Prop::AttachedGridRowSpan, PropValue::I32(v)) => {
            bindings::Grid::SetRowSpan(&handle.as_framework_element(), *v)?;
            Ok(true)
        }
        (Prop::AttachedGridColumnSpan, PropValue::I32(v)) => {
            bindings::Grid::SetColumnSpan(&handle.as_framework_element(), *v)?;
            Ok(true)
        }
        (Prop::AttachedCanvasLeft, PropValue::F64(v)) => {
            bindings::Canvas::SetLeft(&handle.as_ui_element(), *v)?;
            Ok(true)
        }
        (Prop::AttachedCanvasTop, PropValue::F64(v)) => {
            bindings::Canvas::SetTop(&handle.as_ui_element(), *v)?;
            Ok(true)
        }
        (Prop::AttachedCanvasZIndex, PropValue::I32(v)) => {
            bindings::Canvas::SetZIndex(&handle.as_ui_element(), *v)?;
            Ok(true)
        }
        (Prop::AlignLeftWithPanel, PropValue::Bool(v)) => {
            bindings::RelativePanel::SetAlignLeftWithPanel(&handle.as_ui_element(), *v)?;
            Ok(true)
        }
        (Prop::AlignRightWithPanel, PropValue::Bool(v)) => {
            bindings::RelativePanel::SetAlignRightWithPanel(&handle.as_ui_element(), *v)?;
            Ok(true)
        }
        (Prop::AlignTopWithPanel, PropValue::Bool(v)) => {
            bindings::RelativePanel::SetAlignTopWithPanel(&handle.as_ui_element(), *v)?;
            Ok(true)
        }
        (Prop::AlignBottomWithPanel, PropValue::Bool(v)) => {
            bindings::RelativePanel::SetAlignBottomWithPanel(&handle.as_ui_element(), *v)?;
            Ok(true)
        }
        (Prop::AlignHCenterWithPanel, PropValue::Bool(v)) => {
            bindings::RelativePanel::SetAlignHorizontalCenterWithPanel(
                &handle.as_ui_element(),
                *v,
            )?;
            Ok(true)
        }
        (Prop::AlignVCenterWithPanel, PropValue::Bool(v)) => {
            bindings::RelativePanel::SetAlignVerticalCenterWithPanel(&handle.as_ui_element(), *v)?;
            Ok(true)
        }
        (Prop::Padding, PropValue::Thickness(t)) => set_padding(handle, *t),
        (Prop::Padding, PropValue::Unset) => set_padding(handle, Thickness::default()),
        (Prop::Background, PropValue::Color(br)) => set_background(handle, &solid_brush(*br)?),
        (Prop::Background, PropValue::Unset) => set_background(handle, None::<&bindings::Brush>),
        (Prop::Foreground, PropValue::Color(br)) => set_foreground(handle, &solid_brush(*br)?),
        (Prop::Foreground, PropValue::Unset) => set_foreground(handle, None::<&bindings::Brush>),
        (Prop::Fill, PropValue::Color(b)) => {
            handle
                .cast_inner::<bindings::IShape>()?
                .SetFill(&solid_brush(*b)?)?;
            Ok(true)
        }
        (Prop::Fill, PropValue::Unset) => {
            handle.cast_inner::<bindings::IShape>()?.SetFill(None)?;
            Ok(true)
        }
        (Prop::Stroke, PropValue::Color(b)) => {
            handle
                .cast_inner::<bindings::IShape>()?
                .SetStroke(&solid_brush(*b)?)?;
            Ok(true)
        }
        (Prop::Stroke, PropValue::Unset) => {
            handle.cast_inner::<bindings::IShape>()?.SetStroke(None)?;
            Ok(true)
        }
        (Prop::StrokeThickness, PropValue::F64(v)) => {
            handle
                .cast_inner::<bindings::IShape>()?
                .SetStrokeThickness(*v)?;
            Ok(true)
        }
        (Prop::StrokeThickness, PropValue::Unset) => {
            handle
                .cast_inner::<bindings::IShape>()?
                .SetStrokeThickness(0.0)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn set_padding(handle: &Handle, thickness: Thickness) -> Result<bool> {
    match handle {
        Handle::Border(h) => h.SetPadding(thickness)?,
        Handle::StackPanel(h) => h.SetPadding(thickness)?,
        Handle::TextBlock(h) => h.SetPadding(thickness)?,
        Handle::RichTextBlock(h) => h.SetPadding(thickness)?,
        // `Grid` is a `Panel`, not a `Control`, so it has no `IControl::SetPadding`;
        // its padding lives on the `IGrid` interface instead.
        Handle::Grid(h) => h.cast::<bindings::IGrid>()?.SetPadding(thickness)?,
        Handle::SwapChainPanel(h) => h.cast::<bindings::IGrid>()?.SetPadding(thickness)?,
        _ => {
            if let Ok(ctl) = handle.cast_inner::<bindings::IControl>() {
                ctl.SetPadding(thickness)?;
            } else {
                diag::unhandled_modifier("set_prop", Prop::Padding, handle);
            }
        }
    }
    Ok(true)
}

fn set_background(
    handle: &Handle,
    brush: impl windows_core::Param<bindings::Brush>,
) -> Result<bool> {
    match handle {
        Handle::Border(b) => b.SetBackground(brush)?,
        _ => {
            if let Ok(panel) = handle.cast_inner::<bindings::IPanel>() {
                panel.SetBackground(brush)?;
            } else if let Ok(ctl) = handle.cast_inner::<bindings::IControl>() {
                ctl.SetBackground(brush)?;
            } else {
                diag::unhandled_modifier("set_prop", Prop::Background, handle);
            }
        }
    }
    Ok(true)
}

fn set_foreground(
    handle: &Handle,
    brush: impl windows_core::Param<bindings::Brush>,
) -> Result<bool> {
    match handle {
        Handle::TextBlock(h) => h.SetForeground(brush)?,
        Handle::RichTextBlock(h) => h.SetForeground(brush)?,
        _ => {
            if let Ok(ctl) = handle.cast_inner::<bindings::IControl>() {
                ctl.SetForeground(brush)?;
            } else {
                diag::unhandled_modifier("set_prop", Prop::Foreground, handle);
            }
        }
    }
    Ok(true)
}

fn set_border_brush(
    handle: &Handle,
    brush: impl windows_core::Param<bindings::Brush>,
) -> Result<()> {
    match handle {
        Handle::Border(b) => b.SetBorderBrush(brush)?,
        _ => {
            if let Ok(ctl) = handle.cast_inner::<bindings::IControl>() {
                ctl.SetBorderBrush(brush)?;
            } else {
                diag::unhandled_modifier("set_prop", Prop::BorderBrush, handle);
            }
        }
    }
    Ok(())
}

fn set_border_thickness(handle: &Handle, thickness: Thickness) -> Result<()> {
    match handle {
        Handle::Border(b) => b.SetBorderThickness(thickness)?,
        _ => {
            if let Ok(ctl) = handle.cast_inner::<bindings::IControl>() {
                ctl.SetBorderThickness(thickness)?;
            } else {
                diag::unhandled_modifier("set_prop", Prop::BorderThickness, handle);
            }
        }
    }
    Ok(())
}

fn set_font_f64(handle: &Handle, v: f64) -> Result<bool> {
    match handle {
        Handle::TextBlock(h) => h.SetFontSize(v)?,
        Handle::RichTextBlock(h) => h.SetFontSize(v)?,
        _ => {
            if let Ok(ctl) = handle.cast_inner::<bindings::IControl>() {
                ctl.SetFontSize(v)?;
            } else {
                diag::unhandled_modifier("set_prop", Prop::FontSize, handle);
            }
        }
    }
    Ok(true)
}

fn set_font_weight(handle: &Handle, fw: bindings::FontWeight) -> Result<bool> {
    match handle {
        Handle::TextBlock(h) => h.SetFontWeight(fw)?,
        _ => {
            if let Ok(ctl) = handle.cast_inner::<bindings::IControl>() {
                ctl.SetFontWeight(fw)?;
            } else {
                diag::unhandled_modifier("set_prop", Prop::FontWeight, handle);
            }
        }
    }
    Ok(true)
}

fn set_font_family(handle: &Handle, ff: &bindings::FontFamily) -> Result<bool> {
    match handle {
        Handle::TextBlock(h) => h.SetFontFamily(ff)?,
        Handle::RichTextBlock(h) => h.SetFontFamily(ff)?,
        _ => {
            if let Ok(ctl) = handle.cast_inner::<bindings::IControl>() {
                ctl.SetFontFamily(ff)?;
            } else {
                diag::unhandled_modifier("set_prop", Prop::FontFamily, handle);
            }
        }
    }
    Ok(true)
}

fn set_str_items(
    vec: &windows_collections::IVector<windows_core::IInspectable>,
    items: &[String],
) -> Result<()> {
    vec.Clear()?;
    for s in items {
        let insp = windows_reference::IReference::from(s.as_str());
        vec.Append(&insp)?;
    }
    Ok(())
}

fn str_list_as_ivector(
    items: &[String],
) -> windows_collections::IVector<windows_core::IInspectable> {
    let vec: Vec<Option<windows_core::IInspectable>> = items
        .iter()
        .map(|s| Some(windows_reference::IReference::from(s.as_str()).into()))
        .collect();
    vec.into()
}
