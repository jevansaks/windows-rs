use std::cell::Cell;
use std::rc::Rc;

use rustc_hash::FxHashMap;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct LogicalNodeId(pub(super) u64);

enum ProjectedNodes {
    Inline { nodes: [LogicalNodeId; 2], len: u8 },
    Heap(Vec<LogicalNodeId>),
}

impl Default for ProjectedNodes {
    fn default() -> Self {
        Self::Inline {
            nodes: [LogicalNodeId(0); 2],
            len: 0,
        }
    }
}

impl ProjectedNodes {
    fn as_slice(&self) -> &[LogicalNodeId] {
        match self {
            Self::Inline { nodes, len } => &nodes[..*len as usize],
            Self::Heap(nodes) => nodes,
        }
    }

    fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    fn last(&self) -> Option<LogicalNodeId> {
        self.as_slice().last().copied()
    }

    fn push(&mut self, node_id: LogicalNodeId) {
        match self {
            Self::Inline { nodes, len } if *len < 2 => {
                nodes[*len as usize] = node_id;
                *len += 1;
            }
            Self::Inline { nodes, .. } => {
                *self = Self::Heap(vec![nodes[0], nodes[1], node_id]);
            }
            Self::Heap(nodes) => nodes.push(node_id),
        }
    }

    fn pop(&mut self) -> Option<LogicalNodeId> {
        match self {
            Self::Inline { nodes, len } => {
                if *len == 0 {
                    None
                } else {
                    *len -= 1;
                    Some(nodes[*len as usize])
                }
            }
            Self::Heap(nodes) => nodes.pop(),
        }
    }

    fn drain(self, mut f: impl FnMut(LogicalNodeId)) {
        match self {
            Self::Inline { nodes, len } => {
                for node_id in nodes.into_iter().take(len as usize) {
                    f(node_id);
                }
            }
            Self::Heap(nodes) => {
                for node_id in nodes {
                    f(node_id);
                }
            }
        }
    }
}

pub(super) struct LogicalParentGuard {
    active: Rc<Cell<Option<LogicalNodeId>>>,
    previous: Option<LogicalNodeId>,
}

impl Drop for LogicalParentGuard {
    fn drop(&mut self) {
        self.active.set(self.previous);
    }
}

#[derive(Default)]
pub(super) struct MountedLogicalTree {
    components: FxHashMap<LogicalNodeId, ComponentInstance>,
    wrappers: FxHashMap<LogicalNodeId, LogicalWrapperNode>,
    // Logical nodes projecting to a native root, innermost first.
    projections: FxHashMap<ControlId, ProjectedNodes>,
    active_parent: Rc<Cell<Option<LogicalNodeId>>>,
    next_id: u64,
    appeared_listener_count: usize,
    disappeared_listener_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LogicalNodeKind {
    Component,
    Provider,
    ErrorBoundary,
}

pub(super) struct LogicalWrapperNode {
    pub(super) kind: LogicalNodeKind,
    pub(super) node_id: LogicalNodeId,
    pub(super) parent: Option<LogicalNodeId>,
    pub(super) native_root: Option<ControlId>,
    pub(super) child_output: MountedOutput,
    pub(super) rendered: Element,
    pub(super) fallback: bool,
}

impl MountedLogicalTree {
    pub(super) fn allocate_id(&mut self) -> LogicalNodeId {
        let id = LogicalNodeId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("logical component id overflow");
        id
    }

    pub(super) fn enter_parent(&self, node_id: LogicalNodeId) -> LogicalParentGuard {
        let active = Rc::clone(&self.active_parent);
        let previous = active.replace(Some(node_id));
        LogicalParentGuard { active, previous }
    }

    pub(super) fn active_parent(&self) -> Option<LogicalNodeId> {
        self.active_parent.get()
    }

    pub(super) fn instance(&self, node_id: LogicalNodeId) -> Option<&ComponentInstance> {
        self.components.get(&node_id)
    }

