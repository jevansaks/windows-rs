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
    /// Captured defining bodies and pointer/reference referent const, not full native ABI.
    ///
    /// The three record counters retain their record-only meanings. Interface and enum contexts
    /// are counted separately. Shared enums count physical authority bodies represented by an
    /// emitted provider, not output enums or aliases. Named referents contribute identity, not
    /// recursive body proof.
    CapturedBodiesAndReferentConst {
        selected_records: usize,
        alternative_definitions: usize,
        /// Distinct alternative contexts without a defining observation.
        declaration_only_observations: usize,
        interfaces: DefinitionDomainCoverage,
        enums: DefinitionDomainCoverage,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DefinitionDomainCoverage {
    pub selected_definitions: usize,
    pub alternative_definitions: usize,
    pub declaration_only_observations: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DefinitionBodyKind {
    Record,
    Interface,
    Enum,
    Constant,
}

/// Finalized selected emission settings, separate from native-body comparison keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefinitionEmission {
    Interface {
        guid: Option<String>,
        /// Captured declared-method ordinals in emitted order.
        method_order: Vec<usize>,
    },
    Enum {
        repr: Scalar,
        flags: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DefinitionTypeRole {
    Namespace,
    Record,
    Union,
    Enum,
    Alias,
    Variable,
    AnonymousEnumMember,
    Macro,
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
pub struct DefinitionProvider {
    pub identity: DefinitionIdentity,
    pub observation: DefinitionObservation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionEvidence {
    pub identity: DefinitionIdentity,
    pub body_kind: DefinitionBodyKind,
    /// Final output settings, which may belong to `emitted_provider` rather than `selected`.
    pub selected_emission: Option<DefinitionEmission>,
    pub selected: DefinitionObservation,
    pub alternative: DefinitionObservation,
    /// The actual emitted provider when it differs from the selected physical authority body.
    pub emitted_provider: Option<DefinitionProvider>,
    pub outcome: DefinitionComparison,
    /// The same message is prepended to RDL for `Different` only.
    pub message: String,
}

/// Direct emitted scalar members, separate from record/interface/enum body coverage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConstantEvidenceCoverage {
    NotCompared,
    CapturedScalars {
        selected_members: usize,
        captured_observations: usize,
        /// Distinct captured native origins with `definition=true`, not evaluated locations.
        defining_native_bodies: usize,
        matched_alternative_observations: usize,
        unproved_observations: usize,
    },
}

/// An observation whose complete identity is unproved. No property comparison was made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstantNotCompared {
    pub name: String,
    pub selected: bool,
    pub observation: DefinitionObservation,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ConstantMember {
    root: Origin,
    definition: Origin,
    spelling: Location,
    pub name: String,
}

impl From<&Constant> for ConstantMember {
    fn from(constant: &Constant) -> Self {
        Self {
            root: constant.root.clone(),
            definition: constant.definition.clone(),
            spelling: constant.spelling.clone(),
            name: constant.name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScalarObservationValue {
    Integral(i128),
    F32(u32),
    F64(u64),
}

fn scalar_observation_value(value: &Value) -> Result<ScalarObservationValue, String> {
    match value {
        Value::Signed(value) => Ok(ScalarObservationValue::Integral(i128::from(*value))),
        Value::Unsigned(value) => Ok(ScalarObservationValue::Integral(i128::from(*value))),
        Value::F32(bits) if f32::from_bits(*bits).is_finite() => {
            Ok(ScalarObservationValue::F32(*bits))
        }
        Value::F64(bits) if f64::from_bits(*bits).is_finite() => {
            Ok(ScalarObservationValue::F64(*bits))
        }
        Value::F32(_) | Value::F64(_) => Err("non-finite captured float".to_string()),
        _ => Err("unsupported captured value form".to_string()),
    }
}

#[derive(Debug)]
pub(super) struct ScalarConstantObservation {
    header: Option<String>,
    role: Option<DefinitionTypeRole>,
    identity: Result<DefinitionIdentity, String>,
    observation: DefinitionObservation,
    ty: Result<Scalar, String>,
    value: Result<ScalarObservationValue, String>,
}

impl ScalarConstantObservation {
    fn coalesce(&mut self, other: Self) {
        if self.identity != other.identity || self.observation != other.observation {
            self.identity = Err("conflicting original-member identity or provenance".to_string());
        }
        if self.header != other.header {
            self.header = None;
        }
        if self.role != other.role {
            self.role = None;
        }
        if self.ty != other.ty {
            self.ty = Err("conflicting original-member scalar type".to_string());
        }
        if self.value != other.value {
            self.value = Err("conflicting original-member scalar value".to_string());
        }
    }

    fn keys(&self) -> Keys {
        BTreeMap::from([
            (
                ".type".to_string(),
                self.ty
                    .as_ref()
                    .map(|ty| format!("{ty:?}"))
                    .map_err(Clone::clone),
            ),
            (
                ".value".to_string(),
                self.value
                    .as_ref()
                    .map(|value| match value {
                        ScalarObservationValue::Integral(value) => value.to_string(),
                        ScalarObservationValue::F32(bits) => format!("F32-bits:{bits:08x}"),
                        ScalarObservationValue::F64(bits) => format!("F64-bits:{bits:016x}"),
                    })
                    .map_err(Clone::clone),
            ),
        ])
    }
}

fn constant_observation(
    snapshot: &Snapshot,
    constant: &Constant,
    authority: Option<&HeaderAuthority>,
    defining: bool,
) -> DefinitionObservation {
    let mut candidates = BTreeSet::new();
    if let Some(authority) = authority {
        let owners: Vec<_> = if authority.owner.input == constant.root.tu {
            vec![&authority.owner]
        } else {
            authority
                .alternatives
                .iter()
                .filter(|owner| owner.input == constant.root.tu)
                .collect()
        };
        for owner in owners {
            if let Some(registered) = snapshot.partition_authorities.get(&owner.partition)
                && registered.input == constant.root.tu
            {
                candidates.insert((registered.rank, owner.partition.clone()));
            }
        }
    }
    let candidate_partitions: Vec<_> = candidates
        .into_iter()
        .map(|(rank, partition)| (partition, rank))
        .collect();
    let unique = (candidate_partitions.len() == 1).then(|| &candidate_partitions[0]);
    DefinitionObservation {
        partition: unique.map(|(partition, _)| partition.clone()),
        rank: unique.map(|(_, rank)| *rank),
        candidate_partitions,
        input: constant.root.tu.clone(),
        location: constant.spelling.clone(),
        defining_locations: if defining {
            vec![constant.spelling.clone()]
        } else {
            Vec::new()
        },
    }
}

pub(super) fn scalar_constant_index(
    snapshot: &Snapshot,
) -> BTreeMap<ConstantMember, Option<ScalarConstantObservation>> {
    let mut facts = HashMap::new();
    for fact in &snapshot.facts {
        facts
            .entry(&fact.origin)
            .and_modify(|entry| *entry = None)
            .or_insert(Some(fact));
    }
    let facts: HashMap<_, _> = facts
        .into_iter()
        .filter_map(|(origin, fact)| fact.map(|fact| (origin, fact)))
        .collect();
    let mut declarations = HashMap::new();
    for declaration in &snapshot.value_declarations {
        declarations
            .entry(&declaration.origin)
            .and_modify(|entry| *entry = None)
            .or_insert(Some(declaration));
    }
    let mut aliases = HashMap::new();
    for fact in &snapshot.facts {
        if let FactData::Typedef { target } = &fact.data {
            let key = (fact.origin.tu.as_str(), fact.name.as_str(), &fact.spelling);
            aliases
                .entry(key)
                .and_modify(|entry| {
                    if *entry != Some(target) {
                        *entry = None;
                    }
                })
                .or_insert(Some(target));
        }
    }
    let aliases = aliases
        .into_iter()
        .filter_map(|(key, target)| target.map(|target| (key, target)))
        .collect();
    let observations = snapshot.constants.iter().map(|constant| {
        let member = ConstantMember::from(constant);
        if !snapshot.ranked_header_partition_policy {
            return (member, None);
        }
        let fact = facts.get(&constant.root).copied();
        let declaration = declarations.get(&constant.root).copied().flatten();
        let authority = constant_header_authority(&snapshot.header_authorities, fact, constant);
        let header = authority.map(|authority| normalize_name(&authority.owner.root));
        let role = declaration
            .map(|declaration| match declaration.kind {
                ValueDeclarationKind::Variable => DefinitionTypeRole::Variable,
                ValueDeclarationKind::AnonymousEnum => DefinitionTypeRole::AnonymousEnumMember,
            })
            .or_else(|| {
                fact.filter(|fact| {
                    fact.kind == FactKind::Macro
                        && fact.name == constant.name
                        && matches!(
                            fact.data,
                            FactData::Macro {
                                function_like: false,
                                ..
                            }
                        )
                })
                .map(|_| DefinitionTypeRole::Macro)
            });
        let identity = (|| {
            let authority = authority.ok_or("missing physical-header authority")?;
            let role = role.ok_or("missing unambiguous captured source declaration kind")?;
            let scope = if role == DefinitionTypeRole::Macro {
                Vec::new()
            } else {
                com_pointer::checked_semantic_scope(
                    snapshot,
                    &facts,
                    &constant.root,
                    |fact| fact.name.clone(),
                    || format!("constant member {:?}", constant.name),
                )?
            };
            if constant.name.is_empty() || scope.iter().any(|(_, name)| name.is_empty()) {
                return Err("anonymous or unnamed native semantic scope".to_string());
            }
            let scope = scope
                .into_iter()
                .map(|(role, name)| {
                    let role = match role {
                        SemanticTypeRole::Namespace => DefinitionTypeRole::Namespace,
                        SemanticTypeRole::Record => DefinitionTypeRole::Record,
                        SemanticTypeRole::Union => DefinitionTypeRole::Union,
                        SemanticTypeRole::Enum => DefinitionTypeRole::Enum,
                        SemanticTypeRole::Alias => DefinitionTypeRole::Alias,
                    };
                    (role, name)
                })
                .collect();
            Ok(DefinitionIdentity {
                header: header.clone().unwrap(),
                authority_input: authority.owner.input.clone(),
                scope,
                name: constant.name.clone(),
                role,
            })
        })();
        let defining = declaration.is_some_and(|declaration| declaration.definition)
            || fact.is_some_and(|fact| fact.definition);
        let observation = constant_observation(snapshot, constant, authority, defining);
        let ty = extract::macro_scalar_type(&constant.ty, &constant.root.tu, &aliases, None)
            .ok_or_else(|| "unsupported or unresolved own-TU scalar type".to_string());
        (
            member,
            Some(ScalarConstantObservation {
                header,
                role,
                identity,
                observation,
                ty,
                value: scalar_observation_value(&constant.value),
            }),
        )
    });
    let mut result: BTreeMap<_, Option<ScalarConstantObservation>> = BTreeMap::new();
    for (member, observation) in observations {
        match result.entry(member) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(observation);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if let (Some(previous), Some(observation)) = (entry.get_mut(), observation) {
                    previous.coalesce(observation);
                }
            }
        }
    }
    result
}

pub(super) struct ConstantObservations {
    pub rows: Vec<DefinitionEvidence>,
    pub coverage: ConstantEvidenceCoverage,
    pub not_compared: Vec<ConstantNotCompared>,
    pub notes: BTreeMap<ConstantMember, String>,
    pub members: BTreeMap<ConstantMember, ConstantMember>,
}

pub(super) fn observe_constants(
    snapshot: &Snapshot,
    constants: &[PlannedConstant<'_>],
    source_names: Option<&PlanningSourceNames>,
) -> ConstantObservations {
    let mut result = ConstantObservations {
        rows: Vec::new(),
        coverage: ConstantEvidenceCoverage::NotCompared,
        not_compared: Vec::new(),
        notes: BTreeMap::new(),
        members: BTreeMap::new(),
    };
    let Some(source_names) = source_names.filter(|_| snapshot.ranked_header_partition_policy)
    else {
        return result;
    };
    // Header/name/kind only discover candidates. Complete proved identities gate all keys.
    let mut candidates: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for (member, observation) in &source_names.constant_evidence {
        if let Some(observation) = observation {
            candidates
                .entry((&observation.header, &member.name, observation.role))
                .or_default()
                .push((member, observation));
        }
    }
    let mut selected_members = 0;
    let mut captured = BTreeSet::new();
    let mut defining = BTreeSet::new();
    let mut matched_alternatives = BTreeSet::new();
    let mut unproved = BTreeSet::new();
    for planned in constants {
        let current = ConstantMember::from(planned.constant);
        let originals = source_names.constant_members.get(&current);
        if !matches!(
            planned.constant.value,
            Value::Signed(_) | Value::Unsigned(_) | Value::F32(_) | Value::F64(_)
        ) || (!matches!(planned.constant.ty, TypeRef::Scalar(_))
            && !originals.is_some_and(|members| {
                members.iter().any(|member| {
                    source_names
                        .constant_evidence
                        .get(member)
                        .and_then(Option::as_ref)
                        .is_some_and(|observation| observation.ty.is_ok())
                })
            }))
        {
            continue;
        }
        let Some(member) = source_names.constant_member(planned.constant) else {
            selected_members += 1;
            unproved.insert(current.clone());
            if let Some(originals) = originals {
                for member in originals {
                    if let Some(Some(observation)) = source_names.constant_evidence.get(member) {
                        captured.insert(member.clone());
                        if !observation.observation.defining_locations.is_empty() {
                            defining.insert(member.root.clone());
                        }
                    }
                }
            } else {
                captured.insert(current.clone());
            }
            let root = snapshot
                .facts
                .iter()
                .find(|fact| fact.origin == current.root);
            let authority =
                constant_header_authority(&snapshot.header_authorities, root, planned.constant);
            let is_definition = root.is_some_and(|fact| fact.definition)
                || snapshot.value_declarations.iter().any(|declaration| {
                    declaration.origin == current.root && declaration.definition
                });
            if is_definition {
                defining.insert(current.root.clone());
            }
            result.not_compared.push(ConstantNotCompared {
                name: planned.constant.name.clone(),
                selected: true,
                observation: constant_observation(
                    snapshot,
                    planned.constant,
                    authority,
                    is_definition,
                ),
                reason: match originals {
                    Some(originals) => format!(
                        "ambiguous original-member token: {}",
                        originals
                            .iter()
                            .map(|member| format!("{:?}", member.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    None => "missing original-member token".to_string(),
                },
            });
            continue;
        };
        let Some(Some(selected)) = source_names.constant_evidence.get(member) else {
            continue;
        };
        if selected.ty.is_err()
            || matches!(planned.constant.value, Value::Utf8(_) | Value::Utf16(_))
        {
            continue;
        }
        selected_members += 1;
        result.members.insert(current, member.clone());
        captured.insert(member.clone());
        if !selected.observation.defining_locations.is_empty() {
            defining.insert(member.root.clone());
        }
        let identity = match &selected.identity {
            Ok(identity) => identity,
            Err(reason) => {
                unproved.insert(member.clone());
                result.not_compared.push(ConstantNotCompared {
                    name: member.name.clone(),
                    selected: true,
                    observation: selected.observation.clone(),
                    reason: reason.clone(),
                });
                continue;
            }
        };
        let selected_keys = selected.keys();
        let mut alternatives: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for &(other_member, alternative) in
            &candidates[&(&selected.header, &member.name, selected.role)]
        {
            if other_member.root.tu == member.root.tu {
                continue;
            }
            match &alternative.identity {
                Ok(other_identity) if other_identity == identity => {
                    captured.insert(other_member.clone());
                    matched_alternatives.insert(other_member);
                    if !alternative.observation.defining_locations.is_empty() {
                        defining.insert(other_member.root.clone());
                    }
                    alternatives
                        .entry(&other_member.root.tu)
                        .or_default()
                        .push(alternative);
                }
                Err(reason) if unproved.insert(other_member.clone()) => {
                    captured.insert(other_member.clone());
                    result.not_compared.push(ConstantNotCompared {
                        name: other_member.name.clone(),
                        selected: false,
                        observation: alternative.observation.clone(),
                        reason: reason.clone(),
                    });
                }
                _ => {}
            }
        }
        for observations in alternatives.into_values() {
            let keys: Vec<_> = observations
                .iter()
                .map(|observation| observation.keys())
                .collect();
            let coalesce = |key: &str| {
                let first = keys[0][key].clone();
                if keys.iter().all(|keys| keys[key] == first) {
                    first
                } else {
                    Err("same-context scalar observations cannot be coalesced".to_string())
                }
            };
            let alternative_keys = BTreeMap::from([
                (".type".to_string(), coalesce(".type")),
                (".value".to_string(), coalesce(".value")),
            ]);
            let mut outcome = compare(&selected_keys, &alternative_keys)
                .unwrap_or_else(|| DefinitionComparison::Unknown(Vec::new()));
            let mut alternative = observations[0].observation.clone();
            alternative.defining_locations = observations
                .iter()
                .flat_map(|observation| observation.observation.defining_locations.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let unknown = match &mut outcome {
                DefinitionComparison::Different { unknown, .. }
                | DefinitionComparison::Unknown(unknown) => unknown,
            };
            for (side, observation) in [
                ("selected", &selected.observation),
                ("alternative", &alternative),
            ] {
                if observation.partition.is_none() {
                    unknown.push(DefinitionUnknown {
                        path: format!("{side}.provenance"),
                        reason: "missing or ambiguous registered traversing owner".to_string(),
                    });
                }
            }
            if matches!(&outcome, DefinitionComparison::Unknown(unknown) if unknown.is_empty()) {
                continue;
            }
            let mut row = DefinitionEvidence {
                identity: identity.clone(),
                body_kind: DefinitionBodyKind::Constant,
                selected_emission: None,
                selected: selected.observation.clone(),
                alternative,
                emitted_provider: None,
                outcome,
                message: String::new(),
            };
            row.message = render(&row);
            if matches!(row.outcome, DefinitionComparison::Different { .. }) {
                result
                    .notes
                    .entry(member.clone())
                    .or_default()
                    .push_str(&format!("    // windows-clang: {}\n", row.message));
            }
            result.rows.push(row);
        }
    }
    result.coverage = ConstantEvidenceCoverage::CapturedScalars {
        selected_members,
        captured_observations: captured.len(),
        defining_native_bodies: defining.len(),
        matched_alternative_observations: matched_alternatives.len(),
        unproved_observations: unproved.len(),
    };
    result
}

#[cfg(test)]
#[path = "tests/scalar_constants.rs"]
mod scalar_constant_tests;

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

    fn annotations(&self, target: AnnotationTarget) -> Keys {
        let mut families: BTreeMap<&str, Result<BTreeSet<String>, String>> = BTreeMap::new();
        for annotation in annotations_for(&self.snapshot.source_annotations, &target) {
            let (family, value) = match annotation {
                Annotation::PreserveResult => ("preserve_result", Ok("true".to_string())),
                Annotation::DoNotRelease => ("do_not_release", Ok("true".to_string())),
                Annotation::NotNullTerminated => ("not_null_terminated", Ok("true".to_string())),
                Annotation::NullNullTerminated => ("null_null_terminated", Ok("true".to_string())),
                Annotation::CanReturnErrorsAsSuccess => {
                    ("can_return_errors_as_success", Ok("true".to_string()))
                }
                Annotation::CanReturnMultipleSuccessValues => {
                    ("can_return_multiple_success_values", Ok("true".to_string()))
                }
                Annotation::Retained => ("retained", Ok("true".to_string())),
                Annotation::Ansi => ("ansi", Ok("true".to_string())),
                Annotation::Unicode => ("unicode", Ok("true".to_string())),
                Annotation::Agile => ("agile", Ok("true".to_string())),
                Annotation::Const => ("const", Ok("true".to_string())),
                Annotation::In => ("in", Ok("true".to_string())),
                Annotation::Out => ("out", Ok("true".to_string())),
                Annotation::Optional => ("optional", Ok("true".to_string())),
                Annotation::Reserved => ("reserved", Ok("true".to_string())),
                Annotation::ComOutPtr => ("com_out_ptr", Ok("true".to_string())),
                Annotation::Retval => ("retval", Ok("true".to_string())),
                Annotation::SupportedOs(value) => ("supported_os", Ok(value.clone())),
                Annotation::ArrayCountConst(value) | Annotation::MemorySizeConst(value) => {
                    let value = value
                        .parse::<i32>()
                        .map(|value| value.to_string())
                        .map_err(|_| "unsupported captured constant count".to_string());
                    (
                        if matches!(annotation, Annotation::ArrayCountConst(_)) {
                            "array_count_const"
                        } else {
                            "memory_size_const"
                        },
                        value,
                    )
                }
                Annotation::ArrayCountParam(value) | Annotation::MemorySizeParam(value) => {
                    let value = value
                        .parse::<i16>()
                        .map(|value| value.to_string())
                        .map_err(|_| "unsupported captured parameter count".to_string());
                    (
                        if matches!(annotation, Annotation::ArrayCountParam(_)) {
                            "array_count_param"
                        } else {
                            "memory_size_param"
                        },
                        value,
                    )
                }
                _ => (
                    "uncompared",
                    Err("annotation outside the captured comparison domain".to_string()),
                ),
            };
            let values = families
                .entry(family)
                .or_insert_with(|| Ok(BTreeSet::new()));
            match (values.as_mut(), value) {
                (Ok(values), Ok(value)) => {
                    values.insert(value);
                }
                (_, Err(reason)) => *values = Err(reason),
                (Err(_), Ok(_)) => {}
            }
        }
        families
            .into_iter()
            .map(|(family, values)| {
                (
                    format!(".{family}"),
                    values.map(|values| format!("{values:?}")),
                )
            })
            .collect()
    }

    fn parameter_annotation(&self, annotation: &ParamAnnotation, params: &[Parameter]) -> Keys {
        let mut keys = BTreeMap::new();
        for (name, value) in [
            ("input", annotation.input),
            ("output", annotation.output),
            ("optional", annotation.optional),
            ("reserved", annotation.reserved),
            ("com_out_ptr", annotation.com_out_ptr),
            ("retval", annotation.retval),
            ("null_terminated", annotation.null_terminated),
            ("null_null_terminated", annotation.null_null_terminated),
        ] {
            keys.insert(format!(".{name}"), Ok(value.to_string()));
        }
        if let Some(reason) = &annotation.unsupported {
            keys.insert(
                ".unsupported".to_string(),
                Err(format!(
                    "unsupported captured parameter annotation: {reason}"
                )),
            );
        }
        keys.insert(
            ".size.present".to_string(),
            Ok(annotation.size.is_some().to_string()),
        );
        if let Some(size) = &annotation.size {
            keys.insert(".size.bytes".to_string(), Ok(size.bytes.to_string()));
            let value = match &size.value {
                SalSizeValue::Constant(value) => Ok(format!("constant:{value}")),
                SalSizeValue::Parameter(name) | SalSizeValue::IndirectParameter(name) => params
                    .iter()
                    .position(|param| param.name == *name)
                    .map(|index| {
                        format!(
                            "{}:{index}",
                            if matches!(size.value, SalSizeValue::Parameter(_)) {
                                "parameter"
                            } else {
                                "indirect-parameter"
                            }
                        )
                    })
                    .ok_or_else(|| "missing own-method count parameter".to_string()),
                SalSizeValue::Expression(_) => {
                    Err("unsupported captured count expression".to_string())
                }
            };
            keys.insert(".size.value".to_string(), value);
        }
        keys
    }

    fn parameters(&mut self, params: &[Parameter], tu: &str, path: &str) -> Keys {
        let mut keys = BTreeMap::from([(String::new(), Ok(params.len().to_string()))]);
        for (index, param) in params.iter().enumerate() {
            let key = format!(".param[{index}]");
            keys.insert(format!("{key}.name"), Ok(param.name.clone()));
            extend(
                &mut keys,
                &format!("{key}.type"),
                self.ty(&param.ty, tu, &format!("{path}{key}.type")).keys,
            );
            extend(
                &mut keys,
                &format!("{key}.annotation"),
                self.parameter_annotation(&param.annotation, params),
            );
        }
        keys
    }

    fn interface(&mut self, fact: &Fact) -> Keys {
        let tu = &fact.origin.tu;
        let mut keys = BTreeMap::new();
        if let FactData::Interface {
            base,
            guid,
            methods,
        } = &fact.data
        {
            keys.insert(".declared.guid".to_string(), Ok(format!("{guid:?}")));
            if let Some(base) = base {
                extend(
                    &mut keys,
                    ".declared.base",
                    self.ty(base, tu, ".declared.base").keys,
                );
            } else {
                keys.insert(".declared.base".to_string(), Ok("none".to_string()));
            }
            extend(
                &mut keys,
                ".declared.annotations",
                self.annotations(AnnotationTarget::Declaration(fact.origin.clone())),
            );
            keys.insert(
                ".declared.methods".to_string(),
                Ok(methods.len().to_string()),
            );
            for (index, method) in methods.iter().enumerate() {
                let key = format!(".declared.method[{index}]");
                keys.insert(format!("{key}.name"), Ok(method.name.clone()));
                keys.insert(format!("{key}.special"), Ok(method.special.to_string()));
                extend(
                    &mut keys,
                    &format!("{key}.annotations"),
                    self.annotations(AnnotationTarget::Method {
                        declaration: fact.origin.clone(),
                        index,
                    }),
                );
                extend(
                    &mut keys,
                    &format!("{key}.params"),
                    self.parameters(&method.params, tu, &format!("{key}.params")),
                );
                for parameter in 0..method.params.len() {
                    extend(
                        &mut keys,
                        &format!("{key}.params.param[{parameter}].annotations"),
                        self.annotations(AnnotationTarget::MethodParameter {
                            declaration: fact.origin.clone(),
                            method: index,
                            parameter,
                        }),
                    );
                }
                extend(
                    &mut keys,
                    &format!("{key}.result.type"),
                    self.ty(&method.result, tu, &format!("{key}.result.type"))
                        .keys,
                );
                extend(
                    &mut keys,
                    &format!("{key}.result.annotations"),
                    self.annotations(AnnotationTarget::MethodReturn {
                        declaration: fact.origin.clone(),
                        index,
                    }),
                );
            }
        } else {
            keys.insert(
                ".declared".to_string(),
                Err("missing captured declared-interface body, base and annotations".to_string()),
            );
        }
        let snapshot = self.snapshot;
        if let Some(contract) = snapshot.native_com_contracts.get(&fact.origin) {
            keys.insert(".slots.guid".to_string(), Ok(contract.guid.clone()));
            let base = match &fact.data {
                FactData::Interface { base, .. } | FactData::Record { base, .. } => base.as_ref(),
                _ => None,
            };
            if let Some(base) = base {
                extend(
                    &mut keys,
                    ".slots.base",
                    self.ty(base, tu, ".slots.base").keys,
                );
            } else if contract.bases.is_empty() {
                keys.insert(".slots.base".to_string(), Ok("none".to_string()));
            } else {
                // Base IID strings do not establish semantic declaration identity.
                keys.insert(
                    ".slots.base".to_string(),
                    Err("contract base IIDs have no declaration linkage".to_string()),
                );
            }
            keys.insert(
                ".slots.count".to_string(),
                Ok(contract.slots.len().to_string()),
            );
            for (index, slot) in contract.slots.iter().enumerate() {
                let key = format!(".slots.slot[{index}]");
                keys.insert(format!("{key}.name"), Ok(slot.name.clone()));
                keys.insert(format!("{key}.convention"), Ok(slot.convention.to_string()));
                extend(
                    &mut keys,
                    &format!("{key}.params"),
                    self.parameters(&slot.params, tu, &format!("{key}.params")),
                );
                extend(
                    &mut keys,
                    &format!("{key}.result"),
                    self.ty(&slot.result, tu, &format!("{key}.result")).keys,
                );
            }
        } else {
            keys.insert(
                ".slots".to_string(),
                Err("missing captured COM contract".to_string()),
            );
        }
        keys
    }

    fn enumeration(&self, fact: &Fact) -> Keys {
        if self
            .snapshot
            .shared_enum_identities
            .contains_key(&fact.origin)
        {
            return NormalizedType::unknown("shared literal enum is outside the comparison domain")
                .keys;
        }
        self.enumeration_body(fact)
    }

    fn enumeration_body(&self, fact: &Fact) -> Keys {
        let FactData::Enum {
            repr,
            variants,
            fixed,
            scoped,
        } = &fact.data
        else {
            return NormalizedType::unknown("alternative is not a captured enum body").keys;
        };
        let mut keys = BTreeMap::from([
            (".repr".to_string(), Ok(format!("{repr:?}"))),
            (".fixed".to_string(), Ok(fixed.to_string())),
            (".scoped".to_string(), Ok(scoped.to_string())),
            (".variants".to_string(), Ok(variants.len().to_string())),
        ]);
        extend(
            &mut keys,
            ".annotations",
            self.annotations(AnnotationTarget::Declaration(fact.origin.clone())),
        );
        for (index, variant) in variants.iter().enumerate() {
            let key = format!(".variant[{index}]");
            keys.insert(format!("{key}.name"), Ok(variant.name.clone()));
            keys.insert(format!("{key}.value"), Ok(enum_value(variant.value, *repr)));
            extend(
                &mut keys,
                &format!("{key}.annotations"),
                self.annotations(AnnotationTarget::Variant {
                    declaration: fact.origin.clone(),
                    index,
                }),
            );
        }
        keys
    }

    fn definition(&mut self, fact: &Fact, kind: DefinitionBodyKind) -> Keys {
        match kind {
            DefinitionBodyKind::Interface => return self.interface(fact),
            DefinitionBodyKind::Enum => return self.enumeration(fact),
            DefinitionBodyKind::Record => {}
            DefinitionBodyKind::Constant => {
                return NormalizedType::unknown("constant is not a type body").keys;
            }
        }
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

    fn definitions(
        &mut self,
        facts: &[&Fact],
        kind: DefinitionBodyKind,
        shared_enum: bool,
    ) -> Keys {
        let mut shapes = facts.iter().map(|fact| {
            if shared_enum {
                self.enumeration_body(fact)
            } else {
                self.definition(fact, kind)
            }
        });
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
    if let Some(provider) = &row.emitted_provider {
        provenance.push_str(&format!(
            " emitted-provider={:?} header={:?} input={:?} partition={:?} rank={:?} \
             locations={:?}",
            provider.identity.name,
            provider.identity.header,
            provider.observation.input,
            provider.observation.partition,
            provider.observation.rank,
            provider.observation.defining_locations,
        ));
    }
    format!(
        "{} definition {:?} scope={:?} header={:?} selected={:?} rank={} \
         alternative={:?} rank={} comparison={} [{changes}]{provenance}",
        match row.body_kind {
            DefinitionBodyKind::Record => "record",
            DefinitionBodyKind::Interface => "interface",
            DefinitionBodyKind::Enum => "enum",
            DefinitionBodyKind::Constant => "constant",
        },
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
    interface_guids: &BTreeMap<String, String>,
    flag_enums: &BTreeSet<(String, String)>,
    source_names: Option<&PlanningSourceNames>,
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
    let mut kinds = BTreeMap::new();
    let mut emissions = BTreeMap::new();
    let mut enum_providers = HashMap::new();
    for planned in types {
        let fact = planned.fact;
        if let FactData::Enum { repr, .. } = &fact.data {
            let coerced = flag_enums.contains(&(fact.origin.tu.clone(), planned.name.clone()));
            enum_providers.insert(
                &fact.origin,
                (
                    fact,
                    DefinitionEmission::Enum {
                        repr: if coerced {
                            unsigned_scalar(*repr)
                        } else {
                            *repr
                        },
                        flags: coerced || snapshot.clang_flag_enums.contains(&fact.origin),
                    },
                ),
            );
        }
        if fact.definition
            && matches!(
                fact.data,
                FactData::Record { .. } | FactData::Interface { .. } | FactData::Enum { .. }
            )
            && (!matches!(fact.data, FactData::Enum { .. })
                || (fact.kind == FactKind::Enum
                    && !snapshot.shared_enum_identities.contains_key(&fact.origin)))
            && let Some(Ok(identity)) = authorities.identities.get(&fact.origin)
        {
            let identity = DefinitionIdentity::from(identity);
            let (kind, emission) = match &fact.data {
                FactData::Interface { guid, methods, .. } => {
                    let mut method_order = Vec::new();
                    let mut start = 0;
                    while start < methods.len() {
                        let mut end = start + 1;
                        while end < methods.len() && methods[end].name == methods[start].name {
                            end += 1;
                        }
                        method_order.extend((start..end).rev());
                        start = end;
                    }
                    (
                        DefinitionBodyKind::Interface,
                        Some(DefinitionEmission::Interface {
                            guid: guid
                                .as_ref()
                                .or_else(|| interface_guids.get(&planned.name))
                                .cloned(),
                            method_order,
                        }),
                    )
                }
                FactData::Enum { .. } => (
                    DefinitionBodyKind::Enum,
                    Some(enum_providers[&fact.origin].1.clone()),
                ),
                _ => (DefinitionBodyKind::Record, None),
            };
            kinds.insert(identity.clone(), kind);
            emissions.insert(identity.clone(), emission);
            selected.entry(identity).or_default().push(fact);
        }
    }
    let mut shared_providers = BTreeMap::new();
    if let Some(source_names) = source_names {
        for fact in &snapshot.facts {
            if fact.kind != FactKind::Enum
                || !fact.definition
                || !snapshot.shared_enum_identities.contains_key(&fact.origin)
                || fact_header_authority(&snapshot.header_authorities, fact)
                    .is_none_or(|authority| authority.owner.input != fact.origin.tu)
            {
                continue;
            }
            let Some(Ok(provider)) = source_names.shared_enum_providers.get(&fact.origin) else {
                continue;
            };
            let Some((provider, emission)) = enum_providers.get(provider) else {
                continue;
            };
            let Some(Ok(identity)) = authorities.identities.get(&fact.origin) else {
                continue;
            };
            let Some(Ok(provider_identity)) = authorities.identities.get(&provider.origin) else {
                continue;
            };
            let identity = DefinitionIdentity::from(identity);
            kinds.insert(identity.clone(), DefinitionBodyKind::Enum);
            emissions.insert(identity.clone(), Some(emission.clone()));
            selected.entry(identity.clone()).or_default().push(fact);
            shared_providers.insert(
                identity,
                (*provider, DefinitionIdentity::from(provider_identity)),
            );
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
    let mut coverage: [DefinitionDomainCoverage; 3] = std::array::from_fn(|_| Default::default());
    let index = |kind| match kind {
        DefinitionBodyKind::Record => 0,
        DefinitionBodyKind::Interface => 1,
        DefinitionBodyKind::Enum => 2,
        DefinitionBodyKind::Constant => unreachable!(),
    };
    for kind in kinds.values() {
        coverage[index(*kind)].selected_definitions += 1;
    }
    groups.retain(|(identity, _), facts| {
        facts.retain(|fact| fact.definition);
        if facts.is_empty() {
            coverage[index(kinds[identity])].declaration_only_observations += 1;
            false
        } else {
            coverage[index(kinds[identity])].alternative_definitions += 1;
            true
        }
    });
    let [records, interfaces, enums] = coverage;
    let mut normalizer = Normalizer {
        snapshot,
        authorities,
        aliases: HashMap::new(),
        active: HashMap::new(),
    };
    let mut selected_shapes = BTreeMap::new();
    let mut rows = Vec::new();
    for ((identity, input), definitions) in groups {
        let kind = kinds[&identity];
        let selected_definitions = &selected[&identity];
        let (selected_observation, selected_unknown) = observation(
            snapshot,
            &identity.authority_input,
            selected_definitions,
            true,
        );
        let (alternative, alternative_unknown) = observation(snapshot, input, &definitions, false);
        let selected_shape = selected_shapes.entry(identity.clone()).or_insert_with(|| {
            normalizer.definitions(
                selected_definitions,
                kind,
                shared_providers.contains_key(&identity),
            )
        });
        let alternative_shape =
            normalizer.definitions(&definitions, kind, shared_providers.contains_key(&identity));
        let mut outcome = compare(selected_shape, &alternative_shape)
            .unwrap_or_else(|| DefinitionComparison::Unknown(Vec::new()));
        let unknown = match &mut outcome {
            DefinitionComparison::Different { unknown, .. }
            | DefinitionComparison::Unknown(unknown) => unknown,
        };
        unknown.extend(selected_unknown);
        unknown.extend(alternative_unknown);
        let emitted_provider = shared_providers
            .get(&identity)
            .filter(|(_, provider_identity)| *provider_identity != identity)
            .map(|(provider, provider_identity)| {
                let (observation, provider_unknown) =
                    observation(snapshot, &provider.origin.tu, &[*provider], true);
                unknown.extend(provider_unknown.map(|mut unknown| {
                    unknown.path = "emitted_provider.provenance".to_string();
                    unknown
                }));
                DefinitionProvider {
                    identity: provider_identity.clone(),
                    observation,
                }
            });
        unknown.sort();
        unknown.dedup();
        if let DefinitionComparison::Unknown(unknown) = &outcome
            && unknown.is_empty()
        {
            continue;
        }
        let mut row = DefinitionEvidence {
            body_kind: kind,
            selected_emission: emissions[&identity].clone(),
            identity,
            selected: selected_observation,
            alternative,
            emitted_provider,
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
            let origins: BTreeSet<_> = match shared_providers.get(&row.identity) {
                Some((provider, _)) => BTreeSet::from([&provider.origin]),
                None => selected[&row.identity]
                    .iter()
                    .map(|fact| &fact.origin)
                    .collect(),
            };
            for origin in origins {
                notes
                    .entry(origin.clone())
                    .or_default()
                    .push_str(&format!("    // windows-clang: {}\n", row.message));
            }
        }
    }
    DefinitionObservations {
        rows,
        coverage: DefinitionEvidenceCoverage::CapturedBodiesAndReferentConst {
            selected_records: records.selected_definitions,
            alternative_definitions: records.alternative_definitions,
            declaration_only_observations: records.declaration_only_observations,
            interfaces,
            enums,
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
