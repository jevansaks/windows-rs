use std::cell::Cell;
use std::rc::Rc;

pub use super::*;

mod child;
mod context;
mod diff_helpers;
mod logical_tree;
mod modifiers;
mod mounted_tree;
mod templated;
mod widget_dispatch;
mod wrappers;

pub use self::child::compute_lis;
use self::logical_tree::{LogicalNodeId, LogicalNodeKind, LogicalParentGuard, LogicalWrapperNode};
use self::mounted_tree::{MountedTree, OwnedLifecycleNode};

fn output_is_empty(output: MountedOutput) -> bool {
    output.native.is_none() && output.logical.is_none()
}

#[derive(Default)]
struct ReconcilePass {
    forced_nodes: rustc_hash::FxHashSet<LogicalNodeId>,
    forced_controls: rustc_hash::FxHashSet<ControlId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct LogicalSlotId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MountedOutput {
    slot: LogicalSlotId,
    native: Option<ControlId>,
    logical: Option<LogicalNodeId>,
}

impl MountedOutput {
    const fn empty(slot: LogicalSlotId) -> Self {
        Self {
            slot,
            native: None,
            logical: None,
        }
    }
}

struct HostContext {
    context_stack: Rc<ContextStack>,
    marshaller: Option<UiMarshaller>,
    host_id: HostId,
    inner_size: Rc<Cell<WindowSize>>,
    dpi: Rc<Cell<u32>>,
    request_rerender: Rc<dyn Fn()>,
}

impl HostContext {
    fn new() -> Self {
        Self {
            context_stack: Rc::new(ContextStack::new()),
            marshaller: None,
            host_id: HostId::next(),
            inner_size: Rc::new(Cell::new(WindowSize::default())),
            dpi: Rc::new(Cell::new(96_u32)),
            request_rerender: Rc::new(|| {}),
        }
    }
}

/// Diff/apply engine that drives a [`Backend`] from successive [`Element`] trees.
pub struct Reconciler<B: Backend> {
    pub backend: B,
    tree: MountedTree,
    pass: ReconcilePass,
    host: HostContext,
    stats: ReconcileStats,
    root_output: Option<MountedOutput>,
    next_slot_id: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    pub elements_skipped: u64,
    pub elements_diffed: u64,
    pub ui_elements_created: u64,
}

pub struct ComponentInstance {
    node_id: LogicalNodeId,
    parent: Option<LogicalNodeId>,
    native_root: Option<ControlId>,
    rendered_output: MountedOutput,
    pub render_cx: RenderCx,
    pub last_rendered: Element,
    pub last_obj: Rc<dyn ComponentObject>,
    pub read_contexts: rustc_hash::FxHashSet<ContextId>,
}

impl<B: Backend + 'static> Reconciler<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            tree: MountedTree::default(),
            pass: ReconcilePass::default(),
            host: HostContext::new(),
            stats: ReconcileStats::default(),
            root_output: None,
            next_slot_id: 0,
        }
    }

    pub fn set_marshaller(&mut self, marshaller: Option<UiMarshaller>) {
        self.host.marshaller = marshaller;
    }

    pub fn set_host_id(&mut self, host_id: HostId) {
        self.host.host_id = host_id;
    }

    #[cfg(feature = "test")]
    pub fn flush_deferred_unmounts(&mut self) {
        let outputs = std::mem::take(&mut self.tree.templated.deferred_unmounts);
        for output in outputs {
            self.unmount_output(output);
        }
    }

    #[cfg(feature = "test")]
    pub fn defer_templated_unmounts_for_test(&mut self, defer: bool) {
        self.tree.templated.defer_unmounts = defer;
    }

    pub fn context_stack_handle(&self) -> Rc<ContextStack> {
        Rc::clone(&self.host.context_stack)
    }

    fn is_node_state_dirty(&self, node_id: LogicalNodeId) -> bool {
        self.tree
            .logical
            .instance(node_id)
            .is_some_and(|inst| inst.render_cx.peek_state_dirty())
    }

    fn is_control_forced(&self, id: ControlId) -> bool {
        self.pass.forced_controls.contains(&id)
    }

    fn is_output_forced(&self, output: MountedOutput) -> bool {
        output.native.is_some_and(|id| self.is_control_forced(id))
            || output
                .logical
                .is_some_and(|id| self.pass.forced_nodes.contains(&id))
    }

    pub fn reset_stats(&mut self) {
        self.stats = ReconcileStats::default();
    }

    pub fn stats(&self) -> ReconcileStats {
        self.stats
    }

    #[cfg(feature = "test")]
    pub fn debug_forced_components_len(&self) -> usize {
        self.pass.forced_nodes.len()
    }

    #[cfg(feature = "test")]
    pub fn debug_logical_component_count(&self) -> usize {
        self.tree.logical.component_count()
    }

    #[cfg(feature = "test")]
    pub fn debug_logical_node_count(&self) -> usize {
        self.tree.logical.node_count()
    }

    fn allocate_logical_node_id(&mut self) -> LogicalNodeId {
        self.tree.logical.allocate_id()
    }

    fn allocate_slot_id(&mut self) -> LogicalSlotId {
        let id = LogicalSlotId(self.next_slot_id);
        self.next_slot_id = self
            .next_slot_id
            .checked_add(1)
            .expect("logical slot id overflow");
        id
    }

    fn enter_logical_parent(&self, node_id: LogicalNodeId) -> LogicalParentGuard {
        self.tree.logical.enter_parent(node_id)
    }

    fn add_forced_node_path(&mut self, node_id: LogicalNodeId) {
        let mut current = Some(node_id);
        while let Some(id) = current {
            self.pass.forced_nodes.insert(id);
            if let Some(native_root) = self.tree.logical.node_native_root(id) {
                let mut control = Some(native_root);
                while let Some(id) = control {
                    if !self.pass.forced_controls.insert(id) {
                        break;
                    }
                    control = self.tree.parent(id);
                }
            } else if let Some(owner) = self.tree.logical_owner(id) {
                let mut control = Some(owner);
                while let Some(id) = control {
                    if !self.pass.forced_controls.insert(id) {
                        break;
                    }
                    control = self.tree.parent(id);
                }
            }
            current = self.tree.logical.node_parent(id);
        }
    }

    fn add_forced_node_paths(&mut self, node_ids: impl IntoIterator<Item = LogicalNodeId>) {
        for node_id in node_ids {
            self.add_forced_node_path(node_id);
        }
    }

    #[cfg(feature = "test")]
    pub fn force_components_at_control_for_test(&mut self, id: ControlId) {
        let node_ids = self.tree.logical.projected_node_ids(id);
        self.add_forced_node_paths(node_ids);
    }

    pub fn acquire_control(&mut self, kind: ControlKind) -> ControlId {
        self.stats.ui_elements_created += 1;
        let id = self.backend.create(kind);

        self.tree.logical.discard_projected_nodes(id);
        self.tree.templated.lists.remove(&id);
        self.tree.register(id, Some(kind));
        id
    }

    fn take_component_instance(&mut self, node_id: LogicalNodeId) -> Option<ComponentInstance> {
        self.tree.logical.take_component(node_id)
    }

    #[cfg(feature = "test")]
    pub fn debug_appeared_listener_count(&self) -> usize {
        self.tree.logical.appeared_listener_count()
    }

    #[cfg(feature = "test")]
    pub fn debug_disappeared_listener_count(&self) -> usize {
        self.tree.logical.disappeared_listener_count()
    }

    pub fn reconcile(
        &mut self,
        old: Option<&Element>,
        new: &Element,
        existing: Option<ControlId>,
        request_rerender: Rc<dyn Fn()>,
    ) -> Option<ControlId> {
        self.host.request_rerender = request_rerender;
        let output = match (old, self.root_output, existing) {
            (Some(old_el), Some(output), _) => {
                let seeded = self.force_state_dirty_components();
                let result = self.update_output(old_el, new, output);
                debug_assert!(
                    seeded
                        .iter()
                        .all(|node_id| !self.is_node_state_dirty(*node_id)),
                    "a state-dirty component was not re-rendered by the pass"
                );
                result
            }
            (Some(old_el), None, Some(id)) => {
                let seeded = self.force_state_dirty_components();
                let logical = self
                    .tree
                    .logical
                    .current_node(id, LogicalNodeKind::Component);
                let slot = self.allocate_slot_id();
                let result = self.update_output(
                    old_el,
                    new,
                    MountedOutput {
                        slot,
                        native: Some(id),
                        logical,
                    },
                );
                debug_assert!(
                    seeded
                        .iter()
                        .all(|node_id| !self.is_node_state_dirty(*node_id)),
                    "a state-dirty component was not re-rendered by the pass"
                );
                result
            }
            _ => self.mount_output(new),
        };
        self.root_output = (!output_is_empty(output)).then_some(output);
        #[cfg(debug_assertions)]
        {
            self.tree.logical.debug_assert_invariants();
            self.tree.debug_assert_native_ownership();
        }
        output.native
    }

    /// Forces dirty components to render even when unchanged parents can be skipped.
    fn force_state_dirty_components(&mut self) -> Vec<LogicalNodeId> {
        let dirty = self.tree.logical.state_dirty_nodes();
        if !dirty.is_empty() {
            self.add_forced_node_paths(dirty.iter().copied());
        }
        dirty
    }

    fn mount_output(&mut self, el: &Element) -> MountedOutput {
        let slot = self.allocate_slot_id();
        match el {
            Element::Component(ce) => {
                return self.mount_component_output(ce, slot);
            }
            Element::ErrorBoundary(eb) => {
                return self.mount_error_boundary_output_node(eb, slot);
            }
            Element::Provider(pe) => return self.mount_provider_output(pe, slot),
            Element::TemplatedList(tl) => {
                return MountedOutput {
                    slot,
                    native: Some(self.mount_templated_list(tl)),
                    logical: None,
                };
            }
            Element::Custom(c) => {
                return MountedOutput {
                    slot,
                    native: Some(self.mount_custom(c)),
                    logical: None,
                };
            }
            Element::Empty => return MountedOutput::empty(slot),
            _ => {}
        }
        let widget = el.as_widget().unwrap();
        let id = self.mount_widget(widget);
        if let Element::RichTextBlock(rt) = el
            && !rt.paragraphs.is_empty()
        {
            self.backend.set_rich_text_paragraphs(id, &rt.paragraphs);
        }
        MountedOutput {
            slot,
            native: Some(id),
            logical: None,
        }
    }

    pub fn mount(&mut self, el: &Element) -> Option<ControlId> {
        self.mount_output(el).native
    }

    fn update_output(
        &mut self,
        old: &Element,
        new: &Element,
        old_output: MountedOutput,
    ) -> MountedOutput {
        let forced = old_output
            .native
            .is_some_and(|id| self.is_control_forced(id))
            || old_output
                .logical
                .is_some_and(|id| self.pass.forced_nodes.contains(&id));
        if can_skip_update(old, new) && !forced {
            self.stats.elements_skipped += 1;
            return old_output;
        }
        self.stats.elements_diffed += 1;

        if !old.kind_matches(new) {
            self.unmount_output(old_output);
            return self.mount_output(new);
        }

        match (old, new) {
            (Element::Component(o), Element::Component(n)) => {
                return self.update_component_output(o, n, old_output);
            }
            (Element::ErrorBoundary(o), Element::ErrorBoundary(n)) => {
                return self.update_error_boundary_output(o, n, old_output);
            }
            (Element::Provider(o), Element::Provider(n)) => {
                return self.update_provider_output(o, n, old_output);
            }
            (Element::TemplatedList(o), Element::TemplatedList(n)) => {
                let id = old_output.native.unwrap();
                self.update_templated_list(o, n, id);
                return old_output;
            }
            (Element::Custom(o), Element::Custom(n)) => {
                let id = old_output.native.unwrap();
                return MountedOutput {
                    native: Some(self.update_custom(o, n, id)),
                    ..old_output
                };
            }
            (Element::Empty, Element::Empty) => return old_output,
            _ => {}
        }

        let id = old_output
            .native
            .expect("widget update requires a native output");
        let (Some(ow), Some(nw)) = (old.as_widget(), new.as_widget()) else {
            unreachable!("kind_matches guarantees same variant; non-widget variants handled above");
        };
        self.update_widget(ow, nw, id);
        if let (Element::RichTextBlock(o), Element::RichTextBlock(n)) = (old, new)
            && o.paragraphs != n.paragraphs
        {
            self.backend.set_rich_text_paragraphs(id, &n.paragraphs);
        }
        old_output
    }

    pub fn update(&mut self, old: &Element, new: &Element, id: ControlId) -> Option<ControlId> {
        let output = MountedOutput {
            slot: self.allocate_slot_id(),
            native: Some(id),
            logical: self.tree.logical.current_projection(id),
        };
        self.update_output(old, new, output).native
    }

    fn remove_logical_subtree(&mut self, root: LogicalNodeId) {
        self.tree.logical.remove_subtrees([root]);
    }

    fn remove_logical_subtrees(&mut self, roots: impl IntoIterator<Item = LogicalNodeId>) {
        self.tree.logical.remove_subtrees(roots);
    }

    fn unmount_output(&mut self, output: MountedOutput) {
        if let Some(native) = output.native {
            self.unmount(native);
        }
        if let Some(logical) = output.logical {
            self.remove_logical_subtree(logical);
        }
    }

    pub fn unmount_root(&mut self) {
        if let Some(output) = self.root_output.take() {
            self.unmount_output(output);
        }
    }

    pub fn unmount(&mut self, id: ControlId) {
        let mut nodes = vec![id];
        let mut next = 0;
        while next < nodes.len() {
            let node = nodes[next];
            next += 1;
            self.tree.extend_owned_children(node, &mut nodes);
        }

        let mut logical_roots = Vec::new();
        for node in &nodes {
            self.tree
                .extend_owned_logical_roots(*node, &mut logical_roots);
        }
        logical_roots.sort_unstable_by_key(|node| node.0);
        logical_roots.dedup();
        self.remove_logical_subtrees(logical_roots);

        for node in nodes.into_iter().rev() {
            self.tree.logical.remove_projected_nodes(node);

            self.tree.templated.lists.remove(&node);

            // Give external resources a chance to detach before native destroy.
            if let Some(lifecycle) = self.tree.take_before_unmount(node) {
                if let Some(reference) = lifecycle.reference {
                    reference.set_native(None);
                }
                if let Some(callback) = lifecycle.callback {
                    callback.invoke(self.backend.get_native_element(node));
                }
            }

            if let Some(handle) = self.tree.take_custom(node) {
                handle.before_destroy(node, &mut self.backend);
            }

            self.tree.remove_node(node);
            self.backend.destroy(node);
        }
    }

    fn append_output_tracked(&mut self, parent: ControlId, output: MountedOutput) {
        self.tree.append_logical_child(parent, output);
        if let Some(native) = output.native {
            self.tree.append_child(parent, native);
            self.backend.append_child(parent, native);
        }
    }

    fn insert_output_tracked(&mut self, parent: ControlId, index: usize, output: MountedOutput) {
        let index = self.tree.insert_logical_child(parent, index, output);
        if let Some(native) = output.native {
            let native_index = self.tree.native_index(parent, index);
            self.tree.insert_child(parent, native_index, native);
            self.backend.insert_child(parent, native_index, native);
        }
    }

    fn replace_output_tracked(
        &mut self,
        parent: ControlId,
        index: usize,
        output: MountedOutput,
    ) -> MountedOutput {
        let old = self
            .tree
            .logical_child(parent, index)
            .expect("logical child slot missing");
        let native_index = self.tree.native_index(parent, index);
        self.tree.replace_logical_child(parent, index, output);
        match (old.native, output.native) {
            (Some(old), Some(new)) if old != new => {
                self.tree.replace_child(parent, native_index, new);
                self.backend.replace_child(parent, native_index, new);
            }
            (Some(_), Some(_)) => {}
            (Some(_), None) => {
                self.tree.remove_child(parent, native_index);
                self.backend.remove_child(parent, native_index);
            }
            (None, Some(new)) => {
                self.tree.insert_child(parent, native_index, new);
                self.backend.insert_child(parent, native_index, new);
            }
            (None, None) => {}
        }
        old
    }

    fn remove_output_tracked(&mut self, parent: ControlId, index: usize) -> MountedOutput {
        let output = self
            .tree
            .logical_child(parent, index)
            .expect("logical child slot missing");
        let native_index = self.tree.native_index(parent, index);
        self.tree.remove_logical_child(parent, index);
        if output.native.is_some() {
            self.tree.remove_child(parent, native_index);
            self.backend.remove_child(parent, native_index);
        }
        output
    }

    fn move_output_tracked(&mut self, parent: ControlId, from: usize, to: usize) {
        if self
            .tree
            .logical_children(parent)
            .iter()
            .all(|output| output.native.is_some())
        {
            self.tree.move_logical_child(parent, from, to);
            if from != to {
                self.tree.move_child(parent, from, to);
                self.backend.move_child(parent, from, to);
            }
            return;
        }
        let output = self
            .tree
            .logical_child(parent, from)
            .expect("logical child slot missing");
        let from_native = output.native.map(|_| self.tree.native_index(parent, from));
        self.tree.move_logical_child(parent, from, to);
        if let Some(native) = from_native {
            let to_native = self.tree.native_index(parent, to);
            if native != to_native {
                self.tree.move_child(parent, native, to_native);
                self.backend.move_child(parent, native, to_native);
            }
        }
    }

    pub fn append_child_tracked(&mut self, parent: ControlId, child: ControlId) {
        self.tree.append_child(parent, child);
        self.backend.append_child(parent, child);
    }

    pub fn remove_child_tracked(&mut self, parent: ControlId, index: usize) {
        self.tree.remove_child(parent, index);
        self.backend.remove_child(parent, index);
    }

    pub fn replace_child_tracked(&mut self, parent: ControlId, index: usize, new: ControlId) {
        self.tree.replace_child(parent, index, new);
        self.backend.replace_child(parent, index, new);
    }

    pub fn move_child_tracked(&mut self, parent: ControlId, from: usize, to: usize) {
        self.tree.move_child(parent, from, to);
        self.backend.move_child(parent, from, to);
    }

    pub fn insert_child_tracked(&mut self, parent: ControlId, index: usize, child: ControlId) {
        let index = self.tree.insert_child(parent, index, child);
        self.backend.insert_child(parent, index, child);
    }

    pub fn child_at(&self, parent: ControlId, i: usize) -> Option<ControlId> {
        self.tree.child(parent, i)
    }

    pub fn notify_theme_changed(&mut self) {
        self.backend.on_theme_changed();
    }

    pub fn reconcile_children_positional(
        &mut self,
        parent: ControlId,
        old: &[Element],
        new: &[Element],
    ) {
        child::reconcile_positional(self, parent, old, new);
    }

    pub fn reconcile_children(&mut self, parent: ControlId, old: &[Element], new: &[Element]) {
        child::reconcile(self, parent, old, new);
    }

    pub fn set_inner_size_cell(&mut self, cell: Rc<Cell<WindowSize>>) {
        self.host.inner_size = cell;
    }

    pub fn set_dpi_cell(&mut self, cell: Rc<Cell<u32>>) {
        self.host.dpi = cell;
    }

    pub fn clear_forced_components(&mut self) {
        self.pass.forced_nodes.clear();
        self.pass.forced_controls.clear();
    }
}

/// Retains all logical child positions, including empty output.
pub fn collect_live(slice: &[Element]) -> Vec<&Element> {
    slice.iter().collect()
}
