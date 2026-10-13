use super::*;
use com_pointer::{AuthorityTypeIdentity, AuthorityTypes, SemanticTypeRole};

#[cfg(test)]
thread_local! {
    pub(super) static OBSERVATION_CALLS: Cell<usize> = const { Cell::new(0) };
}

/// The comparison domain, not a claim of complete native equivalence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionEvidenceCoverage {
    NotCompared,
    /// Emitted defining records, layout and pointer/reference referent const only.
    ///
    /// COM bodies, enum bodies, standalone aliases, functions, constants, SAL, volatile and
    /// restrict are not compared. Named referents contribute identity, not recursive body proof.
    RecordsAndReferentConst {
        selected_records: usize,
        alternative_definitions: usize,
        /// Distinct alternative contexts without a defining observation.
        declaration_only_observations: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DefinitionTypeRole {
    Namespace,
    Record,
    Union,
    Enum,
    Alias,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DefinitionIdentity {
    pub header: String,
    pub authority_input: String,
    pub scope: Vec<(DefinitionTypeRole, String)>,
    pub name: String,
    pub role: DefinitionTypeRole,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DefinitionObservation {
    pub partition: Option<String>,
    pub rank: Option<usize>,
    /// Traversing candidate partitions and immutable ranks, sorted by rank.
    pub candidate_partitions: Vec<(String, usize)>,
    pub input: String,
    pub location: Location,
    /// All defining locations; declaration-only observations are counted in coverage.
    pub defining_locations: Vec<Location>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DefinitionDifference {
    pub path: String,
    pub selected: String,
    pub alternative: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DefinitionUnknown {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionComparison {
    Different {
        keys: Vec<DefinitionDifference>,
        unknown: Vec<DefinitionUnknown>,
    },
    Unknown(Vec<DefinitionUnknown>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionEvidence {
    pub identity: DefinitionIdentity,
    pub selected: DefinitionObservation,
    pub alternative: DefinitionObservation,
    pub outcome: DefinitionComparison,
    /// The same message is prepended to RDL for `Different` only.
    pub message: String,
}

impl From<&AuthorityTypeIdentity> for DefinitionIdentity {
    fn from(identity: &AuthorityTypeIdentity) -> Self {
        fn role(role: SemanticTypeRole) -> DefinitionTypeRole {
            match role {
                SemanticTypeRole::Namespace => DefinitionTypeRole::Namespace,
                SemanticTypeRole::Record => DefinitionTypeRole::Record,
                SemanticTypeRole::Union => DefinitionTypeRole::Union,
                SemanticTypeRole::Enum => DefinitionTypeRole::Enum,
                SemanticTypeRole::Alias => DefinitionTypeRole::Alias,
            }
        }
        Self {
            header: identity.header.clone(),
            authority_input: identity.input.clone(),
            scope: identity
                .scope
                .iter()
                .map(|(kind, name)| (role(*kind), name.clone()))
                .collect(),
            name: identity.name.clone(),
            role: role(identity.role),
        }
    }
}

type Keys = BTreeMap<String, Result<String, String>>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct NormalizedType {
    is_const: Result<bool, String>,
    keys: Keys,
}

impl NormalizedType {
    fn atom(value: String) -> Self {
        Self {
            is_const: Ok(false),
            keys: BTreeMap::from([(String::new(), Ok(value))]),
        }
    }

    fn unknown(reason: &str) -> Self {
        Self {
            is_const: Err(reason.to_string()),
            keys: BTreeMap::from([(String::new(), Err(reason.to_string()))]),
        }
    }
}

struct Normalizer<'a, 'facts> {
    snapshot: &'a Snapshot,
    authorities: &'a AuthorityTypes<'facts>,
    aliases: HashMap<Origin, NormalizedType>,
    active: HashMap<Origin, String>,
}

struct RecordView<'a> {
    base: Option<&'a TypeRef>,
    fields: &'a [Field],
    size: i64,
    align: i64,
    packing: Option<i64>,
    alignment: Option<i64>,
    union: bool,
}

impl<'a> From<&'a InlineRecord> for RecordView<'a> {
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

impl Normalizer<'_, '_> {
    fn ty(&mut self, ty: &TypeRef, tu: &str, path: &str) -> NormalizedType {
        match ty {
            TypeRef::Void => NormalizedType::atom("void".to_string()),
            TypeRef::Scalar(scalar) => NormalizedType::atom(format!("scalar:{scalar:?}")),
            TypeRef::Named { name, declaration } => {
                let candidates: Vec<_> = self
                    .authorities
                    .facts_index
                    .get(name.as_str())
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|fact| fact.origin.tu == tu && fact.spelling == *declaration)
                    .collect();
                if candidates.is_empty() {
                    return NormalizedType::unknown("missing own-TU declaration");
                }
                let mut shapes = candidates.into_iter().map(|fact| self.named(fact, path));
                let first = shapes.next().unwrap();
                if shapes.any(|shape| shape != first) {
                    return NormalizedType::unknown(
                        "same-context redeclarations cannot be coalesced",
                    );
                }
                first
            }
            TypeRef::Pointer { mutable, target } | TypeRef::Reference { mutable, target } => {
                let target = self.ty(target, tu, &format!("{path}.target"));
                let mut result = NormalizedType::atom(
                    if matches!(ty, TypeRef::Pointer { .. }) {
                        "pointer"
                    } else {
                        "reference"
                    }
                    .to_string(),
                );
                result.keys.insert(
                    ".mutable".to_string(),
                    if !mutable {
                        Ok("false".to_string())
                    } else {
                        target.is_const.map(|is_const| (!is_const).to_string())
                    },
                );
                extend(&mut result.keys, ".target", target.keys);
                result
            }
            TypeRef::Array { target, len } => {
                let mut result = NormalizedType::atom("array".to_string());
                result.keys.insert(".len".to_string(), Ok(len.to_string()));
                extend(
                    &mut result.keys,
                    ".element",
                    self.ty(target, tu, &format!("{path}.element")).keys,
                );
                result
            }
            TypeRef::FunctionPointer {
                convention,
                params,
                result,
            } => self.function(*convention, params.iter(), result, tu, path),
            TypeRef::InlineRecord(record) => NormalizedType {
                is_const: Ok(false),
                keys: self.record(RecordView::from(record.as_ref()), tu, path),
            },
            TypeRef::String
            | TypeRef::Object
            | TypeRef::OpaquePointer { .. }
            | TypeRef::Generic { .. } => {
                NormalizedType::unknown("unsupported observational type shape")
            }
        }
    }

    fn function<'a>(
        &mut self,
        convention: CallingConvention,
        params: impl ExactSizeIterator<Item = &'a TypeRef>,
        result: &TypeRef,
        tu: &str,
        path: &str,
    ) -> NormalizedType {
        let mut shape = NormalizedType::atom("function-pointer".to_string());
        shape
            .keys
            .insert(".convention".to_string(), Ok(format!("{convention:?}")));
        shape
            .keys
            .insert(".params".to_string(), Ok(params.len().to_string()));
        for (index, param) in params.enumerate() {
            let key = format!(".param[{index}]");
            extend(
                &mut shape.keys,
                &key,
                self.ty(param, tu, &format!("{path}{key}")).keys,
            );
        }
        extend(
            &mut shape.keys,
            ".result",
            self.ty(result, tu, &format!("{path}.result")).keys,
        );
        shape
    }

    fn named(&mut self, fact: &Fact, path: &str) -> NormalizedType {
        if !matches!(
            fact.data,
            FactData::Typedef { .. } | FactData::Callback { .. }
        ) {
            if matches!(fact.data, FactData::Unsupported { .. } | FactData::None) {
                return NormalizedType::unknown("unsupported named referent evidence");
            }
            return match self.authorities.identities.get(&fact.origin) {
                Some(Ok(identity)) => NormalizedType::atom(format!(
                    "identity:{:?}",
                    DefinitionIdentity::from(identity)
                )),
                Some(Err(reason)) => NormalizedType::unknown(reason),
                None => NormalizedType::unknown("missing semantic authority identity"),
            };
        };
        if let Some(shape) = self.aliases.get(&fact.origin) {
            return shape.clone();
        }
        let Some(Some(native)) = self.snapshot.native_typedef_types.get(&fact.origin) else {
            return NormalizedType::unknown("missing own-TU canonical alias const evidence");
        };
        let is_const = native.is_const;
        if let Some(backreference) = self.active.get(&fact.origin) {
            return NormalizedType::atom(format!("backreference:{backreference}"));
        }
        self.active.insert(fact.origin.clone(), path.to_string());
        let mut shape = match &fact.data {
            FactData::Typedef { target } => self.ty(target, &fact.origin.tu, path),
            FactData::Callback {
                convention,
                params,
                result,
            } if self
                .snapshot
                .pointer_callback_aliases
                .contains(&fact.origin) =>
            {
                self.function(
                    *convention,
                    params.iter().map(|param| &param.ty),
                    result,
                    &fact.origin.tu,
                    path,
                )
            }
            _ => NormalizedType::unknown("unsupported bare function alias shape"),
        };
        self.active.remove(&fact.origin);
        // Only the alias's top-level const affects its immediately enclosing indirection.
        shape.is_const = Ok(is_const);
        if !shape.keys.values().any(|value| {
            value
                .as_ref()
                .is_ok_and(|value| value.starts_with("backreference:"))
        }) {
            self.aliases.insert(fact.origin.clone(), shape.clone());
        }
        shape
    }

    fn record(&mut self, record: RecordView<'_>, tu: &str, path: &str) -> Keys {
        let mut keys = BTreeMap::from([(String::new(), Ok("record".to_string()))]);
        for (name, value) in [(".size", record.size), (".align", record.align)] {
            keys.insert(name.to_string(), measured(value));
        }
        for (name, value) in [
            (".packing", record.packing),
            (".alignment", record.alignment),
        ] {
            keys.insert(name.to_string(), Ok(format!("{value:?}")));
        }
        keys.insert(".union".to_string(), Ok(record.union.to_string()));
        if let Some(base) = record.base {
            extend(
                &mut keys,
                ".base",
                self.ty(base, tu, &format!("{path}.base")).keys,
            );
        } else {
            keys.insert(".base".to_string(), Ok("none".to_string()));
        }
        keys.insert(".fields".to_string(), Ok(record.fields.len().to_string()));
        for (index, field) in record.fields.iter().enumerate() {
            let key = format!(".field[{index}]");
            keys.insert(format!("{key}.name"), Ok(field.name.clone()));
            for (name, value) in [
                ("offset", field.offset),
                ("align", field.align),
                ("size", field.size),
            ] {
                keys.insert(format!("{key}.{name}"), measured(value));
            }
            keys.insert(
                format!("{key}.bit_width"),
                Ok(format!("{:?}", field.bit_width)),
            );
            extend(
                &mut keys,
                &format!("{key}.type"),
                self.ty(&field.ty, tu, &format!("{path}{key}.type")).keys,
            );
        }
        keys
    }

    fn definition(&mut self, fact: &Fact) -> Keys {
        let FactData::Record {
            base,
            fields,
            size,
            align,
            packing,
            alignment,
            union,
        } = &fact.data
        else {
            return NormalizedType::unknown("alternative is not a captured record body").keys;
        };
        self.record(
            RecordView {
                base: base.as_ref(),
                fields,
                size: *size,
                align: *align,
                packing: *packing,
                alignment: *alignment,
                union: *union,
            },
            &fact.origin.tu,
            "",
        )
    }

    fn definitions(&mut self, facts: &[&Fact]) -> Keys {
        let mut shapes = facts.iter().map(|fact| self.definition(fact));
        let first = shapes.next().unwrap();
        if shapes.any(|shape| shape != first) {
            BTreeMap::from([(
                String::new(),
                Err("same-context defining observations cannot be coalesced".to_string()),
            )])
        } else {
            first
        }
    }
}

fn measured(value: i64) -> Result<String, String> {
    if value < 0 {
        Err("missing native layout measurement".to_string())
    } else {
        Ok(value.to_string())
    }
}

fn extend(keys: &mut Keys, prefix: &str, nested: Keys) {
    keys.extend(
        nested
            .into_iter()
            .map(|(path, value)| (format!("{prefix}{path}"), value)),
    );
}

fn compare(selected: &Keys, alternative: &Keys) -> Option<DefinitionComparison> {
    let paths: BTreeSet<_> = selected.keys().chain(alternative.keys()).collect();
    let mut differences = Vec::new();
    let mut unknown = BTreeSet::new();
    for path in paths {
        if let Some((path, reason)) =
            unknown_at(selected, path).or_else(|| unknown_at(alternative, path))
        {
            unknown.insert(DefinitionUnknown {
                path: display_path(path, selected, alternative),
                reason: reason.to_string(),
            });
            continue;
        }
        let key = display_path(path, selected, alternative);
        let selected = selected.get(path);
        let alternative = alternative.get(path);
        match (selected, alternative) {
            (Some(Ok(left)), Some(Ok(right))) if left == right => {}
            _ => differences.push(DefinitionDifference {
                path: key,
                selected: selected
                    .and_then(|value| value.as_ref().ok())
                    .cloned()
                    .unwrap_or_else(|| "absent".to_string()),
                alternative: alternative
                    .and_then(|value| value.as_ref().ok())
                    .cloned()
                    .unwrap_or_else(|| "absent".to_string()),
            }),
        }
    }
    let unknown: Vec<_> = unknown.into_iter().collect();
    if !differences.is_empty() {
        Some(DefinitionComparison::Different {
            keys: differences,
            unknown,
        })
    } else if !unknown.is_empty() {
        Some(DefinitionComparison::Unknown(unknown))
    } else {
        None
    }
}

fn unknown_at<'a>(keys: &'a Keys, mut path: &'a str) -> Option<(&'a str, &'a str)> {
    loop {
        if let Some(Err(reason)) = keys.get(path) {
            return Some((path, reason));
        }
        if path.is_empty() {
            return None;
        }
        path = path.rsplit_once('.').map_or("", |(parent, _)| parent);
    }
}

fn display_path(path: &str, selected: &Keys, alternative: &Keys) -> String {
    if path.is_empty() {
        return "definition".to_string();
    }
    let mut prefix = String::new();
    let mut result = Vec::new();
    for part in path.trim_start_matches('.').split('.') {
        prefix.push('.');
        prefix.push_str(part);
        if let Some(index) = part
            .strip_prefix("field[")
            .and_then(|part| part.strip_suffix(']'))
        {
            let name_key = format!("{prefix}.name");
            let name = selected
                .get(&name_key)
                .or_else(|| alternative.get(&name_key));
            if let Some(Ok(name)) = name {
                result.push(format!("field[{index}:{name}]"));
                continue;
            }
        }
        result.push(part.to_string());
    }
    result.join(".")
}

fn observation(
    snapshot: &Snapshot,
    input: &str,
    facts: &[&Fact],
    selected: bool,
) -> (DefinitionObservation, Option<DefinitionUnknown>) {
    let mut candidates = BTreeSet::new();
    let mut reasons = BTreeSet::new();
    for fact in facts {
        let Some(authority) = fact_header_authority(&snapshot.header_authorities, fact) else {
            reasons.insert("missing physical-header authority".to_string());
            continue;
        };
        let owners: Vec<_> = if selected {
            vec![&authority.owner]
        } else {
            authority
                .alternatives
                .iter()
                .filter(|owner| owner.input == input)
                .collect()
        };
        if owners.is_empty() {
            reasons.insert("no traversing owner for original input".to_string());
        }
        for owner in owners {
            match snapshot.partition_authorities.get(&owner.partition) {
                Some(registered) if registered.input == input && owner.input == input => {
                    candidates.insert((registered.rank, owner.partition.clone()));
                }
                _ => {
                    reasons.insert(format!(
                        "missing matching registered provenance for partition {:?}",
                        owner.partition
                    ));
                }
            }
        }
    }
    let candidate_partitions: Vec<_> = candidates
        .into_iter()
        .map(|(rank, partition)| (partition, rank))
        .collect();
    let provenance = if reasons.is_empty() && candidate_partitions.len() == 1 {
        Some(candidate_partitions[0].clone())
    } else {
        if candidate_partitions.len() > 1 {
            reasons.insert(format!(
                "ambiguous traversing owners: {candidate_partitions:?}"
            ));
        }
        None
    };
    let unknown = provenance.is_none().then(|| DefinitionUnknown {
        path: format!(
            "{}.provenance",
            if selected { "selected" } else { "alternative" }
        ),
        reason: reasons.into_iter().collect::<Vec<_>>().join("; "),
    });
    let mut locations: Vec<_> = facts.iter().map(|fact| fact.spelling.clone()).collect();
    locations.sort();
    locations.dedup();
    (
        DefinitionObservation {
            partition: provenance.as_ref().map(|(partition, _)| partition.clone()),
            rank: provenance.map(|(_, rank)| rank),
            candidate_partitions,
            input: input.to_string(),
            location: locations.first().unwrap().clone(),
            defining_locations: locations,
        },
        unknown,
    )
}

fn render(row: &DefinitionEvidence) -> String {
    let changes = match &row.outcome {
        DefinitionComparison::Different { keys, .. } => keys
            .iter()
            .map(|key| format!("{}: {:?} -> {:?}", key.path, key.selected, key.alternative))
            .collect::<Vec<_>>()
            .join("; "),
        DefinitionComparison::Unknown(keys) => keys
            .iter()
            .map(|key| format!("{}: {}", key.path, key.reason))
            .collect::<Vec<_>>()
            .join("; "),
    };
    let mut provenance = String::new();
    for (side, observation) in [
        ("selected", &row.selected),
        ("alternative", &row.alternative),
    ] {
        if observation.partition.is_none() {
            provenance.push_str(&format!(
                " {side}-owners={:?}",
                observation.candidate_partitions
            ));
        }
    }
    format!(
        "record definition {:?} scope={:?} header={:?} selected={:?} rank={} \
         alternative={:?} rank={} comparison={} [{changes}]{provenance}",
        row.identity.name,
        row.identity.scope,
        row.identity.header,
        row.selected.input,
        row.selected
            .rank
            .map_or_else(|| "Unknown".to_string(), |rank| rank.to_string()),
        row.alternative.input,
        row.alternative
            .rank
            .map_or_else(|| "Unknown".to_string(), |rank| rank.to_string()),
        if matches!(row.outcome, DefinitionComparison::Different { .. }) {
            "Different"
        } else {
            "Unknown"
        },
    )
}

pub(super) struct DefinitionObservations {
    pub rows: Vec<DefinitionEvidence>,
    pub coverage: DefinitionEvidenceCoverage,
    pub notes: HashMap<Origin, String>,
}

pub(super) fn observe(
    snapshot: &Snapshot,
    bindings: &com_pointer::Bindings<'_>,
    types: &[PlannedFact<'_>],
) -> DefinitionObservations {
    #[cfg(test)]
    OBSERVATION_CALLS.set(OBSERVATION_CALLS.get() + 1);
    let Some(authorities) = bindings.definition_authorities() else {
        return DefinitionObservations {
            rows: Vec::new(),
            coverage: DefinitionEvidenceCoverage::NotCompared,
            notes: HashMap::new(),
        };
    };
    let mut selected: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for planned in types {
        let fact = planned.fact;
        if fact.definition
            && matches!(fact.data, FactData::Record { .. })
            && let Some(Ok(identity)) = authorities.identities.get(&fact.origin)
        {
            selected
                .entry(DefinitionIdentity::from(identity))
                .or_default()
                .push(fact);
        }
    }
    let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for fact in snapshot
        .facts
        .iter()
        .filter(|fact| is_type_declaration_fact(fact))
    {
        let Some(Ok(identity)) = authorities.identities.get(&fact.origin) else {
            continue;
        };
        let identity = DefinitionIdentity::from(identity);
        if !selected.contains_key(&identity) || fact.origin.tu == identity.authority_input {
            continue;
        }
        groups
            .entry((identity, fact.origin.tu.as_str()))
            .or_default()
            .push(fact);
    }
    let mut declaration_only_observations = 0;
    groups.retain(|_, facts| {
        facts.retain(|fact| fact.definition);
        if facts.is_empty() {
            declaration_only_observations += 1;
            false
        } else {
            true
        }
    });
    let alternative_definitions = groups.len();
    let mut normalizer = Normalizer {
        snapshot,
        authorities,
        aliases: HashMap::new(),
        active: HashMap::new(),
    };
    let mut selected_shapes = BTreeMap::new();
    let mut rows = Vec::new();
    for ((identity, input), definitions) in groups {
        let selected_definitions = &selected[&identity];
        let (selected_observation, selected_unknown) = observation(
            snapshot,
            &identity.authority_input,
            selected_definitions,
            true,
        );
        let (alternative, alternative_unknown) = observation(snapshot, input, &definitions, false);
        let selected_shape = selected_shapes
            .entry(identity.clone())
            .or_insert_with(|| normalizer.definitions(selected_definitions));
        let alternative_shape = normalizer.definitions(&definitions);
        let mut outcome = compare(selected_shape, &alternative_shape)
            .unwrap_or_else(|| DefinitionComparison::Unknown(Vec::new()));
        let unknown = match &mut outcome {
            DefinitionComparison::Different { unknown, .. }
            | DefinitionComparison::Unknown(unknown) => unknown,
        };
        unknown.extend(selected_unknown);
        unknown.extend(alternative_unknown);
        unknown.sort();
        unknown.dedup();
        if let DefinitionComparison::Unknown(unknown) = &outcome
            && unknown.is_empty()
        {
            continue;
        }
        let mut row = DefinitionEvidence {
            identity,
            selected: selected_observation,
            alternative,
            outcome,
            message: String::new(),
        };
        row.message = render(&row);
        rows.push(row);
    }
    rows.sort_by(|left, right| {
        (
            &left.identity,
            left.alternative
                .candidate_partitions
                .first()
                .map(|(_, rank)| rank),
            &left.alternative.input,
            &left.alternative.location,
        )
            .cmp(&(
                &right.identity,
                right
                    .alternative
                    .candidate_partitions
                    .first()
                    .map(|(_, rank)| rank),
                &right.alternative.input,
                &right.alternative.location,
            ))
    });
    let mut notes: HashMap<Origin, String> = HashMap::new();
    for row in &rows {
        if matches!(row.outcome, DefinitionComparison::Different { .. }) {
            for fact in &selected[&row.identity] {
                notes
                    .entry(fact.origin.clone())
                    .or_default()
                    .push_str(&format!("    // windows-clang: {}\n", row.message));
            }
        }
    }
    DefinitionObservations {
        rows,
        coverage: DefinitionEvidenceCoverage::RecordsAndReferentConst {
            selected_records: selected.len(),
            alternative_definitions,
            declaration_only_observations,
        },
        notes,
    }
}

#[cfg(test)]
pub(super) fn compare_extracted_callbacks(
    snapshot: &Snapshot,
    name: &str,
    selected_input: &str,
    alternative_input: &str,
) -> Option<DefinitionComparison> {
    let declarations = DeclarationIndex::new(&snapshot.facts);
    let facts_by_origin = snapshot
        .facts
        .iter()
        .map(|fact| (&fact.origin, fact))
        .collect();
    let facts_by_declaration = snapshot
        .facts
        .iter()
        .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
        .collect();
    let source_annotations = snapshot.source_annotation_signatures();
    let effective_annotations = Snapshot::relative_annotation_signatures(&snapshot.annotations);
    let context = com_pointer::Context {
        snapshot,
        declarations: &declarations,
        facts_by_declaration: &facts_by_declaration,
        source_names: None,
        excluded_types: None,
        source_annotations: &source_annotations,
        effective_annotations: &effective_annotations,
    };
    let bindings = com_pointer::Bindings::new(&context, &facts_by_origin);
    let callback = |input: &str| {
        let fact = snapshot
            .facts
            .iter()
            .find(|fact| fact.origin.tu == input && fact.name == name && fact.definition)
            .unwrap();
        let FactData::Callback {
            convention,
            params,
            result,
        } = &fact.data
        else {
            panic!("{fact:?}")
        };
        TypeRef::FunctionPointer {
            convention: *convention,
            params: params.iter().map(|param| param.ty.clone()).collect(),
            result: Box::new(result.clone()),
        }
    };
    let mut normalizer = Normalizer {
        snapshot,
        authorities: bindings.definition_authorities().unwrap(),
        aliases: HashMap::new(),
        active: HashMap::new(),
    };
    let selected = normalizer.ty(&callback(selected_input), selected_input, "");
    let alternative = normalizer.ty(&callback(alternative_input), alternative_input, "");
    compare(&selected.keys, &alternative.keys)
}