    #[cfg(feature = "test")]
    pub(super) fn component_count(&self) -> usize {
        self.components.len()
    }

    #[cfg(feature = "test")]
    pub(super) fn appeared_listener_count(&self) -> usize {
        self.appeared_listener_count
    }

    #[cfg(feature = "test")]
    pub(super) fn disappeared_listener_count(&self) -> usize {
        self.disappeared_listener_count
    }

    pub(super) fn node_kind(&self, node_id: LogicalNodeId) -> Option<LogicalNodeKind> {
        if self.components.contains_key(&node_id) {
            Some(LogicalNodeKind::Component)
        } else {
            self.wrappers.get(&node_id).map(|node| node.kind)
        }
    }

    pub(super) fn node_parent(&self, node_id: LogicalNodeId) -> Option<LogicalNodeId> {
        self.components
            .get(&node_id)
            .and_then(|node| node.parent)
            .or_else(|| self.wrappers.get(&node_id).and_then(|node| node.parent))
    }

    pub(super) fn node_native_root(&self, node_id: LogicalNodeId) -> Option<ControlId> {
        self.components
            .get(&node_id)
            .and_then(|node| node.native_root)
            .or_else(|| {
                self.wrappers
                    .get(&node_id)
                    .and_then(|node| node.native_root)
            })
    }

    pub(super) fn contains_node(&self, node_id: LogicalNodeId) -> bool {
        self.components.contains_key(&node_id) || self.wrappers.contains_key(&node_id)
    }

    #[cfg(any(debug_assertions, feature = "test"))]
    pub(super) fn node_count(&self) -> usize {
        self.components.len() + self.wrappers.len()
    }

    pub(super) fn current_node(
        &self,
        id: ControlId,
        kind: LogicalNodeKind,
    ) -> Option<LogicalNodeId> {
        let node_id = self.projections.get(&id).and_then(ProjectedNodes::last)?;
        (self.node_kind(node_id) == Some(kind)).then_some(node_id)
    }

    pub(super) fn current_projection(&self, id: ControlId) -> Option<LogicalNodeId> {
        self.projections.get(&id).and_then(ProjectedNodes::last)
    }

    pub(super) fn register_component(&mut self, inst: ComponentInstance) {
        if inst.last_obj.has_on_appeared() {
            self.appeared_listener_count += 1;
        }
        if inst.last_obj.has_on_disappeared() {
            self.disappeared_listener_count += 1;
        }
        let node_id = inst.node_id;
        let native_root = inst.native_root;
        let previous = self.components.insert(node_id, inst);
        debug_assert!(previous.is_none(), "logical component registered twice");
        if let Some(id) = native_root {
            self.register_projection(id, node_id);
        }
    }

    pub(super) fn register_wrapper(&mut self, node: LogicalWrapperNode) {
        let native_root = node.native_root;
        let node_id = node.node_id;
        let previous = self.wrappers.insert(node_id, node);
        debug_assert!(previous.is_none(), "logical wrapper registered twice");
        if let Some(id) = native_root {
            self.register_projection(id, node_id);
        }
    }

    pub(super) fn take_component(&mut self, node_id: LogicalNodeId) -> Option<ComponentInstance> {
        let inst = self.components.get(&node_id)?;
        if let Some(id) = inst.native_root {
            self.remove_projection(id, node_id);
        }
        self.remove_component(node_id)
    }

    pub(super) fn take_provider(&mut self, node_id: LogicalNodeId) -> Option<LogicalWrapperNode> {
        self.take_wrapper(node_id, LogicalNodeKind::Provider)
    }

    pub(super) fn take_error_boundary(
        &mut self,
        node_id: LogicalNodeId,
    ) -> Option<LogicalWrapperNode> {
        self.take_wrapper(node_id, LogicalNodeKind::ErrorBoundary)
    }

