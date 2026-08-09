use std::cell::{Cell, RefCell};
use std::rc::Rc;

use test_reactor::{Op, RecordingBackend};
use windows_reactor::Component;
use windows_reactor::Element;
use windows_reactor::Reconciler;
use windows_reactor::RenderCx;
use windows_reactor::component;
use windows_reactor::error_boundary;
use windows_reactor::text_block;

struct Boom {
    boom: Rc<Cell<bool>>,
}
impl Component for Boom {
    fn render(&self, _props: &(), _cx: &mut RenderCx) -> Element {
        assert!(!self.boom.get(), "simulated render failure");
        text_block("healthy").into()
    }
}

struct CleanupChild {
    events: Rc<RefCell<Vec<(&'static str, bool)>>>,
}

impl Component<u8> for CleanupChild {
    fn render(&self, step: &u8, cx: &mut RenderCx) -> Element {
        let events = Rc::clone(&self.events);
        cx.use_effect_with_cleanup((), move || {
            Some(move || {
                events
                    .borrow_mut()
                    .push(("child", std::thread::panicking()));
            })
        });
        assert!(*step < 2, "simulated update failure");
        text_block("healthy").into()
    }
}

struct CleanupParent {
    events: Rc<RefCell<Vec<(&'static str, bool)>>>,
    panic_on_cleanup: bool,
}

impl Component<u8> for CleanupParent {
    fn render(&self, step: &u8, cx: &mut RenderCx) -> Element {
        let events = Rc::clone(&self.events);
        let panic_on_cleanup = self.panic_on_cleanup;
        cx.use_effect_with_cleanup((), move || {
            Some(move || {
                events
                    .borrow_mut()
                    .push(("parent", std::thread::panicking()));
                assert!(!panic_on_cleanup, "simulated cleanup failure");
            })
        });
        component(
            CleanupChild {
                events: Rc::clone(&self.events),
            },
            *step,
        )
    }
}

fn reconcile(
    r: &mut Reconciler<RecordingBackend>,
    old: Option<&Element>,
    new: &Element,
    existing: Option<windows_reactor::ControlId>,
) -> Option<windows_reactor::ControlId> {
    r.reconcile(old, new, existing, Rc::new(|| {}))
}

#[test]
fn panicking_child_on_mount_substitutes_fallback() {
    let boom = Rc::new(Cell::new(true));
    let child = component(
        Boom {
            boom: Rc::clone(&boom),
        },
        (),
    );
    let tree = error_boundary(child, |msg| text_block(format!("fallback: {msg}")).into());

    let mut r = Reconciler::new(RecordingBackend::new());
    let id = reconcile(&mut r, None, &tree, None);
    assert!(id.is_some(), "error boundary must mount a fallback");

    let set_texts: Vec<&Op> = r
        .backend
        .ops
        .iter()
        .filter(|op| {
            matches!(
                op,
                Op::SetProp {
                    prop: windows_reactor::Prop::Text,
                    ..
                }
            )
        })
        .collect();
    assert!(
        set_texts.iter().any(|op| matches!(
            op,
            Op::SetProp {
                value: windows_reactor::PropValue::Str(s),
                ..
            } if s.contains("fallback: simulated render failure")
        )),
        "expected fallback text, got {set_texts:?}"
    );
}

#[test]
fn recovery_after_fix_mounts_healthy_child() {
    let boom = Rc::new(Cell::new(true));
    let child_a = component(
        Boom {
            boom: Rc::clone(&boom),
        },
        (),
    );
    let tree_a = error_boundary(child_a, |_| text_block("fallback").into());

    let mut r = Reconciler::new(RecordingBackend::new());
    let id = reconcile(&mut r, None, &tree_a, None).unwrap();
    assert_eq!(
        r.debug_logical_node_count(),
        1,
        "fallback state belongs to the mounted boundary node"
    );

    boom.set(false);
    let child_b = component(
        Boom {
            boom: Rc::clone(&boom),
        },
        (),
    );
    let tree_b = error_boundary(child_b, |_| text_block("fallback").into());
    let id = reconcile(&mut r, Some(&tree_a), &tree_b, Some(id)).unwrap();
    assert_eq!(
        r.debug_logical_node_count(),
        2,
        "recovery mounts the healthy component beneath the existing boundary"
    );

    let saw_healthy = r.backend.ops.iter().any(|op| {
        matches!(
            op,
            Op::SetProp {
                prop: windows_reactor::Prop::Text,
                value: windows_reactor::PropValue::Str(s),
                ..
            } if s == "healthy"
        )
    });
    assert!(saw_healthy, "expected healthy mount after recovery");

    r.unmount(id);
    assert_eq!(r.debug_logical_node_count(), 0);
}

#[test]
fn nested_boundaries_catch_at_the_nearest_one() {
    let boom = Rc::new(Cell::new(true));
    let child = component(
        Boom {
            boom: Rc::clone(&boom),
        },
        (),
    );

    let inner = error_boundary(child, |_| text_block("inner-fallback").into());
    let outer = error_boundary(inner, |_| text_block("outer-fallback").into());

    let mut r = Reconciler::new(RecordingBackend::new());
    let _ = reconcile(&mut r, None, &outer, None);
    assert_eq!(
        r.debug_logical_node_count(),
        2,
        "both nested boundaries retain independent logical identity"
    );

    let saw_inner = r.backend.ops.iter().any(|op| {
        matches!(
            op,
            Op::SetProp {
                prop: windows_reactor::Prop::Text,
                value: windows_reactor::PropValue::Str(s),
                ..
            } if s == "inner-fallback"
        )
    });
    let saw_outer = r.backend.ops.iter().any(|op| {
        matches!(
            op,
            Op::SetProp {
                prop: windows_reactor::Prop::Text,
                value: windows_reactor::PropValue::Str(s),
                ..
            } if s == "outer-fallback"
        )
    });
    assert!(saw_inner, "inner boundary must catch");
    assert!(!saw_outer, "outer boundary must not fire");
}

#[test]
fn panicking_child_update_runs_effect_cleanup_once() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let child_a = component(
        CleanupParent {
            events: Rc::clone(&events),
            panic_on_cleanup: false,
        },
        0,
    );
    let tree_a = error_boundary(child_a, |_| text_block("fallback").into());

