use super::*;

pub(super) struct EnumSelectionFailure<'a> {
    pub declarations: Vec<&'a Fact>,
    pub cause: EnumOmissionCause,
    pub reason: String,
}

pub(super) fn enum_selection_failure<'a>(
    choices: &[&'a Fact],
    declarations: &BTreeMap<(String, Location), &'a Fact>,
    facts: &HashMap<&'a Origin, &'a Fact>,
    snapshot: &'a Snapshot,
    source_names: Option<&PlanningSourceNames>,
    reason: String,
) -> Option<EnumSelectionFailure<'a>> {
    if choices.is_empty() {
        return None;
    }
    let mut observations = BTreeMap::new();
    let mut identities = BTreeSet::new();
    let mut cause = EnumOmissionCause::ConflictingDeclarations;
    for fact in choices {
        let enumeration = underlying_enum_fact(fact, declarations, &mut BTreeSet::new())?;
        if !enumeration.definition {
            return None;
        }
        observations.insert(&fact.origin, *fact);
        observations.insert(&enumeration.origin, enumeration);
        if let Some(Err(failure)) =
            source_names.and_then(|names| names.shared_enum_providers.get(&enumeration.origin))
        {
            cause = failure.cause();
        }
        if let Some(identity) = snapshot.shared_enum_identities.get(&enumeration.origin) {
            identities.insert((
                &identity.definition.enum_usr,
                &identity.definition.alias_usr,
            ));
        }
    }
    if let Some(names) = source_names {
        for (origin, identity) in &snapshot.shared_enum_identities {
            if !names
                .shared_enum_providers
                .get(origin)
                .is_some_and(Result::is_err)
            {
                continue;
            }
            if identities.contains(&(
                &identity.definition.enum_usr,
                &identity.definition.alias_usr,
            )) {
                let fact = *facts.get(origin)?;
                observations.insert(&fact.origin, fact);
            }
        }
    }
    Some(EnumSelectionFailure {
        declarations: observations.into_values().collect(),
        cause,
        reason,
    })
}

pub(super) struct EnumAffected<'a> {
    pub origins: HashSet<&'a Origin>,
    pub roots: BTreeSet<DependencyRoot<'a>>,
    pub omissions: BTreeSet<MetadataOmission>,
}

impl<'a> DependencyClosureDiagnostics<'a> {
    pub(super) fn set_root_origin(&mut self, root: DependencyNode<'a>, origin: &'a Origin) {
        if self.enabled
            && let DependencyNode::Root(index) = root
        {
            self.root_origins.insert(index, origin);
        }
    }

    pub(super) fn block_enum_fact(
        &mut self,
        fact: &'a Fact,
        cause: EnumOmissionCause,
        reason: &str,
        source: Option<DependencyNode<'a>>,
    ) {
        let node = DependencyNode::Fact(&fact.origin);
        if let Some(source) = source {
            self.add_edge(source, node);
        }
        let reference = DependencyReference {
            name: &fact.name,
            tu: &fact.origin.tu,
            declaration: Some(&fact.spelling),
        };
        self.block(reference, reason, node);
        self.enum_blockers.insert(
            DependencyBlocker {
                reference,
                reason: reason.to_string(),
            },
            cause,
        );
    }

    pub(super) fn block_enum_failure(
        &mut self,
        failure: &EnumSelectionFailure<'a>,
        source: Option<DependencyNode<'a>>,
        declarations: &BTreeMap<(String, Location), &'a Fact>,
    ) {
        for fact in &failure.declarations {
            let enumeration =
                underlying_enum_fact(fact, declarations, &mut BTreeSet::new()).unwrap();
            let node = DependencyNode::Fact(&fact.origin);
            if let Some(source) = source {
                self.add_edge(source, node);
            }
            if fact.origin != enumeration.origin {
                self.add_edge(node, DependencyNode::Fact(&enumeration.origin));
            }
            self.block_enum_fact(enumeration, failure.cause, &failure.reason, None);
        }
    }

    pub(super) fn enum_affected(
        &self,
        snapshot: &Snapshot,
        names: Option<&PlanningSourceNames>,
        fallback: &str,
        facts: &HashMap<&'a Origin, &'a Fact>,
        declarations: &DeclarationIndex<'_>,
    ) -> Result<EnumAffected<'a>, Error> {
        if self
            .blockers
            .keys()
            .any(|blocker| !self.enum_blockers.contains_key(blocker))
        {
            return Err(self.error());
        }
        let declaration = |origin: &Origin| -> OmittedDeclaration {
            let fact = facts[origin];
            let name = names.map_or(fact.name.as_str(), |names| names.fact_name(fact));
            OmittedDeclaration {
                origin: origin.clone(),
                kind: Some(fact.kind),
                name: name.to_string(),
                source: fact.spelling.clone(),
                namespace: snapshot
                    .fact_authority_namespace(fact, declarations)
                    .cloned()
                    .or_else(|| {
                        snapshot
                            .root_owners
                            .get(origin)
                            .map(|owner| owner.namespace.clone())
                    })
                    .unwrap_or_else(|| fallback.to_string()),
                methods: match &fact.data {
                    FactData::Interface { methods, .. } => {
                        methods.iter().map(|method| method.name.clone()).collect()
                    }
                    _ => Vec::new(),
                },
            }
        };
        let mut result = EnumAffected {
            origins: HashSet::new(),
            roots: BTreeSet::new(),
            omissions: BTreeSet::new(),
        };
        for (blocker, cause) in &self.enum_blockers {
            let mut seen = HashSet::new();
            let mut queue = std::collections::VecDeque::new();
            for source in &self.blockers[blocker] {
                queue.push_back((*source, Vec::new()));
            }
            while let Some((node, mut chain)) = queue.pop_front() {
                if !seen.insert(node) {
                    continue;
                }
                match node {
                    DependencyNode::Fact(origin) => {
                        result.origins.insert(origin);
                        chain.push(declaration(origin));
                        let mut dependency_chain = chain.clone();
                        dependency_chain.reverse();
                        result.omissions.insert(MetadataOmission {
                            declaration: declaration(origin),
                            cause: *cause,
                            reason: blocker.reason.clone(),
                            dependency_chain,
                        });
                    }
                    DependencyNode::Root(index) => {
                        let root = self.roots[index];
                        result.roots.insert(root);
                        if root.kind == "constant" {
                            let origin = self.root_origins[&index];
                            let namespace = snapshot
                                .constant_root_owners
                                .get(&(origin.clone(), root.name.to_string()))
                                .map_or_else(
                                    || fallback.to_string(),
                                    |owner| owner.namespace.clone(),
                                );
                            let omitted = OmittedDeclaration {
                                origin: origin.clone(),
                                kind: facts.get(origin).map(|fact| fact.kind),
                                name: root.name.to_string(),
                                source: root.source.clone(),
                                namespace,
                                methods: Vec::new(),
                            };
                            chain.push(omitted.clone());
                            chain.reverse();
                            result.omissions.insert(MetadataOmission {
                                declaration: omitted,
                                cause: *cause,
                                reason: blocker.reason.clone(),
                                dependency_chain: chain,
                            });
                        }
                        continue;
                    }
                }
                for parent in self.reverse_edges.get(&node).into_iter().flatten() {
                    queue.push_back((*parent, chain.clone()));
                }
            }
        }
        Ok(result)
    }
}
