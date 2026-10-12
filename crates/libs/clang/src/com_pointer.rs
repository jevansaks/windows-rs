use super::*;

pub(super) struct Context<'a, 'facts> {
    pub snapshot: &'a Snapshot,
    pub declarations: &'a DeclarationIndex<'facts>,
    pub facts_by_declaration: &'a BTreeMap<(String, Location), &'facts Fact>,
    pub source_names: Option<&'a PlanningSourceNames>,
    pub excluded_types: Option<&'a BTreeSet<String>>,
}

struct TypeExpansion<'a> {
    structural_pairs: &'a mut BTreeSet<(Origin, Origin)>,
    left_aliases: BTreeSet<Origin>,
    right_aliases: BTreeSet<Origin>,
}

impl<'a> TypeExpansion<'a> {
    fn new(structural_pairs: &'a mut BTreeSet<(Origin, Origin)>) -> Self {
        Self {
            structural_pairs,
            left_aliases: BTreeSet::new(),
            right_aliases: BTreeSet::new(),
        }
    }
}

struct RecordLayout<'a> {
    base: Option<&'a TypeRef>,
    fields: &'a [Field],
    size: i64,
    align: i64,
    packing: Option<i64>,
    alignment: Option<i64>,
    union: bool,
}

impl<'a> From<&'a InlineRecord> for RecordLayout<'a> {
    fn from(record: &'a InlineRecord) -> Self {
        Self {
            base: record.base.as_ref(),
            fields: &record.fields,
            size: record.size,
            align: record.align,
            packing: record.packing,
            alignment: record.alignment,
            union: record.union,
        }
    }
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
    ) -> Result<(), String> {
        let (Some(a), Some(b)) = (
            self.snapshot.native_com_contracts.get(&left.origin),
            self.snapshot.native_com_contracts.get(&right.origin),
        ) else {
            return Err(format!(
                "missing native contract for {:?} or {:?}",
                left.origin, right.origin
            ));
        };
        if a.guid != b.guid
            || a.pointer_width != b.pointer_width
            || a.bases != b.bases
            || a.slots.len() != b.slots.len()
        {
            return Err(format!(
                "contract header: IID {:?}/{:?}, pointer width {}/{}, bases {:?}/{:?}, slots {}/{}",
                a.guid,
                b.guid,
                a.pointer_width,
                b.pointer_width,
                a.bases,
                b.bases,
                a.slots.len(),
                b.slots.len()
            ));
        }
        let pair = (left.origin.clone(), right.origin.clone());
        if !seen.insert(pair.clone()) {
            return Ok(());
        }
        let result = (|| {
            for (index, (a, b)) in a.slots.iter().zip(&b.slots).enumerate() {
                if a.name != b.name
                    || a.convention != b.convention
                    || a.params.len() != b.params.len()
                {
                    return Err(format!(
                        "slot {index}: name {:?}/{:?}, native convention {}/{}, parameters {}/{}",
                        a.name,
                        b.name,
                        a.convention,
                        b.convention,
                        a.params.len(),
                        b.params.len()
                    ));
                }
                self.types_equal(
                    &a.result,
                    &left.origin.tu,
                    &b.result,
                    &right.origin.tu,
                    false,
                    seen,
                )
                .map_err(|reason| format!("slot {index} `{}` -> result -> {reason}", a.name))?;
                for (parameter, (a_param, b_param)) in a.params.iter().zip(&b.params).enumerate() {
                    let matched = if a_param.annotation.size.is_some()
                        || b_param.annotation.size.is_some()
                    {
                        if let (Some((a_mut, a_target)), Some((b_mut, b_target))) = (
                            self.pointer_target(&a_param.ty, &left.origin.tu),
                            self.pointer_target(&b_param.ty, &right.origin.tu),
                        ) {
                            if a_mut != b_mut {
                                Err("counted pointer mutability differs".to_string())
                            } else {
                                self.types_equal(
                                    a_target,
                                    &left.origin.tu,
                                    b_target,
                                    &right.origin.tu,
                                    false,
                                    seen,
                                )
                            }
                        } else {
                            Err("counted parameter has no exact native pointer target".to_string())
                        }
                    } else {
                        self.types_equal(
                            &a_param.ty,
                            &left.origin.tu,
                            &b_param.ty,
                            &right.origin.tu,
                            false,
                            seen,
                        )
                    };
                    matched.map_err(|reason| {
                        format!(
                            "slot {index} `{}` -> parameter {parameter} -> {reason}",
                            a.name
                        )
                    })?;
                }
            }
            Ok(())
        })();
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

    fn records_equal(
        &self,
        a: RecordLayout<'_>,
        a_tu: &str,
        b: RecordLayout<'_>,
        b_tu: &str,
        origins: Option<(&Origin, &Origin)>,
        expansion: &mut TypeExpansion<'_>,
    ) -> Result<(), String> {
        if (a.size, a.align, a.packing, a.alignment, a.union)
            != (b.size, b.align, b.packing, b.alignment, b.union)
            || a.fields.len() != b.fields.len()
        {
            return Err(format!(
                "record layout: size {}/{}, alignment {}/{}, packing {:?}/{:?}, \
                 explicit alignment {:?}/{:?}, union {}/{}, fields {}/{}",
                a.size,
                b.size,
                a.align,
                b.align,
                a.packing,
                b.packing,
                a.alignment,
                b.alignment,
                a.union,
                b.union,
                a.fields.len(),
                b.fields.len()
            ));
        }
        let pair = origins.map(|(a, b)| (a.clone(), b.clone()));
        if pair
            .as_ref()
            .is_some_and(|pair| !expansion.structural_pairs.insert(pair.clone()))
        {
            return Ok(());
        }
        let result = (|| {
            match (a.base, b.base) {
                (Some(a_base), Some(b_base)) => self
                    .types_equal_expanded(a_base, a_tu, b_base, b_tu, false, expansion)
                    .map_err(|reason| format!("record base -> {reason}"))?,
                (None, None) => {}
                _ => return Err("record base presence differs".to_string()),
            }
            for (index, (a, b)) in a.fields.iter().zip(b.fields).enumerate() {
                if (a.offset, a.size, a.align, a.bit_width)
                    != (b.offset, b.size, b.align, b.bit_width)
                {
                    return Err(format!(
                        "field {index}: offset {}/{}, size {}/{}, alignment {}/{}, \
                         bit width {:?}/{:?}",
                        a.offset,
                        b.offset,
                        a.size,
                        b.size,
                        a.align,
                        b.align,
                        a.bit_width,
                        b.bit_width
                    ));
                }
                self.types_equal_expanded(&a.ty, a_tu, &b.ty, b_tu, false, expansion)
                    .map_err(|reason| format!("field {index} -> {reason}"))?;
            }
            Ok(())
        })();
        if let Some(pair) = pair {
            expansion.structural_pairs.remove(&pair);
        }
        result
    }

    fn types_equal(
        &self,
        a: &TypeRef,
        a_tu: &str,
        b: &TypeRef,
        b_tu: &str,
        pointer: bool,
        seen: &mut BTreeSet<(Origin, Origin)>,
    ) -> Result<(), String> {
        self.types_equal_expanded(a, a_tu, b, b_tu, pointer, &mut TypeExpansion::new(seen))
    }

    fn types_equal_expanded(
        &self,
        a: &TypeRef,
        a_tu: &str,
        b: &TypeRef,
        b_tu: &str,
        pointer: bool,
        expansion: &mut TypeExpansion<'_>,
    ) -> Result<(), String> {
        if let TypeRef::Named { name, declaration } = a {
            let Some(fact) = self.exact(a_tu, name, declaration) else {
                return Err(format!(
                    "unresolved exact declaration `{name}` at {}:{} in `{a_tu}`",
                    declaration.file, declaration.offset
                ));
            };
            if let FactData::Typedef { target } = &fact.data {
                if !expansion.left_aliases.insert(fact.origin.clone()) {
                    return Err(format!("typedef cycle at {:?}", fact.origin));
                }
                let result = self.types_equal_expanded(target, a_tu, b, b_tu, pointer, expansion);
                expansion.left_aliases.remove(&fact.origin);
                return result
                    .map_err(|reason| format!("alias `{name}` {:?} -> {reason}", fact.origin));
            }
        }
        if let TypeRef::Named { name, declaration } = b {
            let Some(fact) = self.exact(b_tu, name, declaration) else {
                return Err(format!(
                    "unresolved exact declaration `{name}` at {}:{} in `{b_tu}`",
                    declaration.file, declaration.offset
                ));
            };
            if let FactData::Typedef { target } = &fact.data {
                if !expansion.right_aliases.insert(fact.origin.clone()) {
                    return Err(format!("typedef cycle at {:?}", fact.origin));
                }
                let result = self.types_equal_expanded(a, a_tu, target, b_tu, pointer, expansion);
                expansion.right_aliases.remove(&fact.origin);
                return result
                    .map_err(|reason| format!("alias `{name}` {:?} -> {reason}", fact.origin));
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
            ) if a_mut == b_mut => self
                .types_equal_expanded(a, a_tu, b, b_tu, true, expansion)
                .map_err(|reason| format!("pointer target -> {reason}")),
            (
                TypeRef::Reference {
                    mutable: a_mut,
                    target: a,
                },
                TypeRef::Reference {
                    mutable: b_mut,
                    target: b,
                },
            ) if a_mut == b_mut => self
                .types_equal_expanded(a, a_tu, b, b_tu, false, expansion)
                .map_err(|reason| format!("reference layout -> {reason}")),
            (
                TypeRef::Array {
                    target: a,
                    len: a_len,
                },
                TypeRef::Array {
                    target: b,
                    len: b_len,
                },
            ) if a_len == b_len => self
                .types_equal_expanded(a, a_tu, b, b_tu, false, expansion)
                .map_err(|reason| format!("array layout -> {reason}")),
            (TypeRef::InlineRecord(a), TypeRef::InlineRecord(b)) => self
                .records_equal(
                    a.as_ref().into(),
                    a_tu,
                    b.as_ref().into(),
                    b_tu,
                    None,
                    expansion,
                )
                .map_err(|reason| format!("inline record -> {reason}")),
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
                    return Err(format!(
                        "unresolved exact declarations `{a_name}` at {}:{} in `{a_tu}` or \
                         `{b_name}` at {}:{} in `{b_tu}`",
                        a_location.file, a_location.offset, b_location.file, b_location.offset
                    ));
                };
                if pointer
                    && self.snapshot.native_com_contracts.contains_key(&a.origin)
                    && self.snapshot.native_com_contracts.contains_key(&b.origin)
                {
                    return self
                        .contracts_equal(a, b, expansion.structural_pairs)
                        .map_err(|reason| {
                            format!("COM pointees {:?}/{:?} -> {reason}", a.origin, b.origin)
                        });
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
                    ) => self
                        .records_equal(
                            RecordLayout {
                                base: a_base.as_ref(),
                                fields: a_fields,
                                size: *a_size,
                                align: *a_align,
                                packing: *a_pack,
                                alignment: *a_alignment,
                                union: *a_union,
                            },
                            a_tu,
                            RecordLayout {
                                base: b_base.as_ref(),
                                fields: b_fields,
                                size: *b_size,
                                align: *b_align,
                                packing: *b_pack,
                                alignment: *b_alignment,
                                union: *b_union,
                            },
                            b_tu,
                            Some((&a.origin, &b.origin)),
                            &mut TypeExpansion::new(expansion.structural_pairs),
                        )
                        .map_err(|reason| {
                            format!("record {:?}/{:?} -> {reason}", a.origin, b.origin)
                        }),
                    (FactData::Enum { .. }, FactData::Enum { .. }) if a.data == b.data => Ok(()),
                    (FactData::Interface { .. }, FactData::Interface { .. }) => {
                        self.contracts_equal(a, b, expansion.structural_pairs)
                    }
                    _ => Err(format!(
                        "native declaration kinds at {:?}/{:?}: {}/{}",
                        a.origin,
                        b.origin,
                        fact_data_kind(&a.data),
                        fact_data_kind(&b.data)
                    )),
                }
            }
            (TypeRef::Void, TypeRef::Void)
            | (TypeRef::String, TypeRef::String)
            | (TypeRef::Object, TypeRef::Object) => Ok(()),
            (TypeRef::Scalar(a), TypeRef::Scalar(b)) if a == b => Ok(()),
            (
                TypeRef::OpaquePointer {
                    mutable: a_mut,
                    tag: a,
                },
                TypeRef::OpaquePointer {
                    mutable: b_mut,
                    tag: b,
                },
            ) if a_mut == b_mut && a == b => Ok(()),
            _ => Err(format!("unsupported or differing native types {a:?}/{b:?}")),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum Failure {
    Missing,
    Conflicting(Arc<str>),
    Ambiguous,
    Unproved,
}

impl Failure {
    pub fn message(&self, name: &str) -> String {
        let reason = match self {
            Self::Missing => "has no eligible owned interface",
            Self::Conflicting(reason) => {
                return format!(
                    "native COM record `{name}` has conflicting native COM pointer contracts: \
                     {reason}"
                );
            }
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
                    } else if let Some((candidate, reason)) =
                        candidates.iter().find_map(|candidate| {
                            context
                                .contracts_equal(fact, candidate, &mut BTreeSet::new())
                                .err()
                                .map(|reason| (*candidate, reason))
                        })
                    {
                        Err(Failure::Conflicting(format!(
                            "candidate {:?} at {}:{} owner {:?}; provider {:?} at {}:{} owner {:?}; \
                             first mismatch: {reason}",
                            fact.origin, fact.spelling.file, fact.spelling.offset,
                            snapshot.root_owners.get(&fact.origin),
                            candidate.origin, candidate.spelling.file, candidate.spelling.offset,
                            snapshot.root_owners.get(&candidate.origin)
                        ).into()))
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
        self.providers.get(&target.origin).cloned()
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
                    provider
                        .as_ref()
                        .map_err(|failure| Error(failure.message(name)))?;
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