    let mut r = Reconciler::new(RecordingBackend::new());
    let id = reconcile(&mut r, None, &tree_a, None).unwrap();
    assert!(events.borrow().is_empty());

    let child_b = component(
        CleanupParent {
            events: Rc::clone(&events),
            panic_on_cleanup: false,
        },
        1,
    );
    let tree_b = error_boundary(child_b, |_| text_block("fallback").into());
    let id = reconcile(&mut r, Some(&tree_a), &tree_b, Some(id)).unwrap();
    assert!(
        events.borrow().is_empty(),
        "a successful update must return cleanup ownership to the logical tree"
    );

    let child_c = component(
        CleanupParent {
            events: Rc::clone(&events),
            panic_on_cleanup: false,
        },
        2,
    );
    let tree_c = error_boundary(child_c, |_| text_block("fallback").into());
    let id = reconcile(&mut r, Some(&tree_b), &tree_c, Some(id)).unwrap();

    assert_eq!(
        &*events.borrow(),
        &[("child", false), ("parent", false)],
        "the caught subtree must clean up child-first outside active unwinding"
    );

    r.unmount(id);
    assert_eq!(
        &*events.borrow(),
        &[("child", false), ("parent", false)],
        "fallback unmount must not clean up the failed component twice"
    );
}

#[test]
fn cleanup_panic_during_error_recovery_reaches_outer_boundary() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let make_tree = |step| {
        let child = component(
            CleanupParent {
                events: Rc::clone(&events),
                panic_on_cleanup: true,
            },
            step,
        );
        let inner = error_boundary(child, |_| text_block("inner fallback").into());
        error_boundary(inner, |msg| {
            text_block(format!("outer fallback: {msg}")).into()
        })
    };

    let tree_a = make_tree(0);
    let mut r = Reconciler::new(RecordingBackend::new());
    let id = reconcile(&mut r, None, &tree_a, None).unwrap();

    let tree_b = make_tree(2);
    let id = reconcile(&mut r, Some(&tree_a), &tree_b, Some(id)).unwrap();

    assert_eq!(
        &*events.borrow(),
        &[("child", false), ("parent", false)],
        "cleanup panic must occur after render unwinding has ended"
    );
    assert!(r.backend.ops.iter().any(|op| {
        matches!(
            op,
            Op::SetProp {
                prop: windows_reactor::Prop::Text,
                value: windows_reactor::PropValue::Str(text),
                ..
            } if text == "outer fallback: simulated cleanup failure"
        )
    }));

    r.unmount(id);
    assert_eq!(
        &*events.borrow(),
        &[("child", false), ("parent", false)],
        "failed cleanup must not be invoked again"
    );
}
