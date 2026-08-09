use crate::reference::NativeElementRef;
use rustc_hash::FxHashMap;

use super::logical_tree::MountedLogicalTree;
use super::templated::MountedTemplatedTree;
use super::*;

#[derive(Default)]
pub(super) struct MountedTree {
    child_slots: FxHashMap<ControlId, ChildSlots>,
    nodes: FxHashMap<ControlId, MountedNativeNode>,
    headers: FxHashMap<ControlId, MountedOutput>,
    panes: FxHashMap<ControlId, MountedOutput>,
    custom: FxHashMap<ControlId, Box<dyn CustomElement>>,
    before_unmount: FxHashMap<ControlId, BeforeUnmount>,
    pub(super) templated: MountedTemplatedTree,
    pub(super) logical: MountedLogicalTree,
}

#[derive(Default)]
struct ChildSlots {
    native: Vec<ControlId>,
    logical: Option<Vec<MountedOutput>>,
}

struct MountedNativeNode {
    kind: Option<ControlKind>,
    parent: Option<ControlId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum OwnedLifecycleNode {
    Native(ControlId),
    Logical(LogicalNodeId),
}

impl OwnedLifecycleNode {
    fn from_output(output: MountedOutput) -> Option<Self> {
        output
            .native
            .map(Self::Native)
            .or_else(|| output.logical.map(Self::Logical))
    }
}

pub(super) struct BeforeUnmount {
    pub(super) reference: Option<NativeElementRef>,
    pub(super) callback: Option<Callback<Option<windows_core::IInspectable>>>,
}

impl MountedTree {
    pub(super) fn register(&mut self, id: ControlId, kind: Option<ControlKind>) {
        if let Some(slots) = self.child_slots.remove(&id) {
            for child in slots.native {
                self.clear_parent(child, id);
            }
        }
        if let Some(header) = self.headers.remove(&id)
            && let Some(header) = header.native
        {
            self.clear_parent(header, id);
        }
        if let Some(pane) = self.panes.remove(&id)
            && let Some(pane) = pane.native
        {
            self.clear_parent(pane, id);
        }
        self.custom.remove(&id);
        self.before_unmount.remove(&id);
        self.nodes
            .insert(id, MountedNativeNode { kind, parent: None });
    }

    pub(super) fn kind(&self, id: ControlId) -> Option<ControlKind> {
        self.nodes.get(&id).and_then(|node| node.kind)
    }

    pub(super) fn parent(&self, id: ControlId) -> Option<ControlId> {
        self.nodes.get(&id).and_then(|node| node.parent)
    }

    fn set_parent(&mut self, child: ControlId, parent: ControlId) {
        let node = self
            .nodes
            .get_mut(&child)
            .expect("mounted child missing native node");
        debug_assert!(
            node.parent.is_none() || node.parent == Some(parent),
            "native control {child:?} already owned by {:?}",
            node.parent
        );
        node.parent = Some(parent);
    }

    fn clear_parent(&mut self, child: ControlId, parent: ControlId) {
        if let Some(node) = self.nodes.get_mut(&child)
            && node.parent == Some(parent)
        {
            node.parent = None;
        }
    }

    pub(super) fn set_header(&mut self, parent: ControlId, header: Option<MountedOutput>) {
        if let Some(old) = self.headers.remove(&parent)
            && let Some(old) = old.native
        {
            self.clear_parent(old, parent);
        }
        if let Some(header) = header
            && let Some(native) = header.native
        {
            self.set_parent(native, parent);
            self.headers.insert(parent, header);
        } else if let Some(header) = header {
            self.headers.insert(parent, header);
        }
    }

    pub(super) fn header(&self, parent: ControlId) -> Option<MountedOutput> {
        self.headers.get(&parent).copied()
    }

    pub(super) fn set_pane(&mut self, parent: ControlId, pane: Option<MountedOutput>) {
        if let Some(old) = self.panes.remove(&parent)
            && let Some(old) = old.native
        {
            self.clear_parent(old, parent);
        }
        if let Some(pane) = pane
            && let Some(native) = pane.native
        {
            self.set_parent(native, parent);
            self.panes.insert(parent, pane);
        } else if let Some(pane) = pane {
            self.panes.insert(parent, pane);
        }
    }

    pub(super) fn pane(&self, parent: ControlId) -> Option<MountedOutput> {
        self.panes.get(&parent).copied()
    }

    pub(super) fn set_custom(&mut self, id: ControlId, handle: Box<dyn CustomElement>) {
        debug_assert!(self.nodes.contains_key(&id));
        self.custom.insert(id, handle);
    }

