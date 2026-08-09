use super::*;

impl<B: Backend + 'static> Reconciler<B> {
    fn collect_affected_components(
        &self,
        root_id: ControlId,
        changed: &rustc_hash::FxHashSet<ContextId>,
    ) -> Vec<LogicalNodeId> {
        let mut affected = Vec::new();
        let mut stack = vec![root_id];
        let mut logical = Vec::new();
        while let Some(id) = stack.pop() {
            self.tree
                .logical
                .extend_context_subscribers(id, changed, &mut affected);
            self.tree.extend_owned_children(id, &mut stack);
            self.tree.extend_owned_logical_roots(id, &mut logical);
        }
        for node_id in logical.into_iter().rev() {
            affected.extend(
                self.tree
                    .logical
                    .context_subscribers_in_subtree(node_id, changed),
            );
        }
        affected
    }

    pub(super) fn collect_affected_components_for_node(
        &self,
        root: LogicalNodeId,
        changed: &rustc_hash::FxHashSet<ContextId>,
    ) -> Vec<LogicalNodeId> {
        let mut affected = self
            .tree
            .logical
            .context_subscribers_in_subtree(root, changed);

        if let Some(native_root) = self.tree.logical.node_native_root(root) {
            for node_id in self.collect_affected_components(native_root, changed) {
                if !affected.contains(&node_id) {
                    affected.push(node_id);
                }
            }
        }

        affected
    }

    fn force_all_context_subscribers(&mut self, changed: &rustc_hash::FxHashSet<ContextId>) {
        let affected = self.tree.logical.context_subscribers(changed);
        self.add_forced_node_paths(affected);
    }

    pub fn force_context_subscribers(
        &mut self,
        root_id: ControlId,
        context_ids: &rustc_hash::FxHashSet<ContextId>,
    ) {
        let affected = self.collect_affected_components(root_id, context_ids);
        if !affected.is_empty() {
            self.add_forced_node_paths(affected);
        }
    }

    pub fn force_context_subscribers_root(
        &mut self,
        context_ids: &rustc_hash::FxHashSet<ContextId>,
    ) {
        self.force_all_context_subscribers(context_ids);
    }
}