    fn take_wrapper(
        &mut self,
        node_id: LogicalNodeId,
        kind: LogicalNodeKind,
    ) -> Option<LogicalWrapperNode> {
        debug_assert_eq!(
            self.node_kind(node_id),
            Some(kind),
            "logical wrapper update order disagrees with element nesting"
        );
        let node = self.wrappers.get(&node_id)?;
        if let Some(id) = node.native_root {
            self.remove_projection(id, node_id);
        }
        self.wrappers.remove(&node_id)
    }

    fn remove_projection(&mut self, id: ControlId, node_id: LogicalNodeId) {
        if let Some(nodes) = self.projections.get_mut(&id) {
            debug_assert_eq!(
                nodes.last(),
                Some(node_id),
                "logical projection update order disagrees with element nesting"
            );
            if nodes.last() == Some(node_id) {
                nodes.pop();
            } else if let Some(index) = nodes.as_slice().iter().position(|n| *n == node_id) {
                match nodes {
                    ProjectedNodes::Inline { nodes, len } => {
                        for i in index..(*len as usize - 1) {
                            nodes[i] = nodes[i + 1];
                        }
                        *len -= 1;
                    }
                    ProjectedNodes::Heap(nodes) => {
                        nodes.remove(index);
                    }
                }
            }
            self.remove_empty_projection(id);
        }
    }

    pub(super) fn discard_projected_nodes(&mut self, id: ControlId) {
        if let Some(nodes) = self.projections.remove(&id) {
            nodes.drain(|node_id| {
                if self.remove_component(node_id).is_none() {
                    self.remove_wrapper(node_id);
                }
            });
        }
    }

    #[cfg(feature = "test")]
    pub(super) fn projected_node_ids(&self, id: ControlId) -> Vec<LogicalNodeId> {
        self.projections
            .get(&id)
            .map(|nodes| nodes.as_slice().to_vec())
            .unwrap_or_default()
    }

    pub(super) fn refresh_instance(
        &mut self,
        node_id: LogicalNodeId,
        obj: Rc<dyn ComponentObject>,
        parent: Option<LogicalNodeId>,
        native_root: Option<ControlId>,
    ) {
        let Some(inst) = self.components.get_mut(&node_id) else {
            return;
        };
        inst.last_obj = obj;
        inst.parent = parent;
        inst.native_root = native_root;
    }

    pub(super) fn extend_context_subscribers(
        &self,
        id: ControlId,
        changed: &rustc_hash::FxHashSet<ContextId>,
        affected: &mut Vec<LogicalNodeId>,
    ) {
        let Some(node_ids) = self.projections.get(&id) else {
            return;
        };
        affected.extend(
            node_ids
                .as_slice()
                .iter()
                .filter(|node_id| {
                    self.instance(**node_id).is_some_and(|inst| {
                        inst.read_contexts
                            .iter()
                            .any(|context| changed.contains(context))
                    })
                })
                .copied(),
        );
    }

    pub(super) fn context_subscribers(
        &self,
        changed: &rustc_hash::FxHashSet<ContextId>,
    ) -> Vec<LogicalNodeId> {
        self.components
            .values()
            .filter(|inst| {
                inst.read_contexts
                    .iter()
                    .any(|context| changed.contains(context))
            })
            .map(|inst| inst.node_id)
            .collect()
    }

    pub(super) fn context_subscribers_in_subtree(
        &self,
        root: LogicalNodeId,
        changed: &rustc_hash::FxHashSet<ContextId>,
    ) -> Vec<LogicalNodeId> {
        self.components
            .values()
            .filter(|inst| {
                inst.read_contexts
                    .iter()
                    .any(|context| changed.contains(context))
                    && self.is_descendant(inst.node_id, root)
            })
            .map(|inst| inst.node_id)
            .collect()
    }

    fn is_descendant(&self, node_id: LogicalNodeId, root: LogicalNodeId) -> bool {
        let mut current = Some(node_id);
        while let Some(node) = current {
            if node == root {
                return true;
            }
            current = self.node_parent(node);
        }
        false
    }