    pub(super) fn take_custom(&mut self, id: ControlId) -> Option<Box<dyn CustomElement>> {
        self.custom.remove(&id)
    }

    pub(super) fn set_before_unmount(
        &mut self,
        id: ControlId,
        reference: Option<NativeElementRef>,
        callback: Option<Callback<Option<windows_core::IInspectable>>>,
    ) {
        debug_assert!(self.nodes.contains_key(&id));
        if reference.is_some() || callback.is_some() {
            self.before_unmount.insert(
                id,
                BeforeUnmount {
                    reference,
                    callback,
                },
            );
        } else {
            self.before_unmount.remove(&id);
        }
    }

    pub(super) fn take_before_unmount(&mut self, id: ControlId) -> Option<BeforeUnmount> {
        self.before_unmount.remove(&id)
    }

    pub(super) fn children(&self, parent: ControlId) -> &[ControlId] {
        self.child_slots
            .get(&parent)
            .map_or(&[], |slots| slots.native.as_slice())
    }

    pub(super) fn logical_children(&self, parent: ControlId) -> &[MountedOutput] {
        self.child_slots
            .get(&parent)
            .and_then(|slots| slots.logical.as_deref())
            .unwrap_or(&[])
    }

    pub(super) fn logical_child(&self, parent: ControlId, index: usize) -> Option<MountedOutput> {
        self.logical_children(parent).get(index).copied()
    }

    pub(super) fn permute_logical_children(
        &mut self,
        parent: ControlId,
        start: usize,
        new_to_old: &[i32],
        visited: &mut [bool],
    ) {
        let slots = self
            .child_slots
            .get_mut(&parent)
            .expect("mounted parent missing child slots");
        let children = slots
            .logical
            .as_mut()
            .expect("mounted parent missing logical child mirror");
        let end = start
            .checked_add(new_to_old.len())
            .expect("logical child permutation range overflow");
        assert!(
            end <= children.len(),
            "logical child permutation out of range"
        );
        assert!(
            visited.len() >= new_to_old.len(),
            "logical child permutation scratch is too short"
        );

        visited[..new_to_old.len()].fill(false);
        for cycle_start in 0..new_to_old.len() {
            if visited[cycle_start] {
                continue;
            }
            let saved = children[start + cycle_start];
            let mut current = cycle_start;
            loop {
                visited[current] = true;
                let next = usize::try_from(new_to_old[current])
                    .expect("logical child permutation contains a negative index");
                assert!(
                    next < new_to_old.len(),
                    "logical child permutation index out of range"
                );
                if next == cycle_start {
                    children[start + current] = saved;
                    break;
                }
                assert!(
                    !visited[next],
                    "logical child permutation contains a non-cycle mapping"
                );
                children[start + current] = children[start + next];
                current = next;
            }
        }

        debug_assert!(
            slots
                .native
                .iter()
                .copied()
                .eq(children.iter().filter_map(|output| output.native)),
            "logical child permutation disagrees with native children"
        );
    }

    pub(super) fn attach_templated_output(&mut self, parent: ControlId, output: MountedOutput) {
        if let Some(child) = output.native {
            self.set_parent(child, parent);
        }
    }

    pub(super) fn detach_templated_output(&mut self, parent: ControlId, output: MountedOutput) {
        if let Some(child) = output.native {
            self.clear_parent(child, parent);
        }
    }

    pub(super) fn logical_owner(&self, node_id: LogicalNodeId) -> Option<ControlId> {
        self.child_slots
            .iter()
            .find_map(|(parent, slots)| {
                slots
                    .logical
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .any(|output| output.logical == Some(node_id))
                    .then_some(*parent)
            })
            .or_else(|| {
                self.headers.iter().find_map(|(parent, output)| {
                    (output.logical == Some(node_id)).then_some(*parent)
                })
            })
            .or_else(|| {
                self.panes.iter().find_map(|(parent, output)| {
                    (output.logical == Some(node_id)).then_some(*parent)
                })
            })
            .or_else(|| {
                self.templated.lists.iter().find_map(|(parent, state)| {
                    state
                        .rows
                        .values()
                        .any(|row| row.output.logical == Some(node_id))
                        .then_some(*parent)
                })
            })
    }

    pub(super) fn native_index(&self, parent: ControlId, logical_index: usize) -> usize {
        self.logical_children(parent)[..logical_index]
            .iter()
            .filter(|output| output.native.is_some())
            .count()
    }

