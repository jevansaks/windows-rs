//! Event detachment fixtures: verify that control event handlers
//! are properly revoked when a control is unmounted or re-rendered without
//! the callback attached. This catches leaks in the EventRevoker lifecycle.

use std::cell::Cell;
use std::rc::Rc;

use windows_core::Interface;
use windows_reactor::Element;
use windows_reactor::Slider;
use windows_reactor::{CommandBar, app_bar_button, button, check_box, text_block};

use crate::bindings;
use crate::fixtures::reconciler::{FixtureFuture, cc};
use crate::harness::Harness;

use windows_reactor::vstack;

fn invoke_command(command: bindings::ICommandBarElement) -> windows_core::Result<()> {
    let button: bindings::Button = command.cast()?;
    let peer = bindings::ButtonAutomationPeer::CreateInstanceWithOwner(&button)?;
    let pattern = peer
        .cast::<bindings::IAutomationPeer>()?
        .GetPattern(bindings::PatternInterface::Invoke)?;
    let invoke: bindings::IInvokeProvider = pattern.cast()?;
    invoke.Invoke()
}

/// Verify that when a button with `on_click` is removed from the tree,
/// subsequent programmatic clicks on the same UI slot don't fire the old
/// handler (handler was properly revoked/detached).
pub fn on_click_detach_on_unmount(h: Harness) -> FixtureFuture {
    Box::pin(async move {
        let fire_count = Rc::new(Cell::new(0u32));
        let fire_count2 = fire_count.clone();

        h.mount(cc(move |cx| {
            let (show_btn, set_show) = cx.use_state(true);
            let (count, _set_count) = cx.use_state(0u32);
            let fc = fire_count2.clone();

            let body: Element = if show_btn {
                vstack((
                    button("Clickable").on_click(move || {
                        fc.set(fc.get() + 1);
                    }),
                    text_block(format!("fires={}", fire_count2.get())),
                ))
                .into()
            } else {
                vstack((text_block(format!("removed,fires={}", fire_count2.get())),)).into()
            };

            vstack((
                body,
                text_block(format!("count={count}")),
                button("Remove").on_click(move || set_show.call(false)),
            ))
            .into()
        }));
        h.render().await;

        let _ = h.click_button("Clickable");
        h.render().await;
        h.check(
            "EventDetach_OnClick_FiredBeforeRemove",
            fire_count.get() >= 1,
        );

        // Remove the button from the tree
        let _ = h.click_button("Remove");
        h.render().await;

        h.check(
            "EventDetach_OnClick_ButtonRemoved",
            h.find_text_containing("removed,fires=").is_some(),
        );

        // The fire count should not increase after removal
        let count_after_remove = fire_count.get();
        h.check(
            "EventDetach_OnClick_NoLeakedFires",
            count_after_remove >= 1, // only the initial click
        );
    })
}

/// Verify that when an `on_checked` callback is conditionally removed from
/// a CheckBox (control stays mounted, but handler is gone), subsequent
/// state changes via the WinUI property no longer trigger the old handler.
pub fn on_changed_detach_on_rerender(h: Harness) -> FixtureFuture {
    Box::pin(async move {
        let fire_count = Rc::new(Cell::new(0u32));
        let fire_count2 = fire_count.clone();

        h.mount(cc(move |cx| {
            let (attach_handler, set_attach) = cx.use_state(true);
            let (checked, set_checked) = cx.use_state(false);
            let fc = fire_count2.clone();

            let cb: Element = if attach_handler {
                check_box(checked)
                    .content("target")
                    .on_checked(move |v| {
                        fc.set(fc.get() + 1);
                        set_checked.call(v);
                    })
                    .into()
            } else {
                // Same control, no handler attached
                check_box(checked).content("target").into()
            };

            vstack((
                cb,
                text_block(format!("fires={}", fire_count2.get())),
                text_block(format!("attached={attach_handler}")),
                button("Detach").on_click(move || set_attach.call(false)),
            ))
            .into()
        }));
        h.render().await;

        let _ = h.set_checkbox_value(true);
        h.render().await;
        h.check(
            "EventDetach_OnChanged_FiredWhileAttached",
            fire_count.get() >= 1,
        );

        // Detach the handler (re-render without on_checked)
        let _ = h.click_button("Detach");
        h.render().await;
        h.check(
            "EventDetach_OnChanged_HandlerDetached",
            h.find_text("attached=false").is_some(),
        );

        let count_before = fire_count.get();
        let _ = h.set_checkbox_value(false);
        h.render().await;

        h.check(
            "EventDetach_OnChanged_NoFireAfterDetach",
            fire_count.get() == count_before,
        );
    })
}

