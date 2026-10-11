use super::*;

pub(super) struct Context<'a, 'facts> {
    pub snapshot: &'a Snapshot,
    pub declarations: &'a DeclarationIndex<'facts>,
    pub facts_by_declaration: &'a BTreeMap<(String, Location), &'facts Fact>,
    pub source_names: Option<&'a PlanningSourceNames>,
    pub excluded_types: Option<&'a BTreeSet<String>>,
}

impl<'facts> Context<'_, 'facts> {
    fn exact(&self, tu: &str, name: &str, location: &Location) -> Option<&'facts Fact> {
        let fact = *self
            .facts_by_declaration
            .get(&(tu.to_string(), location.clone()))?;
        if name != fact.name
            && self
                .source_names
                .is_none_or(|names| names.fact_name(fact) != name)
        {
            return None;
        }
        let bucket = self.declarations.get(tu, &fact.name, location);
        (!bucket.is_empty()
            && bucket
                .iter()
                .all(|other| same_source_declaration(fact, other)))
        .then_some(fact)
    }

    fn contracts_equal(
        &self,
        left: &Fact,
        right: &Fact,
        seen: &mut BTreeSet<(Origin, Origin)>,
    ) -> bool {
        let (Some(a), Some(b)) = (
            self.snapshot.native_com_contracts.get(&left.origin),
            self.snapshot.native_com_contracts.get(&right.origin),
        ) else {
            return false;
        };
        if a.guid != b.guid
            || a.pointer_width != b.pointer_width
            || a.bases != b.bases
            || a.slots.len() != b.slots.len()
        {
            return false;
        }
        let pair = (left.origin.clone(), right.origin.clone());
        if !seen.insert(pair.clone()) {
            return true;
        }
        let result = a.slots.iter().zip(&b.slots).all(|(a, b)| {
            a.name == b.name
                && a.convention == b.convention
                && a.params.len() == b.params.len()
                && self.types_equal(
                    &a.result,
                    &left.origin.tu,
                    &b.result,
                    &right.origin.tu,
                    false,
                    seen,
                )
                && a.params.iter().zip(&b.params).all(|(a, b)| {
                    if a.annotation.size.is_some() || b.annotation.size.is_some() {
                        if let (Some((a_mut, a_target)), Some((b_mut, b_target))) = (
                            self.pointer_target(&a.ty, &left.origin.tu),
                            self.pointer_target(&b.ty, &right.origin.tu),
                        ) {
                            a_mut == b_mut
                                && self.types_equal(
                                    a_target,
                                    &left.origin.tu,
                                    b_target,
                                    &right.origin.tu,
                                    false,
                                    seen,
                                )
                        } else {
                            false
                        }
                    } else {
                        self.types_equal(
                            &a.ty,
                            &left.origin.tu,
                            &b.ty,
                            &right.origin.tu,
                            false,
                            seen,
                        )
                    }
                })
        });
        seen.remove(&pair);
        result
    }

    fn pointer_target<'b>(&'b self, ty: &'b TypeRef, tu: &str) -> Option<(bool, &'b TypeRef)> {
        let mut current = ty;
        let mut seen = BTreeSet::new();
        loop {
            match current {
                TypeRef::Pointer { mutable, target } => return Some((*mutable, target)),
                TypeRef::Named { name, declaration } => {
                    let fact = self.exact(tu, name, declaration)?;
                    if !seen.insert(&fact.origin) {
                        return None;
                    }
                    let FactData::Typedef { target } = &fact.data else {
                        return None;
                    };
                    current = target;
                }
                _ => return None,
            }
        }
    }

    fn types_equal(
        &self,
        a: &TypeRef,
        a_tu: &str,
        b: &TypeRef,
        b_tu: &str,
        pointer: bool,
        seen: &mut BTreeSet<(Origin, Origin)>,
    ) -> bool {
        if let TypeRef::Named { name, declaration } = a {
            let Some(fact) = self.exact(a_tu, name, declaration) else {
                return false;
            };
            if let FactData::Typedef { target } = &fact.data {
                let pair = (fact.origin.clone(), fact.origin.clone());
                if !seen.insert(pair.clone()) {
                    return false;
                }
                let result = self.types_equal(target, a_tu, b, b_tu, pointer, seen);
                seen.remove(&pair);
                return result;
            }
        }
        if let TypeRef::Named { name, declaration } = b {
            let Some(fact) = self.exact(b_tu, name, declaration) else {
                return false;
            };
            if let FactData::Typedef { target } = &fact.data {
                let pair = (fact.origin.clone(), fact.origin.clone());
                if !seen.insert(pair.clone()) {
                    return false;
                }
                let result = self.types_equal(a, a_tu, target, b_tu, pointer, seen);
                seen.remove(&pair);
                return result;
            }
        }
        match (a, b) {
            (
                TypeRef::Pointer {
                    mutable: a_mut,
                    target: a,
                },
                TypeRef::Pointer {
                    mutable: b_mut,
                    target: b,
                },
            ) => a_mut == b_mut && self.types_equal(a, a_tu, b, b_tu, true, seen),
            (
                TypeRef::Reference {
                    mutable: a_mut,
                    target: a,
                },
                TypeRef::Reference {
                    mutable: b_mut,
                    target: b,
                },
            ) => a_mut == b_mut && self.types_equal(a, a_tu, b, b_tu, false, seen),
            (
                TypeRef::Array {
                    target: a,
                    len: a_len,
                },
                TypeRef::Array {
                    target: b,
                    len: b_len,
                },
            ) => a_len == b_len && self.types_equal(a, a_tu, b, b_tu, false, seen),
            (
                TypeRef::Named {
                    name: a_name,
                    declaration: a_location,
                },
                TypeRef::Named {
                    name: b_name,
                    declaration: b_location,
                },
            ) => {
                let (Some(a), Some(b)) = (
                    self.exact(a_tu, a_name, a_location),
                    self.exact(b_tu, b_name, b_location),
                ) else {
                    return false;
                };
                if pointer
                    && self.snapshot.native_com_contracts.contains_key(&a.origin)
                    && self.snapshot.native_com_contracts.contains_key(&b.origin)
                {
                    return self.contracts_equal(a, b, seen);
                }
                match (&a.data, &b.data) {
                    (
                        FactData::Record {
                            base: a_base,
                            fields: a_fields,
                            size: a_size,
                            align: a_align,
                            packing: a_pack,
                            alignment: a_alignment,
                            union: a_union,
                        },
                        FactData::Record {
                            base: b_base,
                            fields: b_fields,
                            size: b_size,
                            align: b_align,
                            packing: b_pack,
                            alignment: b_alignment,
                            union: b_union,
                        },
                    ) => {
                        if (a_size, a_align, a_pack, a_alignment, a_union)
                            != (b_size, b_align, b_pack, b_alignment, b_union)
                            || a_fields.len() != b_fields.len()
                        {
                            return false;
                        }
                        let pair = (a.origin.clone(), b.origin.clone());
                        if !seen.insert(pair.clone()) {
                            return true;
                        }
                        let base_equal = match (a_base, b_base) {
                            (Some(a_base), Some(b_base)) => {
                                self.types_equal(a_base, a_tu, b_base, b_tu, false, seen)
                            }
                            (None, None) => true,
                            _ => false,
                        };
                        let result = base_equal
                            && a_fields.iter().zip(b_fields).all(|(a, b)| {
                                (a.offset, a.size, a.align, a.bit_width)
                                    == (b.offset, b.size, b.align, b.bit_width)
                                    && self.types_equal(&a.ty, a_tu, &b.ty, b_tu, false, seen)
                            });
                        seen.remove(&pair);
                        result
                    }
                    (FactData::Enum { .. }, FactData::Enum { .. }) => a.data == b.data,
                    (FactData::Interface { .. }, FactData::Interface { .. }) => {
                        self.contracts_equal(a, b, seen)
                    }
                    _ => false,
                }
            }
            (TypeRef::Void, TypeRef::Void)
            | (TypeRef::String, TypeRef::String)
            | (TypeRef::Object, TypeRef::Object) => true,
            (TypeRef::Scalar(a), TypeRef::Scalar(b)) => a == b,
            (
                TypeRef::OpaquePointer {
                    mutable: a_mut,
                    tag: a,
                },
                TypeRef::OpaquePointer {
                    mutable: b_mut,
                    tag: b,
                },
            ) => a_mut == b_mut && a == b,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Failure {
    Missing,
    Conflicting,
    Ambiguous,
    Unproved,
}

impl Failure {
    pub fn message(self, name: &str) -> String {
        let reason = match self {
            Self::Missing => "has no eligible owned interface",
            Self::Conflicting => "has conflicting native COM pointer contracts",
            Self::Ambiguous => "has ambiguous eligible COM interface providers",
            Self::Unproved => "has no proved native COM pointer contract",
        };
        format!("native COM record `{name}` {reason}")
    }
}

pub(super) struct Bindings<'facts> {
    providers: BTreeMap<Origin, Result<&'facts Fact, Failure>>,
}

impl<'facts> Bindings<'facts> {
    pub fn new(context: &Context<'_, 'facts>, facts: &HashMap<&Origin, &'facts Fact>) -> Self {
        let snapshot = context.snapshot;
        if !snapshot.header_partition_policy {
            return Self {
                providers: BTreeMap::new(),
            };
        }
        let mut eligible: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
        for (origin, contract) in &snapshot.native_com_contracts {
            let Some(fact) = facts.get(origin) else {
                continue;
            };
            if fact.definition
                && fact.root
                && fact.parent.is_none()
                && matches!(fact.data, FactData::Interface { .. })
                && snapshot
                    .root_owners
                    .get(origin)
                    .is_some_and(|owner| !owner.exclusions.contains(&fact.name))
                && !snapshot.suppressed_type_origins.contains(origin)
                && !context
                    .excluded_types
                    .is_some_and(|excluded| excluded.contains(&fact.name))
            {
                eligible.entry(&contract.guid).or_default().push(fact);
            }
        }
        let mut providers = BTreeMap::new();
        for origin in &snapshot.unproved_com_records {
            if facts.get(origin).is_some_and(|fact| {
                fact.parent.is_none()
                    && fact.kind == FactKind::Struct
                    && matches!(
                        fact.data,
                        FactData::Record { .. } | FactData::Unsupported { .. }
                    )
            }) && !snapshot.root_owners.contains_key(origin)
            {
                providers.insert(origin.clone(), Err(Failure::Unproved));
            }
        }
        for (origin, contract) in &snapshot.native_com_contracts {
            let Some(fact) = facts.get(origin) else {
                continue;
            };
            if !contract.has_fields
                || fact.parent.is_some()
                || !matches!(fact.data, FactData::Record { .. })
                || snapshot.root_owners.contains_key(origin)
            {
                continue;
            }
            let candidates = eligible
                .get(contract.guid.as_str())
                .map_or(&[][..], Vec::as_slice);
            let result = match candidates {
                [] => Err(Failure::Missing),
                [first, ..] => {
                    let owner = &snapshot.root_owners[&first.origin];
                    if candidates.iter().any(|candidate| {
                        candidate.spelling != first.spelling
                            || candidate.name != first.name
                            || !same_owner_policy(&snapshot.root_owners[&candidate.origin], owner)
                    }) {
                        Err(Failure::Ambiguous)
                    } else if candidates.iter().any(|candidate| {
                        !context.contracts_equal(fact, candidate, &mut BTreeSet::new())
                    }) {
                        Err(Failure::Conflicting)
                    } else {
                        Ok(preferred_fact(candidates))
                    }
                }
            };
            providers.insert(origin.clone(), result);
        }
        Self { providers }
    }

    pub fn provider_for(
        &self,
        fact: &Fact,
        index: &HashMap<&str, Vec<&'facts Fact>>,
    ) -> Option<Result<&'facts Fact, Failure>> {
        let target = typedef_tag_declaration(fact, index).unwrap_or(fact);
        self.providers.get(&target.origin).copied()
    }

    pub fn validate_use(
        &self,
        ty: &TypeRef,
        tu: &str,
        annotations: &[Annotation],
        counted: bool,
        pointer: bool,
        context: &Context<'_, 'facts>,
    ) -> Result<(), Error> {
        if self.providers.is_empty() {
            return Ok(());
        }
        let counted = counted
            || annotations.iter().any(|annotation| {
                matches!(
                    annotation,
                    Annotation::ArrayCountParam(_)
                        | Annotation::ArrayCountConst(_)
                        | Annotation::ArrayCountField(_)
                        | Annotation::MemorySizeParam(_)
                        | Annotation::MemorySizeConst(_)
                )
            });
        self.validate(ty, tu, counted, pointer, context, &mut BTreeSet::new())
    }

    fn validate(
        &self,
        ty: &TypeRef,
        tu: &str,
        counted: bool,
        pointer: bool,
        context: &Context<'_, 'facts>,
        seen: &mut BTreeSet<Origin>,
    ) -> Result<(), Error> {
        match ty {
            TypeRef::Pointer { target, .. } => {
                self.validate(target, tu, false, !counted, context, seen)
            }
            TypeRef::Reference { target, .. } | TypeRef::Array { target, .. } => {
                self.validate(target, tu, false, false, context, seen)
            }
            TypeRef::Named { name, declaration } => {
                let Some(fact) = context.exact(tu, name, declaration) else {
                    if context
                        .declarations
                        .get(tu, name, declaration)
                        .iter()
                        .any(|fact| self.providers.contains_key(&fact.origin))
                    {
                        return Err(Error(format!(
                            "ambiguous exact native COM declaration `{name}` at {}:{} in \
                             translation unit `{tu}`",
                            declaration.file, declaration.offset,
                        )));
                    }
                    return Ok(());
                };
                if let Some(provider) = self.providers.get(&fact.origin) {
                    if !pointer {
                        return Err(Error(format!(
                            "native COM record `{name}` requires object layout at {}:{} in \
                             translation unit `{tu}`; only COM pointer uses can bind an interface",
                            declaration.file, declaration.offset,
                        )));
                    }
                    provider.map_err(|failure| Error(failure.message(name)))?;
                }
                if !seen.insert(fact.origin.clone()) {
                    return Ok(());
                }
                let result = match &fact.data {
                    FactData::Typedef { target } => {
                        self.validate(target, tu, counted, pointer, context, seen)
                    }
                    FactData::Record { base, fields, .. }
                        if !self.providers.contains_key(&fact.origin) =>
                    {
                        if let Some(base) = base {
                            self.validate(base, tu, false, false, context, seen)?;
                        }
                        for field in fields {
                            self.validate(&field.ty, tu, false, false, context, seen)?;
                        }
                        Ok(())
                    }
                    _ => Ok(()),
                };
                seen.remove(&fact.origin);
                result
            }
            TypeRef::FunctionPointer { params, result, .. } => {
                self.validate(result, tu, false, false, context, seen)?;
                for param in params {
                    self.validate(param, tu, false, false, context, seen)?;
                }
                Ok(())
            }
            TypeRef::InlineRecord(record) => {
                for field in &record.fields {
                    self.validate(&field.ty, tu, false, false, context, seen)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