    pub(super) fn append_logical_child(&mut self, parent: ControlId, output: MountedOutput) {
        self.child_slots
            .entry(parent)
            .or_default()
            .logical
            .get_or_insert_with(Vec::new)
            .push(output);
    }

    pub(super) fn insert_logical_child(
        &mut self,
        parent: ControlId,
        index: usize,
        output: MountedOutput,
    ) -> usize {
        let list = self
            .child_slots
            .entry(parent)
            .or_default()
            .logical
            .get_or_insert_with(Vec::new);
        let index = index.min(list.len());
        list.insert(index, output);
        index
    }

    pub(super) fn remove_logical_child(
        &mut self,
        parent: ControlId,
        index: usize,
    ) -> Option<MountedOutput> {
        self.child_slots.get_mut(&parent).and_then(|slots| {
            let logical = slots.logical.as_mut()?;
            (index < logical.len()).then(|| logical.remove(index))
        })
    }

    pub(super) fn replace_logical_child(
        &mut self,
        parent: ControlId,
        index: usize,
        output: MountedOutput,
    ) -> Option<MountedOutput> {
        self.child_slots.get_mut(&parent).and_then(|slots| {
            let logical = slots.logical.as_mut()?;
            (index < logical.len()).then(|| std::mem::replace(&mut logical[index], output))
        })
    }

    pub(super) fn move_logical_child(&mut self, parent: ControlId, from: usize, to: usize) {
        if from == to {
            return;
        }
        if let Some(slots) = self.child_slots.get_mut(&parent)
            && let Some(logical) = slots.logical.as_mut()
            && from < logical.len()
            && to < logical.len()
        {
            let item = logical.remove(from);
            logical.insert(to, item);
        }
    }

    pub(super) fn extend_owned_children(&self, parent: ControlId, children: &mut Vec<ControlId>) {
        children.extend_from_slice(self.children(parent));
        if let Some(header) = self.header(parent).and_then(|output| output.native) {
            children.push(header);
        }
        if let Some(pane) = self.pane(parent).and_then(|output| output.native) {
            children.push(pane);
        }
        if let Some(state) = self.templated.lists.get(&parent) {
            children.extend(state.rows.values().filter_map(|row| row.output.native));
        }
    }

    pub(super) fn extend_owned_lifecycle_children(
        &self,
        parent: ControlId,
        children: &mut Vec<OwnedLifecycleNode>,
    ) {
        if let Some(slots) = self.child_slots.get(&parent) {
            if let Some(logical) = &slots.logical {
                children.extend(
                    logical
                        .iter()
                        .filter_map(|output| OwnedLifecycleNode::from_output(*output)),
                );
            } else {
                children.extend(slots.native.iter().copied().map(OwnedLifecycleNode::Native));
            }
        }
        if let Some(header) = self
            .header(parent)
            .and_then(OwnedLifecycleNode::from_output)
        {
            children.push(header);
        }
        if let Some(pane) = self.pane(parent).and_then(OwnedLifecycleNode::from_output) {
            children.push(pane);
        }
        if let Some(state) = self.templated.lists.get(&parent) {
            children.extend(
                state
                    .rows
                    .values()
                    .filter_map(|row| OwnedLifecycleNode::from_output(row.output)),
            );
        }
    }

    pub(super) fn extend_owned_logical_roots(
        &self,
        parent: ControlId,
        logical_roots: &mut Vec<LogicalNodeId>,
    ) {
        logical_roots.extend(
            self.logical_children(parent)
                .iter()
                .filter_map(|output| output.native.is_none().then_some(output.logical))
                .flatten(),
        );
        if let Some(output) = self.header(parent)
            && output.native.is_none()
            && let Some(logical) = output.logical
        {
            logical_roots.push(logical);
        }
        if let Some(output) = self.pane(parent)
            && output.native.is_none()
            && let Some(logical) = output.logical
        {
            logical_roots.push(logical);
        }
        if let Some(state) = self.templated.lists.get(&parent) {
            logical_roots.extend(
                state
                    .rows
                    .values()
                    .filter_map(|row| row.output.native.is_none().then_some(row.output.logical))
                    .flatten(),
            );
        }
    }

    pub(super) fn child(&self, parent: ControlId, index: usize) -> Option<ControlId> {
        self.children(parent).get(index).copied()
    }

    pub(super) fn child_position(&self, parent: ControlId, child: ControlId) -> Option<usize> {
        self.children(parent).iter().position(|id| *id == child)
    }