    pub(super) fn state_dirty_nodes(&self) -> Vec<LogicalNodeId> {
        self.components
            .iter()
            .filter_map(|(node_id, inst)| inst.render_cx.peek_state_dirty().then_some(*node_id))
            .collect()
    }

    pub(super) fn extend_children(&self, parent: LogicalNodeId, children: &mut Vec<LogicalNodeId>) {
        let start = children.len();
        children.extend(
            self.components
                .values()
                .filter(|node| node.parent == Some(parent))
                .map(|node| node.node_id),
        );
        children.extend(
            self.wrappers
                .values()
                .filter(|node| node.parent == Some(parent))
                .map(|node| node.node_id),
        );
        children[start..].sort_unstable_by_key(|node| node.0);
    }

    fn collect_subtrees(
        &self,
        roots: impl IntoIterator<Item = LogicalNodeId>,
    ) -> Vec<LogicalNodeId> {
        let mut seen = rustc_hash::FxHashSet::default();
        let mut nodes: Vec<_> = roots
            .into_iter()
            .filter(|root| self.contains_node(*root))
            .filter(|root| seen.insert(*root))
            .collect();
        if nodes.is_empty() {
            return nodes;
        }

        let mut children: FxHashMap<LogicalNodeId, Vec<LogicalNodeId>> = FxHashMap::default();
        for node in self.components.values() {
            if let Some(parent) = node.parent {
                children.entry(parent).or_default().push(node.node_id);
            }
        }
        for node in self.wrappers.values() {
            if let Some(parent) = node.parent {
                children.entry(parent).or_default().push(node.node_id);
            }
        }

        let mut index = 0;
        while index < nodes.len() {
            let parent = nodes[index];
            index += 1;
            if let Some(logical_children) = children.get(&parent) {
                nodes.extend(
                    logical_children
                        .iter()
                        .copied()
                        .filter(|child| seen.insert(*child)),
                );
            }
        }
        nodes
    }

    pub(super) fn remove_subtrees(&mut self, roots: impl IntoIterator<Item = LogicalNodeId>) {
        let nodes = self.collect_subtrees(roots);
        for node_id in &nodes {
            if let Some(native) = self.node_native_root(*node_id) {
                self.remove_projection(native, *node_id);
            }
        }
        for node_id in nodes.into_iter().rev() {
            if let Some(mut inst) = self.remove_component(node_id) {
                inst.render_cx.run_cleanups();
            } else {
                self.remove_wrapper(node_id);
            }
        }
    }

    pub(super) fn remove_projected_nodes(&mut self, id: ControlId) {
        if let Some(nodes) = self.projections.remove(&id) {
            nodes.drain(|node_id| {
                if let Some(mut inst) = self.remove_component(node_id) {
                    inst.render_cx.run_cleanups();
                } else {
                    self.remove_wrapper(node_id);
                }
            });
        }
    }

    fn remove_component(&mut self, node_id: LogicalNodeId) -> Option<ComponentInstance> {
        let inst = self.components.remove(&node_id)?;
        if inst.last_obj.has_on_appeared() {
            debug_assert!(
                self.appeared_listener_count > 0,
                "appeared_listener_count underflow: register/take are mismatched"
            );
            self.appeared_listener_count -= 1;
        }
        if inst.last_obj.has_on_disappeared() {
            debug_assert!(
                self.disappeared_listener_count > 0,
                "disappeared_listener_count underflow: register/take are mismatched"
            );
            self.disappeared_listener_count -= 1;
        }
        Some(inst)
    }

    fn remove_wrapper(&mut self, node_id: LogicalNodeId) {
        self.wrappers.remove(&node_id);
    }

    fn register_projection(&mut self, id: ControlId, node_id: LogicalNodeId) {
        self.projections.entry(id).or_default().push(node_id);
    }