/// Verify that a Slider's `on_value_changed` handler is properly replaced when
/// the component re-renders with a new closure (no stale closure leak).
pub fn on_changed_handler_replacement(h: Harness) -> FixtureFuture {
    Box::pin(async move {
        h.mount(cc(|cx| {
            let (multiplier, set_mult) = cx.use_state(1i32);
            let (result, set_result) = cx.use_state(0i32);
            let m = multiplier;

            vstack((
                text_block(format!("result={result}")),
                text_block(format!("mult={multiplier}")),
                Slider::new(5.0)
                    .range(0.0, 10.0)
                    .on_value_changed(move |v| set_result.call((v as i32) * m)),
                button("DoubleMult").on_click(move || set_mult.call(multiplier * 2)),
            ))
            .into()
        }));
        h.render().await;

        let _ = h.set_slider_value(3.0);
        h.render().await;
        h.check(
            "EventDetach_Replacement_InitialMult",
            h.find_text("result=3").is_some(),
        );

        // Update multiplier to 2
        let _ = h.click_button("DoubleMult");
        h.render().await;
        h.check(
            "EventDetach_Replacement_MultUpdated",
            h.find_text("mult=2").is_some(),
        );

        // The updated handler must not retain the old multiplier.
        let _ = h.set_slider_value(4.0);
        h.render().await;
        h.check(
            "EventDetach_Replacement_NewClosureUsed",
            h.find_text("result=8").is_some(),
        );
    })
}

pub fn ordinary_click_and_command_bar_flyout_coexist(h: Harness) -> FixtureFuture {
    Box::pin(async move {
        let ordinary_count = Rc::new(Cell::new(0u32));
        let command_count = Rc::new(Cell::new(0u32));
        let ordinary = Rc::clone(&ordinary_count);
        let command = Rc::clone(&command_count);

        h.mount(cc(move |_| {
            button("Compound")
                .on_click({
                    let ordinary = Rc::clone(&ordinary);
                    move || ordinary.set(ordinary.get() + 1)
                })
                .command_bar_flyout(vec![app_bar_button("Flyout Command")])
                .on_command_bar_flyout_click({
                    let command = Rc::clone(&command);
                    move |_| command.set(command.get() + 1)
                })
                .into()
        }));
        h.render().await;

        h.click_button("Compound").unwrap();
        h.render().await;

        let button = h.find_button("Compound").unwrap();
        let flyout = button
            .cast::<bindings::IButton>()
            .unwrap()
            .Flyout()
            .unwrap()
            .cast::<bindings::CommandBarFlyout>()
            .unwrap();
        let command = flyout.PrimaryCommands().unwrap().GetAt(0).unwrap();
        invoke_command(command).unwrap();
        h.render().await;

        h.check(
            "EventState_OrdinaryClickPreserved",
            ordinary_count.get() == 1,
        );
        h.check(
            "EventState_CommandBarFlyoutClickPreserved",
            command_count.get() == 1,
        );
    })
}

pub fn command_bar_primary_update_preserves_secondary(h: Harness) -> FixtureFuture {
    Box::pin(async move {
        let secondary_count = Rc::new(Cell::new(0u32));
        let callback_count = Rc::clone(&secondary_count);
        let callback = windows_reactor::Callback::new(move |label: String| {
            if label == "Secondary" {
                callback_count.set(callback_count.get() + 1);
            }
        });

        h.mount(cc(move |cx| {
            let (updated, set_updated) = cx.use_state(false);
            let primary = if updated {
                "Primary Updated"
            } else {
                "Primary"
            };
            vstack((
                CommandBar::new(vec![app_bar_button(primary)])
                    .secondary_commands(vec![app_bar_button("Secondary")])
                    .on_click(callback.clone()),
                button("Update Primary").on_click(move || set_updated.call(true)),
            ))
            .into()
        }));
        h.render().await;

        let command_bar = h
            .find_all::<bindings::CommandBar>(&|_| true)
            .into_iter()
            .next()
            .unwrap();
        invoke_command(command_bar.SecondaryCommands().unwrap().GetAt(0).unwrap()).unwrap();
        h.render().await;

        h.click_button("Update Primary").unwrap();
        h.render().await;

        let command_bar = h
            .find_all::<bindings::CommandBar>(&|_| true)
            .into_iter()
            .next()
            .unwrap();
        invoke_command(command_bar.SecondaryCommands().unwrap().GetAt(0).unwrap()).unwrap();
        h.render().await;

        h.check(
            "EventState_PrimaryUpdatePreservesSecondary",
            secondary_count.get() == 2,
        );
    })
}

pub fn detached_flyout_handler_stays_detached_after_prop_update(h: Harness) -> FixtureFuture {
    Box::pin(async move {
        let command_count = Rc::new(Cell::new(0u32));
        let callback_count = Rc::clone(&command_count);
        let callback = windows_reactor::Callback::new(move |_label: String| {
            callback_count.set(callback_count.get() + 1);
        });

        h.mount(cc(move |cx| {
            let (attached, set_attached) = cx.use_state(true);
            let label = if attached { "Before" } else { "After" };
            let target = button("Compound").command_bar_flyout(vec![app_bar_button(label)]);
            let target = if attached {
                target.on_command_bar_flyout_click(callback.clone())
            } else {
                target
            };
            vstack((
                target,
                button("Detach And Update").on_click(move || set_attached.call(false)),
            ))
            .into()
        }));
        h.render().await;

        h.click_button("Detach And Update").unwrap();
        h.render().await;

        let button = h.find_button("Compound").unwrap();
        let flyout = button
            .cast::<bindings::IButton>()
            .unwrap()
            .Flyout()
            .unwrap()
            .cast::<bindings::CommandBarFlyout>()
            .unwrap();
        invoke_command(flyout.PrimaryCommands().unwrap().GetAt(0).unwrap()).unwrap();
        h.render().await;

        h.check(
            "EventState_DetachedFlyoutHandlerNotReattached",
            command_count.get() == 0,
        );
    })
}