    pub(super) fn append_child(&mut self, parent: ControlId, child: ControlId) {
        self.set_parent(child, parent);
        self.child_slots
            .entry(parent)
            .or_default()
            .native
            .push(child);
    }

    pub(super) fn remove_child(&mut self, parent: ControlId, index: usize) -> Option<ControlId> {
        let removed = self
            .child_slots
            .get_mut(&parent)
            .and_then(|slots| (index < slots.native.len()).then(|| slots.native.remove(index)));
        if let Some(child) = removed {
            self.clear_parent(child, parent);
        }
        removed
    }

    pub(super) fn replace_child(
        &mut self,
        parent: ControlId,
        index: usize,
        new: ControlId,
    ) -> Option<ControlId> {
        let replaced = self.child_slots.get_mut(&parent).and_then(|slots| {
            (index < slots.native.len()).then(|| {
                let old = slots.native[index];
                slots.native[index] = new;
                old
            })
        });
        if let Some(old) = replaced {
            self.clear_parent(old, parent);
            self.set_parent(new, parent);
        }
        replaced
    }

    pub(super) fn move_child(&mut self, parent: ControlId, from: usize, to: usize) {
        if from == to {
            return;
        }
        if let Some(slots) = self.child_slots.get_mut(&parent)
            && from < slots.native.len()
            && to < slots.native.len()
        {
            let item = slots.native.remove(from);
            slots.native.insert(to, item);
        }
    }

    pub(super) fn insert_child(
        &mut self,
        parent: ControlId,
        index: usize,
        child: ControlId,
    ) -> usize {
        self.set_parent(child, parent);
        let list = &mut self.child_slots.entry(parent).or_default().native;
        let index = index.min(list.len());
        list.insert(index, child);
        index
    }

    pub(super) fn remove_node(&mut self, id: ControlId) {
        if let Some(slots) = self.child_slots.remove(&id) {
            for child in slots.native {
                self.clear_parent(child, id);
            }
        }
        if let Some(header) = self.headers.remove(&id)
            && let Some(header) = header.native
        {
            self.clear_parent(header, id);
        }
        if let Some(pane) = self.panes.remove(&id)
            && let Some(pane) = pane.native
        {
            self.clear_parent(pane, id);
        }
        self.custom.remove(&id);
        self.before_unmount.remove(&id);
        self.nodes.remove(&id);
    }

    #[cfg(debug_assertions)]
    pub(super) fn debug_assert_native_ownership(&self) {
        let mut owned = rustc_hash::FxHashSet::default();
        let mut record = |parent: ControlId, child: ControlId| {
            debug_assert!(
                owned.insert(child),
                "native control {child:?} has more than one owner"
            );
            debug_assert_eq!(
                self.parent(child),
                Some(parent),
                "native control {child:?} disagrees with its owner"
            );
        };

        for (parent, slots) in &self.child_slots {
            if let Some(outputs) = &slots.logical {
                let native: Vec<_> = outputs.iter().filter_map(|output| output.native).collect();
                debug_assert_eq!(
                    slots.native.as_slice(),
                    native.as_slice(),
                    "logical child mirror disagrees with native children"
                );
                for output in outputs {
                    if let Some(node_id) = output.logical {
                        debug_assert!(
                            self.logical.contains_node(node_id),
                            "logical child output has no mounted node"
                        );
                        debug_assert_eq!(
                            self.logical.node_native_root(node_id),
                            output.native,
                            "logical child output native root disagrees with node"
                        );
                    }
                }
            }
            for child in &slots.native {
                record(*parent, *child);
            }
        }
        for (parent, header) in &self.headers {
            if let Some(header) = header.native {
                record(*parent, header);
            }
        }
        for (parent, pane) in &self.panes {
            if let Some(pane) = pane.native {
                record(*parent, pane);
            }
        }
        for (parent, state) in &self.templated.lists {
            for row in state.rows.values() {
                if let Some(content_id) = row.output.native {
                    record(*parent, content_id);
                }
            }
        }

        for (id, node) in &self.nodes {
            if node.parent.is_some() {
                debug_assert!(
                    owned.contains(id),
                    "native control {id:?} has a parent but is absent from its owner's children"
                );
            }
        }
        for id in self.custom.keys() {
            debug_assert!(
                self.nodes.contains_key(id),
                "custom handle {id:?} has no mounted native node"
            );
        }
        for id in self.before_unmount.keys() {
            debug_assert!(
                self.nodes.contains_key(id),
                "pre-unmount callback {id:?} has no mounted native node"
            );
        }
    }
}