    fn remove_empty_projection(&mut self, id: ControlId) {
        if self
            .projections
            .get(&id)
            .is_some_and(ProjectedNodes::is_empty)
        {
            self.projections.remove(&id);
        }
    }

    pub(super) fn dispatch_appeared(&mut self, id: ControlId, context_stack: &Rc<ContextStack>) {
        if self.appeared_listener_count == 0 {
            return;
        }
        let Some(node_ids) = self.projections.get(&id) else {
            return;
        };
        for node_id in node_ids.as_slice().iter().rev() {
            if let Some(inst) = self.components.get_mut(node_id)
                && inst.last_obj.has_on_appeared()
            {
                inst.render_cx.set_context_stack(Rc::clone(context_stack));
                inst.last_obj.invoke_appeared(&mut inst.render_cx);
            }
        }
    }

    pub(super) fn dispatch_node_appeared(
        &mut self,
        node_id: LogicalNodeId,
        context_stack: &Rc<ContextStack>,
    ) {
        if let Some(inst) = self.components.get_mut(&node_id)
            && inst.last_obj.has_on_appeared()
        {
            inst.render_cx.set_context_stack(Rc::clone(context_stack));
            inst.last_obj.invoke_appeared(&mut inst.render_cx);
        }
    }

    pub(super) fn dispatch_disappeared(&mut self, id: ControlId, context_stack: &Rc<ContextStack>) {
        if self.disappeared_listener_count == 0 {
            return;
        }
        let Some(node_ids) = self.projections.get(&id) else {
            return;
        };
        for node_id in node_ids.as_slice() {
            if let Some(inst) = self.components.get_mut(node_id)
                && inst.last_obj.has_on_disappeared()
            {
                inst.render_cx.set_context_stack(Rc::clone(context_stack));
                inst.last_obj.invoke_disappeared(&mut inst.render_cx);
            }
        }
    }

    pub(super) fn dispatch_node_disappeared(
        &mut self,
        node_id: LogicalNodeId,
        context_stack: &Rc<ContextStack>,
    ) {
        if let Some(inst) = self.components.get_mut(&node_id)
            && inst.last_obj.has_on_disappeared()
        {
            inst.render_cx.set_context_stack(Rc::clone(context_stack));
            inst.last_obj.invoke_disappeared(&mut inst.render_cx);
        }
    }

    #[cfg(debug_assertions)]
    pub(super) fn debug_assert_invariants(&self) {
        let mut indexed = rustc_hash::FxHashSet::default();
        for (control_id, nodes) in &self.projections {
            debug_assert!(
                !nodes.is_empty(),
                "empty logical projection for {control_id:?}"
            );
            for node_id in nodes.as_slice() {
                debug_assert!(
                    indexed.insert(*node_id),
                    "logical node {node_id:?} is indexed more than once"
                );
                debug_assert!(
                    self.contains_node(*node_id),
                    "projected logical node has no instance"
                );
                debug_assert_eq!(
                    self.node_native_root(*node_id),
                    Some(*control_id),
                    "logical node native root disagrees with projection"
                );
            }
        }

        let unprojected = self
            .components
            .values()
            .filter(|node| node.native_root.is_none())
            .count()
            + self
                .wrappers
                .values()
                .filter(|node| node.native_root.is_none())
                .count();
        debug_assert_eq!(
            indexed.len() + unprojected,
            self.node_count(),
            "logical projection index does not cover every node"
        );
        for (node_id, node) in &self.components {
            if let Some(parent) = node.parent {
                debug_assert!(
                    self.contains_node(parent),
                    "logical node parent is not mounted"
                );
            }
            debug_assert_eq!(*node_id, node.node_id);
        }
        for (node_id, node) in &self.wrappers {
            if let Some(parent) = node.parent {
                debug_assert!(
                    self.contains_node(parent),
                    "logical node parent is not mounted"
                );
            }
            debug_assert_eq!(*node_id, node.node_id);
        }
    }
}
