#![allow(non_upper_case_globals)]
#![doc = include_str!("../readme.md")]

use std::borrow::Cow;
#[cfg(test)]
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::{Display, Formatter, Write};
use std::rc::Rc;
use std::sync::Arc;

fn timings_enabled() -> bool {
    ["WINDOWS_CLANG_TIMINGS", "WINDOWS_CLANG_TIMING"]
        .into_iter()
        .filter_map(std::env::var_os)
        .any(|value| value != "0")
}

fn declaration_uuid<'a>(
    snapshot: &'a Snapshot,
    declarations: &DeclarationIndex<'a>,
    name: &str,
    tu: &str,
    declaration: &Location,
) -> Option<&'a str> {
    let guids: BTreeSet<_> = declarations
        .get(tu, name, declaration)
        .iter()
        .filter_map(|fact| snapshot.fact_uuid(fact))
        .collect();
    (guids.len() == 1).then(|| *guids.first().unwrap())
}

fn write_rdl_strict<'a>(
    namespace: &str,
    items: impl IntoIterator<Item = &'a str>,
) -> Result<String, Error> {
    validate_namespace(namespace)?;
    write_rdl(namespace, items)
}

fn validate_namespace(namespace: &str) -> Result<(), Error> {
    if namespace.is_empty()
        || namespace.split('.').any(|part| {
            part.is_empty()
                || !part.chars().enumerate().all(|(index, value)| {
                    value == '_'
                        || value.is_ascii_alphanumeric() && (index > 0 || !value.is_ascii_digit())
                })
        })
    {
        Err(Error(format!("invalid namespace `{namespace}`")))
    } else {
        Ok(())
    }
}

fn timing_target(args: &[&str]) -> String {
    args.iter()
        .find_map(|arg| arg.strip_prefix("--target="))
        .unwrap_or("default")
        .to_string()
}

fn elapsed_ms(start: Option<std::time::Instant>) -> f64 {
    start
        .map(|start| start.elapsed().as_secs_f64() * 1_000.0)
        .unwrap_or_default()
}

struct TimingPhase {
    target: Option<String>,
    name: &'static str,
    start: Option<std::time::Instant>,
    visited: usize,
}

impl TimingPhase {
    fn new(target: Option<&str>, name: &'static str) -> Self {
        let phase = Self {
            target: target.map(str::to_string),
            name,
            start: target.map(|_| std::time::Instant::now()),
            visited: 0,
        };
        phase.event("enter", None);
        phase
    }

    fn event(&self, event: &str, total: Option<usize>) {
        if let Some(target) = &self.target {
            eprintln!(
                "windows-clang timing phase={} target={target} event={event} \
                 visited={} work_total={total:?} elapsed_ms={:.3}",
                self.name,
                self.visited,
                elapsed_ms(self.start),
            );
        }
    }

    fn visit(&mut self, total: Option<usize>) {
        if self.start.is_some() {
            self.visited += 1;
            if self.visited.is_power_of_two() {
                self.event("progress", total);
            }
        }
    }

    fn next(&mut self, name: &'static str) {
        self.event("complete", None);
        self.name = name;
        self.start = self.target.as_ref().map(|_| std::time::Instant::now());
        self.visited = 0;
        self.event("enter", None);
    }

    fn check<T>(&self, result: Result<T, Error>) -> Result<T, Error> {
        if let (Some(target), Err(error)) = (&self.target, &result) {
            eprintln!(
                "windows-clang timing phase={} target={target} event=fail \
                 visited={} elapsed_ms={:.3} error={:?}",
                self.name,
                self.visited,
                elapsed_ms(self.start),
                error.0,
            );
        }
        result
    }

    fn finish<T>(self, result: Result<T, Error>) -> Result<T, Error> {
        let result = self.check(result);
        if result.is_ok() {
            self.event("complete", None);
        }
        result
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;

    #[test]
    fn timing_phase_reports_failure_without_completion() {
        const CHILD: &str = "WINDOWS_CLANG_PHASE_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let result: Result<(), Error> =
                TimingPhase::new(Some("x86_64-pc-windows-msvc"), "test-failure")
                    .finish(Err(Error("controlled phase failure".to_string())));
            assert_eq!(result.unwrap_err().0, "controlled phase failure");
            return;
        }
        let test = concat!(
            module_path!(),
            "::timing_phase_reports_failure_without_completion"
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test.split_once("::").unwrap().1, "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        let events: Vec<_> = stderr
            .lines()
            .filter(|line| line.contains("phase=test-failure"))
            .collect();
        assert_eq!(events.len(), 2, "{stderr}");
        assert!(events[0].contains("event=enter"), "{stderr}");
        assert!(events[1].contains("event=fail"), "{stderr}");
        assert!(events[1].contains("controlled phase failure"), "{stderr}");
        assert!(!stderr.contains("event=complete"), "{stderr}");
    }
}

mod com_pointer;
mod definition_evidence;
pub use definition_evidence::*;
mod enum_diagnostics;
mod extract;
use enum_diagnostics::*;
pub use extract::{
    extract, extract_partitioned, extract_partitioned_with_options,
    extract_ranked_header_partitions, extract_ranked_header_partitions_with_options,
    extract_with_options,
};

mod builder;
pub use builder::{Clang, clang};

mod references;
pub use references::MetadataReferences;

#[derive(Clone, Debug)]
pub struct Input {
    pub name: String,
    pub source: String,
    pub roots: BTreeSet<String>,
    pub root_dirs: BTreeSet<String>,
    pub root_suffixes: BTreeSet<String>,
    pub excluded_roots: BTreeSet<String>,
}

/// Controls translation-unit extraction without changing root or emission policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtractionOptions {
    parallelism: usize,
    module_free_headers: bool,
}

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self {
            parallelism: 1,
            module_free_headers: false,
        }
    }
}

impl ExtractionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of original translation units extracted concurrently.
    ///
    /// Each worker parses, indexes, traverses, and disposes its native translation unit.
    /// Zero and one both select serial extraction.
    pub fn with_parallelism(mut self, parallelism: usize) -> Self {
        self.parallelism = parallelism;
        self
    }

    pub fn parallelism(&self) -> usize {
        self.parallelism
    }

    /// Admits a corpus of textual headers with no modules or imported declarations.
    ///
    /// The caller guarantees this domain for every original translation unit. Native global
    /// typedef enums with complete matching literal definitions may then share a public provider.
    pub fn with_module_free_headers(mut self) -> Self {
        self.module_free_headers = true;
        self
    }
}

#[derive(Clone, Debug)]
pub struct PartitionedInput {
    pub input: Input,
    pub identity: String,
    pub roots: BTreeMap<String, RootPartition>,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RootPartition {
    pub partition: String,
    pub namespace: String,
    pub remaps: BTreeMap<String, String>,
    pub exclusions: BTreeSet<String>,
    pub libraries: BTreeMap<String, String>,
    pub u32_types: BTreeSet<String>,
    pub flags: BTreeSet<String>,
    pub preserved_auto_function_pointer_levels: BTreeSet<String>,
    pub exclude_empty_records: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RootOwner {
    pub input: String,
    pub root: String,
    pub partition: String,
    pub namespace: String,
    pub remaps: BTreeMap<String, String>,
    pub exclusions: BTreeSet<String>,
    pub libraries: BTreeMap<String, String>,
    pub u32_types: BTreeSet<String>,
    pub flags: BTreeSet<String>,
    pub preserved_auto_function_pointer_levels: BTreeSet<String>,
    pub exclude_empty_records: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HeaderPartitionPolicy {
    headers: BTreeMap<String, HeaderPartitionEntry>,
    input_headers: BTreeMap<(String, String), HeaderPartitionEntry>,
    partition_authorities: BTreeMap<String, PartitionAuthority>,
    authority_partition: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PartitionAuthority {
    rank: usize,
    input: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HeaderAuthority {
    owner: RootOwner,
    alternatives: BTreeSet<RootOwner>,
    context_present: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HeaderPartitionEntry {
    header: String,
    partitions: BTreeSet<RootPartition>,
    overrides: BTreeMap<String, BTreeSet<RootPartition>>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PartitionItemKind {
    Type,
    Value,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PartitionConflictReason {
    AmbiguousRootCandidates,
    AmbiguousOwners,
    MissingOwner,
    AuthoritySelected,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PartitionConflict {
    pub name: String,
    pub kind: PartitionItemKind,
    pub reason: PartitionConflictReason,
    pub owners: Vec<RootOwner>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartitionAudit {
    conflicts: Vec<PartitionConflict>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RdlPartition {
    pub partition: String,
    pub namespace: String,
    pub header: String,
}

/// The native declaration removed from a partial metadata result.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OmittedDeclaration {
    pub origin: Origin,
    pub kind: Option<FactKind>,
    pub name: String,
    pub source: Location,
    pub namespace: String,
    /// All methods of an omitted interface, including methods with otherwise valid signatures.
    pub methods: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EnumOmissionCause {
    ConflictingDeclarations,
    ExcludedDependency,
}

/// A whole declaration omitted because its dependency closure reaches an unusable enum.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MetadataOmission {
    pub declaration: OmittedDeclaration,
    pub cause: EnumOmissionCause,
    pub reason: String,
    /// Native declarations from the omitted API through its aliases to the blocking enum.
    pub dependency_chain: Vec<OmittedDeclaration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeaderPartitionReport {
    pub partitions: BTreeMap<RdlPartition, String>,
    pub omissions: Vec<MetadataOmission>,
    pub definition_evidence: Vec<DefinitionEvidence>,
    pub definition_evidence_coverage: DefinitionEvidenceCoverage,
}

impl HeaderPartitionReport {
    pub fn is_partial(&self) -> bool {
        !self.omissions.is_empty()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NamespaceAuthorities {
    routes: Vec<NamespaceAuthority>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NamespaceAuthority {
    Exact { name: String, namespace: String },
    Wildcard { pattern: String, namespace: String },
}

impl NamespaceAuthorities {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_exact(mut self, name: impl Into<String>, namespace: impl Into<String>) -> Self {
        self.routes.push(NamespaceAuthority::Exact {
            name: name.into(),
            namespace: namespace.into(),
        });
        self
    }

    pub fn with_wildcard(
        mut self,
        pattern: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Self {
        self.routes.push(NamespaceAuthority::Wildcard {
            pattern: pattern.into(),
            namespace: namespace.into(),
        });
        self
    }

    fn resolve<'a>(
        &self,
        names: impl IntoIterator<Item = &'a str>,
    ) -> Result<BTreeMap<String, String>, Error> {
        let mut exact = BTreeMap::new();
        let mut wildcards = Vec::new();
        for route in &self.routes {
            let (pattern, namespace, wildcard) = match route {
                NamespaceAuthority::Exact { name, namespace } => {
                    (name.as_str(), namespace.as_str(), false)
                }
                NamespaceAuthority::Wildcard { pattern, namespace } => {
                    (pattern.as_str(), namespace.as_str(), true)
                }
            };
            if pattern.is_empty() {
                return Err(Error("namespace authority pattern is empty".to_string()));
            }
            validate_namespace(namespace)?;
            if wildcard {
                if !pattern.contains('*') {
                    return Err(Error(format!(
                        "namespace authority wildcard `{pattern}` has no `*`"
                    )));
                }
                if wildcards.iter().any(|(existing, _)| existing == pattern) {
                    return Err(Error(format!(
                        "duplicate namespace authority wildcard `{pattern}`"
                    )));
                }
                wildcards.push((pattern.to_string(), namespace.to_string()));
            } else if exact
                .insert(pattern.to_string(), namespace.to_string())
                .is_some()
            {
                return Err(Error(format!(
                    "duplicate namespace authority name `{pattern}`"
                )));
            }
        }
        let mut result = BTreeMap::new();
        for name in names {
            let namespace = if let Some(namespace) = exact.get(name) {
                Some(namespace.clone())
            } else {
                let matches: BTreeSet<_> = wildcards
                    .iter()
                    .filter(|(pattern, _)| wildcard_matches(pattern, name))
                    .map(|(_, namespace)| namespace.clone())
                    .collect();
                match matches.len() {
                    0 => None,
                    1 => Some(matches.first().unwrap().clone()),
                    _ => {
                        return Err(Error(format!(
                            "namespace authority wildcards conflict for `{name}`"
                        )));
                    }
                }
            };
            if let Some(namespace) = namespace {
                result.insert(name.to_string(), namespace);
            }
        }
        Ok(result)
    }
}

impl HeaderPartitionPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a logical partition's rank and original extraction input.
    ///
    /// Lower ranks take precedence. Register the complete policy before selecting inputs to parse.
    pub fn with_partition_authority(
        mut self,
        partition: impl Into<String>,
        rank: usize,
        input: impl Into<String>,
    ) -> Self {
        self.set_partition_authority(partition, rank, input);
        self
    }

    pub fn set_partition_authority(
        &mut self,
        partition: impl Into<String>,
        rank: usize,
        input: impl Into<String>,
    ) {
        self.partition_authorities.insert(
            partition.into(),
            PartitionAuthority {
                rank,
                input: normalize_name(&input.into()),
            },
        );
    }

    fn validate_partition_authorities(&self) -> Result<(), Error> {
        if self.partition_authorities.is_empty() {
            return Ok(());
        }
        let mut ranks = BTreeMap::new();
        for (partition, authority) in &self.partition_authorities {
            if partition.trim().is_empty() || authority.input.trim().is_empty() {
                return Err(Error("header partition authority identity is empty".into()));
            }
            if let Some(other) = ranks.insert(authority.rank, partition) {
                return Err(Error(format!(
                    "header partition authority rank {} is shared by {other:?} and {partition:?}",
                    authority.rank
                )));
            }
        }
        for partition in self
            .headers
            .values()
            .chain(self.input_headers.values())
            .flat_map(|entry| &entry.partitions)
        {
            if !self
                .partition_authorities
                .contains_key(&partition.partition)
            {
                return Err(Error(format!(
                    "header partition {:?} has no registered authority rank and original input",
                    partition.partition
                )));
            }
        }
        Ok(())
    }

    fn resolve_owner(
        &self,
        name: &str,
        kind: Option<PartitionItemKind>,
        owners: BTreeSet<RootOwner>,
        namespace: Option<&str>,
    ) -> Result<(RootOwner, Option<PartitionConflict>), Error> {
        if self.partition_authorities.is_empty() {
            return resolve_header_owner(name, kind, owners, namespace);
        }
        let selected = owners
            .iter()
            .min_by_key(|owner| self.partition_authorities[&owner.partition].rank)
            .ok_or_else(|| Error(format!("traversed item `{name}` has no logical owner")))?;
        if owners.iter().any(|owner| {
            owner.partition == selected.partition && !same_owner_policy(owner, selected)
        }) {
            return Err(Error(format!(
                "authoritative partition {:?} has conflicting policy for `{name}`",
                selected.partition
            )));
        }
        let (owner, _) =
            resolve_header_owner(name, kind, BTreeSet::from([selected.clone()]), namespace)?;
        let conflict = (owners.len() > 1).then(|| {
            kind.map(|kind| PartitionConflict {
                name: name.to_string(),
                kind,
                reason: PartitionConflictReason::AuthoritySelected,
                owners: owners.into_iter().collect(),
            })
        });
        Ok((owner, conflict.flatten()))
    }

    pub fn with_traversed_header(
        mut self,
        header: impl Into<String>,
        partition: RootPartition,
    ) -> Self {
        self.add_traversed_header(header, partition);
        self
    }

    pub fn add_traversed_header(&mut self, header: impl Into<String>, partition: RootPartition) {
        let header = normalize_name(&header.into());
        let key = header.to_ascii_lowercase();
        let entry = self
            .headers
            .entry(key)
            .or_insert_with(|| HeaderPartitionEntry {
                header: header.clone(),
                partitions: BTreeSet::new(),
                overrides: BTreeMap::new(),
            });
        if header < entry.header {
            entry.header = header;
        }
        entry.partitions.insert(partition);
    }

    pub fn with_traversed_header_override(
        mut self,
        header: impl Into<String>,
        name: impl Into<String>,
        partition: RootPartition,
    ) -> Self {
        self.add_traversed_header_override(header, name, partition);
        self
    }

    pub fn add_traversed_header_override(
        &mut self,
        header: impl Into<String>,
        name: impl Into<String>,
        partition: RootPartition,
    ) {
        let header = normalize_name(&header.into());
        let key = header.to_ascii_lowercase();
        let entry = self
            .headers
            .entry(key)
            .or_insert_with(|| HeaderPartitionEntry {
                header: header.clone(),
                partitions: BTreeSet::new(),
                overrides: BTreeMap::new(),
            });
        if header < entry.header {
            entry.header = header;
        }
        entry
            .overrides
            .entry(name.into())
            .or_default()
            .insert(partition);
    }

    pub fn with_traversed_header_for_input(
        mut self,
        input: impl Into<String>,
        header: impl Into<String>,
        partition: RootPartition,
    ) -> Self {
        self.add_traversed_header_for_input(input, header, partition);
        self
    }

    pub fn add_traversed_header_for_input(
        &mut self,
        input: impl Into<String>,
        header: impl Into<String>,
        partition: RootPartition,
    ) {
        let input = normalize_name(&input.into());
        let header = normalize_name(&header.into());
        let key = (input.to_ascii_lowercase(), header.to_ascii_lowercase());
        let entry = self
            .input_headers
            .entry(key)
            .or_insert_with(|| HeaderPartitionEntry {
                header: header.clone(),
                partitions: BTreeSet::new(),
                overrides: BTreeMap::new(),
            });
        if header < entry.header {
            entry.header = header;
        }
        entry.partitions.insert(partition);
    }

    pub fn with_traversed_header_override_for_input(
        mut self,
        input: impl Into<String>,
        header: impl Into<String>,
        name: impl Into<String>,
        partition: RootPartition,
    ) -> Self {
        self.add_traversed_header_override_for_input(input, header, name, partition);
        self
    }

    pub fn add_traversed_header_override_for_input(
        &mut self,
        input: impl Into<String>,
        header: impl Into<String>,
        name: impl Into<String>,
        partition: RootPartition,
    ) {
        let input = normalize_name(&input.into());
        let header = normalize_name(&header.into());
        let key = (input.to_ascii_lowercase(), header.to_ascii_lowercase());
        let entry = self
            .input_headers
            .entry(key)
            .or_insert_with(|| HeaderPartitionEntry {
                header: header.clone(),
                partitions: BTreeSet::new(),
                overrides: BTreeMap::new(),
            });
        if header < entry.header {
            entry.header = header;
        }
        entry
            .overrides
            .entry(name.into())
            .or_default()
            .insert(partition);
    }

    pub fn traversed_headers(&self) -> impl Iterator<Item = (&str, &BTreeSet<RootPartition>)> {
        self.headers
            .values()
            .map(|entry| (entry.header.as_str(), &entry.partitions))
    }

    pub fn traversed_header_paths(&self) -> impl Iterator<Item = &str> {
        self.headers
            .values()
            .chain(self.input_headers.values())
            .map(|entry| entry.header.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.input_headers.is_empty()
    }

    pub fn with_authority_partition(mut self, partition: impl Into<String>) -> Self {
        self.authority_partition = Some(partition.into());
        self
    }

    pub fn set_authority_partition(&mut self, partition: impl Into<String>) {
        self.authority_partition = Some(partition.into());
    }

    #[cfg(test)]
    fn owners_for_name(&self, tu: &str, file: &str, name: Option<&str>) -> BTreeSet<RootOwner> {
        let ranked = !self.partition_authorities.is_empty();
        let normalized_tu = normalize_name(tu).to_ascii_lowercase();
        let file = normalize_name(file);
        let entries = self
            .headers
            .values()
            .filter(|entry| source_path_matches(&entry.header, &file))
            .chain(
                self.input_headers
                    .iter()
                    .filter(|((input, _), entry)| {
                        (ranked || input == &normalized_tu)
                            && source_path_matches(&entry.header, &file)
                    })
                    .map(|(_, entry)| entry),
            );
        let mut defaults = BTreeSet::new();
        let mut overrides = BTreeSet::new();
        for entry in entries {
            defaults.extend(entry.partitions.iter());
            if let Some(partitions) = name.and_then(|name| entry.overrides.get(name)) {
                overrides.extend(partitions);
            }
        }
        let partitions = if overrides.is_empty() {
            defaults
        } else {
            overrides
        };
        partitions
            .into_iter()
            .map(|partition| {
                let input = self
                    .partition_authorities
                    .get(&partition.partition)
                    .map_or(tu, |authority| authority.input.as_str());
                root_owner(input, &file, partition)
            })
            .collect()
    }
}

struct HeaderPartitionLookup<'a> {
    policy: &'a HeaderPartitionPolicy,
    sources: HashMap<String, HashMap<String, HeaderPartitionCandidates<'a>>>,
}

struct HeaderPartitionCandidates<'a> {
    file: String,
    defaults: BTreeSet<&'a RootPartition>,
    overrides: BTreeMap<&'a str, BTreeSet<&'a RootPartition>>,
}

impl<'a> HeaderPartitionLookup<'a> {
    fn new(policy: &'a HeaderPartitionPolicy) -> Self {
        Self {
            policy,
            sources: HashMap::new(),
        }
    }

    fn owners(&mut self, tu: &str, file: &str) -> BTreeSet<RootOwner> {
        self.owners_for_name(tu, file, None)
    }

    fn named_owners(&mut self, tu: &str, file: &str, name: &str) -> BTreeSet<RootOwner> {
        self.owners_for_name(tu, file, Some(name))
    }

    fn owners_for_name(&mut self, tu: &str, file: &str, name: Option<&str>) -> BTreeSet<RootOwner> {
        // Raw keys preserve owner provenance; each lookup borrows one immutable policy.
        let ranked = !self.policy.partition_authorities.is_empty();
        let cache_tu = if ranked { "" } else { tu };
        if self
            .sources
            .get(cache_tu)
            .is_none_or(|files| !files.contains_key(file))
        {
            let normalized_tu = normalize_name(tu).to_ascii_lowercase();
            let normalized_file = normalize_name(file);
            let entries = self
                .policy
                .headers
                .values()
                .filter(|entry| source_path_matches(&entry.header, &normalized_file))
                .chain(
                    self.policy
                        .input_headers
                        .iter()
                        .filter(|((input, _), entry)| {
                            (ranked || input == &normalized_tu)
                                && source_path_matches(&entry.header, &normalized_file)
                        })
                        .map(|(_, entry)| entry),
                );
            let mut defaults = BTreeSet::new();
            let mut overrides: BTreeMap<&str, BTreeSet<&RootPartition>> = BTreeMap::new();
            for entry in entries {
                defaults.extend(entry.partitions.iter());
                for (name, partitions) in &entry.overrides {
                    overrides.entry(name).or_default().extend(partitions.iter());
                }
            }
            self.sources
                .entry(cache_tu.to_string())
                .or_default()
                .insert(
                    file.to_string(),
                    HeaderPartitionCandidates {
                        file: normalized_file,
                        defaults,
                        overrides,
                    },
                );
        }
        let candidates = &self.sources[cache_tu][file];
        let partitions = name
            .and_then(|name| candidates.overrides.get(name))
            .filter(|partitions| !partitions.is_empty())
            .unwrap_or(&candidates.defaults);
        partitions
            .iter()
            .map(|partition| {
                let input = self
                    .policy
                    .partition_authorities
                    .get(&partition.partition)
                    .map_or(tu, |authority| authority.input.as_str());
                root_owner(input, &candidates.file, partition)
            })
            .collect()
    }

    fn refine_owner(
        &mut self,
        authority: &RootOwner,
        name: &str,
        namespace: Option<&str>,
    ) -> Result<RootOwner, Error> {
        self.owners(&authority.input, &authority.root);
        let candidates = &self.sources[""][&authority.root];
        let overrides = candidates.overrides.get(name).cloned().unwrap_or_default();
        let matching: BTreeSet<_> = overrides
            .iter()
            .copied()
            .filter(|partition| {
                namespace.map_or(partition.partition == authority.partition, |namespace| {
                    partition.namespace == namespace
                })
            })
            .collect();
        let choices = if matching.is_empty() {
            &overrides
        } else {
            &matching
        };
        if choices.len() > 1 {
            return Err(Error(format!(
                "traversed item `{name}` has ambiguous output policy overrides"
            )));
        }
        let mut owner = choices.first().map_or_else(
            || authority.clone(),
            |partition| root_owner(&authority.input, &authority.root, partition),
        );
        owner
            .exclusions
            .extend(authority.exclusions.iter().cloned());
        owner.exclude_empty_records |= authority.exclude_empty_records;
        if let Some(namespace) = namespace {
            owner.namespace = namespace.to_string();
        }
        validate_namespace(&owner.namespace)?;
        Ok(owner)
    }
}

#[cfg(test)]
mod header_ownership_tests {
    use super::*;

    fn policy() -> HeaderPartitionPolicy {
        HeaderPartitionPolicy::new()
            .with_traversed_header("common.h", RootPartition::new("common", "Example.Common"))
            .with_traversed_header(
                r"C:\sdk\one\common.h",
                RootPartition::new("one", "Example.One").with_exclusion("EXCLUDED"),
            )
            .with_traversed_header(
                "sdk/two/common.h",
                RootPartition::new("two", "Example.Two").with_remap("SPECIAL", "RENAMED"),
            )
            .with_traversed_header_for_input(
                r"BUILD\FIRST.cpp",
                "common.h",
                RootPartition::new("input", "Example.Input"),
            )
            .with_traversed_header_override(
                "common.h",
                "SPECIAL",
                RootPartition::new("special", "Example.Special"),
            )
            .with_traversed_header_override_for_input(
                "build/first.cpp",
                "common.h",
                "SPECIAL",
                RootPartition::new("input-special", "Example.InputSpecial"),
            )
            .with_traversed_header_override(
                "override.h",
                "ONLY",
                RootPartition::new("only", "Example.Only"),
            )
    }

    #[test]
    fn memoized_owner_sets_match_reference_policy() {
        let policy = policy();
        let mut lookup = HeaderPartitionLookup::new(&policy);
        for _ in 0..2 {
            for tu in [
                "build/first.cpp",
                r"BUILD\First.cpp",
                "build/second.cpp",
                "",
            ] {
                for file in [
                    r"C:\sdk\one\common.h",
                    "c:/SDK/ONE/COMMON.H",
                    "C:/sdk/two/common.h",
                    "common.h",
                    "C:/sdk/unclaimed/common.h",
                    "C:/sdk/one/notcommon.h",
                    "C:/sdk/override.h",
                    "unknown.h",
                    "",
                    "/",
                ] {
                    for name in [
                        None,
                        Some("SPECIAL"),
                        Some("special"),
                        Some("ONLY"),
                        Some("EXCLUDED"),
                    ] {
                        let expected = policy.owners_for_name(tu, file, name);
                        let actual = lookup.owners_for_name(tu, file, name);
                        assert_eq!(actual, expected, "{tu:?} {file:?} {name:?}");
                        assert_eq!(
                            actual.iter().collect::<Vec<_>>(),
                            expected.iter().collect::<Vec<_>>()
                        );
                        if !actual.is_empty() {
                            for namespace in [None, Some("Example.One")] {
                                assert_eq!(
                                    resolve_header_owner(
                                        "TYPE",
                                        Some(PartitionItemKind::Type),
                                        actual.clone(),
                                        namespace
                                    )
                                    .unwrap(),
                                    resolve_header_owner(
                                        "TYPE",
                                        Some(PartitionItemKind::Type),
                                        expected.clone(),
                                        namespace
                                    )
                                    .unwrap()
                                );
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(lookup.sources.len(), 4);
        assert_eq!(lookup.sources.values().map(HashMap::len).sum::<usize>(), 40);
        let other_policy = HeaderPartitionPolicy::new()
            .with_traversed_header("common.h", RootPartition::new("other", "Example.Other"));
        let mut other_lookup = HeaderPartitionLookup::new(&other_policy);
        assert_ne!(
            lookup.owners("build/first.cpp", "common.h"),
            other_lookup.owners("build/first.cpp", "common.h")
        );
        assert_eq!(
            other_lookup.owners("build/first.cpp", "common.h"),
            other_policy.owners_for_name("build/first.cpp", "common.h", None)
        );
    }

    #[test]
    fn memoized_fact_owners_preserve_expansion_fallback() {
        let policy = policy();
        let mut lookup = HeaderPartitionLookup::new(&policy);
        for name in ["TYPE", "SPECIAL", "ONLY"] {
            for (expansion, spelling) in [
                ("C:/sdk/one/common.h", "C:/sdk/two/common.h"),
                ("unknown.h", "C:/sdk/one/common.h"),
                ("C:/sdk/override.h", "C:/sdk/one/common.h"),
                ("unknown.h", "also-unknown.h"),
                ("unknown.h", "unknown.h"),
            ] {
                let fact = Fact {
                    origin: Origin {
                        tu: "build/first.cpp".to_string(),
                        local: 0,
                    },
                    parent: None,
                    kind: FactKind::Typedef,
                    name: name.to_string(),
                    spelling: Location {
                        file: spelling.to_string(),
                        offset: 1,
                    },
                    expansion: Location {
                        file: expansion.to_string(),
                        offset: 2,
                    },
                    definition: true,
                    main_file: false,
                    root: true,
                    system: false,
                    data: FactData::Typedef {
                        target: TypeRef::Scalar(Scalar::I32),
                    },
                };
                let mut expected = policy.owners_for_name(&fact.origin.tu, expansion, Some(name));
                if expected.is_empty() && expansion != spelling {
                    expected = policy.owners_for_name(&fact.origin.tu, spelling, Some(name));
                }
                assert_eq!(header_fact_owner_candidates(&mut lookup, &fact), expected);
            }
        }
    }

    #[test]
    fn memoized_owner_queries_remove_repeated_policy_scans() {
        let mut policy = policy();
        for index in 0..128 {
            policy.add_traversed_header(
                format!("unrelated{index}.h"),
                RootPartition::new(format!("partition{index}"), "Example.Unrelated"),
            );
        }
        let queries = [
            ("build/first.cpp", "C:/sdk/one/common.h", Some("SPECIAL")),
            ("build/second.cpp", "C:/sdk/two/common.h", None),
            ("build/first.cpp", "unknown.h", Some("TYPE")),
        ];
        let start = std::time::Instant::now();
        let reference: Vec<_> = (0..1024)
            .flat_map(|_| queries)
            .map(|(tu, file, name)| std::hint::black_box(policy.owners_for_name(tu, file, name)))
            .collect();
        let reference_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = std::time::Instant::now();
        let mut lookup = HeaderPartitionLookup::new(&policy);
        let memoized: Vec<_> = (0..1024)
            .flat_map(|_| queries)
            .map(|(tu, file, name)| std::hint::black_box(lookup.owners_for_name(tu, file, name)))
            .collect();
        let memoized_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(memoized, reference);
        let sources = lookup.sources.values().map(HashMap::len).sum::<usize>();
        assert_eq!(sources, queries.len());
        let entries = policy.headers.len() + policy.input_headers.len();
        eprintln!(
            "synthetic ownership queries={} policy_entries={entries} reference_entry_visits={} \
             memoized_entry_visits={} source_contexts={sources} reference_ms={reference_ms:.3} \
             memoized_ms={memoized_ms:.3}",
            reference.len(),
            reference.len() * entries,
            sources * entries,
        );
    }

    fn authority_header() -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("tests")
            .join("producer_corrections")
            .join("header-authority.hpp")
            .to_string_lossy()
            .into_owned()
    }

    fn authority_snapshot(inputs: &[&str], required: bool) -> Snapshot {
        let header = authority_header();
        let inputs = inputs.iter().map(|input| {
            let (scalar, value) = if *input == "first.cpp" {
                ("int", 7)
            } else {
                ("unsigned int", 13)
            };
            let root = if required && *input == "second.cpp" {
                "struct REQUESTED_ROOT { AUTHORITY_RECORD payload; };"
            } else {
                ""
            };
            Input::new(
                *input,
                format!(
                    "#define AUTHORITY_SCALAR {scalar}\n#define AUTHORITY_VALUE {value}\n\
                     #include \"{header}\"\n{root}\n"
                ),
            )
            .with_roots([header.clone()])
        });
        extract(inputs, &["-x", "c++", "--target=x86_64-pc-windows-msvc"]).unwrap()
    }

    fn authority_policy(
        first_rank: usize,
        second_rank: usize,
        exclusions: &[&str],
    ) -> HeaderPartitionPolicy {
        let header = authority_header();
        let mut first = RootPartition::new("first", "Example.First");
        first
            .exclusions
            .extend(exclusions.iter().map(|name| name.to_string()));
        HeaderPartitionPolicy::new()
            .with_partition_authority("second", second_rank, "second.cpp")
            .with_partition_authority("first", first_rank, "first.cpp")
            .with_traversed_header_for_input(
                "second.cpp",
                &header,
                RootPartition::new("second", "Example.Second"),
            )
            .with_traversed_header_for_input("first.cpp", &header, first)
    }

    fn assert_authority_roots(plan: &HeaderPartitionPlan, input: &str, scalar: Scalar, value: i64) {
        let aliases: Vec<_> = plan
            .snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "AUTHORITY_ALIAS")
            .collect();
        assert_eq!(aliases.len(), 2);
        let roots: Vec<_> = aliases.iter().filter(|fact| fact.root).collect();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].origin.tu, input);
        assert_eq!(
            roots[0].data,
            FactData::Typedef {
                target: TypeRef::Scalar(scalar)
            }
        );
        let records: Vec<_> = plan
            .snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "AUTHORITY_RECORD" && fact.root)
            .collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].origin.tu, input);
        let FactData::Record { fields, .. } = &records[0].data else {
            panic!();
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].ty, TypeRef::Scalar(scalar));
        assert!(
            !plan.snapshot.root_owners.contains_key(
                &aliases
                    .iter()
                    .find(|fact| fact.origin.tu != input)
                    .unwrap()
                    .origin
            )
        );
        let constants: Vec<_> = plan
            .snapshot
            .constants
            .iter()
            .filter(|constant| constant.name == "AUTHORITY_CONSTANT")
            .collect();
        assert_eq!(constants.len(), 2);
        let chosen: Vec<_> = constants
            .iter()
            .filter(|constant| {
                plan.snapshot
                    .constant_root_owners
                    .contains_key(&(constant.root.clone(), constant.name.clone()))
            })
            .collect();
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].root.tu, input);
        assert_eq!(chosen[0].value, Value::Signed(value));
    }

    #[test]
    fn authority_rank_precedes_execution_order_and_preserves_alternatives() {
        let policy = authority_policy(1, 8, &[]);
        let mut lookup = HeaderPartitionLookup::new(&policy);
        for input in ["first.cpp", "second.cpp", "unparsed.cpp"] {
            assert_eq!(
                lookup.owners(input, &authority_header()),
                policy.owners_for_name(input, &authority_header(), None)
            );
        }
        assert_eq!(lookup.sources.len(), 1);
        let references = BTreeMap::new();
        let options = EmitOptions::new("Example.Common", &references);
        let mut outputs = Vec::new();
        for inputs in [["first.cpp", "second.cpp"], ["second.cpp", "first.cpp"]] {
            let plan = authority_snapshot(&inputs, false)
                .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                .unwrap();
            assert_authority_roots(&plan, "first.cpp", Scalar::I32, 7);
            let audit = plan.audit(&options).unwrap();
            assert!(!audit.is_clean());
            assert!(!audit.has_blocking_conflicts(), "{audit}");
            assert!(
                audit.conflicts().iter().all(|conflict| {
                    conflict.reason == PartitionConflictReason::AuthoritySelected
                })
            );
            let output = plan.emit_with_options(&options).unwrap();
            assert!(output.keys().all(|key| key.namespace == "Example.First"));
            outputs.push(output);
        }
        assert_eq!(outputs[0], outputs[1]);

        let plan = authority_snapshot(&["first.cpp", "second.cpp"], false)
            .into_header_partition_plan(&authority_policy(8, 1, &[]), &NamespaceAuthorities::new())
            .unwrap();
        assert_authority_roots(&plan, "second.cpp", Scalar::U32, 13);
        let output = plan.emit_with_options(&options).unwrap();
        assert!(output.keys().all(|key| key.namespace == "Example.Second"));
    }

    #[test]
    fn authority_output_override_cannot_change_definition_home() {
        let policy = authority_policy(1, 8, &[])
            .with_partition_authority("elsewhere", 0, "unparsed.cpp")
            .with_traversed_header_override(
                authority_header(),
                "AUTHORITY_ALIAS",
                RootPartition::new("elsewhere", "Example.Override"),
            );
        let plan = authority_snapshot(&["second.cpp", "first.cpp"], false)
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap();
        assert_authority_roots(&plan, "first.cpp", Scalar::I32, 7);
        let alias = plan
            .snapshot
            .facts
            .iter()
            .find(|fact| fact.root && fact.name == "AUTHORITY_ALIAS")
            .unwrap();
        let owner = &plan.snapshot.root_owners[&alias.origin];
        assert_eq!(owner.input, "first.cpp");
        assert_eq!(owner.namespace, "Example.Override");
        let references = BTreeMap::new();
        let output = plan
            .emit_with_options(&EmitOptions::new("Example.Common", &references))
            .unwrap();
        assert!(output.iter().any(|(key, text)| {
            key.namespace == "Example.Override" && text.contains("AUTHORITY_ALIAS")
        }));
    }

    #[test]
    fn authority_exclusion_omits_without_competing_context_fallback() {
        let header = authority_header();
        let policy = HeaderPartitionPolicy::new()
            .with_partition_authority("first", 1, "first.cpp")
            .with_partition_authority("second", 8, "second.cpp")
            .with_traversed_header_for_input(
                "first.cpp",
                &header,
                RootPartition::new("first", "Example.First")
                    .with_exclusion("AUTHORITY_ALIAS")
                    .with_exclusion("AUTHORITY_RECORD")
                    .with_exclusion("AUTHORITY_CONSTANT"),
            )
            .with_traversed_header_for_input(
                "second.cpp",
                &header,
                RootPartition::new("second", "Example.Second"),
            );
        let plan = authority_snapshot(&["second.cpp", "first.cpp"], false)
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap();
        assert!(plan.snapshot.facts.iter().all(|fact| !fact.root));
        assert!(plan.snapshot.constant_root_owners.is_empty());
        let references = BTreeMap::new();
        assert!(
            plan.emit_with_options(&EmitOptions::new("Example.Common", &references))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn authority_absence_is_promoted_only_by_reachable_output() {
        let references = BTreeMap::new();
        let options = EmitOptions::new("Example.Common", &references);
        for (required, excluded) in [(false, false), (true, false), (false, true), (true, true)] {
            let exclusions: &[&str] = if excluded { &["AUTHORITY_RECORD"] } else { &[] };
            let policy = authority_policy(1, 8, exclusions).with_traversed_header_for_input(
                "second.cpp",
                "second.cpp",
                RootPartition::new("second", "Example.Second")
                    .with_exclusion("AUTHORITY_SCALAR")
                    .with_exclusion("AUTHORITY_VALUE"),
            );
            let plan = authority_snapshot(&["second.cpp"], required)
                .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                .unwrap();
            let authority = &plan.snapshot.header_authorities
                [&normalize_name(&authority_header()).to_ascii_lowercase()];
            assert_eq!(authority.owner.input, "first.cpp");
            assert!(!authority.context_present);
            assert!(
                plan.snapshot
                    .facts
                    .iter()
                    .filter(|fact| fact.name == "AUTHORITY_RECORD")
                    .all(|fact| !fact.root)
            );
            let result = plan.emit_with_options(&options);
            if required {
                let error = result.unwrap_err().to_string();
                for evidence in [
                    "AUTHORITY_RECORD",
                    "first",
                    "first.cpp",
                    "header-authority.hpp",
                    "REQUESTED_ROOT",
                ] {
                    assert!(error.contains(evidence), "{evidence:?}: {error}");
                }
                if excluded {
                    assert!(
                        error.contains("excluded by authoritative partition"),
                        "{error}"
                    );
                    assert!(!error.contains("RequiredAuthorityContextAbsent"), "{error}");
                } else {
                    assert!(error.contains("RequiredAuthorityContextAbsent"), "{error}");
                }
            } else {
                assert!(result.unwrap().is_empty());
            }
        }
    }

    #[test]
    fn authority_registry_rejects_missing_identity_and_ambiguous_rank() {
        let policy = authority_policy(1, 1, &[]);
        assert!(
            policy
                .validate_partition_authorities()
                .unwrap_err()
                .to_string()
                .contains("authority rank 1 is shared")
        );
        let mut policy = authority_policy(1, 8, &[]);
        policy.set_partition_authority("first", 1, "");
        assert_eq!(
            policy
                .validate_partition_authorities()
                .unwrap_err()
                .to_string(),
            "header partition authority identity is empty"
        );
        let policy = authority_policy(1, 8, &[]).with_traversed_header(
            "other.hpp",
            RootPartition::new("unregistered", "Example.Other"),
        );
        assert!(
            policy
                .validate_partition_authorities()
                .unwrap_err()
                .to_string()
                .contains("unregistered")
        );
    }

    #[test]
    fn authority_home_requires_exact_original_input_and_header_observation() {
        let header = authority_header();
        let observed = format!(
            "#define AUTHORITY_SCALAR int\n#define AUTHORITY_VALUE 7\n#include \"{header}\"\n"
        );
        let requested =
            format!("{observed}struct REQUESTED_ROOT {{ AUTHORITY_RECORD payload; }};\n");
        let snapshot = extract(
            [
                Input::new("first.cpp", ""),
                Input::new("FIRST.cpp", &observed).with_roots([&header]),
                Input::new("second.cpp", requested).with_roots([&header]),
            ],
            &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
        )
        .unwrap();
        let policy = authority_policy(1, 8, &[]).with_traversed_header_for_input(
            "second.cpp",
            "second.cpp",
            RootPartition::new("second", "Example.Second")
                .with_exclusion("AUTHORITY_SCALAR")
                .with_exclusion("AUTHORITY_VALUE"),
        );
        let plan = snapshot
            .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
            .unwrap();
        let authority =
            &plan.snapshot.header_authorities[&normalize_name(&header).to_ascii_lowercase()];
        assert!(plan.snapshot.input_order.contains_key("first.cpp"));
        assert!(!authority.context_present);
        let records: Vec<_> = plan
            .snapshot
            .facts
            .iter()
            .filter(|fact| fact.name == "AUTHORITY_RECORD" && fact.origin.tu == "FIRST.cpp")
            .collect();
        assert_eq!(records.len(), 1);
        assert!(!records[0].root);
        let references = BTreeMap::new();
        let error = plan
            .emit_with_options(&EmitOptions::new("Example.Common", &references))
            .unwrap_err()
            .to_string();
        assert!(error.contains("RequiredAuthorityContextAbsent"), "{error}");
        assert!(error.contains("original input \"first.cpp\""), "{error}");
    }

    #[test]
    fn authority_constant_filter_preserves_unranked_unowned_observations() {
        let snapshot = authority_snapshot(&["first.cpp"], false);
        let header = authority_header();
        let unranked = HeaderPartitionPolicy::new()
            .with_traversed_header(&header, RootPartition::new("first", "Example.First"));
        let ranked = unranked
            .clone()
            .with_partition_authority("first", 0, "first.cpp");
        let unobserved =
            HeaderPartitionPolicy::new().with_partition_authority("first", 0, "first.cpp");
        let references = BTreeMap::new();
        let options = EmitOptions::new("Example.Common", &references);
        for (policy, registered, observed) in [
            (&unranked, false, true),
            (&ranked, true, true),
            (&unobserved, true, false),
        ] {
            let mut plan = snapshot
                .plan_header_partitions(policy, &NamespaceAuthorities::new())
                .unwrap();
            assert!(plan.snapshot.header_partition_policy);
            assert_eq!(plan.snapshot.ranked_header_partition_policy, registered);
            assert_eq!(
                !plan.snapshot.header_authorities.is_empty(),
                registered && observed
            );
            plan.snapshot.constant_root_owners.clear();
            let constant = plan
                .snapshot
                .constants
                .iter()
                .find(|constant| constant.name == "AUTHORITY_CONSTANT")
                .unwrap();
            assert!(!plan.snapshot.root_owners.contains_key(&constant.root));
            assert_eq!(constant.value, Value::Signed(7));
            let output = plan.emit_with_options(&options).unwrap();
            let text = output.values().cloned().collect::<String>();
            assert_eq!(text.contains("AUTHORITY_CONSTANT"), !registered, "{text}");
            if !registered {
                assert!(text.contains("= 7"), "{text}");
            }
            if !observed {
                assert!(output.is_empty());
            }
        }
    }

    #[test]
    fn authority_macro_demand_uses_exact_expansion_provenance() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("tests")
            .join("producer_corrections");
        let definitions = fixtures.join("header-authority-macros.hpp");
        let expansion = fixtures.join("header-authority-expansion.hpp");
        let definitions = definitions.to_string_lossy().into_owned();
        let expansion = expansion.to_string_lossy().into_owned();
        let definition_partition = RootPartition::new("definitions", "Example.Definitions");
        let expansion_partition = RootPartition::new("expansion", "Example.Expansion");
        let request_partition = RootPartition::new("request", "Example.Request");
        let policy = HeaderPartitionPolicy::new()
            .with_partition_authority("definitions", 0, "definitions.cpp")
            .with_partition_authority("expansion", 1, "expansion.cpp")
            .with_partition_authority("request", 2, "request.cpp")
            .with_traversed_header_for_input("definitions.cpp", &definitions, definition_partition)
            .with_traversed_header_for_input(
                "expansion.cpp",
                &expansion,
                expansion_partition.clone(),
            )
            .with_traversed_header_for_input("request.cpp", &expansion, request_partition.clone())
            .with_traversed_header_for_input("expansion.cpp", "expansion.cpp", expansion_partition)
            .with_traversed_header_for_input("request.cpp", "request.cpp", request_partition);
        let references = BTreeMap::new();
        let options = EmitOptions::new("Example.Common", &references);
        for (input, required) in [
            ("expansion.cpp", true),
            ("request.cpp", false),
            ("request.cpp", true),
        ] {
            let root = if required {
                "struct MACRO_REQUESTED_ROOT { MACRO_AUTHORITY_RECORD record; \
                 MACRO_AUTHORITY_ALIAS* alias; };"
            } else {
                ""
            };
            let snapshot = extract(
                [
                    Input::new(input, format!("#include \"{expansion}\"\n{root}\n"))
                        .with_roots([&definitions, &expansion]),
                ],
                &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
            )
            .unwrap();
            let plan = snapshot
                .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
                .unwrap();
            let definition_authority = &plan.snapshot.header_authorities
                [&normalize_name(&definitions).to_ascii_lowercase()];
            assert!(!definition_authority.context_present);
            let expansion_authority =
                &plan.snapshot.header_authorities[&normalize_name(&expansion).to_ascii_lowercase()];
            assert_eq!(
                expansion_authority.context_present,
                input == "expansion.cpp"
            );
            for name in ["MACRO_AUTHORITY_RECORD", "MACRO_AUTHORITY_ALIAS"] {
                let facts: Vec<_> = plan
                    .snapshot
                    .facts
                    .iter()
                    .filter(|fact| fact.name == name)
                    .collect();
                assert_eq!(facts.len(), 1, "{facts:#?}");
                let fact = facts[0];
                assert_eq!(fact.origin.tu, input);
                assert_eq!(fact.spelling.file, normalize_name(&definitions));
                assert_eq!(fact.expansion.file, normalize_name(&expansion));
                assert_eq!(fact.root, input == "expansion.cpp");
                let authority =
                    fact_header_authority(&plan.snapshot.header_authorities, fact).unwrap();
                assert_eq!(authority.owner.partition, "expansion");
                assert_eq!(authority.owner.input, "expansion.cpp");
                let declarations = DeclarationIndex::new(&plan.snapshot.facts);
                let bound = declarations.get(input, name, &fact.spelling);
                assert_eq!(bound.len(), 1);
                assert_eq!(bound[0].origin, fact.origin);
                if let FactData::Record { fields, .. } = &fact.data {
                    assert_eq!(fields.len(), 1);
                    assert_eq!(fields[0].name, "value");
                    assert_eq!(fields[0].ty, TypeRef::Scalar(Scalar::I32));
                }
            }
            let result = plan.emit_with_options(&options);
            if input == "expansion.cpp" {
                let output = result.unwrap();
                let text = output.values().cloned().collect::<String>();
                for declaration in [
                    "struct MACRO_AUTHORITY_ALIAS",
                    "value: i32",
                    "struct MACRO_REQUESTED_ROOT",
                    "record: MACRO_AUTHORITY_ALIAS",
                    "alias: *mut MACRO_AUTHORITY_ALIAS",
                ] {
                    assert!(text.contains(declaration), "{declaration:?}: {text}");
                }
                assert!(!text.contains("struct MACRO_AUTHORITY_RECORD"), "{text}");
                assert!(
                    output
                        .keys()
                        .all(|key| key.namespace == "Example.Expansion")
                );
            } else if required {
                let error = result.unwrap_err().to_string();
                for evidence in [
                    "RequiredAuthorityContextAbsent",
                    "MACRO_AUTHORITY_RECORD",
                    "MACRO_REQUESTED_ROOT",
                    "expansion.cpp",
                    "header-authority-expansion.hpp",
                ] {
                    assert!(error.contains(evidence), "{evidence:?}: {error}");
                }
                assert!(
                    !error.contains("original input \"definitions.cpp\""),
                    "{error}"
                );
            } else {
                assert!(result.unwrap().is_empty());
            }
        }
    }
}

impl PartitionAudit {
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }

    pub fn conflicts(&self) -> &[PartitionConflict] {
        &self.conflicts
    }

    pub fn has_blocking_conflicts(&self) -> bool {
        self.conflicts
            .iter()
            .any(|conflict| conflict.reason != PartitionConflictReason::AuthoritySelected)
    }
}

impl Display for PartitionAudit {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(
            formatter,
            "header partition planning found {} conflict(s):",
            self.conflicts.len()
        )?;
        for conflict in &self.conflicts {
            let kind = match conflict.kind {
                PartitionItemKind::Type => "type",
                PartitionItemKind::Value => "value",
            };
            let reason = match conflict.reason {
                PartitionConflictReason::AmbiguousRootCandidates => {
                    "has ambiguous traversed-header candidates"
                }
                PartitionConflictReason::AmbiguousOwners => "has ambiguous logical owners",
                PartitionConflictReason::MissingOwner => "has no logical owner",
                PartitionConflictReason::AuthoritySelected => {
                    "has ranked traversed-header alternatives"
                }
            };
            let owners = if conflict.owners.is_empty() {
                "none".to_string()
            } else {
                conflict
                    .owners
                    .iter()
                    .map(|owner| {
                        format!("{}:{} -> {}", owner.partition, owner.root, owner.namespace)
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            writeln!(formatter, "- {kind} `{}` {reason}: {owners}", conflict.name)?;
        }
        Ok(())
    }
}

fn wildcard_matches(pattern: &str, name: &str) -> bool {
    let parts: Vec<_> = pattern.split('*').collect();
    let mut remainder = name;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 && !pattern.starts_with('*') {
            let Some(rest) = remainder.strip_prefix(part) else {
                return false;
            };
            remainder = rest;
        } else if let Some(offset) = remainder.find(part) {
            remainder = &remainder[offset + part.len()..];
        } else {
            return false;
        }
    }
    pattern.ends_with('*') || remainder.is_empty()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeReference {
    pub namespace: String,
    pub name: String,
    pub kind: TypeReferenceKind,
    pub enum_members: BTreeSet<String>,
}

impl TypeReference {
    pub fn new(
        namespace: impl Into<String>,
        name: impl Into<String>,
        kind: TypeReferenceKind,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            kind,
            enum_members: BTreeSet::new(),
        }
    }

    pub fn with_enum_members(
        mut self,
        members: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.enum_members
            .extend(members.into_iter().map(Into::into));
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeReferenceKind {
    Enum,
    Interface,
    Type,
}

/// The entry point exported by a native DLL.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NativeImportName {
    Name(String),
    Ordinal(u16),
}

/// A native function's DLL and exported entry point.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NativeImport {
    library: String,
    name: NativeImportName,
}

impl NativeImport {
    pub fn named(library: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            library: library.into(),
            name: NativeImportName::Name(name.into()),
        }
    }

    pub fn ordinal(library: impl Into<String>, ordinal: u16) -> Self {
        Self {
            library: library.into(),
            name: NativeImportName::Ordinal(ordinal),
        }
    }

    pub fn library(&self) -> &str {
        &self.library
    }

    pub fn name(&self) -> &NativeImportName {
        &self.name
    }

    fn metadata_name(&self) -> Cow<'_, str> {
        match &self.name {
            NativeImportName::Name(name) => Cow::Borrowed(name),
            NativeImportName::Ordinal(ordinal) => Cow::Owned(format!("#{ordinal}")),
        }
    }
}

/// Native import contracts keyed by the C linker symbol used by a function fact.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeImports {
    imports: BTreeMap<String, NativeImport>,
}

impl NativeImports {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one contract or accepts an exact duplicate.
    ///
    /// A linker symbol cannot silently select between different DLL entry points.
    pub fn insert(
        &mut self,
        symbol: impl Into<String>,
        import: NativeImport,
    ) -> Result<&mut Self, Error> {
        let symbol = symbol.into();
        if symbol.is_empty() {
            return Err(Error("native import symbol is empty".to_string()));
        }
        if import.library.is_empty() {
            return Err(Error(format!(
                "native import `{symbol}` has an empty library"
            )));
        }
        if matches!(&import.name, NativeImportName::Name(name) if name.is_empty()) {
            return Err(Error(format!(
                "native import `{symbol}` has an empty entry-point name"
            )));
        }
        if let Some(existing) = self.imports.get(&symbol)
            && existing != &import
        {
            return Err(Error(format!(
                "conflicting native import contracts for `{symbol}`: {} and {}",
                format_native_import(existing),
                format_native_import(&import)
            )));
        }
        self.imports.insert(symbol, import);
        Ok(self)
    }

    pub fn get(&self, symbol: &str) -> Option<&NativeImport> {
        self.imports.get(symbol)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &NativeImport)> {
        self.imports
            .iter()
            .map(|(symbol, import)| (symbol.as_str(), import))
    }

    pub fn is_empty(&self) -> bool {
        self.imports.is_empty()
    }

    pub fn len(&self) -> usize {
        self.imports.len()
    }
}

fn format_native_import(import: &NativeImport) -> String {
    match &import.name {
        NativeImportName::Name(name) => format!("{}!{name}", import.library),
        NativeImportName::Ordinal(ordinal) => format!("{}!#{ordinal}", import.library),
    }
}

pub struct EmitOptions<'a> {
    pub namespace: &'a str,
    pub library: Option<&'a str>,
    pub libraries: Option<&'a BTreeMap<String, String>>,
    pub native_imports: Option<&'a NativeImports>,
    pub references: &'a BTreeMap<String, TypeReference>,
    pub excluded: Option<&'a BTreeSet<String>>,
    pub excluded_types: Option<&'a BTreeSet<String>>,
    pub excluded_functions: Option<&'a BTreeSet<String>>,
    pub excluded_constants: Option<&'a BTreeSet<String>>,
    pub functions: Option<&'a BTreeSet<String>>,
    /// Uses SDK PSTR/PWSTR types with per-use const and termination attributes.
    pub mutable_string_aliases: bool,
}

impl<'a> EmitOptions<'a> {
    pub fn new(namespace: &'a str, references: &'a BTreeMap<String, TypeReference>) -> Self {
        Self {
            namespace,
            library: None,
            libraries: None,
            native_imports: None,
            references,
            excluded: None,
            excluded_types: None,
            excluded_functions: None,
            excluded_constants: None,
            functions: None,
            mutable_string_aliases: false,
        }
    }
}

impl Input {
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        let name = normalize_name(&name.into());
        Self {
            roots: BTreeSet::from([name.clone()]),
            root_dirs: BTreeSet::new(),
            root_suffixes: BTreeSet::new(),
            excluded_roots: BTreeSet::new(),
            name,
            source: source.into(),
        }
    }

    pub fn with_roots(mut self, roots: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.roots
            .extend(roots.into_iter().map(|root| normalize_name(&root.into())));
        self
    }

    pub fn with_root_suffixes(
        mut self,
        roots: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.root_suffixes
            .extend(roots.into_iter().map(|root| normalize_name(&root.into())));
        self
    }

    pub fn with_root_dirs(mut self, roots: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.root_dirs.extend(roots.into_iter().map(|root| {
            let mut root = normalize_name(&root.into());
            if !root.ends_with('/') {
                root.push('/');
            }
            root
        }));
        self
    }

    pub fn with_excluded_roots(
        mut self,
        roots: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.excluded_roots
            .extend(roots.into_iter().map(|root| normalize_name(&root.into())));
        self
    }

    /// Excludes declarations whose spelling location is beneath these directories.
    ///
    /// The files remain visible through [`Snapshot::included_files`].
    pub fn with_excluded_source_dirs(
        mut self,
        roots: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        for root in roots {
            let mut root = normalize_name(&root.into());
            while root.ends_with('/') {
                root.pop();
            }
            if !root.is_empty() {
                root.push('/');
                self.excluded_roots.insert(root);
            }
        }
        self
    }

    pub fn partitioned(self, identity: impl Into<String>) -> PartitionedInput {
        PartitionedInput {
            input: self,
            identity: identity.into(),
            roots: BTreeMap::new(),
            arguments: Vec::new(),
        }
    }
}

impl PartitionedInput {
    pub fn with_cpp20(mut self) -> Self {
        self.arguments.push("-std=c++20".to_string());
        self
    }

    pub fn with_include_directory(mut self, path: impl Into<String>) -> Self {
        self.arguments
            .push(format!("-I{}", normalize_name(&path.into())));
        self
    }

    pub fn with_root(
        self,
        root: impl Into<String>,
        partition: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Self {
        self.with_root_partition(root, RootPartition::new(partition, namespace))
    }

    pub fn with_root_partition(
        mut self,
        root: impl Into<String>,
        partition: RootPartition,
    ) -> Self {
        let root = normalize_name(&root.into());
        self.input.roots.insert(root.clone());
        self.roots.insert(root, partition);
        self
    }
}

impl RootPartition {
    pub fn new(partition: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            partition: partition.into(),
            namespace: namespace.into(),
            remaps: BTreeMap::new(),
            exclusions: BTreeSet::new(),
            libraries: BTreeMap::new(),
            u32_types: BTreeSet::new(),
            flags: BTreeSet::new(),
            preserved_auto_function_pointer_levels: BTreeSet::new(),
            exclude_empty_records: false,
        }
    }

    pub fn with_remap(mut self, source: impl Into<String>, target: impl Into<String>) -> Self {
        self.remaps.insert(source.into(), target.into());
        self
    }

    pub fn with_exclusion(mut self, name: impl Into<String>) -> Self {
        self.exclusions.insert(name.into());
        self
    }

    pub fn with_library(mut self, function: impl Into<String>, library: impl Into<String>) -> Self {
        self.libraries.insert(function.into(), library.into());
        self
    }

    pub fn with_u32_type(mut self, name: impl Into<String>) -> Self {
        self.u32_types.insert(name.into());
        self
    }

    pub fn with_flags(mut self, name: impl Into<String>) -> Self {
        self.flags.insert(name.into());
        self
    }

    pub fn with_preserved_auto_function_pointer_level(mut self, name: impl Into<String>) -> Self {
        self.preserved_auto_function_pointer_levels
            .insert(name.into());
        self
    }

    pub fn exclude_empty_records(mut self) -> Self {
        self.exclude_empty_records = true;
        self
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Origin {
    pub tu: String,
    pub local: u32,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Location {
    pub file: String,
    pub offset: u32,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IncludedFile {
    pub input: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Annotation {
    SetLastError,
    ImportLibrary(String),
    PreserveResult,
    RaiiFree(String),
    InvalidHandle(String),
    FreeWith(String),
    DoNotRelease,
    NotNullTerminated,
    NullNullTerminated,
    ArrayCountParam(String),
    ArrayCountConst(String),
    ArrayCountField(String),
    MemorySizeParam(String),
    MemorySizeConst(String),
    CanReturnErrorsAsSuccess,
    CanReturnMultipleSuccessValues,
    Retained,
    IgnoreIfReturn(String),
    AlsoUsableFor(String),
    AssociatedEnum(String),
    AssociatedConstant(String),
    NativeInheritance(String),
    StructSizeField(String),
    NativeEncoding(String),
    Ansi,
    Unicode,
    Agile,
    Const,
    StaticLibrary(String),
    SupportedOs(String),
    In,
    Out,
    Optional,
    Reserved,
    ComOutPtr,
    Retval,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AnnotationTarget {
    Declaration(Origin),
    Return(Origin),
    Parameter {
        declaration: Origin,
        index: usize,
    },
    Field {
        declaration: Origin,
        index: usize,
    },
    NestedField {
        declaration: Origin,
        path: Vec<usize>,
    },
    Variant {
        declaration: Origin,
        index: usize,
    },
    Method {
        declaration: Origin,
        index: usize,
    },
    MethodReturn {
        declaration: Origin,
        index: usize,
    },
    MethodParameter {
        declaration: Origin,
        method: usize,
        parameter: usize,
    },
}

impl AnnotationTarget {
    fn origin(&self) -> &Origin {
        match self {
            Self::Declaration(origin) | Self::Return(origin) => origin,
            Self::Parameter { declaration, .. }
            | Self::Field { declaration, .. }
            | Self::NestedField { declaration, .. }
            | Self::Variant { declaration, .. }
            | Self::Method { declaration, .. }
            | Self::MethodReturn { declaration, .. }
            | Self::MethodParameter { declaration, .. } => declaration,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Scalar {
    Bool,
    F32,
    F64,
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TypeRef {
    Void,
    String,
    Object,
    Scalar(Scalar),
    Named {
        name: String,
        declaration: Location,
    },
    Pointer {
        mutable: bool,
        target: Box<Self>,
    },
    Reference {
        mutable: bool,
        target: Box<Self>,
    },
    FunctionPointer {
        convention: CallingConvention,
        params: Vec<Self>,
        result: Box<Self>,
    },
    OpaquePointer {
        mutable: bool,
        tag: String,
    },
    Array {
        target: Box<Self>,
        len: usize,
    },
    Generic {
        name: String,
        declaration: Location,
        args: Vec<Self>,
    },
    InlineRecord(Box<InlineRecord>),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct InlineRecord {
    pub name: Option<String>,
    pub base: Option<TypeRef>,
    pub fields: Vec<Field>,
    pub size: i64,
    pub align: i64,
    pub packing: Option<i64>,
    pub alignment: Option<i64>,
    pub union: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Field {
    pub name: String,
    pub ty: TypeRef,
    pub offset: i64,
    pub align: i64,
    pub size: i64,
    pub bit_width: Option<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Variant {
    pub name: String,
    pub value: i64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Parameter {
    pub name: String,
    pub ty: TypeRef,
    pub annotation: ParamAnnotation,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Method {
    pub name: String,
    pub params: Vec<Parameter>,
    pub result: TypeRef,
    pub special: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeType(Arc<NativeTypeData>);

impl std::ops::Deref for NativeType {
    type Target = NativeTypeData;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
impl std::ops::DerefMut for NativeType {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Arc::make_mut(&mut self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeTypeData {
    kind: i32,
    is_const: bool,
    is_volatile: bool,
    is_restrict: bool,
    size: i64,
    address_space: u32,
    spelling: String,
    layer: NativeTypeLayer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NativeTypeLayer {
    Primitive,
    Record(Box<NativeRecord>),
    Interface(Box<NativeDeclaration>),
    Enum(Box<NativeEnum>),
    Indirect(Box<NativeType>),
    Array {
        target: Box<NativeType>,
        len: u64,
    },
    Function {
        convention: i32,
        variadic: bool,
        ref_qualifier: i32,
        params: Vec<NativeType>,
        result: Box<NativeType>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeDeclaration {
    tu: String,
    kind: i32,
    name: String,
    spelling: Location,
    expansion: Location,
    anonymous: bool,
    parent: NativeParent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeParent {
    kind: i32,
    name: String,
    spelling: Option<Location>,
    expansion: Option<Location>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeRecord {
    declaration: NativeDeclaration,
    align: i64,
    aggregate_abi_proved: bool,
    fields: Vec<NativeField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeField {
    declaration: NativeDeclaration,
    ordinal: usize,
    offset: i64,
    align: i64,
    bit_width: Option<u32>,
    ty: NativeType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeEnum {
    declaration: NativeDeclaration,
    underlying: NativeType,
    scoped: bool,
    variants: Vec<NativeVariant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeVariant {
    declaration: NativeDeclaration,
    signed_value: i64,
    unsigned_value: u64,
}

#[derive(Clone, Debug, Eq)]
struct NativeComMethod {
    name: String,
    convention: i32,
    params: Vec<Parameter>,
    result: TypeRef,
    native_params: Vec<Option<NativeType>>,
    native_result: Option<NativeType>,
    native_exception: Option<i32>,
    native_owner: Option<Arc<NativeComCallable>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeComCallable {
    declaration: NativeDeclaration,
    params: Vec<NativeDeclaration>,
}

impl PartialEq for NativeComMethod {
    fn eq(&self, other: &Self) -> bool {
        // Compiler evidence does not participate in legacy contract equality.
        self.name == other.name
            && self.convention == other.convention
            && self.params == other.params
            && self.result == other.result
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeComContract {
    guid: String,
    has_fields: bool,
    pointer_width: i32,
    bases: Vec<String>,
    slots: Vec<NativeComMethod>,
}

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct ParamAnnotation {
    pub input: bool,
    pub output: bool,
    pub optional: bool,
    pub reserved: bool,
    pub com_out_ptr: bool,
    pub retval: bool,
    pub null_terminated: bool,
    pub null_null_terminated: bool,
    pub size: Option<SalSize>,
    pub unsupported: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SalSize {
    pub bytes: bool,
    pub value: SalSizeValue,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SalSizeValue {
    Constant(i32),
    Parameter(String),
    IndirectParameter(String),
    Expression(String),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CallingConvention {
    Platform,
    C,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FunctionLinkage {
    None,
    Internal,
    UniqueExternal,
    External,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FunctionExclusion {
    Definition { linkage: FunctionLinkage },
    NonExternalLinkage { linkage: FunctionLinkage },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FunctionSignature {
    pub link_name: String,
    pub convention: CallingConvention,
    pub params: Vec<Parameter>,
    pub result: TypeRef,
    pub variadic: bool,
    pub noreturn: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FactData {
    Callback {
        convention: CallingConvention,
        params: Vec<Parameter>,
        result: TypeRef,
    },
    Class {
        guid: String,
    },
    Enum {
        repr: Scalar,
        variants: Vec<Variant>,
        fixed: bool,
        scoped: bool,
    },
    EnumFlag {
        target: String,
    },
    Guid {
        value: String,
    },
    PropertyKey {
        ty: &'static str,
        guid: String,
        pid: u32,
    },
    Macro {
        function_like: bool,
        tokens: Vec<String>,
    },
    Function {
        link_name: String,
        convention: CallingConvention,
        params: Vec<Parameter>,
        result: TypeRef,
        variadic: bool,
        noreturn: bool,
    },
    NonEmittableFunction {
        reason: FunctionExclusion,
        signature: FunctionSignature,
    },
    Interface {
        base: Option<TypeRef>,
        guid: Option<String>,
        methods: Vec<Method>,
    },
    Record {
        base: Option<TypeRef>,
        fields: Vec<Field>,
        size: i64,
        align: i64,
        packing: Option<i64>,
        alignment: Option<i64>,
        union: bool,
    },
    Typedef {
        target: TypeRef,
    },
    Unsupported {
        reason: String,
    },
    None,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FactKind {
    Class,
    Enum,
    EnumFlag,
    Function,
    Guid,
    Macro,
    Namespace,
    Struct,
    Typedef,
    Union,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Fact {
    pub origin: Origin,
    pub parent: Option<Origin>,
    pub kind: FactKind,
    pub name: String,
    pub spelling: Location,
    pub expansion: Location,
    pub definition: bool,
    pub main_file: bool,
    pub root: bool,
    pub system: bool,
    pub data: FactData,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Value {
    F32(u32),
    F64(u64),
    Signed(i64),
    Unsigned(u64),
    Utf8(String),
    Utf16(String),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Constant {
    pub root: Origin,
    pub definition: Origin,
    pub spelling: Location,
    pub name: String,
    pub ty: TypeRef,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ValueDeclarationKind {
    Variable,
    AnonymousEnum,
}

/// Compiler-observed source evidence for a value origin that has no metadata fact.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ValueDeclaration {
    pub origin: Origin,
    pub parent: Option<Origin>,
    pub kind: ValueDeclarationKind,
    pub name: String,
    pub spelling: Location,
    pub expansion: Location,
    pub definition: bool,
    pub main_file: bool,
    pub root: bool,
    pub system: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoclassAlias {
    pub canonical: CanonicalRecord,
    pub guids: Vec<(Origin, String)>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CanonicalRecordKind {
    Class,
    Struct,
}

/// Exact native record identity, distinct from its physical declaration observations.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CanonicalRecord {
    Source(Origin),
    CompilerGenerated {
        anchor: Origin,
        kind: CanonicalRecordKind,
    },
}

impl CanonicalRecord {
    fn anchor(&self) -> &Origin {
        match self {
            Self::Source(origin) | Self::CompilerGenerated { anchor: origin, .. } => origin,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClassCanonicalAnchor {
    kind: CanonicalRecordKind,
    parent: Option<Origin>,
    name: String,
    spelling: Location,
    expansion: Location,
    main_file: bool,
    system: bool,
    template_name: String,
    template_spelling: Location,
    template_expansion: Location,
    template_partial: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    facts: Vec<Fact>,
    constants: Vec<Constant>,
    value_declarations: Vec<ValueDeclaration>,
    included_files: Vec<IncludedFile>,
    declare_handles: Vec<DeclareHandle>,
    annotations: BTreeMap<AnnotationTarget, Vec<Annotation>>,
    source_annotations: BTreeMap<AnnotationTarget, Vec<Annotation>>,
    function_annotations: BTreeMap<AnnotationTarget, Vec<Annotation>>,
    sal_constant_sizes: SalConstantSizes,
    declaration_guids: BTreeMap<Origin, String>,
    semantic_parents: BTreeMap<Origin, Option<Origin>>,
    class_canonical_origins: BTreeMap<Origin, CanonicalRecord>,
    class_canonical_anchors: BTreeMap<Origin, ClassCanonicalAnchor>,
    shared_enum_identities: BTreeMap<Origin, SharedEnumIdentity>,
    native_com_contracts: BTreeMap<Origin, Arc<NativeComContract>>,
    native_typedef_types: BTreeMap<Origin, Option<NativeType>>,
    unproved_com_records: BTreeSet<Origin>,
    observation_only_class_origins: BTreeSet<Origin>,
    pointer_callback_aliases: BTreeSet<Origin>,
    pointer_only_class_layouts: BTreeMap<Origin, FactData>,
    embeddable_class_layouts: BTreeSet<Origin>,
    clang_flag_enums: BTreeSet<Origin>,
    root_owners: BTreeMap<Origin, RootOwner>,
    constant_root_owners: BTreeMap<(Origin, String), RootOwner>,
    root_partitions: BTreeMap<(String, String), RootOwner>,
    header_authorities: BTreeMap<String, HeaderAuthority>,
    partition_authorities: BTreeMap<String, PartitionAuthority>,
    partition_inputs: BTreeMap<String, String>,
    input_order: BTreeMap<String, usize>,
    partition_exclusions: Vec<ExcludedPartitionDeclaration>,
    forced_flags: BTreeSet<Origin>,
    suppressed_type_origins: BTreeSet<Origin>,
    projected_type_names: BTreeMap<Origin, String>,
    namespace_authorities: BTreeMap<String, String>,
    fact_namespace_authorities: BTreeMap<Origin, String>,
    constant_namespace_authorities: BTreeMap<(Origin, String), String>,
    header_partition_policy: bool,
    ranked_header_partition_policy: bool,
    header_authority_partition: Option<String>,
    timing_target: Option<String>,
}

/// An unsupported size hint on a selected native parameter that is retained without a count.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SalCountDiagnostic {
    pub declaration: Origin,
    pub source: Location,
    pub parameter: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SalCountError {
    Unsupported(String),
    Invalid(String),
}

impl Display for SalCountError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(reason) | Self::Invalid(reason) => reason.fmt(formatter),
        }
    }
}

type SalConstantSizes = BTreeMap<AnnotationTarget, Result<SalSize, SalCountError>>;

type SalConstantObservation<'a> = (&'a AnnotationTarget, &'a Result<SalSize, SalCountError>);

struct SalConstantObservations<'a> {
    by_use:
        BTreeMap<&'a Location, BTreeMap<RouteAnnotationTarget, Vec<SalConstantObservation<'a>>>>,
    #[cfg(test)]
    inspected: Cell<usize>,
}

impl<'a> SalConstantObservations<'a> {
    fn new(sizes: &'a SalConstantSizes, facts: &HashMap<&Origin, &'a Fact>) -> Self {
        let mut by_use = BTreeMap::<_, BTreeMap<_, Vec<_>>>::new();
        for (target, size) in sizes {
            if let Some(fact) = facts.get(target.origin()) {
                by_use
                    .entry(&fact.spelling)
                    .or_default()
                    .entry(route_annotation_target(target).1)
                    .or_default()
                    .push((target, size));
            }
        }
        Self {
            by_use,
            #[cfg(test)]
            inspected: Cell::new(0),
        }
    }

    fn get(
        &self,
        spelling: &Location,
        slot: RouteAnnotationTarget,
    ) -> &[SalConstantObservation<'a>] {
        self.by_use
            .get(spelling)
            .and_then(|slots| slots.get(&slot))
            .map_or(&[], Vec::as_slice)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DeclareHandle {
    alias: Origin,
    record: Origin,
}

struct DeclarationIndex<'a> {
    facts: HashMap<(&'a str, &'a str, &'a Location), Vec<&'a Fact>>,
    #[cfg(test)]
    inspected_candidates: Cell<usize>,
}

impl<'a> DeclarationIndex<'a> {
    fn new(facts: &'a [Fact]) -> Self {
        let mut result = Self {
            facts: HashMap::with_capacity(facts.len()),
            #[cfg(test)]
            inspected_candidates: Cell::new(0),
        };
        for fact in facts {
            result
                .facts
                .entry((&fact.origin.tu, &fact.name, &fact.spelling))
                .or_default()
                .push(fact);
        }
        result
    }

    fn get<'b>(&'b self, tu: &'b str, name: &'b str, declaration: &'b Location) -> &'b [&'a Fact] {
        let facts = self
            .facts
            .get(&(tu, name, declaration))
            .map(Vec::as_slice)
            .unwrap_or_default();
        #[cfg(test)]
        self.inspected_candidates
            .set(self.inspected_candidates.get() + facts.len());
        facts
    }
}

#[cfg(test)]
struct TypedefDeclarationIndex<'a> {
    facts: HashMap<(&'a str, &'a str, &'a Location), Vec<&'a Fact>>,
    #[cfg(test)]
    visited_facts: usize,
    #[cfg(test)]
    lookups: Cell<usize>,
    #[cfg(test)]
    inspected_candidates: Cell<usize>,
}

#[cfg(test)]
impl<'a> TypedefDeclarationIndex<'a> {
    fn new(facts: &'a [Fact]) -> Self {
        let mut result = Self {
            facts: HashMap::new(),
            #[cfg(test)]
            visited_facts: 0,
            #[cfg(test)]
            lookups: Cell::new(0),
            #[cfg(test)]
            inspected_candidates: Cell::new(0),
        };
        for fact in facts {
            #[cfg(test)]
            {
                result.visited_facts += 1;
            }
            if !matches!(fact.data, FactData::Typedef { .. }) {
                continue;
            }
            result
                .facts
                .entry((&fact.origin.tu, &fact.name, &fact.spelling))
                .or_default()
                .push(fact);
        }
        result
    }

    fn get<'b>(&'b self, tu: &'b str, name: &'b str, declaration: &'b Location) -> &'b [&'a Fact] {
        let facts = self
            .facts
            .get(&(tu, name, declaration))
            .map(Vec::as_slice)
            .unwrap_or_default();
        #[cfg(test)]
        {
            self.lookups.set(self.lookups.get() + 1);
            self.inspected_candidates
                .set(self.inspected_candidates.get() + facts.len());
        }
        facts
    }

    #[cfg(test)]
    fn metrics(&self) -> (usize, usize, usize) {
        (
            self.visited_facts,
            self.lookups.get(),
            self.inspected_candidates.get(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct HeaderPartitionPlan {
    snapshot: Snapshot,
    root_conflicts: Vec<PartitionConflict>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExcludedPartitionDeclaration {
    name: String,
    origin: Origin,
    spelling: Location,
    kind: FactKind,
    data_kind: &'static str,
    uuid: Option<String>,
    definition: bool,
    root: bool,
    owner: RootOwner,
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.facts == other.facts
            && self.constants == other.constants
            && self.value_declarations == other.value_declarations
            && self.declare_handles == other.declare_handles
            && self.annotations == other.annotations
            && self.source_annotations == other.source_annotations
            && self.function_annotations == other.function_annotations
            && self.sal_constant_sizes == other.sal_constant_sizes
            && self.declaration_guids == other.declaration_guids
            && self.semantic_parents == other.semantic_parents
            && self.class_canonical_origins == other.class_canonical_origins
            && self.class_canonical_anchors == other.class_canonical_anchors
            && self.shared_enum_identities == other.shared_enum_identities
            && self.native_com_contracts == other.native_com_contracts
            && self.unproved_com_records == other.unproved_com_records
            && self.observation_only_class_origins == other.observation_only_class_origins
            && self.pointer_callback_aliases == other.pointer_callback_aliases
            && self.pointer_only_class_layouts == other.pointer_only_class_layouts
            && self.embeddable_class_layouts == other.embeddable_class_layouts
            && self.clang_flag_enums == other.clang_flag_enums
            && self.root_owners == other.root_owners
            && self.constant_root_owners == other.constant_root_owners
            && self.root_partitions == other.root_partitions
            && self.header_authorities == other.header_authorities
            && self.partition_authorities == other.partition_authorities
            && self.partition_inputs == other.partition_inputs
            && self.partition_exclusions == other.partition_exclusions
            && self.forced_flags == other.forced_flags
            && self.suppressed_type_origins == other.suppressed_type_origins
            && self.projected_type_names == other.projected_type_names
            && self.namespace_authorities == other.namespace_authorities
            && self.fact_namespace_authorities == other.fact_namespace_authorities
            && self.constant_namespace_authorities == other.constant_namespace_authorities
            && self.header_partition_policy == other.header_partition_policy
            && self.ranked_header_partition_policy == other.ranked_header_partition_policy
            && self.header_authority_partition == other.header_authority_partition
    }
}

impl Eq for Snapshot {}

impl Snapshot {
    /// Exact native class/struct redeclaration bindings within each original translation unit.
    ///
    /// Compiler-created entities have no physical canonical declaration. Their anchor identifies
    /// the first native observation in the group, not a source definition or allocation type.
    pub fn class_canonical_origins(&self) -> &BTreeMap<Origin, CanonicalRecord> {
        &self.class_canonical_origins
    }

    /// GUID-value aliases, not allocation types. All physical UUID observations are retained.
    ///
    /// A required use of an alias as a native type still needs a supported type definition.
    pub fn coclass_aliases(&self) -> Result<BTreeMap<Origin, CoclassAlias>, Error> {
        let facts: BTreeMap<_, _> = self.facts.iter().map(|fact| (&fact.origin, fact)).collect();
        let mut declarations = BTreeMap::new();
        let mut guids: BTreeMap<&CanonicalRecord, Vec<(Origin, String)>> = BTreeMap::new();
        let mut allocation_types = BTreeSet::new();
        for (member, canonical) in &self.class_canonical_origins {
            let fact = facts.get(member);
            let excluded = self.partition_exclusions.iter().find(|excluded| {
                excluded.origin == *member
                    && matches!(excluded.kind, FactKind::Class | FactKind::Struct)
            });
            if self.class_canonical_anchors.contains_key(member) {
                if fact.is_some()
                    || excluded.is_some()
                    || canonical != &CanonicalRecord::Source(member.clone())
                {
                    return Err(Error(
                        "invalid private native class canonical anchor".into(),
                    ));
                }
                continue;
            }
            if fact.is_none() && excluded.is_none() {
                return Err(Error("missing native class member".into()));
            }
            let anchor = canonical.anchor();
            let canonical_is_class = facts
                .get(anchor)
                .is_some_and(|fact| matches!(fact.kind, FactKind::Class | FactKind::Struct))
                || self.partition_exclusions.iter().any(|excluded| {
                    excluded.origin == *anchor
                        && matches!(excluded.kind, FactKind::Class | FactKind::Struct)
                })
                || self.class_canonical_anchors.contains_key(anchor);
            if fact.is_some_and(|fact| !matches!(fact.kind, FactKind::Class | FactKind::Struct))
                || !canonical_is_class
                || member.tu != anchor.tu
                || self.class_canonical_origins.get(anchor) != Some(canonical)
            {
                return Err(Error("invalid owned native class canonical binding".into()));
            }
            let spelling = fact
                .map(|fact| &fact.spelling)
                .or_else(|| excluded.map(|excluded| &excluded.spelling))
                .ok_or_else(|| Error("missing native class source observation".into()))?;
            let key = (member.tu.as_str(), spelling);
            if let Some(previous) = declarations.insert(key, canonical)
                && previous != canonical
            {
                return Err(Error(
                    "conflicting exact native class declaration bindings".into(),
                ));
            }
            if fact.is_some_and(|fact| {
                fact.definition || matches!(fact.data, FactData::Unsupported { .. })
            }) || excluded
                .is_some_and(|excluded| excluded.definition || excluded.data_kind == "Unsupported")
            {
                allocation_types.insert(canonical);
            }
            if let Some(guid) = self.declaration_guids.get(member) {
                if fact.is_some_and(|fact| matches!(&fact.data, FactData::Class { guid: observed } if observed != guid))
                    || excluded.is_some_and(|excluded| excluded.uuid.as_ref() != Some(guid))
                {
                    return Err(Error("missing native class UUID observation".into()));
                }
                let observations = guids.entry(canonical).or_default();
                if observations.iter().any(|(_, previous)| previous != guid) {
                    return Err(Error(
                        "conflicting UUID observations for exact native class".into(),
                    ));
                }
                observations.push((member.clone(), guid.clone()));
            } else if fact.is_some_and(|fact| matches!(fact.data, FactData::Class { .. })) {
                return Err(Error("missing native class UUID observation".into()));
            }
        }
        let mut aliases = BTreeMap::new();
        for fact in &self.facts {
            let FactData::Typedef {
                target: TypeRef::Named { declaration, .. },
            } = &fact.data
            else {
                continue;
            };
            let Some(canonical) = declarations.get(&(fact.origin.tu.as_str(), declaration)) else {
                continue;
            };
            if allocation_types.contains(canonical) {
                continue;
            }
            if let Some(observations) = guids.get(canonical) {
                aliases.insert(
                    fact.origin.clone(),
                    CoclassAlias {
                        canonical: (*canonical).clone(),
                        guids: observations.clone(),
                    },
                );
            }
        }
        Ok(aliases)
    }

    pub fn facts(&self) -> &[Fact] {
        &self.facts
    }

    pub fn constants(&self) -> &[Constant] {
        &self.constants
    }

    /// Native scalar variables and anonymous enums backing constants without a [`Fact`].
    ///
    /// An anonymous enum's constants share its origin. Macros and named enum variants retain
    /// their existing fact-backed origins instead of adding duplicate evidence here.
    pub fn value_declarations(&self) -> &[ValueDeclaration] {
        &self.value_declarations
    }

    pub fn included_files(&self) -> &[IncludedFile] {
        &self.included_files
    }

    pub fn annotations(&self) -> &BTreeMap<AnnotationTarget, Vec<Annotation>> {
        &self.annotations
    }

    pub fn unsupported(&self) -> impl Iterator<Item = (&Fact, &str)> {
        self.facts.iter().filter_map(|fact| {
            if let FactData::Unsupported { reason } = &fact.data {
                Some((fact, reason.as_str()))
            } else {
                None
            }
        })
    }

    fn fact_uuid<'a>(&'a self, fact: &'a Fact) -> Option<&'a str> {
        fact_uuid(fact).or_else(|| self.declaration_guids.get(&fact.origin).map(String::as_str))
    }

    pub fn dump(&self) -> String {
        let mut result = String::new();
        for fact in &self.facts {
            let parent = fact
                .parent
                .as_ref()
                .map_or(String::new(), |parent| format!(" <- {}", origin(parent)));
            result.push_str(&format!(
                "{} {:?}/{} {} [{}:{} -> {}:{}]{}{}{}{}\n",
                origin(&fact.origin),
                fact.kind,
                fact_data_kind(&fact.data),
                fact.name,
                fact.spelling.file,
                fact.spelling.offset,
                fact.expansion.file,
                fact.expansion.offset,
                if fact.definition { " definition" } else { "" },
                if fact.main_file { " main" } else { "" },
                if fact.system { " system" } else { "" },
                parent,
            ));
        }
        for constant in &self.constants {
            let route = format!(
                "{} -> {}",
                origin(&constant.root),
                origin(&constant.definition)
            );
            result.push_str(&format!(
                "{} Constant {} {:?} = {:?}\n",
                route, constant.name, constant.ty, constant.value
            ));
        }
        result
    }

    pub fn emit(&self, namespace: &str) -> Result<String, Error> {
        let references = BTreeMap::new();
        self.emit_with_options(&EmitOptions::new(namespace, &references))
    }

    pub fn emit_with_library(&self, namespace: &str, library: &str) -> Result<String, Error> {
        let references = BTreeMap::new();
        let mut options = EmitOptions::new(namespace, &references);
        options.library = Some(library);
        self.emit_with_options(&options)
    }

    pub fn emit_with_options(&self, options: &EmitOptions<'_>) -> Result<String, Error> {
        let timing = self.timing_target.is_some();
        let items = self.emit_items(options)?;
        let format_time = timing.then(std::time::Instant::now);
        let phase = TimingPhase::new(self.timing_target.as_deref(), "rdl-format");
        let result = phase.finish(write_rdl(
            options.namespace,
            items.values().map(|(_, item)| item.as_str()),
        ))?;
        if timing {
            eprintln!(
                "windows-clang timing phase=rdl-format target={} mode=single headers=1 bytes={} elapsed_ms={:.3}",
                self.timing_target.as_deref().unwrap(),
                result.len(),
                elapsed_ms(format_time)
            );
        }
        Ok(result)
    }

    /// Reports unrepresented size hints for the selected ordinary output without emitting RDL.
    pub fn sal_count_diagnostics(
        &self,
        options: &EmitOptions<'_>,
    ) -> Result<Vec<SalCountDiagnostic>, Error> {
        Ok(self
            .plan_with_options(options)?
            .sal_count_diagnostics
            .into_iter()
            .collect())
    }

    pub fn emit_by_header_with_options(
        &self,
        options: &EmitOptions<'_>,
    ) -> Result<BTreeMap<String, String>, Error> {
        let timing = self.timing_target.is_some();
        let items = self.emit_items(options)?;
        let format_time = timing.then(std::time::Instant::now);
        let mut partitions: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (_, (header, item)) in items {
            partitions.entry(header).or_default().push(item);
        }
        let result: BTreeMap<String, String> = partitions
            .into_iter()
            .map(|(header, items)| {
                Ok((
                    header,
                    write_rdl(options.namespace, items.iter().map(String::as_str))?,
                ))
            })
            .collect::<Result<_, Error>>()?;
        if timing {
            eprintln!(
                "windows-clang timing phase=rdl-format target={} mode=by-header headers={} bytes={} elapsed_ms={:.3}",
                self.timing_target.as_deref().unwrap(),
                result.len(),
                result.values().map(String::len).sum::<usize>(),
                elapsed_ms(format_time)
            );
        }
        Ok(result)
    }

    pub fn emit_partitioned_with_options(
        self,
        options: &EmitOptions<'_>,
    ) -> Result<BTreeMap<RdlPartition, String>, Error> {
        self.emit_partitioned_with_options_and_authorities(
            options,
            &NamespaceAuthorities::default(),
        )
    }

    pub fn emit_partitioned_with_options_and_authorities(
        mut self,
        options: &EmitOptions<'_>,
        authorities: &NamespaceAuthorities,
    ) -> Result<BTreeMap<RdlPartition, String>, Error> {
        self.apply_namespace_authorities(authorities)?;
        self.emit_partitioned_prepared(options)
    }

    pub fn plan_header_partitions(
        &self,
        policy: &HeaderPartitionPolicy,
        authorities: &NamespaceAuthorities,
    ) -> Result<HeaderPartitionPlan, Error> {
        self.clone().into_header_partition_plan(policy, authorities)
    }

    /// Consumes the snapshot while preparing a header partition plan.
    ///
    /// Use this after the caller finishes inspecting the extracted snapshot to avoid cloning the
    /// complete snapshot before planning.
    pub fn into_header_partition_plan(
        mut self,
        policy: &HeaderPartitionPolicy,
        authorities: &NamespaceAuthorities,
    ) -> Result<HeaderPartitionPlan, Error> {
        let mut phase = TimingPhase::new(self.timing_target.as_deref(), "namespace-authorities");
        phase.check(self.apply_namespace_authorities(authorities))?;
        phase.next("header-ownership");
        let root_conflicts = phase.check(self.apply_header_partition_policy(policy))?;
        phase.event("complete", None);
        Ok(HeaderPartitionPlan {
            snapshot: self,
            root_conflicts,
        })
    }

    pub fn emit_header_partitions_with_options(
        &self,
        policy: &HeaderPartitionPolicy,
        options: &EmitOptions<'_>,
    ) -> Result<BTreeMap<RdlPartition, String>, Error> {
        self.emit_header_partitions_with_options_and_authorities(
            policy,
            options,
            &NamespaceAuthorities::default(),
        )
    }

    pub fn emit_header_partitions_with_options_and_authorities(
        &self,
        policy: &HeaderPartitionPolicy,
        options: &EmitOptions<'_>,
        authorities: &NamespaceAuthorities,
    ) -> Result<BTreeMap<RdlPartition, String>, Error> {
        self.plan_header_partitions(policy, authorities)?
            .emit_with_options(options)
    }

    fn apply_namespace_authorities(
        &mut self,
        authorities: &NamespaceAuthorities,
    ) -> Result<(), Error> {
        self.namespace_authorities = authorities.resolve(
            self.facts
                .iter()
                .map(|fact| fact.name.as_str())
                .chain(self.constants.iter().map(|constant| constant.name.as_str())),
        )?;
        self.fact_namespace_authorities = self
            .facts
            .iter()
            .filter_map(|fact| {
                self.namespace_authorities
                    .get(&fact.name)
                    .map(|namespace| (fact.origin.clone(), namespace.clone()))
            })
            .collect();
        self.constant_namespace_authorities = self
            .constants
            .iter()
            .filter_map(|constant| {
                self.namespace_authorities
                    .get(&constant.name)
                    .map(|namespace| {
                        (
                            (constant.definition.clone(), constant.name.clone()),
                            namespace.clone(),
                        )
                    })
            })
            .collect();
        Ok(())
    }

    fn emit_partitioned_prepared(
        self,
        options: &EmitOptions<'_>,
    ) -> Result<BTreeMap<RdlPartition, String>, Error> {
        let target = self.timing_target.clone();
        let mut phase = TimingPhase::new(target.as_deref(), "partition-reconciliation");
        let (snapshot, display_names, source_names) =
            self.into_partitioned_planning_snapshot(options);
        phase.next("partition-planning");
        let plan =
            phase.check(snapshot.plan_partitioned(options, &display_names, &source_names))?;
        phase.next("partition-routing");
        let candidates =
            phase.check(snapshot.partition_route_candidates(&plan, &source_names, options))?;
        let routes = phase.check(snapshot.resolve_partition_routes(candidates))?;
        phase.event("complete", None);
        snapshot.format_partitioned_plan(plan, options, &routes, &display_names, target.as_deref())
    }

    fn format_partitioned_plan(
        &self,
        plan: Plan<'_>,
        options: &EmitOptions<'_>,
        routes: &BTreeMap<(String, OutputKind), RootOwner>,
        display_names: &BTreeMap<String, String>,
        target: Option<&str>,
    ) -> Result<BTreeMap<RdlPartition, String>, Error> {
        let timing = target.is_some();
        let format_time = timing.then(std::time::Instant::now);
        let mut phase = TimingPhase::new(target, "rdl-format");
        let items =
            phase.check(self.format_items(plan, options, Some(routes), Some(display_names)))?;
        let mut partitions: BTreeMap<RdlPartition, Vec<String>> = BTreeMap::new();
        for (key, (_, item)) in items {
            phase.visit(None);
            let route = routes.get(&key).unwrap();
            partitions
                .entry(RdlPartition {
                    partition: route.partition.clone(),
                    namespace: route.namespace.clone(),
                    header: route.root.clone(),
                })
                .or_default()
                .push(item);
        }
        let result = partitions
            .into_iter()
            .map(|(partition, items)| {
                Ok((
                    partition.clone(),
                    write_rdl_strict(&partition.namespace, items.iter().map(String::as_str))?,
                ))
            })
            .collect::<Result<_, Error>>();
        let result: BTreeMap<RdlPartition, String> = phase.finish(result)?;
        if timing {
            eprintln!(
                "windows-clang timing phase=rdl-format target={} mode=partitioned partitions={} bytes={} elapsed_ms={:.3}",
                target.unwrap(),
                result.len(),
                result.values().map(String::len).sum::<usize>(),
                elapsed_ms(format_time)
            );
        }
        Ok(result)
    }

    fn emit_items(
        &self,
        options: &EmitOptions<'_>,
    ) -> Result<BTreeMap<(String, OutputKind), (String, String)>, Error> {
        let timing = self.timing_target.is_some();
        let target = self.timing_target.as_deref().unwrap_or("default");
        let plan_time = timing.then(std::time::Instant::now);
        let plan = self.plan_with_options(options)?;
        if timing {
            eprintln!(
                "windows-clang timing phase=planning target={target} facts={} constants={} types={} values={} functions={} output_constants={} elapsed_ms={:.3}",
                self.facts.len(),
                self.constants.len(),
                plan.types.len(),
                plan.values.len(),
                plan.functions.len(),
                plan.constants.len(),
                elapsed_ms(plan_time)
            );
        }
        self.format_items(plan, options, None, None)
    }

    fn plan_with_options<'a>(&'a self, options: &'a EmitOptions<'_>) -> Result<Plan<'a>, Error> {
        self.plan(PlanningOptions {
            references: options.references,
            excluded_types: options.excluded_types.or(options.excluded),
            excluded_functions: options.excluded_functions.or(options.excluded),
            excluded_constants: options.excluded_constants.or(options.excluded),
            selected_functions: options.functions,
            display_names: None,
            source_names: None,
            mutable_string_aliases: options.mutable_string_aliases,
            allow_enum_omissions: false,
            omission_namespace: options.namespace,
        })
    }

    fn format_items(
        &self,
        plan: Plan<'_>,
        options: &EmitOptions<'_>,
        routes: Option<&BTreeMap<(String, OutputKind), RootOwner>>,
        display_names: Option<&BTreeMap<String, String>>,
    ) -> Result<BTreeMap<(String, OutputKind), (String, String)>, Error> {
        let mut phase = TimingPhase::new(self.timing_target.as_deref(), "rdl-items");
        let result = self.format_items_inner(plan, options, routes, display_names, &mut phase);
        phase.finish(result)
    }

    fn format_items_inner(
        &self,
        mut plan: Plan<'_>,
        options: &EmitOptions<'_>,
        routes: Option<&BTreeMap<(String, OutputKind), RootOwner>>,
        display_names: Option<&BTreeMap<String, String>>,
        phase: &mut TimingPhase,
    ) -> Result<BTreeMap<(String, OutputKind), (String, String)>, Error> {
        let timing = self.timing_target.is_some();
        let target = self.timing_target.as_deref().unwrap_or("default");
        let emission_time = timing.then(std::time::Instant::now);
        for diagnostic in &plan.sal_count_diagnostics {
            eprintln!(
                "windows-clang: unsupported SAL count projection: {}; \
                 retaining native parameter without a count relationship",
                diagnostic.reason
            );
        }
        let mut local_types = BTreeMap::new();
        let mut routed_types = BTreeMap::new();
        let facts_by_origin = routes.is_some().then(|| {
            self.facts
                .iter()
                .map(|fact| (&fact.origin, fact))
                .collect::<HashMap<_, _>>()
        });
        if let Some(routes) = routes {
            let mut local_type_namespaces: BTreeMap<Location, BTreeSet<String>> = BTreeMap::new();
            for planned in &plan.types {
                routed_types.insert(
                    planned.name.clone(),
                    routes[&(planned.name.clone(), OutputKind::Type)]
                        .namespace
                        .clone(),
                );
            }
            let planned_type_origins: BTreeSet<_> = plan
                .types
                .iter()
                .map(|planned| &planned.fact.origin)
                .collect();
            let facts_by_origin = facts_by_origin.as_ref().unwrap();
            let local_reference_names: BTreeSet<_> = plan
                .types
                .iter()
                .filter_map(|planned| {
                    let name = display_names
                        .and_then(|names| names.get(&planned.fact.name))
                        .map_or(planned.fact.name.as_str(), String::as_str);
                    (options.references.contains_key(name)
                        && ((self
                            .pointer_only_class_layouts
                            .contains_key(&planned.fact.origin)
                            && matches!(planned.fact.data, FactData::Record { .. }))
                            || is_native_namespaced_declaration(planned.fact, facts_by_origin)))
                    .then_some(name)
                })
                .collect();
            for fact in self.facts.iter().filter(|fact| is_type_fact(fact)) {
                if canonical_raw_pointer_name(&fact.name)
                    && matches!(fact.data, FactData::Typedef { .. })
                    && !planned_type_origins.contains(&fact.origin)
                {
                    continue;
                }
                let display_name = display_names
                    .and_then(|names| names.get(&fact.name))
                    .map_or(fact.name.as_str(), String::as_str);
                if local_reference_names.contains(display_name)
                    && !planned_type_origins.contains(&fact.origin)
                    && is_abi_reference_declaration_for_name(
                        fact,
                        facts_by_origin,
                        options.references,
                        display_name,
                    )
                {
                    continue;
                }
                let emitted_name = plan
                    .type_names
                    .get(&fact.name)
                    .map_or(fact.name.as_str(), String::as_str);
                let Some(namespace) = routed_types.get(emitted_name) else {
                    continue;
                };
                local_type_namespaces
                    .entry(fact.spelling.clone())
                    .or_default()
                    .insert(namespace.clone());
            }
            for fact in self
                .facts
                .iter()
                .filter(|fact| !planned_type_origins.contains(&fact.origin) && is_type_fact(fact))
            {
                let name = display_names
                    .and_then(|names| names.get(&fact.name))
                    .map_or(fact.name.as_str(), String::as_str);
                if !local_reference_names.contains(name)
                    || !is_abi_reference_declaration_for_name(
                        fact,
                        facts_by_origin,
                        options.references,
                        name,
                    )
                {
                    continue;
                }
                let reference = &options.references[name];
                if reference.name != name {
                    continue;
                }
                local_type_namespaces
                    .entry(fact.spelling.clone())
                    .or_default()
                    .insert(reference.namespace.clone());
            }
            let mut uuid_namespaces: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
            for planned in &plan.types {
                let Some(guid) = self.fact_uuid(planned.fact) else {
                    continue;
                };
                uuid_namespaces
                    .entry((planned.fact.name.as_str(), guid))
                    .or_default()
                    .insert(
                        routes[&(planned.name.clone(), OutputKind::Type)]
                            .namespace
                            .as_str(),
                    );
            }
            for fact in &self.facts {
                let Some(guid) = self.fact_uuid(fact) else {
                    continue;
                };
                let Some(namespaces) = uuid_namespaces.get(&(fact.name.as_str(), guid)) else {
                    continue;
                };
                if namespaces.len() == 1 {
                    local_type_namespaces
                        .entry(fact.spelling.clone())
                        .or_default()
                        .insert((*namespaces.first().unwrap()).to_string());
                }
            }
            local_types.extend(local_type_namespaces.into_iter().filter_map(
                |(declaration, namespaces)| {
                    (namespaces.len() == 1)
                        .then(|| (declaration, namespaces.into_iter().next().unwrap()))
                },
            ));
        }
        if let Some(display_names) = display_names {
            for name in plan.type_names.values_mut() {
                if let Some(display) = display_names.get(name) {
                    *name = display.clone();
                }
            }
            for (internal, display) in display_names {
                plan.type_names
                    .entry(internal.clone())
                    .or_insert_with(|| display.clone());
            }
        }
        let mut items = BTreeMap::new();
        let planned = plan
            .values
            .into_iter()
            .map(|planned| (planned, OutputKind::Value))
            .chain(
                plan.types
                    .into_iter()
                    .map(|planned| (planned, OutputKind::Type)),
            );
        for (planned, kind) in planned {
            phase.visit(None);
            let fact = planned.fact;
            let output_name = display_names
                .and_then(|names| names.get(&planned.name))
                .unwrap_or(&planned.name);
            let namespace = routes
                .and_then(|routes| routes.get(&(planned.name.clone(), kind)))
                .map(|owner| owner.namespace.as_str());
            let projection = TypeProjection::new(
                &plan.type_names,
                &plan.interface_names,
                &fact.origin.tu,
                &local_types,
                &routed_types,
                (&plan.type_choices, plan.sal_constant_sizes),
                namespace,
            );
            let mut item = match &fact.data {
                FactData::Callback {
                    convention,
                    params,
                    result,
                } => {
                    let projection = TypeProjection::new(
                        &plan.type_names,
                        &plan.interface_names,
                        &fact.origin.tu,
                        &local_types,
                        &routed_types,
                        (&plan.type_choices, plan.sal_constant_sizes),
                        namespace,
                    );
                    write_callback(
                        output_name,
                        *convention,
                        params,
                        result,
                        &projection,
                        &self.annotations,
                        &fact.origin,
                    )?
                }
                FactData::Class { guid } => {
                    format!(
                        "    const {}: GUID = {};\n",
                        rdl_ident(output_name),
                        rdl_uuid(guid)
                    )
                }
                FactData::Guid { value } => {
                    format!(
                        "    const {}: GUID = {};\n",
                        rdl_ident(output_name),
                        rdl_uuid(value)
                    )
                }
                FactData::PropertyKey { ty, guid, pid } => {
                    let route_name = self
                        .projected_type_names
                        .get(&fact.origin)
                        .map_or(*ty, String::as_str);
                    let emitted_name = plan
                        .type_names
                        .get(route_name)
                        .map_or(route_name, String::as_str);
                    let ty =
                        qualify_routed_type_as(route_name, emitted_name, &routed_types, namespace);
                    format!(
                        "    #[guid({})]\n    const {}: {} = {pid};\n",
                        rdl_uuid(guid),
                        rdl_ident(output_name),
                        ty
                    )
                }
                FactData::Typedef {
                    target: TypeRef::InlineRecord(record),
                } => {
                    let projection = TypeProjection::new(
                        &plan.type_names,
                        &plan.interface_names,
                        &fact.origin.tu,
                        &local_types,
                        &routed_types,
                        (&plan.type_choices, plan.sal_constant_sizes),
                        namespace,
                    );
                    let item = write_named_record(
                        &rdl_ident(output_name),
                        &record.fields,
                        record.packing,
                        record.alignment,
                        record.union,
                        &projection,
                        Some((&self.annotations, &fact.origin)),
                    )?;
                    format!(
                        "{}{item}",
                        annotation_lines(
                            annotations_for(
                                &self.annotations,
                                &AnnotationTarget::Declaration(fact.origin.clone()),
                            ),
                            "    ",
                        )?
                    )
                }
                FactData::Typedef { target } => {
                    format!(
                        "{}    type {} = {};\n",
                        annotation_lines(
                            annotations_for(
                                &self.annotations,
                                &AnnotationTarget::Declaration(fact.origin.clone()),
                            ),
                            "    ",
                        )?,
                        rdl_ident(output_name),
                        planned_emitted_type_name(target, &projection)
                    )
                }
                FactData::Enum {
                    repr,
                    variants,
                    scoped,
                    ..
                } => {
                    let source_flags = self.clang_flag_enums.contains(&fact.origin);
                    let coerced_flags = plan
                        .flag_enums
                        .contains(&(fact.origin.tu.clone(), planned.name.clone()));
                    let flags = source_flags || coerced_flags;
                    // Source attributes preserve the declared representation. Macro and policy
                    // flags retain their existing same-width unsigned projection.
                    let repr = if coerced_flags {
                        unsigned_scalar(*repr)
                    } else {
                        *repr
                    };
                    let mut item = format!(
                        "{}    #[repr({})]\n{}{}    enum {} {{\n",
                        annotation_lines(
                            annotations_for(
                                &self.annotations,
                                &AnnotationTarget::Declaration(fact.origin.clone()),
                            ),
                            "    ",
                        )?,
                        scalar_name(repr),
                        if flags { "    #[flags]\n" } else { "" },
                        if *scoped { "    #[scoped]\n" } else { "" },
                        rdl_ident(output_name)
                    );
                    for (index, variant) in variants.iter().enumerate() {
                        item.push_str(&format!(
                            "        {}{} = {},\n",
                            annotation_inline(annotations_for(
                                &self.annotations,
                                &AnnotationTarget::Variant {
                                    declaration: fact.origin.clone(),
                                    index,
                                },
                            ))?,
                            rdl_ident(&variant.name),
                            enum_value(variant.value, repr)
                        ));
                    }
                    item.push_str("    }\n");
                    item
                }
                FactData::Record {
                    fields,
                    packing,
                    alignment,
                    union,
                    ..
                } => {
                    let projection = TypeProjection::new(
                        &plan.type_names,
                        &plan.interface_names,
                        &fact.origin.tu,
                        &local_types,
                        &routed_types,
                        (&plan.type_choices, plan.sal_constant_sizes),
                        namespace,
                    );
                    let item = write_named_record(
                        &rdl_ident(output_name),
                        fields,
                        *packing,
                        *alignment,
                        *union,
                        &projection,
                        Some((&self.annotations, &fact.origin)),
                    )?;
                    format!(
                        "{}{}{item}",
                        annotation_lines(
                            annotations_for(
                                &self.annotations,
                                &AnnotationTarget::Declaration(fact.origin.clone()),
                            ),
                            "    ",
                        )?,
                        self.fact_uuid(fact).map_or_else(String::new, |guid| {
                            format!("    #[guid({})]\n", rdl_uuid(guid))
                        }),
                    )
                }
                FactData::Interface {
                    base,
                    guid,
                    methods,
                } => {
                    let projection = TypeProjection::new(
                        &plan.type_names,
                        &plan.interface_names,
                        &fact.origin.tu,
                        &local_types,
                        &routed_types,
                        (&plan.type_choices, plan.sal_constant_sizes),
                        namespace,
                    );
                    write_interface(
                        &rdl_ident(output_name),
                        base.as_ref(),
                        guid.as_deref().or_else(|| {
                            plan.interface_guids.get(&planned.name).map(String::as_str)
                        }),
                        methods,
                        &projection,
                        &self.annotations,
                        &fact.origin,
                    )?
                }
                _ => {
                    return Err(Error(format!(
                        "planned type `{}` is not emittable",
                        fact.name
                    )));
                }
            };
            if kind == OutputKind::Type
                && let Some(notes) = plan.definition_notes.get(&fact.origin)
            {
                item.insert_str(0, notes);
            }
            if items
                .insert(
                    (planned.name.clone(), kind),
                    (fact.spelling.file.clone(), item),
                )
                .is_some()
            {
                return Err(Error(format!("duplicate planned name `{}`", planned.name)));
            }
        }
        for planned in plan.functions {
            phase.visit(None);
            let function = planned.fact;
            let ambiguity_note = planned
                .ambiguity
                .as_ref()
                .map(|ambiguity| function_ambiguity_note(&planned.name, ambiguity))
                .unwrap_or_default();
            let output_name = display_names
                .and_then(|names| names.get(&planned.name))
                .unwrap_or(&planned.name);
            let FactData::Function {
                link_name: _,
                convention,
                params,
                result,
                variadic,
                noreturn,
            } = &function.data
            else {
                return Err(Error(format!(
                    "planned function `{}` has no signature",
                    function.name
                )));
            };
            let namespace = routes
                .and_then(|routes| routes.get(&(planned.name.clone(), OutputKind::Value)))
                .map(|owner| owner.namespace.as_str());
            let projection = TypeProjection::new(
                &plan.type_names,
                &plan.interface_names,
                &function.origin.tu,
                &local_types,
                &routed_types,
                (&plan.type_choices, plan.sal_constant_sizes),
                namespace,
            );
            let callable = CallableTarget::Function(&function.origin);
            let mut params =
                write_params(params, &projection, &self.function_annotations, callable)?;
            if *variadic {
                params.push("...".to_string());
            }
            let params = params.join(", ");
            let result = if *result == TypeRef::Void {
                String::new()
            } else {
                let annotations = annotations_for(
                    &self.function_annotations,
                    &AnnotationTarget::Return(function.origin.clone()),
                );
                format!(
                    " -> {}",
                    projection
                        .use_type(result, annotations)
                        .annotated_name(annotations)?
                )
            };
            let declaration_annotations = annotations_for(
                &self.function_annotations,
                &AnnotationTarget::Declaration(function.origin.clone()),
            );
            let route =
                routes.and_then(|routes| routes.get(&(planned.name.clone(), OutputKind::Value)));
            let native_import = self
                .resolve_native_import(function, route, options)?
                .ok_or_else(|| {
                    Error(format!(
                        "function `{}` requires an import library",
                        function.name
                    ))
                })?;
            let abi = calling_convention(*convention);
            let set_last_error = declaration_annotations.contains(&Annotation::SetLastError);
            let import_name = native_import.metadata_name();
            let library = native_import.library();
            let library = if planned.name == import_name.as_ref() {
                format!(
                    "#[library({library:?}{})]",
                    if set_last_error {
                        ", set_last_error"
                    } else {
                        ""
                    }
                )
            } else {
                format!(
                    "#[library({library:?}, import = {import_name:?}{})]",
                    if set_last_error {
                        ", set_last_error"
                    } else {
                        ""
                    }
                )
            };
            let item = format!(
                "{ambiguity_note}{}{}    {library}\n    extern{abi} fn {}({params}){result};\n",
                annotation_lines(declaration_annotations, "    ")?,
                if *noreturn { "    #[noreturn]\n" } else { "" },
                rdl_ident(output_name),
            );
            if items
                .insert(
                    (planned.name.clone(), OutputKind::Value),
                    (function.spelling.file.clone(), item),
                )
                .is_some()
            {
                return Err(Error(format!("duplicate planned name `{}`", planned.name)));
            }
        }
        for planned in plan.constants {
            phase.visit(None);
            let constant = planned.constant;
            let output_name = display_names
                .and_then(|names| names.get(&constant.name))
                .unwrap_or(&constant.name);
            let namespace = routes
                .and_then(|routes| routes.get(&(constant.name.clone(), OutputKind::Value)))
                .map(|owner| owner.namespace.as_str());
            let projection = TypeProjection::new(
                &plan.type_names,
                &plan.interface_names,
                &constant.root.tu,
                &local_types,
                &routed_types,
                (&plan.type_choices, plan.sal_constant_sizes),
                namespace,
            );
            let ty = if routes.is_some()
                && !matches!(constant.value, Value::Utf8(_) | Value::Utf16(_))
            {
                constant_type_name(&constant.ty, &plan.pointer_interface_aliases, &projection)
                    .unwrap_or_else(|| planned.ty.clone())
            } else {
                planned.ty.clone()
            };
            let encoding = match &constant.value {
                Value::Utf8(_) => "    #[encoding(\"ansi\")]\n",
                Value::Utf16(_) => "    #[encoding(\"utf-16\")]\n",
                _ => "",
            };
            let item = format!(
                "{}{encoding}    const {}: {} = {};\n",
                annotation_lines(
                    annotations_for(
                        &self.annotations,
                        &AnnotationTarget::Declaration(constant.root.clone()),
                    ),
                    "    ",
                )?,
                rdl_ident(output_name),
                ty,
                value_name(&constant.value)
            );
            if items
                .insert(
                    (constant.name.clone(), OutputKind::Value),
                    (constant.spelling.file.clone(), item),
                )
                .is_some()
            {
                return Err(Error(format!("duplicate planned name `{}`", constant.name)));
            }
        }
        if timing {
            let headers = items
                .values()
                .map(|(header, _)| header.as_str())
                .collect::<BTreeSet<_>>()
                .len();
            eprintln!(
                "windows-clang timing phase=rdl-items target={target} items={} headers={headers} bytes={} elapsed_ms={:.3}",
                items.len(),
                items.values().map(|(_, item)| item.len()).sum::<usize>(),
                elapsed_ms(emission_time)
            );
        }
        Ok(items)
    }

    fn apply_header_partition_policy(
        &mut self,
        policy: &HeaderPartitionPolicy,
    ) -> Result<Vec<PartitionConflict>, Error> {
        policy.validate_partition_authorities()?;
        let ranked = !policy.partition_authorities.is_empty();
        self.root_owners.clear();
        self.constant_root_owners.clear();
        self.root_partitions.clear();
        self.header_authorities.clear();
        self.partition_authorities
            .clone_from(&policy.partition_authorities);
        self.partition_inputs.clear();
        self.partition_exclusions.clear();
        self.forced_flags.clear();
        self.suppressed_type_origins.clear();
        self.header_partition_policy = true;
        self.ranked_header_partition_policy = ranked;
        self.header_authority_partition
            .clone_from(&policy.authority_partition);
        if self
            .header_authority_partition
            .as_deref()
            .is_some_and(|partition| partition.trim().is_empty())
        {
            return Err(Error(
                "header authority partition identity is empty".to_string(),
            ));
        }

        let translation_units: BTreeSet<_> = self
            .facts
            .iter()
            .map(|fact| fact.origin.tu.clone())
            .chain(
                self.constants
                    .iter()
                    .map(|constant| constant.root.tu.clone()),
            )
            .chain(self.input_order.keys().filter(|_| ranked).cloned())
            .collect();
        self.partition_inputs
            .extend(translation_units.iter().map(|tu| (tu.clone(), tu.clone())));
        let physical_sources: BTreeSet<_> = self
            .facts
            .iter()
            .flat_map(|fact| {
                [
                    (fact.origin.tu.clone(), fact.spelling.file.clone()),
                    (fact.origin.tu.clone(), fact.expansion.file.clone()),
                ]
            })
            .chain(
                self.constants
                    .iter()
                    .map(|constant| (constant.root.tu.clone(), constant.spelling.file.clone())),
            )
            .chain(
                self.included_files
                    .iter()
                    .filter(|_| ranked)
                    .map(|included| (included.input.clone(), included.path.clone())),
            )
            .collect();
        let contexts: BTreeSet<_> = physical_sources
            .iter()
            .map(|(tu, file)| {
                (
                    normalize_name(tu),
                    normalize_name(file).to_ascii_lowercase(),
                )
            })
            .collect();
        let mut lookup = HeaderPartitionLookup::new(policy);
        for (tu, file) in physical_sources {
            let owners = lookup.owners(&tu, &file);
            if ranked && !owners.is_empty() {
                let (owner, _) = policy.resolve_owner(&file, None, owners.clone(), None)?;
                let context_present = self.input_order.contains_key(&owner.input)
                    && contexts.contains(&(
                        owner.input.clone(),
                        normalize_name(&owner.root).to_ascii_lowercase(),
                    ));
                self.header_authorities.insert(
                    normalize_name(&file).to_ascii_lowercase(),
                    HeaderAuthority {
                        owner: owner.clone(),
                        alternatives: owners,
                        context_present,
                    },
                );
                self.root_partitions
                    .insert((owner.input.clone(), file), owner);
            } else if owners.len() == 1 {
                self.root_partitions
                    .insert((tu, file), owners.first().unwrap().clone());
            }
        }

        let mut conflicts = Vec::new();
        for fact in &mut self.facts {
            fact.root = false;
            if ranked {
                let Some(authority) = fact_header_authority(&self.header_authorities, fact) else {
                    continue;
                };
                if fact.origin.tu != authority.owner.input {
                    continue;
                }
                let namespace = self
                    .namespace_authorities
                    .get(&fact.name)
                    .map(String::as_str);
                let mut owner = lookup.refine_owner(&authority.owner, &fact.name, namespace)?;
                owner.input.clone_from(&fact.origin.tu);
                if authority.alternatives.len() > 1
                    && let Some(kind) = fact_partition_item_kind(fact)
                {
                    conflicts.push(PartitionConflict {
                        name: fact.name.clone(),
                        kind,
                        reason: PartitionConflictReason::AuthoritySelected,
                        owners: authority.alternatives.iter().cloned().collect(),
                    });
                }
                if !owner_excludes_fact(&owner, fact)
                    && !self.observation_only_class_origins.contains(&fact.origin)
                {
                    fact.root = true;
                }
                self.root_owners.insert(fact.origin.clone(), owner);
                continue;
            }
            let candidates = header_fact_owner_candidates(&mut lookup, fact);
            if candidates.is_empty() {
                continue;
            }
            let retained: BTreeSet<_> = candidates
                .iter()
                .filter(|owner| !owner_excludes_fact(owner, fact))
                .cloned()
                .collect();
            let excluded = retained.is_empty();
            let selected = if excluded { candidates } else { retained };
            let kind = fact_partition_item_kind(fact);
            let namespace = self
                .namespace_authorities
                .get(&fact.name)
                .map(String::as_str);
            let (owner, conflict) = resolve_header_owner(&fact.name, kind, selected, namespace)?;
            if !excluded && let Some(conflict) = conflict {
                conflicts.push(conflict);
            }
            self.root_owners.insert(fact.origin.clone(), owner);
            if !excluded && !self.observation_only_class_origins.contains(&fact.origin) {
                fact.root = true;
            }
        }

        let mut associated_constant_owners: BTreeMap<String, BTreeSet<RootOwner>> = BTreeMap::new();
        let retained_origins: BTreeSet<_> = self
            .facts
            .iter()
            .filter(|fact| fact.root)
            .map(|fact| fact.origin.clone())
            .collect();
        let selected_annotations = self.annotations_with_selected_functions();
        let mut associated_enum_annotations: BTreeMap<String, BTreeSet<AnnotationTarget>> =
            BTreeMap::new();
        for (target, annotations) in &selected_annotations {
            for annotation in annotations {
                if let Annotation::AssociatedEnum(name) = annotation {
                    associated_enum_annotations
                        .entry(name.clone())
                        .or_default()
                        .insert(target.clone());
                }
            }
        }
        let mut associated_enum_remaps = BTreeMap::new();
        if !associated_enum_annotations.is_empty() {
            let facts_by_origin: HashMap<_, _> =
                self.facts.iter().map(|fact| (&fact.origin, fact)).collect();
            let facts_by_declaration: BTreeMap<_, _> = self
                .facts
                .iter()
                .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
                .collect();
            let requested_associated_enums: BTreeSet<_> = associated_enum_annotations
                .keys()
                .map(String::as_str)
                .collect();
            let mut associated_enum_providers: BTreeMap<(String, String), BTreeSet<Origin>> =
                BTreeMap::new();
            for fact in &self.facts {
                if !requested_associated_enums.contains(fact.name.as_str()) {
                    continue;
                }
                let Some(enumeration) =
                    underlying_enum_fact(fact, &facts_by_declaration, &mut BTreeSet::new())
                else {
                    continue;
                };
                let providers = associated_enum_providers
                    .entry((fact.origin.tu.clone(), fact.name.clone()))
                    .or_default();
                providers.insert(fact.origin.clone());
                providers.insert(enumeration.origin.clone());
            }
            let mut traversed_associated_enum_providers: BTreeMap<&str, Vec<&Fact>> =
                BTreeMap::new();
            for ((_, name), providers) in &associated_enum_providers {
                traversed_associated_enum_providers
                    .entry(name)
                    .or_default()
                    .extend(
                        providers
                            .iter()
                            .filter(|origin| retained_origins.contains(origin))
                            .map(|origin| facts_by_origin[origin]),
                    );
            }
            let mut associated_enum_routes: BTreeMap<
                String,
                (
                    BTreeSet<Origin>,
                    BTreeSet<RootOwner>,
                    BTreeSet<AnnotationTarget>,
                ),
            > = BTreeMap::new();
            for (name, targets) in &associated_enum_annotations {
                for target in targets {
                    let origin = target.origin();
                    if !retained_origins.contains(origin) {
                        continue;
                    }
                    let Some(owner) = self.root_owners.get(origin) else {
                        continue;
                    };
                    let Some(fact) = facts_by_origin.get(origin) else {
                        continue;
                    };
                    let target_slot = route_annotation_target(target).1;
                    let mut source_tus = BTreeSet::new();
                    let mut route_targets = BTreeSet::new();
                    for candidate_target in targets {
                        if route_annotation_target(candidate_target).1 != target_slot {
                            continue;
                        }
                        let candidate_origin = candidate_target.origin();
                        if ranked && candidate_origin.tu != origin.tu {
                            continue;
                        }
                        let Some(candidate) = facts_by_origin.get(candidate_origin) else {
                            continue;
                        };
                        if extract::annotation_declarations_compatible(
                            fact,
                            candidate,
                            &facts_by_origin,
                        ) {
                            source_tus.insert(candidate_origin.tu.clone());
                            route_targets.insert(candidate_target.clone());
                        }
                    }
                    let routed_names: BTreeSet<_> = source_tus
                        .iter()
                        .flat_map(|tu| {
                            associated_enum_providers
                                .get(&(tu.clone(), name.clone()))
                                .into_iter()
                                .flatten()
                        })
                        .filter_map(|origin| {
                            let provider = facts_by_origin[origin];
                            let owner = self.root_owners.get(origin)?;
                            (provider.name == *name && !owner_excludes_fact(owner, provider))
                                .then(|| owner.remaps.get(name).unwrap_or(name).clone())
                        })
                        .collect();
                    if routed_names.len() > 1 {
                        return Err(Error(format!(
                            "associated enum `{name}` has conflicting routed provider remaps"
                        )));
                    }
                    if let Some(remap) = routed_names.first().filter(|remap| *remap != name) {
                        for target in &route_targets {
                            associated_enum_remaps
                                .insert((target.clone(), name.clone()), remap.clone());
                        }
                    }
                    let providers: BTreeSet<_> = source_tus
                        .into_iter()
                        .flat_map(|tu| {
                            associated_enum_providers
                                .get(&(tu, name.clone()))
                                .into_iter()
                                .flatten()
                                .cloned()
                        })
                        .filter(|provider| {
                            !traversed_associated_enum_providers
                                .get(name.as_str())
                                .is_some_and(|traversed| {
                                    has_equivalent_source_provider(
                                        facts_by_origin[provider],
                                        traversed,
                                    )
                                })
                        })
                        .collect();
                    // An equivalent provider traversed in any extraction input keeps its route.
                    if providers.is_empty() {
                        continue;
                    }
                    let route = associated_enum_routes.entry(name.clone()).or_default();
                    route.0.extend(providers);
                    route.1.insert(owner.clone());
                    route.2.extend(route_targets);
                }
            }
            for (name, (providers, mut candidates, targets)) in associated_enum_routes {
                candidates.retain(|owner| !owner.exclusions.contains(&name));
                if candidates.is_empty() {
                    continue;
                }
                let namespace = self.namespace_authorities.get(&name).map(String::as_str);
                let authority_resolves = namespace.is_some_and(|namespace| {
                    equivalent_associated_enum_owner_policies(&name, &candidates, namespace)
                });
                let (owner, mut conflict) = resolve_header_owner(
                    &name,
                    Some(PartitionItemKind::Type),
                    candidates,
                    namespace,
                )?;
                if authority_resolves {
                    conflict = None;
                }
                if let Some(conflict) = conflict {
                    conflicts.push(conflict);
                }
                if let Some(remap) = owner.remaps.get(&name) {
                    for target in targets {
                        associated_enum_remaps.insert((target, name.clone()), remap.clone());
                    }
                }
                for origin in providers {
                    self.root_owners.insert(origin, owner.clone());
                }
            }
        }
        if !associated_enum_remaps.is_empty() {
            for annotation_map in [&mut self.annotations, &mut self.function_annotations] {
                for (target, annotations) in annotation_map {
                    for annotation in annotations {
                        let Annotation::AssociatedEnum(name) = annotation else {
                            continue;
                        };
                        let Some(remap) =
                            associated_enum_remaps.get(&(target.clone(), name.clone()))
                        else {
                            continue;
                        };
                        name.clone_from(remap);
                    }
                }
            }
        }

        let selected_annotations = self.annotations_with_selected_functions();
        for (target, annotations) in &selected_annotations {
            let AnnotationTarget::Declaration(origin) = target else {
                continue;
            };
            if !retained_origins.contains(origin) {
                continue;
            }
            let Some(owner) = self.root_owners.get(origin) else {
                continue;
            };
            for annotation in annotations {
                if let Annotation::AssociatedConstant(name) = annotation {
                    associated_constant_owners
                        .entry(name.clone())
                        .or_default()
                        .insert(owner.clone());
                }
            }
        }
        let facts_by_origin: BTreeMap<_, _> = self
            .facts
            .iter()
            .map(|fact| (fact.origin.clone(), fact))
            .collect();
        let mut constants = Vec::with_capacity(self.constants.len());
        for constant in std::mem::take(&mut self.constants) {
            if ranked {
                let authority = facts_by_origin
                    .get(&constant.root)
                    .and_then(|fact| {
                        self.header_authorities
                            .get(&normalize_name(&fact.expansion.file).to_ascii_lowercase())
                    })
                    .or_else(|| {
                        self.header_authorities
                            .get(&normalize_name(&constant.spelling.file).to_ascii_lowercase())
                    });
                if let Some(authority) = authority
                    && constant.root.tu == authority.owner.input
                {
                    let namespace = self
                        .constant_namespace_authorities
                        .get(&(constant.definition.clone(), constant.name.clone()))
                        .map(String::as_str);
                    let mut owner =
                        lookup.refine_owner(&authority.owner, &constant.name, namespace)?;
                    owner.input.clone_from(&constant.root.tu);
                    if !owner.exclusions.contains(&constant.name) {
                        self.constant_root_owners
                            .insert((constant.root.clone(), constant.name.clone()), owner);
                    }
                }
                constants.push(constant);
                continue;
            }
            let mut candidates = associated_constant_owners
                .get(&constant.name)
                .cloned()
                .unwrap_or_else(|| {
                    if let Some(fact) = facts_by_origin.get(&constant.root) {
                        header_fact_owner_candidates(&mut lookup, fact)
                    } else {
                        lookup.named_owners(
                            &constant.root.tu,
                            &constant.spelling.file,
                            &constant.name,
                        )
                    }
                });
            candidates.retain(|owner| !owner.exclusions.contains(&constant.name));
            if candidates.is_empty() {
                continue;
            }
            let namespace = self
                .namespace_authorities
                .get(&constant.name)
                .map(String::as_str);
            let (owner, conflict) = resolve_header_owner(
                &constant.name,
                Some(PartitionItemKind::Value),
                candidates,
                namespace,
            )?;
            if let Some(conflict) = conflict {
                conflicts.push(conflict);
            }
            self.constant_root_owners
                .insert((constant.root.clone(), constant.name.clone()), owner);
            constants.push(constant);
        }
        self.constants = constants;
        conflicts.sort();
        conflicts.dedup();
        Ok(conflicts)
    }

    fn require_fact_header_authority(&self, name: &str, fact: &Fact) -> Result<(), Error> {
        let Some(authority) = fact_header_authority(&self.header_authorities, fact) else {
            return Ok(());
        };
        let owner = &authority.owner;
        if owner.exclusions.contains(name) {
            return Err(Error(format!(
                "required `{name}` is excluded by authoritative partition {:?}, \
                 original input {:?}, header {:?}; no alternative context is eligible",
                owner.partition, owner.input, owner.root
            )));
        }
        if authority.context_present {
            return Ok(());
        }
        Err(Error(format!(
            "RequiredAuthorityContextAbsent: required `{name}`, partition {:?}, \
             original input {:?}, header {:?}",
            owner.partition, owner.input, owner.root
        )))
    }

    fn require_declaration_header_authority(
        &self,
        tu: &str,
        name: &str,
        declaration: &Location,
        declarations: &DeclarationIndex<'_>,
    ) -> Result<(), Error> {
        if self.ranked_header_partition_policy {
            for fact in declarations
                .get(tu, name, declaration)
                .iter()
                .filter(|fact| is_type_declaration_fact(fact))
            {
                self.require_fact_header_authority(name, fact)?;
            }
        }
        Ok(())
    }

    fn into_partitioned_planning_snapshot(
        mut self,
        options: &EmitOptions<'_>,
    ) -> (Self, BTreeMap<String, String>, PlanningSourceNames) {
        let source_names = PlanningSourceNames::new(&self);
        self.promote_selected_pointer_class_layouts(options);
        self.apply_partition_type_settings();
        self.apply_partition_exclusions();
        self.apply_partition_remaps();
        let declarations = DeclarationIndex::new(&self.facts);
        let rooted_fact_namespaces: BTreeMap<_, _> = self
            .facts
            .iter()
            .filter(|fact| fact.root)
            .filter_map(|fact| {
                let owner = self.root_owners.get(&fact.origin)?;
                let namespace = self
                    .fact_authority_namespace(fact, &declarations)
                    .unwrap_or(&owner.namespace);
                Some((fact.origin.clone(), namespace.clone()))
            })
            .collect();
        let retained_canonical_pointer_declarations = {
            let mut candidates = BTreeSet::new();
            let callback_requirements = callback_dependency_pointer_alias_requirements(
                &declarations,
                self.facts.iter().filter(|fact| {
                    matches!(fact.data, FactData::Function { .. })
                        && pointer_alias_root_is_selected(fact, options)
                }),
            );
            for fact in self
                .facts
                .iter()
                .filter(|fact| pointer_alias_root_is_selected(fact, options))
            {
                collect_fact_pointer_alias_candidates(
                    fact,
                    &callback_requirements,
                    &mut candidates,
                );
            }
            candidates
                .into_iter()
                .map(|(tu, declaration, name)| {
                    (tu.to_string(), declaration.clone(), name.to_string())
                })
                .collect::<BTreeSet<_>>()
        };
        let mut variants: BTreeMap<&str, BTreeMap<String, Vec<&Fact>>> = BTreeMap::new();
        for fact in self
            .facts
            .iter()
            .filter(|fact| fact.root && partition_collision_symbol(fact))
        {
            let Some(namespace) = rooted_fact_namespaces.get(&fact.origin) else {
                continue;
            };
            variants
                .entry(&fact.name)
                .or_default()
                .entry(namespace.clone())
                .or_default()
                .push(fact);
        }
        let canonical_pointer_collisions: BTreeSet<_> = variants
            .iter()
            .filter(|(name, namespaces)| {
                namespaces.len() > 1
                    && canonical_raw_pointer_name(name)
                    && namespaces
                        .values()
                        .flatten()
                        .all(|fact| matches!(fact.data, FactData::Typedef { .. }))
                    && namespaces.values().flatten().any(|fact| {
                        retained_canonical_pointer_declarations.contains(&(
                            fact.origin.tu.clone(),
                            fact.spelling.clone(),
                            fact.name.clone(),
                        ))
                    })
            })
            .map(|(name, _)| (*name).to_string())
            .collect();
        let collisions: BTreeSet<_> = variants
            .into_iter()
            .filter_map(|(name, namespaces)| {
                let all_typedefs = namespaces
                    .values()
                    .flatten()
                    .all(|fact| matches!(fact.data, FactData::Typedef { .. }));
                let canonical_typedef = canonical_named_type(name).is_some() && all_typedefs;
                if namespaces.len() <= 1
                    || (canonical_typedef && !canonical_pointer_collisions.contains(name))
                {
                    return None;
                }
                let mut source_namespaces: BTreeMap<&Location, BTreeSet<&str>> = BTreeMap::new();
                for (namespace, facts) in &namespaces {
                    for fact in facts {
                        source_namespaces
                            .entry(partition_declaration_location(fact))
                            .or_default()
                            .insert(namespace);
                    }
                }
                let independent_header_routes = self.header_partition_policy
                    && source_namespaces.len() > 1
                    && source_namespaces
                        .values()
                        .all(|namespaces| namespaces.len() == 1);
                if independent_header_routes {
                    return Some(name.to_string());
                }
                let distinct: BTreeSet<_> = namespaces
                    .values()
                    .flatten()
                    .map(|fact| &fact.data)
                    .collect();
                if distinct.len() <= 1 {
                    return None;
                }
                let equivalent_typedefs = all_typedefs
                    && namespaces
                        .values()
                        .flatten()
                        .filter_map(|fact| partition_collision_typedef_target(fact, &declarations))
                        .collect::<BTreeSet<_>>()
                        .len()
                        == 1;
                (!equivalent_typedefs).then_some(name.to_string())
            })
            .collect();
        if collisions.is_empty() {
            return (self, BTreeMap::new(), source_names);
        }
        let scoped_collision_fact = |fact: &Fact| {
            !canonical_pointer_collisions.contains(&fact.name)
                || retained_canonical_pointer_declarations.contains(&(
                    fact.origin.tu.clone(),
                    fact.spelling.clone(),
                    fact.name.clone(),
                ))
        };

        let mut source_namespaces: BTreeMap<(Location, String), BTreeSet<String>> = BTreeMap::new();
        for fact in self
            .facts
            .iter()
            .filter(|fact| partition_collision_symbol(fact) && collisions.contains(&fact.name))
            .filter(|fact| scoped_collision_fact(fact))
        {
            if let Some(namespace) = rooted_fact_namespaces.get(&fact.origin) {
                source_namespaces
                    .entry((
                        partition_declaration_location(fact).clone(),
                        fact.name.clone(),
                    ))
                    .or_default()
                    .insert(namespace.clone());
            }
        }
        let rooted_collision_facts: BTreeMap<&str, Vec<&Fact>> = self
            .facts
            .iter()
            .filter(|fact| partition_collision_symbol(fact) && collisions.contains(&fact.name))
            .filter(|fact| scoped_collision_fact(fact))
            .filter(|fact| rooted_fact_namespaces.contains_key(&fact.origin))
            .fold(BTreeMap::new(), |mut facts, fact| {
                facts.entry(fact.name.as_str()).or_default().push(fact);
                facts
            });
        let fact_namespaces: BTreeMap<_, _> = self
            .facts
            .iter()
            .filter(|fact| partition_collision_symbol(fact) && collisions.contains(&fact.name))
            .filter(|fact| scoped_collision_fact(fact))
            .filter_map(|fact| {
                let namespace = rooted_fact_namespaces
                    .get(&fact.origin)
                    .cloned()
                    .or_else(|| {
                        let namespaces = source_namespaces.get(&(
                            partition_declaration_location(fact).clone(),
                            fact.name.clone(),
                        ))?;
                        (namespaces.len() == 1).then(|| namespaces.first().unwrap().clone())
                    })
                    .or_else(|| {
                        if !self.suppressed_type_origins.contains(&fact.origin) {
                            return None;
                        }
                        let candidates = rooted_collision_facts.get(fact.name.as_str())?;
                        if candidates.is_empty()
                            || candidates.iter().any(|candidate| {
                                !partition_collision_equivalent(fact, candidate, &declarations)
                            })
                        {
                            return None;
                        }
                        rooted_fact_namespaces
                            .get(&preferred_fact(candidates).origin)
                            .cloned()
                    })?;
                Some((fact.origin.clone(), namespace))
            })
            .collect();
        if self.header_partition_policy {
            let mut namespace_facts: BTreeMap<(String, String), Vec<&Fact>> = BTreeMap::new();
            for fact in self
                .facts
                .iter()
                .filter(|fact| fact.root && partition_collision_symbol(fact))
            {
                let Some(namespace) = rooted_fact_namespaces.get(&fact.origin) else {
                    continue;
                };
                namespace_facts
                    .entry((fact.name.clone(), namespace.clone()))
                    .or_default()
                    .push(fact);
            }
            let noncanonical_roots: BTreeSet<_> = namespace_facts
                .values()
                .filter(|facts| {
                    facts.len() > 1
                        && facts.iter().all(|fact| {
                            partition_collision_equivalent(facts[0], fact, &declarations)
                        })
                })
                .flat_map(|facts| {
                    let canonical = facts
                        .iter()
                        .copied()
                        .min_by_key(|fact| {
                            (
                                self.root_owners.get(&fact.origin).unwrap(),
                                partition_declaration_location(fact),
                                &fact.origin,
                            )
                        })
                        .unwrap();
                    facts
                        .iter()
                        .copied()
                        .filter(move |fact| fact.origin != canonical.origin)
                        .map(|fact| fact.origin.clone())
                })
                .collect();
            for fact in &mut self.facts {
                if noncanonical_roots.contains(&fact.origin) {
                    fact.root = false;
                }
            }
        }
        let mut scoped_names = BTreeMap::new();
        let mut display_names = BTreeMap::new();
        for (index, (name, namespace)) in self
            .facts
            .iter()
            .filter(|fact| partition_collision_symbol(fact) && collisions.contains(&fact.name))
            .filter_map(|fact| {
                fact_namespaces
                    .get(&fact.origin)
                    .map(|namespace| (fact.name.clone(), namespace.clone()))
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .enumerate()
        {
            let scoped = format!("__partition_{index}_{name}");
            display_names.insert(scoped.clone(), name.clone());
            scoped_names.insert((name, namespace), scoped);
        }

        let mut declarations = ScopedDeclarationIndex::new(&self.facts, &collisions);
        for fact in &self.facts {
            let Some(namespace) = fact_namespaces.get(&fact.origin) else {
                continue;
            };
            let Some(scoped) = scoped_names.get(&(fact.name.clone(), namespace.clone())) else {
                continue;
            };
            declarations.insert(fact, scoped.clone());
        }
        let projected_scoped_names: BTreeMap<_, _> = self
            .facts
            .iter()
            .filter_map(|fact| {
                let FactData::PropertyKey { ty, .. } = &fact.data else {
                    return None;
                };
                let projected = self
                    .projected_type_names
                    .get(&fact.origin)
                    .map_or(*ty, String::as_str);
                if !collisions.contains(projected) {
                    return None;
                }
                let candidates: BTreeSet<_> = self
                    .facts
                    .iter()
                    .filter(|candidate| {
                        candidate.origin.tu == fact.origin.tu && candidate.name == projected
                    })
                    .filter_map(|candidate| {
                        let namespace = fact_namespaces.get(&candidate.origin)?;
                        scoped_names
                            .get(&(projected.to_string(), namespace.clone()))
                            .cloned()
                    })
                    .collect();
                (candidates.len() == 1)
                    .then(|| (fact.origin.clone(), candidates.into_iter().next().unwrap()))
            })
            .collect();
        self.projected_type_names.extend(projected_scoped_names);

        for fact in &mut self.facts {
            let original = fact.name.clone();
            if let Some(namespace) = fact_namespaces.get(&fact.origin)
                && let Some(scoped) = scoped_names.get(&(original, namespace.clone()))
            {
                fact.name.clone_from(scoped);
            }
            rename_fact_types(&mut fact.data, &fact.origin.tu, &declarations, &collisions);
        }
        for constant in &mut self.constants {
            let original = constant.name.clone();
            let owner = self
                .constant_root_owners
                .get(&(constant.root.clone(), original.clone()))
                .or_else(|| self.root_owners.get(&constant.root));
            let namespace = owner.map(|owner| owner.namespace.clone());
            if let Some(namespace) = namespace
                && let Some(scoped) = scoped_names.get(&(original.clone(), namespace))
            {
                constant.name.clone_from(scoped);
                if let Some(owner) = self
                    .constant_root_owners
                    .remove(&(constant.root.clone(), original.clone()))
                {
                    self.constant_root_owners
                        .insert((constant.root.clone(), scoped.clone()), owner);
                }
                if let Some(namespace) = self
                    .constant_namespace_authorities
                    .remove(&(constant.definition.clone(), original))
                {
                    self.constant_namespace_authorities
                        .insert((constant.definition.clone(), scoped.clone()), namespace);
                }
            }
            rename_type_ref(
                &mut constant.ty,
                &constant.root.tu,
                &declarations,
                &collisions,
            );
        }
        (self, display_names, source_names)
    }

    fn promote_selected_pointer_class_layouts(&mut self, options: &EmitOptions<'_>) {
        let Some(selected) = options.functions else {
            return;
        };
        if !self.header_partition_policy || self.pointer_only_class_layouts.is_empty() {
            return;
        }

        let mut queue = Vec::new();
        for fact in self.facts.iter().filter(|fact| {
            fact.root
                && self.root_owners.contains_key(&fact.origin)
                && pointer_alias_root_is_selected(fact, options)
                && matches!(
                    &fact.data,
                    FactData::Function { link_name, .. } if selected.contains(link_name)
                )
        }) {
            queue_fact_type_refs(&fact.data, &fact.origin.tu, &mut queue);
        }

        let declarations = DeclarationIndex::new(&self.facts);
        let mut seen = BTreeSet::new();
        let mut promoted = BTreeSet::new();
        while let Some((tu, ty)) = queue.pop() {
            match ty {
                TypeRef::Pointer { target, .. }
                | TypeRef::Reference { target, .. }
                | TypeRef::Array { target, .. } => queue.push((tu, *target)),
                TypeRef::FunctionPointer { params, result, .. } => {
                    queue.push((tu.clone(), *result));
                    queue.extend(params.into_iter().map(|param| (tu.clone(), param)));
                }
                TypeRef::Generic { args, .. } => {
                    queue.extend(args.into_iter().map(|arg| (tu.clone(), arg)));
                }
                TypeRef::InlineRecord(record) => {
                    if let Some(base) = record.base {
                        queue.push((tu.clone(), base));
                    }
                    queue.extend(
                        record
                            .fields
                            .into_iter()
                            .map(|field| (tu.clone(), field.ty)),
                    );
                }
                TypeRef::Named { name, declaration } => {
                    if !seen.insert((tu.clone(), name.clone(), declaration.clone())) {
                        continue;
                    }
                    for fact in declarations.get(&tu, &name, &declaration) {
                        if let Some(layout) = self.pointer_only_class_layouts.get(&fact.origin)
                            && promoted.insert(fact.origin.clone())
                        {
                            queue_fact_type_refs(layout, &fact.origin.tu, &mut queue);
                        }
                        queue_fact_type_refs(&fact.data, &fact.origin.tu, &mut queue);
                    }
                }
                TypeRef::Void
                | TypeRef::String
                | TypeRef::Object
                | TypeRef::Scalar(_)
                | TypeRef::OpaquePointer { .. } => {}
            }
        }
        drop(declarations);

        for fact in &mut self.facts {
            if promoted.contains(&fact.origin) {
                fact.data = self.pointer_only_class_layouts[&fact.origin].clone();
            }
        }
    }

    fn apply_partition_remaps(&mut self) {
        let mut source_remaps: BTreeMap<(Location, String), BTreeSet<String>> = BTreeMap::new();
        for fact in &self.facts {
            if let Some(owner) = self.root_owners.get(&fact.origin)
                && let Some(target) = owner.remaps.get(&fact.name)
            {
                source_remaps
                    .entry((fact.spelling.clone(), fact.name.clone()))
                    .or_default()
                    .insert(target.clone());
            }
        }
        let fact_remaps: BTreeMap<_, _> = self
            .facts
            .iter()
            .filter_map(|fact| {
                let target = self
                    .root_owners
                    .get(&fact.origin)
                    .and_then(|owner| owner.remaps.get(&fact.name))
                    .cloned()
                    .or_else(|| {
                        let targets =
                            source_remaps.get(&(fact.spelling.clone(), fact.name.clone()))?;
                        (targets.len() == 1).then(|| targets.first().unwrap().clone())
                    })?;
                Some((fact.origin.clone(), target))
            })
            .collect();
        let declarations: BTreeMap<_, _> = self
            .facts
            .iter()
            .filter_map(|fact| {
                fact_remaps.get(&fact.origin).map(|target| {
                    (
                        (
                            fact.origin.tu.clone(),
                            fact.spelling.clone(),
                            fact.name.clone(),
                        ),
                        target.clone(),
                    )
                })
            })
            .collect();
        let mut projected_remaps: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
        for fact in &self.facts {
            if let Some(target) = fact_remaps.get(&fact.origin) {
                projected_remaps
                    .entry((fact.origin.tu.clone(), fact.name.clone()))
                    .or_default()
                    .insert(target.clone());
            }
        }
        for fact in &self.facts {
            let FactData::PropertyKey { ty, .. } = &fact.data else {
                continue;
            };
            let Some(targets) = projected_remaps.get(&(fact.origin.tu.clone(), (*ty).to_string()))
            else {
                continue;
            };
            if targets.len() == 1 {
                self.projected_type_names
                    .insert(fact.origin.clone(), targets.first().unwrap().clone());
            }
        }
        for fact in &mut self.facts {
            remap_fact_types(
                &mut fact.data,
                &fact.origin.tu,
                &declarations,
                &self.root_partitions,
            );
            if let Some(target) = fact_remaps.get(&fact.origin) {
                fact.name = target.clone();
            }
        }
        for constant in &mut self.constants {
            remap_type_ref(
                &mut constant.ty,
                &constant.root.tu,
                &declarations,
                &self.root_partitions,
            );
            let original = constant.name.clone();
            let owner = self
                .constant_root_owners
                .get(&(constant.root.clone(), original.clone()))
                .or_else(|| self.root_owners.get(&constant.root));
            let target = owner.and_then(|owner| owner.remaps.get(&constant.name).cloned());
            if let Some(target) = target {
                constant.name.clone_from(&target);
                if let Some(owner) = self
                    .constant_root_owners
                    .remove(&(constant.root.clone(), original.clone()))
                {
                    self.constant_root_owners
                        .insert((constant.root.clone(), target.clone()), owner);
                }
                if let Some(namespace) = self
                    .constant_namespace_authorities
                    .remove(&(constant.definition.clone(), original))
                {
                    self.constant_namespace_authorities
                        .insert((constant.definition.clone(), target), namespace);
                }
            }
        }
    }

    fn apply_partition_exclusions(&mut self) {
        let declaration_guids = &self.declaration_guids;
        self.facts.retain_mut(|fact| {
            let Some(owner) = self.root_owners.get(&fact.origin).or_else(|| {
                self.root_partitions
                    .get(&(fact.origin.tu.clone(), fact.spelling.file.clone()))
            }) else {
                return true;
            };
            let empty_record = owner.exclude_empty_records
                && matches!(&fact.data, FactData::Record { fields, .. } if fields.is_empty());
            if !owner.exclusions.contains(&fact.name) && !empty_record {
                return true;
            }
            self.partition_exclusions
                .push(ExcludedPartitionDeclaration {
                    name: fact.name.clone(),
                    origin: fact.origin.clone(),
                    spelling: fact.spelling.clone(),
                    kind: fact.kind,
                    data_kind: fact_data_kind(&fact.data),
                    uuid: fact_uuid(fact)
                        .or_else(|| declaration_guids.get(&fact.origin).map(String::as_str))
                        .map(str::to_string),
                    definition: fact.definition,
                    root: fact.root,
                    owner: owner.clone(),
                });
            if is_type_fact(fact) {
                fact.root = false;
                self.suppressed_type_origins.insert(fact.origin.clone());
                true
            } else {
                false
            }
        });
        self.constants.retain(|constant| {
            self.constant_root_owners
                .get(&(constant.root.clone(), constant.name.clone()))
                .or_else(|| self.root_owners.get(&constant.root))
                .or_else(|| {
                    self.root_partitions
                        .get(&(constant.root.tu.clone(), constant.spelling.file.clone()))
                })
                .is_none_or(|owner| !owner.exclusions.contains(&constant.name))
        });
    }

    fn project_suppressed_declare_handles(&mut self) {
        let projections: Vec<_> = self
            .declare_handles
            .iter()
            .filter(|handle| self.suppressed_type_origins.contains(&handle.record))
            .filter(|handle| !self.suppressed_type_origins.contains(&handle.alias))
            .filter_map(|handle| {
                let record = self
                    .facts
                    .iter()
                    .find(|fact| fact.origin == handle.record)?;
                Some((
                    handle.alias.clone(),
                    record.name.clone(),
                    record.spelling.clone(),
                ))
            })
            .collect();
        for (alias_origin, record_name, record_declaration) in projections {
            let Some(alias) = self
                .facts
                .iter_mut()
                .find(|fact| fact.origin == alias_origin)
            else {
                continue;
            };
            let handle_name = alias.name.clone();
            let FactData::Typedef { target } = &mut alias.data else {
                continue;
            };
            if !matches!(
                target,
                TypeRef::Pointer {
                    mutable: true,
                    target,
                } if matches!(
                    target.as_ref(),
                    TypeRef::Named { name, declaration }
                        if name == &record_name && declaration == &record_declaration
                )
            ) {
                continue;
            }
            *target = TypeRef::OpaquePointer {
                mutable: true,
                tag: handle_name,
            };
        }
    }

    fn apply_partition_type_settings(&mut self) {
        let u32_sources: BTreeSet<_> = self
            .facts
            .iter()
            .filter(|fact| {
                self.root_owners
                    .get(&fact.origin)
                    .or_else(|| {
                        self.root_partitions
                            .get(&(fact.origin.tu.clone(), fact.spelling.file.clone()))
                    })
                    .is_some_and(|owner| owner.u32_types.contains(&fact.name))
            })
            .map(|fact| (fact.spelling.clone(), fact.name.clone()))
            .collect();
        let mut pointer_callback_aliases = PointerCallbackAliases::new();
        for fact in self
            .facts
            .iter()
            .filter(|fact| self.pointer_callback_aliases.contains(&fact.origin))
        {
            pointer_callback_aliases
                .entry(fact.origin.tu.clone())
                .or_default()
                .entry(fact.spelling.clone())
                .or_default()
                .insert(fact.name.clone());
        }
        for fact in &mut self.facts {
            let owner = self.root_owners.get(&fact.origin).or_else(|| {
                self.root_partitions
                    .get(&(fact.origin.tu.clone(), fact.spelling.file.clone()))
            });
            let force_u32 = owner.is_some_and(|owner| owner.u32_types.contains(&fact.name))
                || (owner.is_none()
                    && u32_sources.contains(&(fact.spelling.clone(), fact.name.clone())));
            if force_u32 {
                fact.kind = FactKind::Typedef;
                fact.definition = true;
                fact.data = FactData::Typedef {
                    target: TypeRef::Scalar(Scalar::U32),
                };
            } else if let Some(owner) = owner {
                if owner.flags.contains(&fact.name) {
                    self.forced_flags.insert(fact.origin.clone());
                }
                preserve_auto_function_pointer_levels(
                    &mut fact.data,
                    &owner.preserved_auto_function_pointer_levels,
                    &fact.origin.tu,
                    &pointer_callback_aliases,
                );
            }
        }

        for constant in &mut self.constants {
            if let Some(owner) = self
                .constant_root_owners
                .get(&(constant.root.clone(), constant.name.clone()))
                .or_else(|| self.root_owners.get(&constant.root))
                .or_else(|| {
                    self.root_partitions
                        .get(&(constant.root.tu.clone(), constant.spelling.file.clone()))
                })
            {
                preserve_auto_function_pointer_level(
                    &mut constant.ty,
                    &owner.preserved_auto_function_pointer_levels,
                    &constant.root.tu,
                    &pointer_callback_aliases,
                );
            }
        }
    }

    fn authority_candidates<'a>(
        &self,
        facts: &[&'a Fact],
        declarations: &DeclarationIndex<'_>,
    ) -> Vec<&'a Fact> {
        let owned_sources: BTreeSet<_> = facts
            .iter()
            .filter(|fact| self.root_owners.contains_key(&fact.origin))
            .map(|fact| {
                (
                    fact.name.as_str(),
                    &fact.spelling,
                    fact.kind,
                    fact.definition,
                )
            })
            .collect();
        let facts: Vec<_> = facts
            .iter()
            .copied()
            .filter(|fact| {
                self.root_owners.contains_key(&fact.origin)
                    || !owned_sources.contains(&(
                        fact.name.as_str(),
                        &fact.spelling,
                        fact.kind,
                        fact.definition,
                    ))
            })
            .collect();
        let Some(namespace) = facts
            .iter()
            .find_map(|fact| self.fact_authority_namespace(fact, declarations))
        else {
            let projected: Vec<_> = facts
                .iter()
                .copied()
                .filter(|fact| {
                    !self.type_projection_suppressed(fact, declarations, &mut BTreeSet::new())
                })
                .collect();
            return if projected.is_empty() {
                facts
            } else {
                projected
            };
        };
        let matching: Vec<_> = facts
            .iter()
            .copied()
            .filter(|fact| {
                self.root_owners
                    .get(&fact.origin)
                    .or_else(|| {
                        self.root_partitions
                            .get(&(fact.origin.tu.clone(), fact.spelling.file.clone()))
                    })
                    .is_some_and(|owner| owner.namespace == *namespace)
            })
            .collect();
        if matching.is_empty() {
            facts.clone()
        } else {
            matching
        }
    }

    fn fact_authority_namespace<'a>(
        &'a self,
        fact: &Fact,
        declarations: &DeclarationIndex<'_>,
    ) -> Option<&'a String> {
        self.fact_namespace_authorities
            .get(&fact.origin)
            .or_else(|| {
                let FactData::Typedef {
                    target: TypeRef::Named { name, declaration },
                } = &fact.data
                else {
                    return None;
                };
                declarations
                    .get(&fact.origin.tu, name, declaration)
                    .iter()
                    .find_map(|target| self.fact_namespace_authorities.get(&target.origin))
            })
    }

    fn type_projection_suppressed(
        &self,
        fact: &Fact,
        declarations: &DeclarationIndex<'_>,
        seen: &mut BTreeSet<(String, Location)>,
    ) -> bool {
        if self.suppressed_type_origins.contains(&fact.origin) {
            return true;
        }
        let FactData::Typedef { target } = &fact.data else {
            return false;
        };
        self.type_ref_projection_suppressed(target, &fact.origin.tu, declarations, seen)
    }

    fn type_ref_projection_suppressed(
        &self,
        ty: &TypeRef,
        tu: &str,
        declarations: &DeclarationIndex<'_>,
        seen: &mut BTreeSet<(String, Location)>,
    ) -> bool {
        match ty {
            TypeRef::Named { name, declaration } => {
                if !seen.insert((tu.to_string(), declaration.clone())) {
                    return false;
                }
                declarations
                    .get(tu, name, declaration)
                    .iter()
                    .any(|fact| self.type_projection_suppressed(fact, declarations, seen))
            }
            TypeRef::Pointer { target, .. }
            | TypeRef::Reference { target, .. }
            | TypeRef::Array { target, .. } => {
                self.type_ref_projection_suppressed(target, tu, declarations, seen)
            }
            _ => false,
        }
    }

    fn route_projection_suppression(&self) -> HashMap<Origin, bool> {
        let facts: HashMap<_, _> = self
            .facts
            .iter()
            .map(|fact| (fact.origin.clone(), fact))
            .collect();
        let mut declarations: HashMap<(String, String, Location), Vec<Origin>> = HashMap::new();
        for fact in &self.facts {
            declarations
                .entry((
                    fact.origin.tu.clone(),
                    fact.name.clone(),
                    fact.spelling.clone(),
                ))
                .or_default()
                .push(fact.origin.clone());
        }
        let mut result = HashMap::new();
        for fact in self
            .facts
            .iter()
            .filter(|fact| self.root_owners.contains_key(&fact.origin))
        {
            self.route_fact_projection_suppressed(
                &fact.origin,
                &facts,
                &declarations,
                &mut result,
                &mut HashSet::new(),
            );
        }
        result
    }

    fn route_fact_projection_suppressed(
        &self,
        origin: &Origin,
        facts: &HashMap<Origin, &Fact>,
        declarations: &HashMap<(String, String, Location), Vec<Origin>>,
        memo: &mut HashMap<Origin, bool>,
        visiting: &mut HashSet<Origin>,
    ) -> bool {
        if let Some(&suppressed) = memo.get(origin) {
            return suppressed;
        }
        if self.suppressed_type_origins.contains(origin) {
            memo.insert(origin.clone(), true);
            return true;
        }
        if !visiting.insert(origin.clone()) {
            return false;
        }
        let suppressed = facts.get(origin).is_some_and(|fact| {
            let FactData::Typedef { target } = &fact.data else {
                return false;
            };
            self.route_type_ref_projection_suppressed(
                target,
                &fact.origin.tu,
                facts,
                declarations,
                memo,
                visiting,
            )
        });
        visiting.remove(origin);
        memo.insert(origin.clone(), suppressed);
        suppressed
    }

    fn route_type_ref_projection_suppressed(
        &self,
        ty: &TypeRef,
        tu: &str,
        facts: &HashMap<Origin, &Fact>,
        declarations: &HashMap<(String, String, Location), Vec<Origin>>,
        memo: &mut HashMap<Origin, bool>,
        visiting: &mut HashSet<Origin>,
    ) -> bool {
        match ty {
            TypeRef::Named { name, declaration } => declarations
                .get(&(tu.to_string(), name.clone(), declaration.clone()))
                .is_some_and(|origins| {
                    origins.iter().any(|origin| {
                        self.route_fact_projection_suppressed(
                            origin,
                            facts,
                            declarations,
                            memo,
                            visiting,
                        )
                    })
                }),
            TypeRef::Pointer { target, .. }
            | TypeRef::Reference { target, .. }
            | TypeRef::Array { target, .. } => self.route_type_ref_projection_suppressed(
                target,
                tu,
                facts,
                declarations,
                memo,
                visiting,
            ),
            _ => false,
        }
    }

    fn unresolved_local_type_error(&self, name: &str, tu: &str, declaration: &Location) -> Error {
        let normalized_declaration = normalize_name(&declaration.file);
        let declaration_owners: Vec<_> = self
            .root_partitions
            .iter()
            .filter(|((candidate_tu, root), _)| {
                candidate_tu == tu && normalize_name(root) == normalized_declaration
            })
            .map(|((_, root), owner)| format_owner(root, owner))
            .collect();
        let declaration_owners = if declaration_owners.is_empty() {
            "none".to_string()
        } else {
            declaration_owners.join("; ")
        };
        let mut candidates = Vec::new();
        for fact in self.facts.iter().filter(|fact| fact.name == name) {
            let owner = self.root_owners.get(&fact.origin).or_else(|| {
                self.root_partitions
                    .get(&(fact.origin.tu.clone(), fact.spelling.file.clone()))
            });
            let rejection = if let FactData::Unsupported { reason } = &fact.data {
                format!("unsupported declaration: {reason}")
            } else if matches!(fact.data, FactData::Class { .. }) {
                "classified as a coclass GUID value, not a type fact".to_string()
            } else if !is_type_fact(fact) {
                "not a type fact".to_string()
            } else if fact.origin.tu == tu && fact.spelling == *declaration {
                "exact declaration match was unexpectedly rejected".to_string()
            } else if fact.origin.tu == tu && fact.spelling.file == declaration.file {
                "same-header declaration was unexpectedly rejected".to_string()
            } else if let Some(owner) = owner {
                let matches_owner =
                    self.root_partitions
                        .iter()
                        .any(|((candidate_tu, root), declaration_owner)| {
                            candidate_tu == tu
                                && normalize_name(root) == normalized_declaration
                                && declaration_owner.partition == owner.partition
                                && declaration_owner.namespace == owner.namespace
                        });
                if matches_owner {
                    "same tagged partition/namespace was unexpectedly rejected".to_string()
                } else {
                    "tagged owner differs from declaration-path owner".to_string()
                }
            } else {
                "candidate has no tagged owner".to_string()
            };
            candidates.push(format!(
                "tu={:?} source={}:{} root={} owner={} uuid={} kind={:?}/{} definition={} rejection={}",
                fact.origin.tu,
                fact.spelling.file,
                fact.spelling.offset,
                fact.root,
                owner.map_or_else(|| "none".to_string(), |owner| {
                    format_owner(&owner.root, owner)
                }),
                self.fact_uuid(fact).unwrap_or("none"),
                fact.kind,
                fact_data_kind(&fact.data),
                fact.definition,
                rejection,
            ));
        }
        for excluded in self
            .partition_exclusions
            .iter()
            .filter(|excluded| excluded.name == name)
        {
            candidates.push(format!(
                "tu={:?} source={}:{} root={} owner={} uuid={} kind={:?}/{} definition={} rejection={}",
                excluded.origin.tu,
                excluded.spelling.file,
                excluded.spelling.offset,
                excluded.root,
                format_owner(&excluded.owner.root, &excluded.owner),
                excluded.uuid.as_deref().unwrap_or("none"),
                excluded.kind,
                excluded.data_kind,
                excluded.definition,
                "excluded by owner setting",
            ));
        }
        if candidates.is_empty() {
            candidates.push("none".to_string());
        }
        Error(format!(
            "unresolved local type `{name}` in translation unit `{tu}` at {}:{} \
             (normalized declaration path `{normalized_declaration}`); tagged declaration-path \
             owners: {declaration_owners}; same-name candidates:\n  {}",
            declaration.file,
            declaration.offset,
            candidates.join("\n  "),
        ))
    }

    fn resolve_partition_routes(
        &self,
        candidates: BTreeMap<(String, OutputKind), RouteCandidate<'_>>,
    ) -> Result<BTreeMap<(String, OutputKind), RootOwner>, Error> {
        candidates
            .into_iter()
            .map(|(key, candidate)| {
                let owner = if candidate.header_partition_policy {
                    resolve_equivalent_header_route(&candidate)
                        .map_err(|reason| route_candidate_error(&candidate, reason))?
                } else {
                    self.authoritative_owner(
                        &candidate.name,
                        candidate.kind,
                        candidate.owners(),
                        candidate.namespace.as_deref(),
                    )?
                };
                validate_namespace(&owner.namespace)?;
                if owner.partition.trim().is_empty() {
                    return Err(Error(format!(
                        "selected item `{}` has an empty partition identity",
                        candidate.name
                    )));
                }
                Ok((key, owner))
            })
            .collect()
    }

    fn partition_route_conflicts(
        &self,
        candidates: &BTreeMap<(String, OutputKind), RouteCandidate<'_>>,
    ) -> Vec<PartitionConflict> {
        let mut conflicts = candidates
            .values()
            .filter_map(route_candidate_conflict)
            .collect::<Vec<_>>();
        conflicts.sort();
        conflicts.dedup();
        conflicts
    }

    fn partition_route_candidates<'a>(
        &'a self,
        plan: &Plan<'a>,
        source_names: &PlanningSourceNames,
        options: &EmitOptions<'_>,
    ) -> Result<BTreeMap<(String, OutputKind), RouteCandidate<'a>>, Error> {
        let declarations = DeclarationIndex::new(&self.facts);
        let annotation_signatures = self.route_annotation_signatures();
        let function_annotation_signatures = self.function_route_annotation_signatures();
        let empty_annotations = Rc::new(Vec::new());
        let route_context = RouteClaimContext {
            annotations: &annotation_signatures,
            empty_annotations: &empty_annotations,
            flag_enums: &plan.flag_enums,
            options,
        };
        let projection_suppression = self
            .header_partition_policy
            .then(|| self.route_projection_suppression());
        let projection_suppressed = |fact: &Fact| {
            projection_suppression
                .as_ref()
                .is_some_and(|suppression| suppression[&fact.origin])
        };
        let projected_fact_keys: BTreeSet<_> = self
            .facts
            .iter()
            .filter(|fact| {
                self.root_owners.contains_key(&fact.origin) && !projection_suppressed(fact)
            })
            .map(|fact| (&fact.name, fact.kind, fact.definition, &fact.data))
            .collect();
        let projected_source_keys: BTreeSet<_> = self
            .facts
            .iter()
            .filter(|fact| {
                self.root_owners.contains_key(&fact.origin) && !projection_suppressed(fact)
            })
            .map(|fact| (&fact.name, fact.kind, fact.definition, &fact.spelling))
            .collect();
        let mut fact_claims: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        let mut source_fact_claims: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for fact in &self.facts {
            if let Some(owner) = self.root_owners.get(&fact.origin) {
                let fact_key = (&fact.name, fact.kind, fact.definition, &fact.data);
                let source_key = (&fact.name, fact.kind, fact.definition, &fact.spelling);
                let suppressed = projection_suppressed(fact);
                let annotations = annotation_signatures
                    .get(&fact.origin)
                    .cloned()
                    .unwrap_or_else(|| empty_annotations.clone());
                let claim = self.fact_route_claim(
                    fact,
                    fact,
                    owner,
                    annotations,
                    &plan.flag_enums,
                    options,
                )?;
                if !suppressed || !projected_fact_keys.contains(&fact_key) {
                    fact_claims
                        .entry(fact_key)
                        .or_default()
                        .insert(claim.clone());
                }
                if !suppressed || !projected_source_keys.contains(&source_key) {
                    source_fact_claims
                        .entry(source_key)
                        .or_default()
                        .insert(claim);
                }
            }
        }
        let mut result = BTreeMap::new();
        for planned in &plan.types {
            let canonical_typedef = canonical_named_type(&planned.fact.name).is_some()
                && matches!(planned.fact.data, FactData::Typedef { .. });
            let mut claims = if canonical_typedef {
                BTreeSet::new()
            } else {
                fact_claims
                    .get(&(
                        &planned.fact.name,
                        planned.fact.kind,
                        planned.fact.definition,
                        &planned.fact.data,
                    ))
                    .cloned()
                    .unwrap_or_default()
            };
            claims.extend(
                source_fact_claims
                    .get(&(
                        &planned.fact.name,
                        planned.fact.kind,
                        planned.fact.definition,
                        &planned.fact.spelling,
                    ))
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            let namespace = self
                .fact_authority_namespace(planned.fact, &declarations)
                .cloned();
            let key = (planned.name.clone(), OutputKind::Type);
            result.insert(
                key,
                self.fact_route_candidate(
                    planned.fact,
                    (
                        planned.name.clone(),
                        source_names.fact_name(planned.fact).to_string(),
                    ),
                    OutputKind::Type,
                    claims,
                    namespace,
                    &route_context,
                )?,
            );
        }
        for planned in &plan.values {
            let claims = fact_claims
                .get(&(
                    &planned.fact.name,
                    planned.fact.kind,
                    planned.fact.definition,
                    &planned.fact.data,
                ))
                .cloned()
                .unwrap_or_default();
            let namespace = self
                .fact_authority_namespace(planned.fact, &declarations)
                .cloned();
            let key = (planned.name.clone(), OutputKind::Value);
            result.insert(
                key,
                self.fact_route_candidate(
                    planned.fact,
                    (
                        planned.name.clone(),
                        source_names.fact_name(planned.fact).to_string(),
                    ),
                    OutputKind::Value,
                    claims,
                    namespace,
                    &route_context,
                )?,
            );
        }
        for planned in &plan.functions {
            let function = planned.fact;
            let selected_annotations = function_annotation_signatures
                .get(&function.origin)
                .cloned()
                .unwrap_or_else(|| empty_annotations.clone());
            let mut claims = BTreeSet::new();
            for &declaration in &planned.declarations {
                let Some(owner) = self.root_owners.get(&declaration.origin) else {
                    continue;
                };
                claims.insert(self.fact_route_claim(
                    function,
                    declaration,
                    owner,
                    selected_annotations.clone(),
                    &plan.flag_enums,
                    options,
                )?);
            }
            let namespace = self
                .fact_authority_namespace(function, &declarations)
                .cloned();
            let key = (planned.name.clone(), OutputKind::Value);
            result.insert(
                key,
                self.fact_route_candidate_with_annotations(
                    function,
                    (
                        planned.name.clone(),
                        source_names.fact_name(function).to_string(),
                    ),
                    OutputKind::Value,
                    claims,
                    namespace,
                    selected_annotations,
                    &route_context,
                )?,
            );
        }
        for planned in &plan.constants {
            let constant = planned.constant;
            let mut owners: BTreeSet<_> = self
                .constant_root_owners
                .get(&(constant.root.clone(), constant.name.clone()))
                .or_else(|| self.root_owners.get(&constant.root))
                .cloned()
                .into_iter()
                .collect();
            let namespace = self
                .constant_namespace_authorities
                .get(&(constant.definition.clone(), constant.name.clone()))
                .cloned();
            self.add_constant_authority_fallback(&mut owners, constant, namespace.as_deref())?;
            let annotations = annotation_signatures
                .get(&constant.root)
                .cloned()
                .unwrap_or_else(|| empty_annotations.clone());
            let claims: BTreeSet<_> = owners
                .iter()
                .map(|owner| self.constant_route_claim(constant, owner, annotations.clone()))
                .collect();
            let preferred = self
                .constant_root_owners
                .get(&(constant.root.clone(), constant.name.clone()))
                .or_else(|| self.root_owners.get(&constant.root))
                .map(|owner| self.constant_route_claim(constant, owner, annotations))
                .filter(|claim| claims.contains(claim))
                .or_else(|| (claims.len() == 1).then(|| claims.first().unwrap().clone()));
            let anchored = !claims.is_empty() || namespace.is_some();
            let key = (constant.name.clone(), OutputKind::Value);
            result.insert(
                key,
                RouteCandidate {
                    name: constant.name.clone(),
                    source_name: source_names.constant_name(constant).to_string(),
                    kind: OutputKind::Value,
                    claims,
                    preferred,
                    namespace,
                    anchored,
                    header_partition_policy: self.header_partition_policy,
                },
            );
        }
        if self.header_partition_policy {
            self.assign_default_dependency_owners(
                plan,
                &mut result,
                options.namespace,
                &route_context,
            )?;
        }
        Ok(result)
    }

    fn fact_route_candidate<'a>(
        &'a self,
        fact: &'a Fact,
        names: (String, String),
        kind: OutputKind,
        claims: BTreeSet<RouteClaim<'a>>,
        namespace: Option<String>,
        context: &RouteClaimContext<'_, '_>,
    ) -> Result<RouteCandidate<'a>, Error> {
        let annotations = context
            .annotations
            .get(&fact.origin)
            .cloned()
            .unwrap_or_else(|| context.empty_annotations.clone());
        self.fact_route_candidate_with_annotations(
            fact,
            names,
            kind,
            claims,
            namespace,
            annotations,
            context,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn fact_route_candidate_with_annotations<'a>(
        &'a self,
        fact: &'a Fact,
        names: (String, String),
        kind: OutputKind,
        mut claims: BTreeSet<RouteClaim<'a>>,
        namespace: Option<String>,
        annotations: Rc<RouteAnnotations>,
        context: &RouteClaimContext<'_, '_>,
    ) -> Result<RouteCandidate<'a>, Error> {
        if claims.is_empty() {
            let mut owners = BTreeSet::new();
            self.add_authority_fallback(&mut owners, fact, namespace.as_deref())?;
            for owner in owners {
                claims.insert(self.fact_route_claim(
                    fact,
                    fact,
                    &owner,
                    annotations.clone(),
                    context.flag_enums,
                    context.options,
                )?);
            }
        }
        let preferred = self
            .root_owners
            .get(&fact.origin)
            .map(|owner| {
                self.fact_route_claim(
                    fact,
                    fact,
                    owner,
                    annotations,
                    context.flag_enums,
                    context.options,
                )
            })
            .transpose()?
            .filter(|claim| claims.contains(claim))
            .or_else(|| (claims.len() == 1).then(|| claims.first().unwrap().clone()));
        Ok(RouteCandidate {
            name: names.0,
            source_name: names.1,
            kind,
            anchored: !claims.is_empty() || namespace.is_some(),
            claims,
            preferred,
            namespace,
            header_partition_policy: self.header_partition_policy,
        })
    }

    fn assign_default_dependency_owners<'a>(
        &'a self,
        plan: &Plan<'a>,
        routes: &mut BTreeMap<(String, OutputKind), RouteCandidate<'a>>,
        default_namespace: &str,
        context: &RouteClaimContext<'_, '_>,
    ) -> Result<(), Error> {
        validate_namespace(default_namespace)?;
        for planned in &plan.types {
            let key = (planned.name.clone(), OutputKind::Type);
            let route = routes.get_mut(&key).unwrap();
            if planned.fact.root
                || self.suppressed_type_origins.contains(&planned.fact.origin)
                || !route.claims.is_empty()
                || route.namespace.is_some()
            {
                continue;
            }
            let owner = authority_root_owner(
                default_namespace,
                default_namespace,
                &planned.fact.expansion.file,
            );
            let annotations = context
                .annotations
                .get(&planned.fact.origin)
                .cloned()
                .unwrap_or_else(|| context.empty_annotations.clone());
            let claim = self.fact_route_claim(
                planned.fact,
                planned.fact,
                &owner,
                annotations,
                context.flag_enums,
                context.options,
            )?;
            route.preferred = Some(claim.clone());
            route.claims.insert(claim);
        }
        Ok(())
    }

    fn route_annotation_signatures(&self) -> BTreeMap<Origin, Rc<RouteAnnotations>> {
        let mut result: BTreeMap<Origin, RouteAnnotations> = BTreeMap::new();
        for (target, annotations) in &self.annotations {
            let (origin, target) = route_annotation_target(target);
            let annotations: Vec<_> = annotations
                .iter()
                .filter(|annotation| !matches!(annotation, Annotation::ImportLibrary(_)))
                .cloned()
                .collect();
            if !annotations.is_empty() {
                result
                    .entry(origin.clone())
                    .or_default()
                    .push((target, annotations));
            }
        }
        result
            .into_iter()
            .map(|(origin, mut annotations)| {
                annotations.sort();
                (origin, Rc::new(annotations))
            })
            .collect()
    }

    fn source_annotation_signatures(&self) -> BTreeMap<Origin, Rc<RouteAnnotations>> {
        Self::relative_annotation_signatures(&self.source_annotations)
    }

    fn relative_annotation_signatures(
        annotations: &BTreeMap<AnnotationTarget, Vec<Annotation>>,
    ) -> BTreeMap<Origin, Rc<RouteAnnotations>> {
        let mut result: BTreeMap<Origin, RouteAnnotations> = BTreeMap::new();
        for (target, annotations) in annotations {
            let (origin, target) = route_annotation_target(target);
            result
                .entry(origin.clone())
                .or_default()
                .push((target, annotations.clone()));
        }
        result
            .into_iter()
            .map(|(origin, mut annotations)| {
                annotations.sort();
                (origin, Rc::new(annotations))
            })
            .collect()
    }

    fn function_route_annotation_signatures(&self) -> BTreeMap<Origin, Rc<RouteAnnotations>> {
        let mut result: BTreeMap<Origin, RouteAnnotations> = BTreeMap::new();
        for (target, annotations) in &self.function_annotations {
            let (origin, target) = route_annotation_target(target);
            let annotations: Vec<_> = annotations
                .iter()
                .filter(|annotation| !matches!(annotation, Annotation::ImportLibrary(_)))
                .cloned()
                .collect();
            if !annotations.is_empty() {
                result
                    .entry(origin.clone())
                    .or_default()
                    .push((target, annotations));
            }
        }
        result
            .into_iter()
            .map(|(origin, mut annotations)| {
                annotations.sort();
                (origin, Rc::new(annotations))
            })
            .collect()
    }

    fn resolve_native_import(
        &self,
        fact: &Fact,
        owner: Option<&RootOwner>,
        options: &EmitOptions<'_>,
    ) -> Result<Option<NativeImport>, Error> {
        let FactData::Function { link_name, .. } = &fact.data else {
            return Ok(None);
        };
        let configured_library = annotations_for(
            &self.function_annotations,
            &AnnotationTarget::Declaration(fact.origin.clone()),
        )
        .iter()
        .find_map(|annotation| match annotation {
            Annotation::ImportLibrary(library) => Some(library.as_str()),
            _ => None,
        })
        .or_else(|| owner.and_then(|owner| owner.libraries.get(link_name).map(String::as_str)))
        .or_else(|| {
            options
                .libraries
                .and_then(|libraries| libraries.get(link_name).map(String::as_str))
        })
        .or(options.library);
        let native_import = options
            .native_imports
            .and_then(|imports| imports.get(link_name));

        if let Some(native_import) = native_import {
            if let Some(configured_library) = configured_library
                && !configured_library.eq_ignore_ascii_case(native_import.library())
            {
                return Err(Error(format!(
                    "function `{}` has conflicting import libraries `{configured_library}` and \
                     `{}` for linker symbol `{link_name}`",
                    fact.name,
                    native_import.library()
                )));
            }
            Ok(Some(native_import.clone()))
        } else {
            Ok(configured_library.map(|library| NativeImport::named(library, link_name.clone())))
        }
    }

    fn fact_route_claim<'a>(
        &'a self,
        fact: &'a Fact,
        declaration: &'a Fact,
        owner: &RootOwner,
        annotations: Rc<RouteAnnotations>,
        flag_enums: &BTreeSet<(String, String)>,
        options: &EmitOptions<'_>,
    ) -> Result<RouteClaim<'a>, Error> {
        Ok(RouteClaim {
            owner: owner.clone(),
            semantics: RouteSemantics {
                item: RouteItemSemantics::Fact {
                    kind: fact.kind,
                    definition: fact.definition,
                    data: &fact.data,
                },
                annotations,
                uuid: self.fact_uuid(fact),
                flags: flag_enums.contains(&(fact.origin.tu.clone(), fact.name.clone())),
                native_import: self.resolve_native_import(declaration, Some(owner), options)?,
            },
        })
    }

    fn constant_route_claim<'a>(
        &'a self,
        constant: &'a Constant,
        owner: &RootOwner,
        annotations: Rc<RouteAnnotations>,
    ) -> RouteClaim<'a> {
        RouteClaim {
            owner: owner.clone(),
            semantics: RouteSemantics {
                item: RouteItemSemantics::Constant {
                    ty: &constant.ty,
                    value: &constant.value,
                },
                annotations,
                uuid: None,
                flags: false,
                native_import: None,
            },
        }
    }

    fn add_authority_fallback(
        &self,
        owners: &mut BTreeSet<RootOwner>,
        fact: &Fact,
        namespace: Option<&str>,
    ) -> Result<(), Error> {
        if !owners.is_empty() || namespace.is_none() {
            return Ok(());
        }
        let namespace = namespace.unwrap();
        validate_namespace(namespace)?;
        if self.header_partition_policy {
            let partition = self
                .header_authority_partition
                .as_deref()
                .unwrap_or(namespace);
            owners.insert(authority_root_owner(
                partition,
                namespace,
                &fact.expansion.file,
            ));
            return Ok(());
        }
        let Some(input) = self.partition_inputs.get(&fact.origin.tu) else {
            return Ok(());
        };
        owners.insert(RootOwner {
            input: input.clone(),
            root: fact.expansion.file.clone(),
            partition: input.clone(),
            namespace: namespace.to_string(),
            remaps: BTreeMap::new(),
            exclusions: BTreeSet::new(),
            libraries: BTreeMap::new(),
            u32_types: BTreeSet::new(),
            flags: BTreeSet::new(),
            preserved_auto_function_pointer_levels: BTreeSet::new(),
            exclude_empty_records: false,
        });
        Ok(())
    }

    fn add_constant_authority_fallback(
        &self,
        owners: &mut BTreeSet<RootOwner>,
        constant: &Constant,
        namespace: Option<&str>,
    ) -> Result<(), Error> {
        if !owners.is_empty() {
            return Ok(());
        }
        if self.header_partition_policy
            && let Some(namespace) = namespace
        {
            validate_namespace(namespace)?;
            let partition = self
                .header_authority_partition
                .as_deref()
                .unwrap_or(namespace);
            let root = self
                .facts
                .iter()
                .find(|fact| fact.origin == constant.root)
                .map_or(constant.spelling.file.as_str(), |fact| {
                    fact.expansion.file.as_str()
                });
            owners.insert(authority_root_owner(partition, namespace, root));
            return Ok(());
        }
        let mut input_owners = self
            .root_partitions
            .iter()
            .filter(|((tu, _), _)| tu == &constant.root.tu)
            .map(|(_, owner)| owner.clone())
            .collect::<BTreeSet<_>>();
        let owner = if let Some(namespace) = namespace {
            let Some(input) = self.partition_inputs.get(&constant.root.tu) else {
                return Ok(());
            };
            validate_namespace(namespace)?;
            RootOwner {
                input: input.clone(),
                root: constant.spelling.file.clone(),
                partition: input.clone(),
                namespace: namespace.to_string(),
                remaps: BTreeMap::new(),
                exclusions: BTreeSet::new(),
                libraries: BTreeMap::new(),
                u32_types: BTreeSet::new(),
                flags: BTreeSet::new(),
                preserved_auto_function_pointer_levels: BTreeSet::new(),
                exclude_empty_records: false,
            }
        } else {
            let Some(mut owner) = input_owners.pop_first() else {
                return Ok(());
            };
            if input_owners.iter().any(|candidate| {
                candidate.partition != owner.partition || candidate.namespace != owner.namespace
            }) {
                return Ok(());
            }
            owner.root.clone_from(&constant.spelling.file);
            owner
        };
        owners.insert(owner);
        Ok(())
    }

    fn input_rank(&self, tu: &str) -> usize {
        self.input_order.get(tu).copied().unwrap_or(usize::MAX)
    }

    fn selected_function_origins(&self) -> BTreeSet<Origin> {
        let mut selected = BTreeMap::<(String, String), &Fact>::new();
        for fact in &self.facts {
            if !fact.root {
                continue;
            }
            let FactData::Function { link_name, .. } = &fact.data else {
                continue;
            };
            selected
                .entry((fact.name.clone(), link_name.clone()))
                .and_modify(|current| {
                    if (
                        self.input_rank(&fact.origin.tu),
                        &fact.spelling,
                        &fact.expansion,
                        &fact.origin,
                    ) < (
                        self.input_rank(&current.origin.tu),
                        &current.spelling,
                        &current.expansion,
                        &current.origin,
                    ) {
                        *current = fact;
                    }
                })
                .or_insert(fact);
        }
        selected
            .into_values()
            .map(|fact| fact.origin.clone())
            .collect()
    }

    fn annotations_with_selected_functions(&self) -> BTreeMap<AnnotationTarget, Vec<Annotation>> {
        let function_origins: BTreeSet<_> = self
            .facts
            .iter()
            .filter(|fact| fact.kind == FactKind::Function)
            .map(|fact| fact.origin.clone())
            .collect();
        let selected_origins = self.selected_function_origins();
        let mut result = self
            .annotations
            .iter()
            .filter(|(target, _)| !function_origins.contains(target.origin()))
            .map(|(target, annotations)| (target.clone(), annotations.clone()))
            .collect::<BTreeMap<_, _>>();
        result.extend(
            self.function_annotations
                .iter()
                .filter(|(target, _)| selected_origins.contains(target.origin()))
                .map(|(target, annotations)| (target.clone(), annotations.clone())),
        );
        result
    }

    fn choose_constant_root<'a>(
        &self,
        name: &str,
        roots: &[&'a Constant],
    ) -> Result<&'a Constant, Error> {
        let Some(first) = roots.first() else {
            return Err(Error(format!("missing constant root `{name}`")));
        };
        if !roots.iter().all(|constant| {
            constant_types_match(&constant.ty, &first.ty) && constant.value == first.value
        }) {
            return Err(Error(format!("ambiguous constant root `{name}`")));
        }
        roots
            .iter()
            .min_by_key(|constant| {
                (
                    self.input_rank(&constant.root.tu),
                    &constant.spelling,
                    &constant.definition,
                )
            })
            .copied()
            .ok_or_else(|| Error(format!("missing constant root `{name}`")))
    }

    fn choose_common_function_root<'a>(
        &self,
        name: &str,
        roots: &[&'a Fact],
        facts_by_origin: &HashMap<&Origin, &Fact>,
        annotation_signatures: &BTreeMap<Origin, Rc<RouteAnnotations>>,
        source_names: Option<&PlanningSourceNames>,
    ) -> Result<FunctionRootSelection<'a>, Error> {
        let Some(first) = roots.first() else {
            return Err(Error(format!("missing function root `{name}`")));
        };
        let first_annotations = annotation_signatures.get(&first.origin);
        let first_parent = function_parent_path(first, facts_by_origin, source_names);
        let fully_equal = roots.iter().all(|fact| {
            fact.kind == first.kind
                && fact.definition == first.definition
                && fact.data == first.data
                && function_parent_path(fact, facts_by_origin, source_names) == first_parent
                && annotation_signatures.get(&fact.origin) == first_annotations
        });
        let mut declarations = roots.to_vec();
        declarations.sort_by(|left, right| {
            (
                self.input_rank(&left.origin.tu),
                &left.spelling,
                &left.expansion,
                &left.origin,
            )
                .cmp(&(
                    self.input_rank(&right.origin.tu),
                    &right.spelling,
                    &right.expansion,
                    &right.origin,
                ))
        });
        let fact = declarations[0];
        let ambiguity = (!fully_equal).then(|| FunctionAmbiguity {
            declarations: declarations
                .iter()
                .map(|declaration| FunctionDeclarationVariant {
                    fact: declaration,
                    selected: declaration.origin == fact.origin,
                    parent: function_parent_path(declaration, facts_by_origin, source_names),
                    annotations: annotation_signatures
                        .get(&declaration.origin)
                        .cloned()
                        .unwrap_or_else(|| Rc::new(Vec::new())),
                })
                .collect(),
        });
        Ok(FunctionRootSelection {
            fact,
            declarations,
            ambiguity,
        })
    }

    fn authoritative_owner(
        &self,
        name: &str,
        kind: OutputKind,
        owners: BTreeSet<RootOwner>,
        namespace: Option<&str>,
    ) -> Result<RootOwner, Error> {
        let Some(namespace) = namespace else {
            return unique_owner(name, kind, owners);
        };
        let matching: BTreeSet<_> = owners
            .iter()
            .filter(|owner| owner.namespace == namespace)
            .cloned()
            .collect();
        if !matching.is_empty() {
            return unique_owner(name, kind, matching);
        }
        let Some(mut owner) = owners.into_iter().next() else {
            let kind = match kind {
                OutputKind::Type => "type",
                OutputKind::Value => "value",
            };
            return Err(Error(format!(
                "selected {kind} `{name}` has no tagged root owner"
            )));
        };
        if owner.partition.trim().is_empty() {
            return Err(Error(format!(
                "selected item `{name}` has an empty partition identity"
            )));
        }
        owner.namespace = namespace.to_string();
        Ok(owner)
    }

    fn selected_sal_constant_size<'a>(
        &self,
        target: &AnnotationTarget,
        size: &'a Result<SalSize, SalCountError>,
        facts: &HashMap<&Origin, &Fact>,
        observations: &SalConstantObservations<'_>,
    ) -> Result<&'a Result<SalSize, SalCountError>, Error> {
        let fact = facts.get(target.origin()).unwrap();
        let policy = |fact: &Fact| {
            self.root_owners.get(&fact.origin).or_else(|| {
                self.root_partitions
                    .get(&(fact.origin.tu.clone(), fact.spelling.file.clone()))
            })
        };
        let selected_policy = policy(fact);
        let slot = route_annotation_target(target).1;
        let parent = function_parent_path(fact, facts, None);
        for &(observation_target, observation_size) in observations.get(&fact.spelling, slot) {
            #[cfg(test)]
            observations.inspected.set(observations.inspected.get() + 1);
            if observation_target == target {
                continue;
            }
            let Some(observation) = facts.get(observation_target.origin()) else {
                continue;
            };
            if self.ranked_header_partition_policy && observation.origin.tu != fact.origin.tu {
                continue;
            }
            let observation_policy = policy(observation);
            let same_policy = match (selected_policy, observation_policy) {
                (Some(left), Some(right)) => same_owner_policy(left, right),
                (None, None) => true,
                _ => false,
            };
            if !same_policy
                || !same_source_declaration(fact, observation)
                || function_parent_path(observation, facts, None) != parent
                || observation_size == size
            {
                continue;
            }
            return Err(Error(format!(
                "conflicting native SAL count contexts: source={}:{} \
                 selected_target={target:?} tu={:?} policy={selected_policy:?} value={size:?}; \
                 source={}:{} observed_target={observation_target:?} tu={:?} \
                 policy={observation_policy:?} value={observation_size:?}",
                fact.spelling.file,
                fact.spelling.offset,
                fact.origin.tu,
                observation.spelling.file,
                observation.spelling.offset,
                observation.origin.tu,
            )));
        }
        Ok(size)
    }

    fn plan_partitioned(
        &self,
        options: &EmitOptions<'_>,
        display_names: &BTreeMap<String, String>,
        source_names: &PlanningSourceNames,
    ) -> Result<Plan<'_>, Error> {
        self.plan_partitioned_with_enum_policy(options, display_names, source_names, false)
    }

    fn plan_partitioned_with_enum_policy(
        &self,
        options: &EmitOptions<'_>,
        display_names: &BTreeMap<String, String>,
        source_names: &PlanningSourceNames,
        allow_enum_omissions: bool,
    ) -> Result<Plan<'_>, Error> {
        let references = scoped_planning_map(options.references, display_names);
        let excluded_types =
            scoped_planning_set(options.excluded_types.or(options.excluded), display_names);
        let excluded_functions = scoped_planning_set(
            options.excluded_functions.or(options.excluded),
            display_names,
        );
        let excluded_constants = scoped_planning_set(
            options.excluded_constants.or(options.excluded),
            display_names,
        );
        self.plan(PlanningOptions {
            references: references.as_ref(),
            excluded_types: excluded_types.as_deref(),
            excluded_functions: excluded_functions.as_deref(),
            excluded_constants: excluded_constants.as_deref(),
            selected_functions: options.functions,
            display_names: Some(display_names),
            source_names: Some(source_names),
            mutable_string_aliases: options.mutable_string_aliases,
            allow_enum_omissions,
            omission_namespace: options.namespace,
        })
    }

    fn plan(&self, options: PlanningOptions<'_>) -> Result<Plan<'_>, Error> {
        let mut phase = TimingPhase::new(self.timing_target.as_deref(), "plan-indexes");
        let result = self.plan_inner(options, &mut phase);
        phase.finish(result)
    }

    fn plan_inner(
        &self,
        options: PlanningOptions<'_>,
        phase: &mut TimingPhase,
    ) -> Result<Plan<'_>, Error> {
        let PlanningOptions {
            references,
            excluded_types,
            excluded_functions,
            excluded_constants,
            selected_functions,
            display_names,
            source_names,
            mutable_string_aliases,
            allow_enum_omissions,
            omission_namespace,
        } = options;
        let timing = self.timing_target.is_some();
        let target = self.timing_target.as_deref().unwrap_or("default");
        let mut phase_time = timing.then(std::time::Instant::now);
        #[derive(Default)]
        struct Roots<'a> {
            types: Vec<&'a Fact>,
            values: Vec<&'a Fact>,
            functions: Vec<&'a Fact>,
            constants: Vec<&'a Constant>,
        }

        let declarations = DeclarationIndex::new(&self.facts);
        let facts_by_origin: HashMap<_, _> =
            self.facts.iter().map(|fact| (&fact.origin, fact)).collect();
        let sal_constant_observations =
            SalConstantObservations::new(&self.sal_constant_sizes, &facts_by_origin);
        let source_annotation_signatures = self.source_annotation_signatures();
        let effective_annotation_signatures =
            Self::relative_annotation_signatures(&self.annotations);
        let source_fact_name = |fact: &Fact| {
            source_names.map_or_else(
                || fact.name.clone(),
                |names| names.fact_name(fact).to_string(),
            )
        };
        let mut interface_names_by_source_identity = BTreeMap::new();
        let mut interface_names_by_source_name = BTreeMap::new();
        for fact in self
            .facts
            .iter()
            .filter(|fact| matches!(fact.data, FactData::Interface { .. }))
        {
            let source_name = source_fact_name(fact);
            interface_names_by_source_identity
                .entry((fact.origin.tu.clone(), source_name.clone()))
                .or_insert_with(BTreeSet::new)
                .insert(fact.name.clone());
            interface_names_by_source_name
                .entry(source_name)
                .or_insert_with(BTreeSet::new)
                .insert(fact.name.clone());
        }
        let associated_interface_name = |fact: &Fact| {
            let source_name = source_fact_name(fact);
            let interface = source_name.strip_prefix("IID_")?;
            if let Some(names) = interface_names_by_source_identity
                .get(&(fact.origin.tu.clone(), interface.to_string()))
                && names.len() == 1
            {
                return names.first().cloned();
            }
            let names = interface_names_by_source_name.get(interface)?;
            (names.len() == 1).then(|| names.first().unwrap().clone())
        };
        let rooted_interface_iids: BTreeSet<_> = self
            .facts
            .iter()
            .filter(|fact| fact.root && matches!(fact.data, FactData::Guid { .. }))
            .filter_map(|fact| {
                Some((
                    fact.origin.tu.clone(),
                    source_fact_name(fact).strip_prefix("IID_")?.to_string(),
                ))
            })
            .collect();
        let is_identified_native_interface_root = |fact: &Fact| {
            self.header_partition_policy
                && fact.root
                && is_identified_native_interface(
                    fact,
                    &source_fact_name(fact),
                    &facts_by_origin,
                    &rooted_interface_iids,
                )
        };
        let is_flat_root = |fact: &Fact| {
            !self.observation_only_class_origins.contains(&fact.origin)
                && (is_flat_declaration(fact, &facts_by_origin, references, true)
                    || is_identified_native_interface_root(fact))
        };
        let is_flat_dependency = |fact| {
            is_flat_declaration(fact, &facts_by_origin, references, false)
                || is_identified_native_interface_root(fact)
        };
        let interfaces: BTreeSet<_> = self
            .facts
            .iter()
            .filter(|fact| {
                is_flat_dependency(fact) && matches!(fact.data, FactData::Interface { .. })
            })
            .map(|fact| fact.name.as_str())
            .collect();
        let mut declared_interface_guids = BTreeMap::new();
        for fact in self.facts.iter().filter(|fact| {
            fact.root && matches!(fact.data, FactData::Interface { .. }) && is_flat_root(fact)
        }) {
            let FactData::Interface {
                guid: Some(guid), ..
            } = &fact.data
            else {
                continue;
            };
            if let Some(previous) =
                declared_interface_guids.insert(fact.name.as_str(), guid.as_str())
                && previous != guid
            {
                return Err(Error(format!(
                    "interface `{}` has conflicting UUID attributes",
                    fact.name
                )));
            }
        }
        let mut interface_guids = BTreeMap::new();
        for fact in self.facts.iter().filter(|fact| {
            fact.root && matches!(fact.data, FactData::Guid { .. }) && is_flat_root(fact)
        }) {
            let source_name = source_fact_name(fact);
            let Some(source_interface) = source_name.strip_prefix("IID_") else {
                continue;
            };
            let Some(interface) = associated_interface_name(fact) else {
                continue;
            };
            if !interfaces.contains(interface.as_str())
                || declared_interface_guids.contains_key(interface.as_str())
            {
                continue;
            }
            let FactData::Guid { value } = &fact.data else {
                unreachable!()
            };
            if let Some(previous) = interface_guids.insert(interface, value.clone())
                && previous != *value
            {
                return Err(Error(format!(
                    "interface `{source_interface}` has conflicting IID declarations"
                )));
            }
        }

        let mut roots: BTreeMap<&str, Roots<'_>> = BTreeMap::new();
        let associated_constants: BTreeSet<_> = self
            .annotations
            .values()
            .flatten()
            .filter_map(|annotation| match annotation {
                Annotation::AssociatedConstant(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        for fact in self
            .facts
            .iter()
            .filter(|fact| fact.root && is_root_fact(fact) && is_flat_root(fact))
            .filter(|fact| {
                let source_name = source_fact_name(fact);
                let Some(source_interface) = source_name.strip_prefix("IID_") else {
                    return true;
                };
                let FactData::Guid { value } = &fact.data else {
                    return true;
                };
                if references
                    .get(source_interface)
                    .is_some_and(|reference| reference.kind == TypeReferenceKind::Interface)
                {
                    return false;
                }
                let Some(interface) = associated_interface_name(fact) else {
                    return true;
                };
                if !interfaces.contains(interface.as_str()) {
                    return true;
                }
                declared_interface_guids
                    .get(interface.as_str())
                    .is_some_and(|declared| *declared != value)
            })
        {
            let roots = roots.entry(&fact.name).or_default();
            if is_value_fact(fact) {
                roots.values.push(fact);
            } else {
                roots.types.push(fact);
            }
        }
        for fact in self
            .facts
            .iter()
            .filter(|fact| {
                if !fact.root || !matches!(fact.data, FactData::Function { .. }) {
                    return false;
                }
                if is_flat_root(fact) {
                    return true;
                }
                self.header_partition_policy
                    && self.root_owners.contains_key(&fact.origin)
                    && selected_functions.is_some_and(|functions| {
                        matches!(
                            &fact.data,
                            FactData::Function { link_name, .. } if functions.contains(link_name)
                        )
                    })
            })
            .filter(|fact| excluded_functions.is_none_or(|excluded| !excluded.contains(&fact.name)))
            .filter(|fact| {
                selected_functions.is_none_or(|functions| {
                    matches!(
                        &fact.data,
                        FactData::Function { link_name, .. } if functions.contains(link_name)
                    )
                })
            })
        {
            roots.entry(&fact.name).or_default().functions.push(fact);
        }
        for constant in &self.constants {
            if self.ranked_header_partition_policy
                && !self
                    .constant_root_owners
                    .contains_key(&(constant.root.clone(), constant.name.clone()))
                && !self.root_owners.contains_key(&constant.root)
            {
                continue;
            }
            if excluded_constants.is_some_and(|excluded| excluded.contains(&constant.name))
                && !associated_constants.contains(constant.name.as_str())
            {
                continue;
            }
            roots
                .entry(&constant.name)
                .or_default()
                .constants
                .push(constant);
        }

        let mut facts_index: HashMap<&str, Vec<&Fact>> = HashMap::new();
        let mut exact_dependency_facts_index: HashMap<&str, Vec<&Fact>> = HashMap::new();
        for fact in &self.facts {
            if is_flat_dependency(fact) {
                facts_index.entry(&fact.name).or_default().push(fact);
            } else if self.header_partition_policy && is_type_declaration_fact(fact) {
                exact_dependency_facts_index
                    .entry(&fact.name)
                    .or_default()
                    .push(fact);
            }
        }
        let canonical_typedef_declarations: HashSet<_> = self
            .facts
            .iter()
            .filter(|fact| {
                canonical_named_type(&fact.name).is_some()
                    && matches!(fact.data, FactData::Typedef { .. })
            })
            .map(|fact| (fact.origin.tu.as_str(), &fact.spelling))
            .collect();
        let facts_by_declaration: BTreeMap<_, _> = self
            .facts
            .iter()
            .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
            .collect();
        let com_context = com_pointer::Context {
            snapshot: self,
            declarations: &declarations,
            facts_by_declaration: &facts_by_declaration,
            source_names,
            excluded_types,
            source_annotations: &source_annotation_signatures,
            effective_annotations: &effective_annotation_signatures,
        };
        let com_bindings = com_pointer::Bindings::new(&com_context, &facts_by_origin);
        phase.next("plan-roots");
        let extended_reference_enums: BTreeSet<_> = excluded_types
            .into_iter()
            .flatten()
            .filter_map(|name| {
                let reference = references.get(name)?;
                if reference.kind != TypeReferenceKind::Enum {
                    return None;
                }
                if reference.enum_members.is_empty()
                    || self
                        .facts
                        .iter()
                        .filter(|fact| fact.name == *name)
                        .filter_map(|fact| {
                            underlying_enum_fact(fact, &facts_by_declaration, &mut BTreeSet::new())
                        })
                        .filter_map(|fact| match &fact.data {
                            FactData::Enum { variants, .. } => Some(variants.as_slice()),
                            _ => None,
                        })
                        .flatten()
                        .any(|variant| !reference.enum_members.contains(&variant.name))
                {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();
        let excluded_declarations: BTreeSet<_> = excluded_types
            .into_iter()
            .flat_map(|excluded| {
                let extended_reference_enums = &extended_reference_enums;
                self.facts.iter().filter_map(move |fact| {
                    if !excluded.contains(&fact.name)
                        || extended_reference_enums.contains(&fact.name)
                    {
                        return None;
                    }
                    if let FactData::Typedef {
                        target: TypeRef::Named { declaration, .. },
                    } = &fact.data
                    {
                        Some((fact.origin.tu.clone(), declaration.clone()))
                    } else {
                        None
                    }
                })
            })
            .collect();
        let excluded_local_names: BTreeSet<_> = excluded_types
            .into_iter()
            .flat_map(|excluded| {
                let extended_reference_enums = &extended_reference_enums;
                self.facts.iter().filter_map(move |fact| {
                    if !excluded.contains(&fact.name)
                        || extended_reference_enums.contains(&fact.name)
                    {
                        return None;
                    }
                    if let FactData::Typedef {
                        target: TypeRef::Named { name, .. },
                    } = &fact.data
                    {
                        Some((fact.origin.tu.clone(), name.clone()))
                    } else {
                        None
                    }
                })
            })
            .collect();
        let mut type_roots = vec![];
        let mut value_roots = vec![];
        let mut functions = vec![];
        let mut root_errors = vec![];
        let mut enum_root_failures = vec![];
        let mut failed_type_roots = BTreeSet::new();
        let mut failed_function_roots = BTreeSet::new();
        let mut constants = vec![];
        let mut root_names = BTreeSet::new();
        let mut shape_cache = ShapeCache::default();

        let coclass_aliases = self.coclass_aliases()?;
        for (name, roots) in roots {
            phase.visit(None);
            if !roots.types.is_empty() && !roots.functions.is_empty() {
                let candidates: Vec<_> = roots
                    .types
                    .iter()
                    .chain(&roots.functions)
                    .copied()
                    .collect();
                root_errors.push(root_selection_error(
                    self,
                    name,
                    "collision",
                    Error(format!("type and function roots collide on `{name}`")),
                    &candidates,
                    &[],
                ));
            }
            if !roots.values.is_empty()
                && !excluded_constants.is_some_and(|excluded| excluded.contains(name))
            {
                match choose_value_root(name, &roots.values) {
                    Ok(value) => value_roots.push(value),
                    Err(error) => root_errors.push(root_selection_error(
                        self,
                        name,
                        "value",
                        error,
                        &roots.values,
                        &[],
                    )),
                }
            }
            if !roots.constants.is_empty() {
                match self.choose_constant_root(name, &roots.constants) {
                    Ok(constant) => constants.push(constant),
                    Err(error) => root_errors.push(root_selection_error(
                        self,
                        name,
                        "constant",
                        error,
                        &[],
                        &roots.constants,
                    )),
                }
            }
            let types_alias_value_class =
                types_alias_value_class(&roots.types, &roots.values, &coclass_aliases);
            if !roots.types.is_empty() && !types_alias_value_class {
                let excluded = roots.types.iter().any(|fact| {
                    excluded_declarations.contains(&(fact.origin.tu.clone(), fact.spelling.clone()))
                        || excluded_local_names
                            .contains(&(fact.origin.tu.clone(), fact.name.clone()))
                }) || (excluded_types
                    .is_some_and(|excluded| excluded.contains(name))
                    && !extended_reference_enums.contains(name))
                    || (references.contains_key(name)
                        && !roots
                            .types
                            .iter()
                            .any(|fact| defines_local_type(name, fact)));
                if !excluded {
                    let authority = self.authority_candidates(&roots.types, &declarations);
                    match choose_proven_type_root(
                        &com_context,
                        name,
                        &authority,
                        &facts_index,
                        &mut shape_cache,
                        source_names,
                        &com_bindings,
                    ) {
                        Ok(root) => {
                            let tagged = authority
                                .iter()
                                .any(|fact| self.root_owners.contains_key(&fact.origin));
                            let routed = authority.iter().any(|fact| {
                                self.fact_authority_namespace(fact, &declarations).is_some()
                            });
                            if tagged
                                || routed
                                || !matches!(root.data, FactData::Typedef { .. })
                                || (self.root_partitions.is_empty()
                                    && !self.header_partition_policy)
                            {
                                root_names.insert(name.to_string());
                                type_roots.push(root);
                            }
                        }
                        Err(error) => {
                            failed_type_roots.insert(name.to_string());
                            if allow_enum_omissions
                                && let Some(failure) = enum_selection_failure(
                                    &authority,
                                    &facts_by_declaration,
                                    &facts_by_origin,
                                    self,
                                    source_names,
                                    error.to_string(),
                                )
                            {
                                enum_root_failures.push(failure);
                            } else {
                                root_errors.push(type_root_selection_error(
                                    self,
                                    name,
                                    "initial",
                                    error,
                                    &roots.types,
                                    &authority,
                                    &facts_index,
                                ));
                            }
                        }
                    }
                }
            }
            if !roots.functions.is_empty() {
                let mut by_link_name: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
                for function in roots.functions {
                    let FactData::Function { link_name, .. } = &function.data else {
                        unreachable!()
                    };
                    by_link_name.entry(link_name).or_default().push(function);
                }
                let split_names = by_link_name.len() > 1;
                for (link_name, roots) in by_link_name {
                    let selection = match self.choose_common_function_root(
                        name,
                        &roots,
                        &facts_by_origin,
                        &source_annotation_signatures,
                        source_names,
                    ) {
                        Ok(selection) => selection,
                        Err(error) => {
                            failed_function_roots.insert(link_name.to_string());
                            root_errors.push(root_selection_error(
                                self,
                                name,
                                "function",
                                error,
                                &roots,
                                &[],
                            ));
                            continue;
                        }
                    };
                    functions.push(PlannedFunction {
                        name: if split_names {
                            link_name.to_string()
                        } else {
                            selection.fact.name.clone()
                        },
                        fact: selection.fact,
                        declarations: selection.declarations,
                        ambiguity: selection.ambiguity,
                    });
                }
            }
        }
        let selected_annotations = self
            .header_partition_policy
            .then(|| self.annotations_with_selected_functions());
        if let Some(selected_annotations) = &selected_annotations {
            // Associated enum names are semantic type dependencies of selected roots.
            let selected_origins: BTreeSet<_> = type_roots
                .iter()
                .chain(value_roots.iter())
                .map(|fact| fact.origin.clone())
                .chain(
                    functions
                        .iter()
                        .map(|function| function.fact.origin.clone()),
                )
                .chain(constants.iter().map(|constant| constant.root.clone()))
                .collect();
            let associated_enums: BTreeSet<_> = selected_annotations
                .iter()
                .filter(|(target, _)| selected_origins.contains(target.origin()))
                .flat_map(|(_, annotations)| {
                    annotations.iter().filter_map(move |annotation| {
                        let Annotation::AssociatedEnum(name) = annotation else {
                            return None;
                        };
                        Some(name.clone())
                    })
                })
                .collect();
            for name in associated_enums {
                if root_names.contains(&name)
                    || failed_type_roots.contains(&name)
                    || (excluded_types.is_some_and(|excluded| excluded.contains(&name))
                        && !extended_reference_enums.contains(&name))
                {
                    continue;
                }
                let matches: Vec<_> = facts_index
                    .get(name.as_str())
                    .into_iter()
                    .chain(exact_dependency_facts_index.get(name.as_str()))
                    .flatten()
                    .copied()
                    .filter(|fact| {
                        self.root_owners.contains_key(&fact.origin)
                            && !self.suppressed_type_origins.contains(&fact.origin)
                            && underlying_enum_fact(
                                fact,
                                &facts_by_declaration,
                                &mut BTreeSet::new(),
                            )
                            .is_some()
                    })
                    .collect();
                if matches.is_empty() {
                    if allow_enum_omissions && !references.contains_key(&name) {
                        let excluded: Vec<_> = facts_index
                            .get(name.as_str())
                            .into_iter()
                            .chain(exact_dependency_facts_index.get(name.as_str()))
                            .flatten()
                            .copied()
                            .filter(|fact| self.root_owners.contains_key(&fact.origin))
                            .filter(|fact| {
                                self.type_projection_suppressed(
                                    fact,
                                    &declarations,
                                    &mut BTreeSet::new(),
                                )
                            })
                            .collect();
                        if let Some(mut failure) = enum_selection_failure(
                            &excluded,
                            &facts_by_declaration,
                            &facts_by_origin,
                            self,
                            source_names,
                            format!(
                                "owner-excluded associated enum `{name}` has no eligible provider"
                            ),
                        ) {
                            failure.cause = EnumOmissionCause::ExcludedDependency;
                            enum_root_failures.push(failure);
                        }
                    }
                    continue;
                }
                let authority = self.authority_candidates(&matches, &declarations);
                match choose_proven_type_root(
                    &com_context,
                    &name,
                    &authority,
                    &facts_index,
                    &mut shape_cache,
                    source_names,
                    &com_bindings,
                ) {
                    Ok(root) => {
                        root_names.insert(name);
                        type_roots.push(root);
                    }
                    Err(error) => {
                        if allow_enum_omissions
                            && let Some(failure) = enum_selection_failure(
                                &authority,
                                &facts_by_declaration,
                                &facts_by_origin,
                                self,
                                source_names,
                                error.to_string(),
                            )
                        {
                            enum_root_failures.push(failure);
                        } else {
                            root_errors.push(type_root_selection_error(
                                self,
                                &name,
                                "associated",
                                error,
                                &matches,
                                &authority,
                                &facts_index,
                            ));
                        }
                    }
                }
            }
        }
        if let Some(selected) = selected_functions {
            let found: BTreeSet<_> = functions
                .iter()
                .filter_map(|function| match &function.fact.data {
                    FactData::Function { link_name, .. } => Some(link_name.as_str()),
                    _ => None,
                })
                .collect();
            for missing in selected.iter().filter(|name| {
                !found.contains(name.as_str()) && !failed_function_roots.contains(name.as_str())
            }) {
                for fact in self.facts.iter().filter(|fact| {
                    matches!(&fact.data, FactData::Function { link_name, .. } if link_name == missing)
                }) {
                    self.require_fact_header_authority(missing, fact)?;
                }
                root_errors.push(root_selection_error(
                    self,
                    missing,
                    "selected function",
                    Error(format!("selected function `{missing}` was not found")),
                    &[],
                    &[],
                ));
            }
        }
        if !root_errors.is_empty() {
            return Err(aggregate_root_selection_errors(root_errors));
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-roots target={target} elapsed_ms={:.3}",
                elapsed_ms(phase_time)
            );
            phase_time = Some(std::time::Instant::now());
        }

        let callback_pointer_alias_requirements = callback_dependency_pointer_alias_requirements(
            &declarations,
            functions.iter().map(|function| function.fact),
        );
        let failed_enum_sources: HashMap<_, _> = enum_root_failures
            .iter()
            .flat_map(|failure| {
                failure
                    .declarations
                    .iter()
                    .filter(|fact| matches!(fact.data, FactData::Enum { .. }))
                    .map(move |fact| {
                        let name =
                            source_names.map_or(fact.name.as_str(), |names| names.fact_name(fact));
                        ((name, &fact.spelling), failure)
                    })
            })
            .collect();
        let mut type_choices = BTreeMap::new();
        let mut sal_count_diagnostics = BTreeSet::new();
        let mut omissions = BTreeSet::new();
        let (mut facts_by_name, mut retained_pointer_aliases, mut dependency_diagnostics) = loop {
            phase.next("plan-dependencies");
            type_choices.clear();
            sal_count_diagnostics.clear();
            let mut facts = BTreeSet::new();
            let mut queue = vec![];
            let mut pointer_alias_candidates = BTreeSet::new();
            let mut retained_pointer_aliases = BTreeSet::new();
            let mut dependency_diagnostics =
                DependencyClosureDiagnostics::new(self.header_partition_policy);
            for root in &type_roots {
                let root_node = dependency_diagnostics.add_root(
                    "type",
                    &root.name,
                    &root.origin.tu,
                    &root.expansion,
                );
                if allow_enum_omissions {
                    dependency_diagnostics.set_root_origin(root_node, &root.origin);
                }
                let fact_node = DependencyNode::Fact(&root.origin);
                dependency_diagnostics.add_edge(root_node, fact_node);
                if facts.insert(root.origin.clone()) {
                    collect_fact_pointer_alias_candidates(
                        root,
                        &callback_pointer_alias_requirements,
                        &mut pointer_alias_candidates,
                    );
                    queue_type_edges(
                        root,
                        &self.projected_type_names,
                        &self.annotations,
                        Some(&self.sal_constant_sizes),
                        fact_node,
                        &mut queue,
                    );
                }
            }
            for root in &value_roots {
                let root_node = dependency_diagnostics.add_root(
                    "value",
                    &root.name,
                    &root.origin.tu,
                    &root.expansion,
                );
                if allow_enum_omissions {
                    dependency_diagnostics.set_root_origin(root_node, &root.origin);
                }
                let fact_node = DependencyNode::Fact(&root.origin);
                dependency_diagnostics.add_edge(root_node, fact_node);
                collect_fact_pointer_alias_candidates(
                    root,
                    &callback_pointer_alias_requirements,
                    &mut pointer_alias_candidates,
                );
                queue_type_edges(
                    root,
                    &self.projected_type_names,
                    &self.annotations,
                    Some(&self.sal_constant_sizes),
                    fact_node,
                    &mut queue,
                );
            }
            for constant in &constants {
                let root_node = dependency_diagnostics.add_root(
                    "constant",
                    &constant.name,
                    &constant.root.tu,
                    &constant.spelling,
                );
                if allow_enum_omissions {
                    dependency_diagnostics.set_root_origin(root_node, &constant.root);
                }
                collect_pointer_alias_candidates(
                    &constant.ty,
                    constant.root.tu.as_str(),
                    &callback_pointer_alias_requirements,
                    &mut pointer_alias_candidates,
                );
                queue.push((
                    constant.root.tu.as_str(),
                    TypeEdge::Type(&constant.ty),
                    root_node,
                ));
            }
            for function in &functions {
                let fact = function.fact;
                let root_node = dependency_diagnostics.add_root(
                    "function",
                    &fact.name,
                    &fact.origin.tu,
                    &fact.expansion,
                );
                if allow_enum_omissions {
                    dependency_diagnostics.set_root_origin(root_node, &fact.origin);
                }
                let fact_node = DependencyNode::Fact(&fact.origin);
                dependency_diagnostics.add_edge(root_node, fact_node);
                collect_fact_pointer_alias_candidates(
                    fact,
                    &callback_pointer_alias_requirements,
                    &mut pointer_alias_candidates,
                );
                queue_function_edges(
                    fact,
                    &self.function_annotations,
                    Some(&self.sal_constant_sizes),
                    fact_node,
                    &mut queue,
                );
            }
            for failure in &enum_root_failures {
                dependency_diagnostics.block_enum_failure(failure, None, &facts_by_declaration);
            }
            if allow_enum_omissions {
                let selected_nodes: HashMap<_, _> = dependency_diagnostics
                    .root_origins
                    .iter()
                    .map(|(index, origin)| {
                        let node = if dependency_diagnostics.roots[*index].kind == "constant" {
                            DependencyNode::Root(*index)
                        } else {
                            DependencyNode::Fact(origin)
                        };
                        (*origin, node)
                    })
                    .collect();
                let mut enum_targets: HashMap<&str, Vec<&Fact>> = HashMap::new();
                for fact in type_roots
                    .iter()
                    .copied()
                    .chain(
                        enum_root_failures
                            .iter()
                            .flat_map(|failure| failure.declarations.iter().copied()),
                    )
                    .chain(
                        source_names
                            .into_iter()
                            .flat_map(|names| names.shared_enum_providers.iter())
                            .filter(|(_, provider)| provider.is_err())
                            .map(|(origin, _)| facts_by_origin[origin]),
                    )
                {
                    if underlying_enum_fact(fact, &facts_by_declaration, &mut BTreeSet::new())
                        .is_some()
                    {
                        enum_targets.entry(&fact.name).or_default().push(fact);
                    }
                }
                for (target, annotations) in selected_annotations.iter().flatten() {
                    let Some(&source) = selected_nodes.get(target.origin()) else {
                        continue;
                    };
                    for annotation in annotations {
                        let Annotation::AssociatedEnum(name) = annotation else {
                            continue;
                        };
                        for fact in enum_targets.get(name.as_str()).into_iter().flatten() {
                            dependency_diagnostics
                                .add_edge(source, DependencyNode::Fact(&fact.origin));
                        }
                    }
                }
            }

            while let Some((tu, edge, source)) = queue.pop() {
                phase.visit(None);
                let value_type = match edge {
                    TypeEdge::Use(ty, annotations) => Some((ty, annotations, false, false)),
                    TypeEdge::Parameter(param, annotations, size) => Some((
                        &param.ty,
                        annotations,
                        param.annotation.size.is_some() || size.is_some(),
                        false,
                    )),
                    TypeEdge::Type(ty)
                        if !matches!(
                            source,
                            DependencyNode::Fact(origin)
                                if matches!(facts_by_origin[origin].data, FactData::Typedef { .. })
                        ) =>
                    {
                        Some((ty, &[][..], false, false))
                    }
                    TypeEdge::Pointee(ty, _) => Some((ty, &[][..], false, true)),
                    _ => None,
                };
                if let Some((ty, annotations, counted, pointer)) = value_type
                    && let Err(error) = com_bindings.validate_use(
                        ty,
                        tu,
                        annotations,
                        counted,
                        pointer,
                        &com_context,
                    )
                {
                    let mut reference = ty;
                    while let TypeRef::Pointer { target, .. }
                    | TypeRef::Reference { target, .. }
                    | TypeRef::Array { target, .. } = reference
                    {
                        reference = target;
                    }
                    if self.ranked_header_partition_policy
                        && let TypeRef::Named { name, declaration } = reference
                    {
                        dependency_diagnostics.block(
                            DependencyReference {
                                name,
                                tu,
                                declaration: Some(declaration),
                            },
                            error.to_string(),
                            source,
                        );
                        continue;
                    }
                    return Err(error);
                }
                let (ty, parent_mutability) = match edge {
                    TypeEdge::Type(ty) => (ty, None),
                    TypeEdge::Pointee(ty, mutable) => (ty, Some(mutable)),
                    TypeEdge::Use(ty, annotations) => {
                        let choice = if mutable_string_aliases {
                            string_type_choice(
                                ty,
                                tu,
                                None,
                                annotations,
                                &facts_by_declaration,
                                source_names,
                            )?
                        } else {
                            None
                        }
                        .unwrap_or(TypeChoice::Declared(ty));
                        type_choices.insert((tu, edge), choice);
                        queue.push((tu, choice.edge(), source));
                        continue;
                    }
                    TypeEdge::Parameter(param, annotations, resolved_size) => {
                        let choice = if mutable_string_aliases {
                            string_type_choice(
                                &param.ty,
                                tu,
                                Some(&param.annotation),
                                annotations,
                                &facts_by_declaration,
                                source_names,
                            )
                            .map_err(|error| {
                                parameter_use_error(error, param, source, &facts_by_origin)
                            })?
                            .unwrap_or(TypeChoice::Declared(&param.ty))
                        } else {
                            parameter_string_name(param).map_or(
                                TypeChoice::Declared(&param.ty),
                                |name| TypeChoice::Canonical {
                                    name,
                                    mutable: false,
                                },
                            )
                        };
                        let selected_size = resolved_size
                            .map(|(target, size)| {
                                self.selected_sal_constant_size(
                                    target,
                                    size,
                                    &facts_by_origin,
                                    &sal_constant_observations,
                                )
                            })
                            .transpose()
                            .map_err(|error| {
                                parameter_use_error(error, param, source, &facts_by_origin)
                            })?;
                        let failure = if let Some(Err(error)) = selected_size {
                            Some(error.clone())
                        } else {
                            match selected_size
                                .and_then(|size| size.as_ref().ok())
                                .or(param.annotation.size.as_ref())
                                .map(|size| &size.value)
                            {
                                Some(SalSizeValue::Expression(value)) => {
                                    Some(SalCountError::Unsupported(format!(
                                        "cannot preserve SAL count expression `{value}` \
                                     in translation unit `{tu}`"
                                    )))
                                }
                                Some(SalSizeValue::Constant(value)) if *value < 0 => Some(
                                    SalCountError::Invalid(format!("negative SAL count `{value}`")),
                                ),
                                _ => None,
                            }
                        };
                        if let Some(failure) = failure {
                            let error = parameter_use_error(
                                Error(failure.to_string()),
                                param,
                                source,
                                &facts_by_origin,
                            );
                            if matches!(failure, SalCountError::Invalid(_))
                                || matches!(choice, TypeChoice::String { .. })
                            {
                                return Err(error);
                            }
                            let DependencyNode::Fact(origin) = source else {
                                return Err(error);
                            };
                            sal_count_diagnostics.insert(SalCountDiagnostic {
                                declaration: origin.clone(),
                                source: facts_by_origin[origin].spelling.clone(),
                                parameter: param.name.clone(),
                                reason: error.to_string(),
                            });
                        }
                        type_choices.insert((tu, edge), choice);
                        queue.push((tu, choice.edge(), source));
                        continue;
                    }
                    TypeEdge::Projected(name) => {
                        let reference = DependencyReference {
                            name,
                            tu,
                            declaration: None,
                        };
                        dependency_diagnostics.process(reference);
                        if references.contains_key(name) && !root_names.contains(name) {
                            dependency_diagnostics.resolve(reference);
                            continue;
                        }
                        let matches: Vec<_> = facts_index
                            .get(name)
                            .into_iter()
                            .flatten()
                            .copied()
                            .filter(|fact| fact.origin.tu == tu && is_type_fact(fact))
                            .collect();
                        let fact = match choose_type_root_cached(
                            name,
                            &matches,
                            &facts_index,
                            &mut shape_cache,
                        ) {
                            Ok(fact) => fact,
                            Err(error) => {
                                let error =
                                    if mutable_string_aliases && matches!(name, "PSTR" | "PWSTR") {
                                        Error(format!(
                                            "string projection in translation unit `{tu}`: {error}"
                                        ))
                                    } else {
                                        error
                                    };
                                if self.header_partition_policy {
                                    if allow_enum_omissions
                                        && let Some(failure) = enum_selection_failure(
                                            &matches,
                                            &facts_by_declaration,
                                            &facts_by_origin,
                                            self,
                                            source_names,
                                            error.to_string(),
                                        )
                                    {
                                        dependency_diagnostics.block_enum_failure(
                                            &failure,
                                            Some(source),
                                            &facts_by_declaration,
                                        );
                                    } else {
                                        dependency_diagnostics.block(
                                            reference,
                                            error.to_string(),
                                            source,
                                        );
                                    }
                                    continue;
                                }
                                return Err(error);
                            }
                        };
                        if mutable_string_aliases && matches!(name, "PSTR" | "PWSTR") {
                            let valid = if let FactData::Typedef { target } = &fact.data {
                                native_string_type(target, tu, &facts_by_declaration, source_names)
                                    .is_some_and(|native| native.name == name && native.mutable)
                            } else {
                                false
                            };
                            if !valid {
                                let error = Error(format!(
                                    "string projection requires `{name}` to be a native mutable \
                                     single-character pointer typedef in translation unit `{tu}`"
                                ));
                                if self.header_partition_policy {
                                    dependency_diagnostics.block(
                                        reference,
                                        error.to_string(),
                                        source,
                                    );
                                    continue;
                                }
                                return Err(error);
                            }
                        }
                        dependency_diagnostics.resolve(reference);
                        if (self.header_partition_policy || mutable_string_aliases)
                            && canonical_string_name(name).is_some()
                            && is_pointer_alias_fact(
                                fact,
                                &facts_by_declaration,
                                &mut BTreeSet::new(),
                            )
                        {
                            retained_pointer_aliases.insert(fact.name.as_str());
                        }
                        let fact_node = DependencyNode::Fact(&fact.origin);
                        dependency_diagnostics.add_edge(source, fact_node);
                        if facts.insert(fact.origin.clone()) {
                            collect_fact_pointer_alias_candidates(
                                fact,
                                &callback_pointer_alias_requirements,
                                &mut pointer_alias_candidates,
                            );
                            queue_type_edges(
                                fact,
                                &self.projected_type_names,
                                &self.annotations,
                                Some(&self.sal_constant_sizes),
                                fact_node,
                                &mut queue,
                            );
                        }
                        continue;
                    }
                };
                let (name, declaration) = match ty {
                    TypeRef::Pointer { .. } => {
                        if mutable_string_aliases {
                            let mut current = ty;
                            let mut previous = None;
                            while let TypeRef::Pointer { mutable, target } = current {
                                if previous.is_some_and(|previous| previous != *mutable) {
                                    return Err(Error(format!(
                                        "mixed raw pointer constness in translation unit `{tu}` \
                                         needs a native typedef boundary; RDL cannot represent \
                                         different const qualifiers at each pointer rank"
                                    )));
                                }
                                previous = Some(*mutable);
                                current = target;
                            }
                        }
                        let (mutable, _, target) = pointer_run(ty);
                        queue.push((tu, TypeEdge::Pointee(target, mutable), source));
                        continue;
                    }
                    TypeRef::Reference { mutable, target } => {
                        queue.push((tu, TypeEdge::Pointee(target, *mutable), source));
                        continue;
                    }
                    TypeRef::FunctionPointer { params, result, .. } if allow_enum_omissions => {
                        queue.push((tu, TypeEdge::Type(result), source));
                        for param in params {
                            queue.push((tu, TypeEdge::Type(param), source));
                        }
                        continue;
                    }
                    TypeRef::FunctionPointer { .. } | TypeRef::OpaquePointer { .. } => continue,
                    TypeRef::Array { target, .. } => {
                        queue.push((tu, TypeEdge::Type(target), source));
                        continue;
                    }
                    TypeRef::Generic { args, .. } => {
                        for arg in args {
                            queue.push((tu, TypeEdge::Type(arg), source));
                        }
                        continue;
                    }
                    TypeRef::InlineRecord(record) => {
                        for field in &record.fields {
                            queue.push((tu, TypeEdge::Use(&field.ty, &[]), source));
                        }
                        continue;
                    }
                    TypeRef::Named { name, declaration } => (name, declaration),
                    _ => continue,
                };
                type_choices.insert((tu, edge), TypeChoice::Declared(ty));
                let reference = DependencyReference {
                    name,
                    tu,
                    declaration: Some(declaration),
                };
                dependency_diagnostics.process(reference);
                if excluded_local_names.contains(&(tu.to_string(), name.clone()))
                    && !canonical_typedef_declarations.contains(&(tu, declaration))
                {
                    dependency_diagnostics.resolve(reference);
                    continue;
                }
                let exact_local_dependency = self.header_partition_policy
                    && facts_index
                        .get(name.as_str())
                        .into_iter()
                        .chain(exact_dependency_facts_index.get(name.as_str()))
                        .flatten()
                        .copied()
                        .any(|fact| {
                            fact.origin.tu == tu
                                && fact.spelling == *declaration
                                && is_type_declaration_fact(fact)
                                && !self.suppressed_type_origins.contains(&fact.origin)
                                && ((self.pointer_only_class_layouts.contains_key(&fact.origin)
                                    && matches!(fact.data, FactData::Record { .. }))
                                    || is_native_namespaced_declaration(fact, &facts_by_origin))
                        });
                if references.contains_key(name)
                    && !root_names.contains(name)
                    && !exact_local_dependency
                {
                    dependency_diagnostics.resolve(reference);
                    continue;
                }
                let canonical_string = canonical_string_name(name);
                if canonical_string.is_some_and(|canonical| {
                    references.contains_key(canonical)
                        && !root_names.contains(name)
                        && !root_names.contains(canonical)
                }) {
                    dependency_diagnostics.resolve(reference);
                    continue;
                }
                let pointer_alias_candidate =
                    pointer_alias_candidates.contains(&(tu, declaration, name.as_str()));
                let retain_pointer_alias = (self.header_partition_policy || mutable_string_aliases)
                    && (pointer_alias_candidate || canonical_string.is_some());
                if let Some(canonical_string) = canonical_string
                    && canonical_typedef_declarations.contains(&(tu, declaration))
                    && !(retain_pointer_alias && canonical_pointer_alias_name(name))
                {
                    type_choices.insert(
                        (tu, edge),
                        TypeChoice::Canonical {
                            name: canonical_string,
                            mutable: false,
                        },
                    );
                    dependency_diagnostics.resolve(reference);
                    continue;
                }
                if let Err(error) =
                    self.require_declaration_header_authority(tu, name, declaration, &declarations)
                {
                    dependency_diagnostics.block(reference, error.to_string(), source);
                    continue;
                }
                let mut matches: Vec<_> = facts_index
                    .get(name.as_str())
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|fact| {
                        fact.origin.tu == tu && fact.spelling == *declaration && is_type_fact(fact)
                    })
                    .collect();
                let mut exact_non_flat = false;
                if matches.is_empty() && self.header_partition_policy {
                    matches.extend(
                        exact_dependency_facts_index
                            .get(name.as_str())
                            .into_iter()
                            .flatten()
                            .copied()
                            .filter(|fact| {
                                fact.origin.tu == tu
                                    && fact.spelling == *declaration
                                    && is_type_declaration_fact(fact)
                            }),
                    );
                    exact_non_flat = !matches.is_empty();
                }
                if matches.is_empty() {
                    matches.extend(
                        facts_index
                            .get(name.as_str())
                            .into_iter()
                            .flatten()
                            .copied()
                            .filter(|fact| {
                                fact.origin.tu == tu
                                    && fact.spelling.file == declaration.file
                                    && is_type_fact(fact)
                            }),
                    );
                }
                if matches.is_empty()
                    && let Some(owner) = self
                        .root_partitions
                        .get(&(tu.to_string(), declaration.file.clone()))
                {
                    matches.extend(
                        facts_index
                            .get(name.as_str())
                            .into_iter()
                            .flatten()
                            .copied()
                            .filter(|fact| is_type_fact(fact))
                            .filter(|fact| {
                                self.root_owners.get(&fact.origin).is_some_and(|candidate| {
                                    candidate.partition == owner.partition
                                        && candidate.namespace == owner.namespace
                                }) || self
                                    .root_partitions
                                    .get(&(fact.origin.tu.clone(), fact.spelling.file.clone()))
                                    .is_some_and(|candidate| {
                                        candidate.partition == owner.partition
                                            && candidate.namespace == owner.namespace
                                    })
                            }),
                    );
                }
                let selected = match matches.as_slice() {
                    [fact] => *fact,
                    [] => match self.unresolved_local_type_error(name, tu, declaration) {
                        error if self.header_partition_policy => {
                            dependency_diagnostics.block(reference, error.to_string(), source);
                            continue;
                        }
                        error => return Err(error),
                    },
                    choices if exact_non_flat => match choose_type_root_cached(
                        name,
                        choices,
                        &exact_dependency_facts_index,
                        &mut shape_cache,
                    ) {
                        Ok(fact) => fact,
                        Err(error) if self.header_partition_policy => {
                            if allow_enum_omissions
                                && let Some(failure) = enum_selection_failure(
                                    choices,
                                    &facts_by_declaration,
                                    &facts_by_origin,
                                    self,
                                    source_names,
                                    error.to_string(),
                                )
                            {
                                dependency_diagnostics.block_enum_failure(
                                    &failure,
                                    Some(source),
                                    &facts_by_declaration,
                                );
                            } else {
                                dependency_diagnostics.block(reference, error.to_string(), source);
                            }
                            continue;
                        }
                        Err(error) => return Err(error),
                    },
                    choices => {
                        match choose_type_root_cached(name, choices, &facts_index, &mut shape_cache)
                        {
                            Ok(fact) => fact,
                            Err(error) if self.header_partition_policy => {
                                if allow_enum_omissions
                                    && let Some(failure) = enum_selection_failure(
                                        choices,
                                        &facts_by_declaration,
                                        &facts_by_origin,
                                        self,
                                        source_names,
                                        error.to_string(),
                                    )
                                {
                                    dependency_diagnostics.block_enum_failure(
                                        &failure,
                                        Some(source),
                                        &facts_by_declaration,
                                    );
                                } else {
                                    dependency_diagnostics.block(
                                        reference,
                                        error.to_string(),
                                        source,
                                    );
                                }
                                continue;
                            }
                            Err(error) => return Err(error),
                        }
                    }
                };
                let mut fact = selected;
                if self.ranked_header_partition_policy {
                    fact = match com_bindings.authoritative_type(
                        &com_context,
                        fact,
                        &facts_index,
                        &mut shape_cache,
                    ) {
                        Ok(fact) => fact,
                        Err(error) => {
                            dependency_diagnostics.block(reference, error.to_string(), source);
                            continue;
                        }
                    };
                }
                let com_provider = com_bindings.provider_for(selected, &facts_index);
                if let Some(provider) = &com_provider {
                    let provider = *provider
                        .as_ref()
                        .map_err(|failure| Error(failure.message(name)))?;
                    if provider.name == fact.name {
                        fact = provider;
                        if matches!(edge, TypeEdge::Pointee(..)) {
                            type_choices.insert((tu, edge), TypeChoice::ComInterface(ty));
                        }
                    }
                }
                let complete_com_record = typedef_tag_declaration(selected, &facts_index)
                    .or(Some(selected))
                    .is_some_and(|target| {
                        self.unproved_com_records.contains(&target.origin)
                            || (matches!(target.data, FactData::Record { .. })
                                && self
                                    .native_com_contracts
                                    .get(&target.origin)
                                    .is_some_and(|contract| contract.has_fields))
                    });
                if !self.ranked_header_partition_policy
                    && !self.root_owners.contains_key(&fact.origin)
                    && com_provider.is_none()
                    && !complete_com_record
                    && let Some(guid) = declaration_uuid(self, &declarations, name, tu, declaration)
                {
                    let owned: Vec<_> = facts_index
                        .get(name.as_str())
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(|candidate| is_type_fact(candidate))
                        .filter(|candidate| self.fact_uuid(candidate) == Some(guid))
                        .filter(|candidate| self.root_owners.contains_key(&candidate.origin))
                        .collect();
                    let owners: BTreeSet<_> = owned
                        .iter()
                        .filter_map(|candidate| self.root_owners.get(&candidate.origin))
                        .map(|owner| (&owner.partition, &owner.namespace))
                        .collect();
                    if owners.len() > 1 {
                        let error = Error(format!(
                            "local type `{name}` at {}:{} matches UUID `{guid}` in multiple tagged owners",
                            declaration.file, declaration.offset
                        ));
                        if self.header_partition_policy {
                            dependency_diagnostics.block(reference, error.to_string(), source);
                            continue;
                        }
                        return Err(error);
                    }
                    if !owned.is_empty() {
                        fact = match choose_type_root_cached(
                            name,
                            &owned,
                            &facts_index,
                            &mut shape_cache,
                        ) {
                            Ok(fact) => fact,
                            Err(error) if self.header_partition_policy => {
                                dependency_diagnostics.block(reference, error.to_string(), source);
                                continue;
                            }
                            Err(error) => return Err(error),
                        };
                    }
                }
                if let FactData::Unsupported { reason } = &fact.data {
                    let error = Error(format!(
                        "unsupported type `{name}` in translation unit `{tu}`: {reason}"
                    ));
                    if self.header_partition_policy {
                        dependency_diagnostics.block(reference, error.to_string(), source);
                        continue;
                    }
                    return Err(error);
                }
                if !is_type_fact(fact) {
                    let error = self.unresolved_local_type_error(name, tu, declaration);
                    if self.header_partition_policy {
                        dependency_diagnostics.block(reference, error.to_string(), source);
                        continue;
                    }
                    return Err(error);
                };
                if allow_enum_omissions
                    && matches!(fact.data, FactData::Enum { .. })
                    && let Some(failure) = failed_enum_sources.get(&(
                        source_names.map_or(fact.name.as_str(), |names| names.fact_name(fact)),
                        &fact.spelling,
                    ))
                {
                    dependency_diagnostics.block_enum_fact(
                        fact,
                        failure.cause,
                        &failure.reason,
                        Some(source),
                    );
                    continue;
                }
                if matches!(fact.data, FactData::Enum { .. })
                    && let Some(provider) = source_names
                        .filter(|names| {
                            !names
                                .excluded_native_dependencies
                                .get(&fact.origin)
                                .is_some_and(|binding| binding.conflicting)
                        })
                        .and_then(|names| names.shared_enum_providers.get(&fact.origin))
                {
                    match provider {
                        Ok(origin) => fact = facts_by_origin[origin],
                        Err(error) => {
                            if allow_enum_omissions {
                                dependency_diagnostics.block_enum_fact(
                                    fact,
                                    error.cause(),
                                    error.message(),
                                    Some(source),
                                );
                                continue;
                            }
                            dependency_diagnostics.block(reference, error.message(), source);
                            continue;
                        }
                    }
                }
                if let Some(source_names) = source_names
                    && let Some(binding) =
                        source_names.excluded_native_dependencies.get(&fact.origin)
                {
                    let mut reason = if binding.conflicting {
                        format!("conflicting exact native source authority for `{name}`")
                    } else {
                        format!(
                            "owner-excluded native dependency `{name}` is required without a \
                             retained public alias"
                        )
                    };
                    if let DependencyNode::Fact(origin) = source {
                        let parent = facts_by_origin[origin];
                        write!(
                            reason,
                            "\n   exact dependency path: `{}` at {}:{} -> `{}` at {}:{} \
                             in translation unit `{tu}`",
                            source_names.fact_name(parent),
                            parent.spelling.file,
                            parent.spelling.offset,
                            source_names.fact_name(fact),
                            fact.spelling.file,
                            fact.spelling.offset,
                        )
                        .unwrap();
                    }
                    for origin in &binding.authorities {
                        let authority = facts_by_origin[origin];
                        let owner = &self.root_owners[origin];
                        write!(
                            reason,
                            "\n   source authority: partition `{}` namespace `{}`, input `{}`, \
                             declaration {}:{}, origin {:?}",
                            owner.partition,
                            owner.namespace,
                            owner.input,
                            authority.spelling.file,
                            authority.spelling.offset,
                            authority.origin,
                        )
                        .unwrap();
                    }
                    if binding.conflicting {
                        reason.push_str(
                            "\n   native declaration/payload/parent context or source owner \
                             policies do not establish one excluded authority",
                        );
                    }
                    if allow_enum_omissions {
                        dependency_diagnostics.block_enum_fact(
                            fact,
                            if binding.conflicting {
                                EnumOmissionCause::ConflictingDeclarations
                            } else {
                                EnumOmissionCause::ExcludedDependency
                            },
                            &reason,
                            Some(source),
                        );
                    } else {
                        dependency_diagnostics.block(reference, reason, source);
                    }
                    continue;
                }
                let native_name = source_fact_name(fact);
                let canonical = canonical_named_type(&native_name);
                let string_alias =
                    is_string_alias_fact(fact, &facts_by_declaration, &mut BTreeSet::new());
                let retain_pointer_alias = if string_alias {
                    retain_pointer_alias
                } else {
                    parent_mutability
                        .zip(canonical_raw_pointer_mutability(&native_name))
                        .is_some_and(|(parent, alias)| parent != alias)
                };
                if let Some(canonical) = canonical
                    && matches!(fact.data, FactData::Typedef { .. })
                    && !retain_pointer_alias
                {
                    type_choices.insert(
                        (tu, edge),
                        TypeChoice::Canonical {
                            name: canonical,
                            mutable: canonical_raw_pointer_mutability(&native_name) == Some(true),
                        },
                    );
                    dependency_diagnostics.resolve(reference);
                    continue;
                }
                dependency_diagnostics.resolve(reference);
                if (retain_pointer_alias
                    || (self.header_partition_policy && canonical.is_none() && !string_alias))
                    && is_pointer_alias_fact(fact, &facts_by_declaration, &mut BTreeSet::new())
                {
                    retained_pointer_aliases.insert(fact.name.as_str());
                }
                let fact_node = DependencyNode::Fact(&fact.origin);
                dependency_diagnostics.add_edge(source, fact_node);
                if facts.insert(fact.origin.clone()) {
                    collect_fact_pointer_alias_candidates(
                        fact,
                        &callback_pointer_alias_requirements,
                        &mut pointer_alias_candidates,
                    );
                    queue_type_edges(
                        fact,
                        &self.projected_type_names,
                        &self.annotations,
                        Some(&self.sal_constant_sizes),
                        fact_node,
                        &mut queue,
                    );
                }
            }

            if dependency_diagnostics.is_blocked() && !allow_enum_omissions {
                if timing && self.header_partition_policy {
                    eprintln!(
                        "windows-clang timing phase=plan-dependencies target={target} \
                         selected_roots={} processed_unique_dependencies={} \
                         resolved_dependencies={} unique_blockers={} elapsed_ms={:.3}",
                        dependency_diagnostics.selected_roots(),
                        dependency_diagnostics.processed_references(),
                        dependency_diagnostics.resolved_references(),
                        dependency_diagnostics.unique_blockers(),
                        elapsed_ms(phase_time),
                    );
                }
                return Err(dependency_diagnostics.error());
            }
            let known_blockers = dependency_diagnostics.unique_blockers();
            let known_affected = if allow_enum_omissions && dependency_diagnostics.is_blocked() {
                Some(dependency_diagnostics.enum_affected(
                    self,
                    source_names,
                    omission_namespace,
                    &facts_by_origin,
                    &declarations,
                )?)
            } else {
                None
            };
            let mut grouped: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
            for fact in facts
                .into_iter()
                .map(|origin| facts_by_origin[&origin])
                .filter(|fact| {
                    !known_affected
                        .as_ref()
                        .is_some_and(|affected| affected.origins.contains(&fact.origin))
                })
            {
                grouped.entry(&fact.name).or_default().push(fact);
            }

            let collisions: Vec<_> = constants
                .iter()
                .filter(|constant| {
                    !known_affected.as_ref().is_some_and(|affected| {
                        affected.roots.contains(&DependencyRoot {
                            kind: "constant",
                            name: &constant.name,
                            tu: &constant.root.tu,
                            source: &constant.spelling,
                        })
                    })
                })
                .filter_map(|constant| {
                    grouped
                        .get(constant.name.as_str())
                        .map(|choices| (constant, choices))
                })
                .collect();
            if !collisions.is_empty() {
                let mut added = false;
                for (constant, choices) in collisions {
                    let root = choose_type_root_cached(
                        &constant.name,
                        choices,
                        &facts_index,
                        &mut shape_cache,
                    )
                    .map_err(|error| {
                        aggregate_root_selection_errors(vec![type_root_selection_error(
                            self,
                            &constant.name,
                            "constant-collision",
                            error,
                            choices,
                            choices,
                            &facts_index,
                        )])
                    })?;
                    if root_names.insert(constant.name.clone()) {
                        type_roots.push(root);
                        added = true;
                    }
                }
                if added {
                    continue;
                }
            }

            let mut facts_by_name = BTreeMap::new();
            phase.next("plan-contextual-types");
            for (name, choices) in grouped {
                phase.visit(None);
                let authority = self.authority_candidates(&choices, &declarations);
                let selected = match choose_proven_type_root(
                    &com_context,
                    name,
                    &authority,
                    &facts_index,
                    &mut shape_cache,
                    source_names,
                    &com_bindings,
                ) {
                    Ok(fact) => fact,
                    Err(error) => {
                        if allow_enum_omissions
                            && let Some(failure) = enum_selection_failure(
                                &authority,
                                &facts_by_declaration,
                                &facts_by_origin,
                                self,
                                source_names,
                                error.to_string(),
                            )
                        {
                            dependency_diagnostics.block_enum_failure(
                                &failure,
                                None,
                                &facts_by_declaration,
                            );
                            continue;
                        }
                        return Err(aggregate_root_selection_errors(vec![
                            type_root_selection_error(
                                self,
                                name,
                                "post-closure",
                                error,
                                &choices,
                                &authority,
                                &facts_index,
                            ),
                        ]));
                    }
                };
                facts_by_name.insert(name, selected);
            }
            if allow_enum_omissions && dependency_diagnostics.is_blocked() {
                let affected = match known_affected {
                    Some(affected)
                        if dependency_diagnostics.unique_blockers() == known_blockers =>
                    {
                        affected
                    }
                    _ => dependency_diagnostics.enum_affected(
                        self,
                        source_names,
                        omission_namespace,
                        &facts_by_origin,
                        &declarations,
                    )?,
                };
                omissions.extend(affected.omissions);
                facts_by_name.retain(|_, fact| !affected.origins.contains(&fact.origin));
                type_roots.retain(|fact| !affected.origins.contains(&fact.origin));
                value_roots.retain(|fact| !affected.origins.contains(&fact.origin));
                functions.retain(|function| !affected.origins.contains(&function.fact.origin));
                constants.retain(|constant| {
                    !affected.roots.contains(&DependencyRoot {
                        kind: "constant",
                        name: &constant.name,
                        tu: &constant.root.tu,
                        source: &constant.spelling,
                    })
                });
                for root in affected.roots {
                    root_names.remove(root.name);
                }
            }
            if timing && self.header_partition_policy {
                eprintln!(
                    "windows-clang timing phase=plan-dependencies target={target} \
                     selected_roots={} processed_unique_dependencies={} \
                     resolved_dependencies={} unique_blockers={} omissions={} elapsed_ms={:.3}",
                    dependency_diagnostics.selected_roots(),
                    dependency_diagnostics.processed_references(),
                    dependency_diagnostics.resolved_references(),
                    dependency_diagnostics.unique_blockers(),
                    omissions.len(),
                    elapsed_ms(phase_time),
                );
                phase_time = Some(std::time::Instant::now());
            }
            break (
                facts_by_name,
                retained_pointer_aliases,
                dependency_diagnostics,
            );
        };
        phase.next("plan-layout-closure");
        loop {
            let retained_names: BTreeSet<_> = retained_pointer_aliases
                .iter()
                .map(|name| canonical_string_name(name).unwrap_or(name))
                .collect();
            let additions: Vec<_> = facts_by_name
                .iter()
                .filter(|(name, _)| !retained_pointer_aliases.contains(**name))
                .filter_map(|(name, fact)| {
                    let FactData::Typedef { target } = &fact.data else {
                        return None;
                    };
                    if matches!(
                        target,
                        TypeRef::Named { name, .. } if canonical_string_name(name).is_some()
                    ) {
                        return None;
                    }
                    type_ref_uses_alias(target, &retained_names).then_some(*name)
                })
                .collect();
            if additions.is_empty() {
                break;
            }
            retained_pointer_aliases.extend(additions);
        }
        let mut validated_layouts: HashSet<_> = facts_by_name
            .values()
            .filter(|fact| {
                !matches!(fact.data, FactData::Typedef { .. })
                    && !matches!(fact.data, FactData::Record { .. } if !fact.definition)
            })
            .map(|fact| fact.origin.clone())
            .collect();
        let mut safe_layouts: HashMap<String, HashSet<Location>> = HashMap::new();
        for fact in facts_index.values().flatten().copied().filter(|fact| {
            !matches!(fact.data, FactData::Typedef { .. })
                && !matches!(fact.data, FactData::Record { .. } if !fact.definition)
        }) {
            safe_layouts
                .entry(fact.origin.tu.clone())
                .or_default()
                .insert(fact.spelling.clone());
        }
        loop {
            let additions: Vec<_> = facts_index
                .values()
                .flatten()
                .copied()
                .filter(|fact| {
                    !safe_layouts
                        .get(&fact.origin.tu)
                        .is_some_and(|safe| safe.contains(&fact.spelling))
                })
                .filter_map(|fact| match &fact.data {
                    FactData::Typedef { target }
                        if known_complete_layout(
                            target,
                            &fact.origin.tu,
                            &safe_layouts,
                            references,
                        ) =>
                    {
                        Some((fact.origin.tu.clone(), fact.spelling.clone()))
                    }
                    _ => None,
                })
                .collect();
            if additions.is_empty() {
                break;
            }
            for (tu, declaration) in additions {
                safe_layouts.entry(tu).or_default().insert(declaration);
            }
        }
        let layout = LayoutContext {
            facts_index: &facts_index,
            planned_types: &facts_by_name,
            pointer_only_class_layouts: &self.pointer_only_class_layouts,
            embeddable_class_layouts: &self.embeddable_class_layouts,
            display_names,
        };
        let validation_time = timing.then(std::time::Instant::now);
        phase.next("plan-validate-types");
        for fact in facts_by_name.values() {
            phase.visit(None);
            validate_fact_layouts(fact, &layout, &mut safe_layouts, &mut validated_layouts)?;
            validate_fact_value_abi(fact, &layout)?;
        }
        for fact in &value_roots {
            phase.visit(None);
            validate_fact_layouts(fact, &layout, &mut safe_layouts, &mut validated_layouts)?;
            validate_fact_value_abi(fact, &layout)?;
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-validate-types target={target} elapsed_ms={:.3}",
                elapsed_ms(validation_time)
            );
        }
        let validation_time = timing.then(std::time::Instant::now);
        phase.next("plan-validate-functions");
        for function in &functions {
            phase.visit(Some(functions.len()));
            validate_fact_layouts(
                function.fact,
                &layout,
                &mut safe_layouts,
                &mut validated_layouts,
            )?;
            validate_fact_value_abi(function.fact, &layout)?;
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-validate-functions target={target} elapsed_ms={:.3}",
                elapsed_ms(validation_time)
            );
        }
        let validation_time = timing.then(std::time::Instant::now);
        phase.next("plan-validate-constants");
        for constant in &constants {
            phase.visit(Some(constants.len()));
            validate_complete_layout(
                &constant.ty,
                &constant.root.tu,
                &layout,
                &mut safe_layouts,
                &mut BTreeSet::new(),
                &mut validated_layouts,
            )?;
            validate_pointer_only_class_value(
                &constant.ty,
                &constant.root.tu,
                &layout,
                &mut BTreeSet::new(),
                NativeClassValueUse::Abi,
            )?;
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-validate-constants target={target} elapsed_ms={:.3}",
                elapsed_ms(validation_time)
            );
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-closure target={target} elapsed_ms={:.3}",
                elapsed_ms(phase_time)
            );
            phase_time = Some(std::time::Instant::now());
        }

        let mut local_roots = root_names.clone();
        phase.next("plan-aliases");
        local_roots.extend(
            facts_by_name
                .iter()
                .filter(|(name, fact)| {
                    (self.pointer_only_class_layouts.contains_key(&fact.origin)
                        && matches!(fact.data, FactData::Record { .. }))
                        || (references.contains_key(**name)
                            && is_native_namespaced_declaration(fact, &facts_by_origin))
                })
                .map(|(name, _)| (*name).to_string()),
        );
        let required: BTreeSet<String> = facts_by_name
            .keys()
            .map(|name| (*name).to_string())
            .collect();
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-required target={target} elapsed_ms={:.3}",
                elapsed_ms(phase_time)
            );
            phase_time = Some(std::time::Instant::now());
        }

        let mut enum_alias_candidates: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for fact in &self.facts {
            if let FactData::Typedef {
                target: TypeRef::Named { name, declaration },
            } = &fact.data
                && name != &fact.name
                && !references.contains_key(name)
                && let Some(selected) = facts_by_name.get(name.as_str())
                && facts_index
                    .get(name.as_str())
                    .into_iter()
                    .flatten()
                    .any(|target| {
                        target.origin.tu == fact.origin.tu
                            && target.spelling == *declaration
                            && target.spelling.file == fact.spelling.file
                            && target.definition
                            && (matches!(
                                target.data,
                                FactData::Enum { .. } | FactData::Record { .. }
                            ) || (matches!(target.data, FactData::Interface { .. })
                                && selected.name.starts_with('_')))
                            && same_source_declaration(selected, target)
                    })
            {
                enum_alias_candidates
                    .entry(selected.name.as_str())
                    .or_default()
                    .push(fact.name.as_str());
            }
        }
        let mut type_names: BTreeMap<_, _> = enum_alias_candidates
            .into_iter()
            .filter_map(|(target, aliases)| {
                let aliases: BTreeSet<_> = aliases.into_iter().collect();
                if aliases.len() != 1 {
                    return None;
                }
                Some((target.to_string(), (*aliases.first().unwrap()).to_string()))
            })
            .collect();
        for name in &retained_pointer_aliases {
            type_names
                .entry((*name).to_string())
                .or_insert_with(|| canonical_string_name(name).unwrap_or(name).to_string());
        }
        for name in &extended_reference_enums {
            let enum_names: BTreeSet<_> = self
                .facts
                .iter()
                .filter(|fact| fact.name == *name)
                .filter_map(|fact| {
                    underlying_enum_fact(fact, &facts_by_declaration, &mut BTreeSet::new())
                })
                .map(|fact| fact.name.as_str())
                .collect();
            if let [enum_name] = enum_names.into_iter().collect::<Vec<_>>().as_slice() {
                type_names.insert((*enum_name).to_string(), name.clone());
            }
        }
        for fact in &self.facts {
            if let FactData::Typedef {
                target: TypeRef::Pointer { target, .. },
            } = &fact.data
                && let TypeRef::Named { name, .. } = target.as_ref()
                && let Some(public_name) = name.strip_prefix('_')
                && fact.name == format!("P{public_name}")
                && !references.contains_key(name)
                && facts_index
                    .get(name.as_str())
                    .into_iter()
                    .flatten()
                    .any(|target| target.origin.tu == fact.origin.tu)
            {
                let public_aliases_private = facts_index
                    .get(public_name)
                    .into_iter()
                    .flatten()
                    .any(|candidate| {
                        matches!(
                            &candidate.data,
                            FactData::Typedef {
                                target: TypeRef::Named {
                                    name: target_name,
                                    ..
                                },
                            } if target_name == name
                        )
                    });
                if !public_aliases_private
                    && facts_index
                        .get(public_name)
                        .into_iter()
                        .flatten()
                        .any(|candidate| {
                            matches!(
                                candidate.data,
                                FactData::Class { .. }
                                    | FactData::Callback { .. }
                                    | FactData::Enum { .. }
                                    | FactData::Interface { .. }
                                    | FactData::Record { .. }
                            ) || defines_local_type(public_name, candidate)
                        })
                {
                    continue;
                }
                type_names
                    .entry(name.clone())
                    .or_insert_with(|| public_name.to_string());
            }
        }
        let mut external_alias_candidates: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        if let Some(excluded) = excluded_types {
            for fact in &self.facts {
                if !excluded.contains(&fact.name) {
                    continue;
                }
                let Some(reference) = references.get(&fact.name) else {
                    continue;
                };
                if extended_reference_enums.contains(&fact.name) {
                    continue;
                }
                if let FactData::Typedef {
                    target: TypeRef::Named { name, .. },
                } = &fact.data
                {
                    if references.contains_key(name) || named_type_shape(name).is_some() {
                        continue;
                    }
                    external_alias_candidates
                        .entry(name)
                        .or_default()
                        .insert(format!(
                            "{}::{}",
                            reference.namespace.replace('.', "::"),
                            reference.name
                        ));
                }
            }
        }
        for (name, aliases) in external_alias_candidates {
            if let [alias] = aliases.into_iter().collect::<Vec<_>>().as_slice() {
                type_names
                    .entry(name.to_string())
                    .or_insert_with(|| alias.clone());
            }
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-aliases target={target} elapsed_ms={:.3}",
                elapsed_ms(phase_time)
            );
            phase_time = Some(std::time::Instant::now());
        }
        for (name, reference) in references {
            let excluded = excluded_types.is_some_and(|excluded| excluded.contains(name))
                && !extended_reference_enums.contains(name);
            if excluded {
                type_names.insert(
                    name.clone(),
                    format!(
                        "{}::{}",
                        reference.namespace.replace('.', "::"),
                        reference.name
                    ),
                );
            } else if !local_roots.contains(name) {
                type_names.entry(name.clone()).or_insert_with(|| {
                    format!(
                        "{}::{}",
                        reference.namespace.replace('.', "::"),
                        reference.name
                    )
                });
            }
        }
        let mut internal_type_names = BTreeMap::new();
        phase.next("plan-interfaces");
        let mut internal_aliases = BTreeSet::new();
        if !self.root_partitions.is_empty() || self.header_partition_policy {
            loop {
                let mut added = false;
                for (name, fact) in &facts_by_name {
                    if root_names.contains(*name)
                        || internal_aliases.contains(*name)
                        || retained_pointer_aliases.contains(*name)
                        || references.contains_key(*name)
                        || self.fact_authority_namespace(fact, &declarations).is_some()
                        || facts_index
                            .get(*name)
                            .into_iter()
                            .flatten()
                            .any(|candidate| self.root_owners.contains_key(&candidate.origin))
                    {
                        continue;
                    }
                    let FactData::Typedef { target } = &fact.data else {
                        continue;
                    };
                    if !internal_alias_target_resolved(target, &internal_aliases) {
                        continue;
                    }
                    let interface_names = BTreeSet::new();
                    let local_types = BTreeMap::new();
                    let routed_types = BTreeMap::new();
                    let projection = TypeProjection::new(
                        &internal_type_names,
                        &interface_names,
                        &fact.origin.tu,
                        &local_types,
                        &routed_types,
                        (&type_choices, None),
                        None,
                    );
                    let target_name = planned_emitted_type_name(target, &projection);
                    let target_name = type_names.get(&target_name).cloned().unwrap_or(target_name);
                    internal_type_names.insert((*name).to_string(), target_name);
                    internal_aliases.insert((*name).to_string());
                    added = true;
                }
                if !added {
                    break;
                }
            }
        }
        type_names.extend(internal_type_names);
        for (name, fact) in &facts_by_name {
            if required.contains(*name)
                && canonical_named_type(name).is_some()
                && !matches!(fact.data, FactData::Typedef { .. })
            {
                type_names
                    .entry((*name).to_string())
                    .or_insert_with(|| (*name).to_string());
            }
        }
        let alias_names: BTreeSet<_> = type_names
            .iter()
            .filter_map(|(source, target)| (source != target).then_some(target.as_str()))
            .collect();
        if self.header_partition_policy {
            let blockers: Vec<_> = facts_by_name
                .iter()
                .filter(|(name, _)| required.contains(**name))
                .filter(|(name, _)| !internal_aliases.contains(**name))
                .filter(|(name, _)| !alias_names.contains(*name))
                .filter_map(|(_, fact)| {
                    let name = type_names
                        .get(fact.name.as_str())
                        .map_or(fact.name.as_str(), String::as_str);
                    (self.suppressed_type_origins.contains(&fact.origin)
                        && name == fact.name
                        && self.fact_authority_namespace(fact, &declarations).is_none())
                    .then_some(*fact)
                })
                .collect();
            if !blockers.is_empty() {
                if allow_enum_omissions
                    && blockers
                        .iter()
                        .all(|fact| matches!(fact.data, FactData::Enum { .. }))
                {
                    for fact in blockers {
                        dependency_diagnostics.block_enum_fact(
                            fact,
                            EnumOmissionCause::ExcludedDependency,
                            "owner-excluded enum has no retained public alias",
                            None,
                        );
                    }
                    let affected = dependency_diagnostics.enum_affected(
                        self,
                        source_names,
                        omission_namespace,
                        &facts_by_origin,
                        &declarations,
                    )?;
                    omissions.extend(affected.omissions);
                    facts_by_name.retain(|_, fact| !affected.origins.contains(&fact.origin));
                    value_roots.retain(|fact| !affected.origins.contains(&fact.origin));
                    functions.retain(|function| !affected.origins.contains(&function.fact.origin));
                    constants.retain(|constant| {
                        !affected.roots.contains(&DependencyRoot {
                            kind: "constant",
                            name: &constant.name,
                            tu: &constant.root.tu,
                            source: &constant.spelling,
                        })
                    });
                } else {
                    return Err(owner_exclusion_error(
                        self,
                        &dependency_diagnostics,
                        &blockers,
                        display_names,
                    ));
                }
            }
        }
        let mut types: Vec<_> = facts_by_name
            .into_iter()
            .filter(|(name, _)| required.contains(*name))
            .filter(|(name, _)| !internal_aliases.contains(*name))
            .filter(|(name, _)| !alias_names.contains(*name))
            .map(|(_, fact)| {
                let name = type_names
                    .get(fact.name.as_str())
                    .cloned()
                    .unwrap_or_else(|| fact.name.clone());
                if !self.header_partition_policy
                    && self.suppressed_type_origins.contains(&fact.origin)
                    && name == fact.name
                    && self.fact_authority_namespace(fact, &declarations).is_none()
                {
                    return if let Some(owner) = self.root_owners.get(&fact.origin) {
                        Err(Error(format!(
                            "owner-excluded local type `{}` in partition `{}` namespace `{}` is \
                             required without a retained public alias",
                            fact.name, owner.partition, owner.namespace
                        )))
                    } else {
                        Err(Error(format!(
                            "owner-excluded local type `{}` in translation unit `{}` is required \
                             without a retained public alias or logical owner",
                            fact.name, fact.origin.tu
                        )))
                    };
                }
                Ok(PlannedFact { name, fact })
            })
            .collect::<Result<_, Error>>()?;
        let values: Vec<_> = value_roots
            .into_iter()
            .map(|fact| PlannedFact {
                name: fact.name.clone(),
                fact,
            })
            .collect();
        let mut interface_names = BTreeSet::new();
        for planned in &types {
            if matches!(planned.fact.data, FactData::Interface { .. }) {
                for fact in facts_index
                    .get(planned.fact.name.as_str())
                    .into_iter()
                    .flatten()
                {
                    if matches!(fact.data, FactData::Interface { .. })
                        && same_source_declaration(planned.fact, fact)
                    {
                        interface_names.insert((fact.origin.tu.clone(), fact.name.clone()));
                    }
                }
            }
        }
        let translation_units: BTreeSet<_> = self
            .facts
            .iter()
            .map(|fact| fact.origin.tu.as_str())
            .collect();
        for (name, reference) in references {
            if reference.kind == TypeReferenceKind::Interface
                && (excluded_types.is_some_and(|excluded| excluded.contains(name))
                    || !local_roots.contains(name))
            {
                interface_names.extend(
                    translation_units
                        .iter()
                        .map(|tu| ((*tu).to_string(), name.clone())),
                );
            }
        }
        let mut interface_aliases: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for fact in &self.facts {
            let FactData::Typedef {
                target: TypeRef::Named { name: target, .. } | TypeRef::Generic { name: target, .. },
            } = &fact.data
            else {
                continue;
            };
            if interface_names.contains(&(fact.origin.tu.clone(), target.clone())) {
                interface_aliases
                    .entry((fact.origin.tu.clone(), target.clone()))
                    .or_default()
                    .push(fact.name.clone());
            }
        }
        let mut interface_queue: Vec<_> = interface_names.iter().cloned().collect();
        while let Some(key) = interface_queue.pop() {
            phase.visit(None);
            for alias in interface_aliases.get(&key).into_iter().flatten() {
                let alias = (key.0.clone(), alias.clone());
                if interface_names.insert(alias.clone()) {
                    interface_queue.push(alias);
                }
            }
        }
        let mut pointer_interface_aliases = BTreeMap::new();
        for fact in &self.facts {
            let FactData::Typedef {
                target:
                    TypeRef::Pointer {
                        mutable,
                        target: native_target,
                    },
            } = &fact.data
            else {
                continue;
            };
            let (TypeRef::Named { name: target, .. } | TypeRef::Generic { name: target, .. }) =
                native_target.as_ref()
            else {
                continue;
            };
            if !interface_names.contains(&(fact.origin.tu.clone(), target.clone()))
                && !matches!(
                    type_choices.get(&(
                        fact.origin.tu.as_str(),
                        TypeEdge::Pointee(native_target, *mutable),
                    )),
                    Some(TypeChoice::ComInterface(_))
                )
            {
                continue;
            }
            let projected = type_names
                .get(target)
                .cloned()
                .unwrap_or_else(|| target.clone());
            if let Some(previous) =
                pointer_interface_aliases.insert(fact.name.clone(), projected.clone())
                && previous != projected
            {
                return Err(Error(format!(
                    "interface pointer alias `{}` has conflicting targets",
                    fact.name
                )));
            }
        }
        let mut pointer_aliases: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for fact in &self.facts {
            let FactData::Typedef {
                target: TypeRef::Named { name: target, .. } | TypeRef::Generic { name: target, .. },
            } = &fact.data
            else {
                continue;
            };
            pointer_aliases.entry(target).or_default().push(&fact.name);
        }
        let mut pointer_alias_queue: Vec<_> = pointer_interface_aliases.keys().cloned().collect();
        while let Some(target) = pointer_alias_queue.pop() {
            phase.visit(None);
            let projected = pointer_interface_aliases[&target].clone();
            for alias in pointer_aliases.get(target.as_str()).into_iter().flatten() {
                if let Some(previous) = pointer_interface_aliases.get(*alias) {
                    if previous != &projected {
                        return Err(Error(format!(
                            "interface pointer alias `{alias}` has conflicting targets"
                        )));
                    }
                } else {
                    pointer_interface_aliases.insert((*alias).to_string(), projected.clone());
                    pointer_alias_queue.push((*alias).to_string());
                }
            }
        }
        for (alias, target) in &pointer_interface_aliases {
            type_names.insert(alias.clone(), target.clone());
        }
        types.retain(|planned| !pointer_interface_aliases.contains_key(&planned.fact.name));
        let local_types = BTreeMap::new();
        let routed_types = BTreeMap::new();
        phase.next("plan-constant-projection");
        let mut constants: Vec<_> = constants
            .into_iter()
            .filter_map(|constant| {
                phase.visit(None);
                let ty = match &constant.value {
                    Value::Utf8(_) | Value::Utf16(_) => Some("String".to_string()),
                    _ => {
                        let projection = TypeProjection::new(
                            &type_names,
                            &interface_names,
                            &constant.root.tu,
                            &local_types,
                            &routed_types,
                            (&type_choices, None),
                            None,
                        );
                        constant_type_name(&constant.ty, &pointer_interface_aliases, &projection)
                    }
                }?;
                Some(PlannedConstant { constant, ty })
            })
            .collect();
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-interfaces target={target} elapsed_ms={:.3}",
                elapsed_ms(phase_time)
            );
            phase_time = Some(std::time::Instant::now());
        }
        phase.next("plan-finalize");
        let mut flag_enums: BTreeSet<_> = self
            .facts
            .iter()
            .filter_map(|fact| {
                if let FactData::EnumFlag { target } = &fact.data {
                    Some((fact.origin.tu.clone(), target.clone()))
                } else {
                    None
                }
            })
            .collect();
        flag_enums.extend(
            self.facts
                .iter()
                .filter(|fact| self.forced_flags.contains(&fact.origin))
                .map(|fact| (fact.origin.tu.clone(), fact.name.clone())),
        );
        let mut type_output_names = BTreeSet::new();
        for planned in &types {
            if !type_output_names.insert(planned.name.as_str()) {
                return Err(Error(format!("duplicate planned name `{}`", planned.name)));
            }
        }
        let mut value_output_names = BTreeSet::new();
        for planned in &values {
            if !value_output_names.insert(planned.name.as_str()) {
                return Err(Error(format!("duplicate planned name `{}`", planned.name)));
            }
        }
        for planned in &constants {
            if !value_output_names.insert(planned.constant.name.as_str()) {
                return Err(Error(format!(
                    "duplicate planned name `{}`",
                    planned.constant.name
                )));
            }
        }
        for function in &functions {
            if type_output_names.contains(function.name.as_str())
                || !value_output_names.insert(function.name.as_str())
            {
                return Err(Error(format!("duplicate planned name `{}`", function.name)));
            }
        }
        constants.sort_by(|left, right| left.constant.name.cmp(&right.constant.name));
        functions.sort_by(|left, right| left.name.cmp(&right.name));
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-finalize target={target} elapsed_ms={:.3}",
                elapsed_ms(phase_time)
            );
        }
        let DefinitionObservations {
            rows: definition_evidence,
            coverage: definition_evidence_coverage,
            notes: definition_notes,
        } = observe(
            self,
            &com_bindings,
            &types,
            &interface_guids,
            &flag_enums,
            source_names,
        );
        Ok(Plan {
            definition_evidence,
            definition_evidence_coverage,
            definition_notes,
            omissions: omissions.into_iter().collect(),
            types,
            values,
            functions,
            constants,
            type_names,
            interface_names,
            type_choices,
            sal_constant_sizes: Some(&self.sal_constant_sizes),
            sal_count_diagnostics,
            pointer_interface_aliases,
            interface_guids,
            flag_enums,
        })
    }
}

impl HeaderPartitionPlan {
    /// Reports unrepresented size hints using this plan's header and selection policies.
    pub fn sal_count_diagnostics(
        &self,
        options: &EmitOptions<'_>,
    ) -> Result<Vec<SalCountDiagnostic>, Error> {
        let (mut snapshot, display_names, source_names) = self
            .snapshot
            .clone()
            .into_partitioned_planning_snapshot(options);
        snapshot.project_suppressed_declare_handles();
        Ok(snapshot
            .plan_partitioned(options, &display_names, &source_names)?
            .sal_count_diagnostics
            .into_iter()
            .collect())
    }

    pub fn audit(&self, options: &EmitOptions<'_>) -> Result<PartitionAudit, Error> {
        let (mut snapshot, display_names, source_names) = self
            .snapshot
            .clone()
            .into_partitioned_planning_snapshot(options);
        snapshot.project_suppressed_declare_handles();
        let plan = snapshot.plan_partitioned(options, &display_names, &source_names)?;
        let candidates = snapshot.partition_route_candidates(&plan, &source_names, options)?;
        let mut conflicts = self.root_conflicts.clone();
        conflicts.extend(snapshot.partition_route_conflicts(&candidates));
        conflicts.sort();
        conflicts.dedup();
        Ok(PartitionAudit { conflicts })
    }

    pub fn emit_with_options(
        self,
        options: &EmitOptions<'_>,
    ) -> Result<BTreeMap<RdlPartition, String>, Error> {
        Ok(self.emit_with_report(options)?.partitions)
    }

    /// Emits strict metadata with structured definition evidence and comparison coverage.
    ///
    /// Selected declaration and dependency failures remain errors; this does not allow omissions.
    /// Unknown evidence is returned without an RDL difference note.
    pub fn emit_with_report(
        self,
        options: &EmitOptions<'_>,
    ) -> Result<HeaderPartitionReport, Error> {
        self.emit_report(options, false)
    }

    /// Emits valid partial metadata for enum conflicts and reports every affected declaration.
    ///
    /// An affected record or interface is omitted whole, including all interface methods.
    /// Non-enum planning failures and invalid configuration remain errors.
    pub fn emit_with_enum_diagnostics(
        self,
        options: &EmitOptions<'_>,
    ) -> Result<HeaderPartitionReport, Error> {
        self.emit_report(options, true)
    }

    fn emit_report(
        self,
        options: &EmitOptions<'_>,
        allow_enum_omissions: bool,
    ) -> Result<HeaderPartitionReport, Error> {
        if allow_enum_omissions {
            validate_namespace(options.namespace)?;
        }
        let target = self.snapshot.timing_target.clone();
        let mut phase = TimingPhase::new(target.as_deref(), "partition-reconciliation");
        let (mut snapshot, display_names, source_names) =
            self.snapshot.into_partitioned_planning_snapshot(options);
        snapshot.project_suppressed_declare_handles();
        phase.next("partition-planning");
        let plan = phase.check(snapshot.plan_partitioned_with_enum_policy(
            options,
            &display_names,
            &source_names,
            allow_enum_omissions,
        ))?;
        let omissions = plan.omissions.clone();
        let definition_evidence = plan.definition_evidence.clone();
        let definition_evidence_coverage = plan.definition_evidence_coverage.clone();
        let ambiguity_summary = plan.function_ambiguity_summary();
        phase.next("partition-routing");
        let candidates =
            phase.check(snapshot.partition_route_candidates(&plan, &source_names, options))?;
        let mut conflicts = self.root_conflicts;
        conflicts.extend(snapshot.partition_route_conflicts(&candidates));
        conflicts.sort();
        conflicts.dedup();
        let audit = PartitionAudit { conflicts };
        if audit.has_blocking_conflicts() {
            return phase.check(Err(Error(audit.to_string())));
        }
        let routes = phase.check(snapshot.resolve_partition_routes(candidates))?;
        phase.event("complete", None);
        let result = snapshot.format_partitioned_plan(
            plan,
            options,
            &routes,
            &display_names,
            target.as_deref(),
        )?;
        if let Some(summary) = ambiguity_summary {
            eprintln!("{summary}");
        }
        if !audit.is_clean() {
            eprintln!("{audit}");
        }
        Ok(HeaderPartitionReport {
            partitions: result,
            omissions,
            definition_evidence,
            definition_evidence_coverage,
        })
    }
}

fn function_ambiguity_note(name: &str, ambiguity: &FunctionAmbiguity<'_>) -> String {
    let mut result = format!(
        "    // windows-clang: function redeclarations symbol={name:?} selection=input-order\n"
    );
    for declaration in &ambiguity.declarations {
        let source = function_declaration_source(declaration.fact);
        result.push_str(&format!(
            "    // windows-clang: declaration selected={} input={:?} source={source:?} \
             parent={:?} signature={:?} annotations={:?}\n",
            declaration.selected,
            declaration.fact.origin.tu,
            declaration.parent,
            declaration.fact.data,
            declaration.annotations
        ));
    }
    result
}

fn function_declaration_source(fact: &Fact) -> String {
    format!(
        "{}:{}",
        normalize_name(&fact.spelling.file),
        fact.spelling.offset
    )
}

fn write_rdl<'a>(
    namespace: &str,
    items: impl IntoIterator<Item = &'a str>,
) -> Result<String, Error> {
    let namespaces: Vec<_> = namespace
        .split('.')
        .filter(|name| !name.is_empty())
        .collect();
    if namespaces.is_empty() {
        return Err(Error("namespace is empty".to_string()));
    }
    let mut result = String::from("#[win32]\n");
    for (depth, namespace) in namespaces.iter().enumerate() {
        result.push_str(&format!(
            "{}mod {} {{\n",
            "    ".repeat(depth),
            rdl_ident(namespace)
        ));
    }
    let indent = "    ".repeat(namespaces.len() - 1);
    for item in items {
        for line in item.lines() {
            result.push_str(&indent);
            result.push_str(line);
            result.push('\n');
        }
    }
    for depth in (0..namespaces.len()).rev() {
        result.push_str(&format!("{}}}\n", "    ".repeat(depth)));
    }
    Ok(result)
}

struct PlannedFact<'a> {
    fact: &'a Fact,
    name: String,
}

struct PlannedConstant<'a> {
    constant: &'a Constant,
    ty: String,
}

struct PlannedFunction<'a> {
    fact: &'a Fact,
    name: String,
    declarations: Vec<&'a Fact>,
    ambiguity: Option<FunctionAmbiguity<'a>>,
}

struct FunctionRootSelection<'a> {
    fact: &'a Fact,
    declarations: Vec<&'a Fact>,
    ambiguity: Option<FunctionAmbiguity<'a>>,
}

struct FunctionAmbiguity<'a> {
    declarations: Vec<FunctionDeclarationVariant<'a>>,
}

struct FunctionDeclarationVariant<'a> {
    fact: &'a Fact,
    selected: bool,
    parent: Option<FunctionParentPath>,
    annotations: Rc<RouteAnnotations>,
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum OutputKind {
    Value,
    Type,
}

struct Plan<'a> {
    definition_evidence: Vec<DefinitionEvidence>,
    definition_evidence_coverage: DefinitionEvidenceCoverage,
    definition_notes: HashMap<Origin, String>,
    omissions: Vec<MetadataOmission>,
    types: Vec<PlannedFact<'a>>,
    values: Vec<PlannedFact<'a>>,
    functions: Vec<PlannedFunction<'a>>,
    constants: Vec<PlannedConstant<'a>>,
    type_names: BTreeMap<String, String>,
    interface_names: BTreeSet<(String, String)>,
    type_choices: BTreeMap<(&'a str, TypeEdge<'a>), TypeChoice<'a>>,
    sal_constant_sizes: Option<&'a SalConstantSizes>,
    sal_count_diagnostics: BTreeSet<SalCountDiagnostic>,
    pointer_interface_aliases: BTreeMap<String, String>,
    interface_guids: BTreeMap<String, String>,
    flag_enums: BTreeSet<(String, String)>,
}

impl<'a> Plan<'a> {
    fn function_ambiguities(&self) -> impl Iterator<Item = (&str, &FunctionAmbiguity<'a>)> {
        self.functions.iter().filter_map(|function| {
            function
                .ambiguity
                .as_ref()
                .map(|ambiguity| (function.name.as_str(), ambiguity))
        })
    }

    fn function_ambiguity_summary(&self) -> Option<String> {
        let ambiguities = self.function_ambiguities().collect::<Vec<_>>();
        if ambiguities.is_empty() {
            return None;
        }
        let inventory = ambiguities
            .iter()
            .map(|(name, ambiguity)| {
                let declarations = ambiguity
                    .declarations
                    .iter()
                    .map(|declaration| {
                        format!(
                            "selected={} input={:?} source={:?}",
                            declaration.selected,
                            declaration.fact.origin.tu,
                            function_declaration_source(declaration.fact)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name:?}[{declarations}]")
            })
            .collect::<Vec<_>>()
            .join("; ");
        Some(format!(
            "windows-clang: function redeclarations count={} inventory={inventory}",
            ambiguities.len()
        ))
    }
}

struct PlanningOptions<'a> {
    references: &'a BTreeMap<String, TypeReference>,
    excluded_types: Option<&'a BTreeSet<String>>,
    excluded_functions: Option<&'a BTreeSet<String>>,
    excluded_constants: Option<&'a BTreeSet<String>>,
    selected_functions: Option<&'a BTreeSet<String>>,
    display_names: Option<&'a BTreeMap<String, String>>,
    source_names: Option<&'a PlanningSourceNames>,
    mutable_string_aliases: bool,
    allow_enum_omissions: bool,
    omission_namespace: &'a str,
}

struct ExcludedNativeDependency {
    authorities: Vec<Origin>,
    conflicting: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct SharedEnumDefinition {
    enum_usr: String,
    alias_usr: String,
    tokens: Vec<String>,
    data: FactData,
}

#[derive(Clone, Debug, PartialEq)]
struct SharedEnumIdentity {
    definition: Arc<SharedEnumDefinition>,
    enumeration: Origin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SharedEnumProviderFailure {
    ConflictingDefinitions,
    ConflictingOwners,
    MissingProvider,
}

impl SharedEnumProviderFailure {
    fn message(self) -> &'static str {
        match self {
            Self::ConflictingDefinitions => "conflicting native shared-enum literal definitions",
            Self::ConflictingOwners => "shared native enum has conflicting public namespace owners",
            Self::MissingProvider => "shared native enum has no eligible public provider",
        }
    }

    fn cause(self) -> EnumOmissionCause {
        match self {
            Self::MissingProvider => EnumOmissionCause::ExcludedDependency,
            Self::ConflictingDefinitions | Self::ConflictingOwners => {
                EnumOmissionCause::ConflictingDeclarations
            }
        }
    }
}

struct PlanningSourceNames {
    facts: BTreeMap<Origin, String>,
    constants: BTreeMap<(Origin, Origin, Location), BTreeSet<String>>,
    excluded_native_dependencies: BTreeMap<Origin, ExcludedNativeDependency>,
    shared_enum_providers: BTreeMap<Origin, Result<Origin, SharedEnumProviderFailure>>,
    #[cfg(test)]
    exclusion_index_sizes: (usize, usize),
}

impl PlanningSourceNames {
    fn new(snapshot: &Snapshot) -> Self {
        let shared_enum_providers = Self::shared_enum_providers(snapshot);
        let mut excluded_native_dependencies = BTreeMap::new();
        #[cfg(test)]
        let mut exclusion_index_sizes = (0, 0);
        if snapshot.header_partition_policy {
            let excluded_groups: HashSet<_> = snapshot
                .facts
                .iter()
                .filter(|fact| {
                    fact.definition
                        && matches!(fact.data, FactData::Enum { .. })
                        && snapshot
                            .root_owners
                            .get(&fact.origin)
                            .is_some_and(|owner| owner.exclusions.contains(&fact.name))
                })
                .map(|fact| {
                    (
                        std::mem::discriminant(&fact.kind),
                        fact.name.as_str(),
                        &fact.spelling,
                    )
                })
                .collect();
            if !excluded_groups.is_empty() {
                let mut authorities: HashMap<_, Vec<&Fact>> = HashMap::new();
                let mut parents = HashMap::new();
                for fact in &snapshot.facts {
                    if matches!(
                        fact.kind,
                        FactKind::Namespace | FactKind::Class | FactKind::Struct | FactKind::Union
                    ) {
                        parents
                            .entry(&fact.origin)
                            .and_modify(|entry| *entry = None)
                            .or_insert(Some(fact));
                    }
                    let key = (
                        std::mem::discriminant(&fact.kind),
                        fact.name.as_str(),
                        &fact.spelling,
                    );
                    if fact.definition
                        && matches!(fact.data, FactData::Enum { .. })
                        && snapshot.root_owners.contains_key(&fact.origin)
                        && excluded_groups.contains(&key)
                    {
                        authorities.entry(key).or_default().push(fact);
                    }
                }
                #[cfg(test)]
                {
                    exclusion_index_sizes =
                        (authorities.values().map(Vec::len).sum(), parents.len());
                }
                for fact in &snapshot.facts {
                    if !fact.definition
                        || !matches!(fact.data, FactData::Enum { .. })
                        || snapshot.root_owners.contains_key(&fact.origin)
                    {
                        continue;
                    }
                    let Some(choices) = authorities.get(&(
                        std::mem::discriminant(&fact.kind),
                        fact.name.as_str(),
                        &fact.spelling,
                    )) else {
                        continue;
                    };
                    let context = Self::raw_enum_parent_context(fact, &parents, snapshot);
                    let conflicting = choices.len() != 1
                        || context.is_none()
                        || choices.iter().any(|candidate| {
                            !same_source_declaration(fact, candidate)
                                || match (
                                    &context,
                                    Self::raw_enum_parent_context(candidate, &parents, snapshot),
                                ) {
                                    (Some(left), Some(right)) => {
                                        left.len() != right.len()
                                            || left.iter().zip(&right).any(|(left, right)| {
                                                !same_source_declaration(left, right)
                                            })
                                    }
                                    _ => true,
                                }
                        });
                    excluded_native_dependencies.insert(
                        fact.origin.clone(),
                        ExcludedNativeDependency {
                            authorities: choices.iter().map(|fact| fact.origin.clone()).collect(),
                            conflicting,
                        },
                    );
                }
            }
        }
        let facts = snapshot
            .facts
            .iter()
            .map(|fact| (fact.origin.clone(), fact.name.clone()))
            .collect();
        let mut constants: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for constant in &snapshot.constants {
            constants
                .entry((
                    constant.root.clone(),
                    constant.definition.clone(),
                    constant.spelling.clone(),
                ))
                .or_default()
                .insert(constant.name.clone());
        }
        Self {
            facts,
            constants,
            excluded_native_dependencies,
            shared_enum_providers,
            #[cfg(test)]
            exclusion_index_sizes,
        }
    }

    fn shared_enum_providers(
        snapshot: &Snapshot,
    ) -> BTreeMap<Origin, Result<Origin, SharedEnumProviderFailure>> {
        let mut result = BTreeMap::new();
        if !snapshot.header_partition_policy || snapshot.shared_enum_identities.is_empty() {
            return result;
        }
        let facts: BTreeMap<_, _> = snapshot
            .facts
            .iter()
            .filter(|fact| {
                matches!(fact.kind, FactKind::Enum | FactKind::Typedef)
                    && snapshot.shared_enum_identities.contains_key(&fact.origin)
                    && (!snapshot.ranked_header_partition_policy
                        || fact_header_authority(&snapshot.header_authorities, fact)
                            .is_none_or(|authority| fact.origin.tu == authority.owner.input))
            })
            .map(|fact| (&fact.origin, fact))
            .collect();
        let mut groups: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (origin, identity) in &snapshot.shared_enum_identities {
            if facts.contains_key(origin) {
                groups
                    .entry((
                        &identity.definition.enum_usr,
                        &identity.definition.alias_usr,
                    ))
                    .or_default()
                    .push((origin, identity));
            }
        }
        for members in groups.values() {
            let declarations: BTreeSet<_> = members
                .iter()
                .filter_map(|(_, identity)| {
                    facts.get(&identity.enumeration).map(|fact| &fact.spelling)
                })
                .collect();
            if declarations.len() <= 1 {
                continue;
            }
            let cohort: Vec<_> = members
                .iter()
                .copied()
                .filter(|(origin, _)| {
                    !snapshot.ranked_header_partition_policy
                        || facts.get(*origin).is_some_and(|fact| {
                            fact_header_authority(&snapshot.header_authorities, fact).is_some()
                        })
                })
                .collect();
            let matches_definition =
                |identity: &SharedEnumIdentity, definition: &SharedEnumDefinition| {
                    identity.definition.as_ref() == definition
                        && facts
                            .get(&identity.enumeration)
                            .is_some_and(|fact| fact.data == identity.definition.data)
                };
            let first = cohort
                .first()
                .map(|(_, identity)| identity.definition.as_ref());
            let compatible = first.is_none_or(|first| {
                cohort
                    .iter()
                    .all(|(_, identity)| matches_definition(identity, first))
            });
            let providers: Vec<_> = cohort
                .iter()
                .filter_map(|(origin, identity)| {
                    if **origin != identity.enumeration {
                        return None;
                    }
                    let fact = facts.get(*origin)?;
                    let owner = snapshot.root_owners.get(*origin)?;
                    (fact.root
                        && !owner.exclusions.contains(&fact.name)
                        && !snapshot.suppressed_type_origins.contains(*origin))
                    .then_some((fact, owner))
                })
                .collect();
            let selected = if !compatible {
                Err(SharedEnumProviderFailure::ConflictingDefinitions)
            } else if let Some((provider, owner)) = providers.first() {
                if providers
                    .iter()
                    .any(|(_, candidate)| !same_owner_policy(candidate, owner))
                {
                    Err(SharedEnumProviderFailure::ConflictingOwners)
                } else {
                    Ok(provider.origin.clone())
                }
            } else {
                Err(SharedEnumProviderFailure::MissingProvider)
            };
            for (origin, identity) in members {
                let admission = if selected.is_ok()
                    && first.is_some_and(|first| !matches_definition(identity, first))
                {
                    Err(SharedEnumProviderFailure::ConflictingDefinitions)
                } else {
                    selected.clone()
                };
                result.insert((*origin).clone(), admission);
            }
        }
        result
    }

    fn raw_enum_parent_context<'a>(
        fact: &Fact,
        parents: &HashMap<&Origin, Option<&'a Fact>>,
        snapshot: &Snapshot,
    ) -> Option<Vec<&'a Fact>> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        let mut parent = fact.parent.as_ref();
        while let Some(origin) = parent {
            if origin.tu != fact.origin.tu || !seen.insert(origin) {
                return None;
            }
            let current = parents.get(origin).copied().flatten()?;
            if current.name.is_empty()
                || current.spelling.file.is_empty()
                || snapshot.observation_only_class_origins.contains(origin)
                || snapshot.class_canonical_anchors.contains_key(origin)
                || snapshot
                    .class_canonical_origins
                    .get(origin)
                    .is_some_and(|canonical| {
                        matches!(canonical, CanonicalRecord::CompilerGenerated { .. })
                            || snapshot
                                .class_canonical_anchors
                                .contains_key(canonical.anchor())
                    })
            {
                return None;
            }
            result.push(current);
            parent = current.parent.as_ref();
        }
        Some(result)
    }

    fn fact_name<'a>(&'a self, fact: &'a Fact) -> &'a str {
        self.facts
            .get(&fact.origin)
            .map_or(fact.name.as_str(), String::as_str)
    }

    fn constant_name<'a>(&'a self, constant: &'a Constant) -> &'a str {
        self.constants
            .get(&(
                constant.root.clone(),
                constant.definition.clone(),
                constant.spelling.clone(),
            ))
            .filter(|names| names.len() == 1)
            .and_then(BTreeSet::first)
            .map_or(constant.name.as_str(), String::as_str)
    }
}

fn partition_collision_typedef_target(
    fact: &Fact,
    declarations: &DeclarationIndex<'_>,
) -> Option<TypeRef> {
    let FactData::Typedef { target } = &fact.data else {
        return None;
    };
    Some(partition_collision_type(
        target,
        &fact.origin.tu,
        declarations,
        &mut BTreeSet::new(),
    ))
}

fn partition_collision_equivalent(
    left: &Fact,
    right: &Fact,
    declarations: &DeclarationIndex<'_>,
) -> bool {
    left.data == right.data
        || partition_collision_typedef_target(left, declarations)
            .zip(partition_collision_typedef_target(right, declarations))
            .is_some_and(|(left, right)| left == right)
}

fn partition_collision_type(
    ty: &TypeRef,
    tu: &str,
    declarations: &DeclarationIndex<'_>,
    seen: &mut BTreeSet<(String, String, Location)>,
) -> TypeRef {
    match ty {
        TypeRef::Named { name, declaration } => {
            let key = (tu.to_string(), name.clone(), declaration.clone());
            if !seen.insert(key.clone()) {
                return ty.clone();
            }
            let targets: BTreeSet<_> = declarations
                .get(tu, name, declaration)
                .iter()
                .filter_map(|fact| match &fact.data {
                    FactData::Typedef { target } => Some(target),
                    _ => None,
                })
                .collect();
            let result = if let [target] = targets.into_iter().collect::<Vec<_>>().as_slice() {
                partition_collision_type(target, tu, declarations, seen)
            } else {
                ty.clone()
            };
            seen.remove(&key);
            result
        }
        TypeRef::Pointer { mutable, target } => TypeRef::Pointer {
            mutable: *mutable,
            target: Box::new(partition_collision_type(target, tu, declarations, seen)),
        },
        TypeRef::Reference { mutable, target } => TypeRef::Reference {
            mutable: *mutable,
            target: Box::new(partition_collision_type(target, tu, declarations, seen)),
        },
        TypeRef::FunctionPointer {
            convention,
            params,
            result,
        } => TypeRef::FunctionPointer {
            convention: *convention,
            params: params
                .iter()
                .map(|param| partition_collision_type(param, tu, declarations, seen))
                .collect(),
            result: Box::new(partition_collision_type(result, tu, declarations, seen)),
        },
        TypeRef::Array { target, len } => TypeRef::Array {
            target: Box::new(partition_collision_type(target, tu, declarations, seen)),
            len: *len,
        },
        TypeRef::Generic {
            name,
            declaration,
            args,
        } => TypeRef::Generic {
            name: name.clone(),
            declaration: declaration.clone(),
            args: args
                .iter()
                .map(|arg| partition_collision_type(arg, tu, declarations, seen))
                .collect(),
        },
        TypeRef::InlineRecord(record) => {
            let mut record = (**record).clone();
            record.base = record
                .base
                .as_ref()
                .map(|base| partition_collision_type(base, tu, declarations, seen));
            for field in &mut record.fields {
                field.ty = partition_collision_type(&field.ty, tu, declarations, seen);
            }
            TypeRef::InlineRecord(Box::new(record))
        }
        _ => ty.clone(),
    }
}

fn scoped_planning_map<'a, T: Clone>(
    values: &'a BTreeMap<String, T>,
    display_names: &BTreeMap<String, String>,
) -> Cow<'a, BTreeMap<String, T>> {
    if !display_names
        .iter()
        .any(|(internal, display)| !values.contains_key(internal) && values.contains_key(display))
    {
        return Cow::Borrowed(values);
    }
    let mut scoped = values.clone();
    for (internal, display) in display_names {
        if let Some(value) = values.get(display) {
            scoped
                .entry(internal.clone())
                .or_insert_with(|| value.clone());
        }
    }
    Cow::Owned(scoped)
}

fn scoped_planning_set<'a>(
    values: Option<&'a BTreeSet<String>>,
    display_names: &BTreeMap<String, String>,
) -> Option<Cow<'a, BTreeSet<String>>> {
    let values = values?;
    if !display_names
        .iter()
        .any(|(internal, display)| !values.contains(internal) && values.contains(display))
    {
        return Some(Cow::Borrowed(values));
    }
    let mut scoped = values.clone();
    for (internal, display) in display_names {
        if values.contains(display) {
            scoped.insert(internal.clone());
        }
    }
    Some(Cow::Owned(scoped))
}

type RouteAnnotations = Vec<(RouteAnnotationTarget, Vec<Annotation>)>;

struct RouteClaimContext<'a, 'options> {
    annotations: &'a BTreeMap<Origin, Rc<RouteAnnotations>>,
    empty_annotations: &'a Rc<RouteAnnotations>,
    flag_enums: &'a BTreeSet<(String, String)>,
    options: &'a EmitOptions<'options>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RouteAnnotationTarget {
    Declaration,
    Return,
    Parameter(usize),
    Field(usize),
    NestedField(Vec<usize>),
    Variant(usize),
    Method(usize),
    MethodReturn(usize),
    MethodParameter { method: usize, parameter: usize },
}

fn route_annotation_target(target: &AnnotationTarget) -> (&Origin, RouteAnnotationTarget) {
    match target {
        AnnotationTarget::Declaration(origin) => (origin, RouteAnnotationTarget::Declaration),
        AnnotationTarget::Return(origin) => (origin, RouteAnnotationTarget::Return),
        AnnotationTarget::Parameter { declaration, index } => {
            (declaration, RouteAnnotationTarget::Parameter(*index))
        }
        AnnotationTarget::Field { declaration, index } => {
            (declaration, RouteAnnotationTarget::Field(*index))
        }
        AnnotationTarget::NestedField { declaration, path } => (
            declaration,
            RouteAnnotationTarget::NestedField(path.clone()),
        ),
        AnnotationTarget::Variant { declaration, index } => {
            (declaration, RouteAnnotationTarget::Variant(*index))
        }
        AnnotationTarget::Method { declaration, index } => {
            (declaration, RouteAnnotationTarget::Method(*index))
        }
        AnnotationTarget::MethodReturn { declaration, index } => {
            (declaration, RouteAnnotationTarget::MethodReturn(*index))
        }
        AnnotationTarget::MethodParameter {
            declaration,
            method,
            parameter,
        } => (
            declaration,
            RouteAnnotationTarget::MethodParameter {
                method: *method,
                parameter: *parameter,
            },
        ),
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RouteItemSemantics<'a> {
    Fact {
        kind: FactKind,
        definition: bool,
        data: &'a FactData,
    },
    Constant {
        ty: &'a TypeRef,
        value: &'a Value,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RouteSemantics<'a> {
    item: RouteItemSemantics<'a>,
    annotations: Rc<RouteAnnotations>,
    uuid: Option<&'a str>,
    flags: bool,
    native_import: Option<NativeImport>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RouteClaim<'a> {
    owner: RootOwner,
    semantics: RouteSemantics<'a>,
}

struct RouteCandidate<'a> {
    name: String,
    source_name: String,
    kind: OutputKind,
    claims: BTreeSet<RouteClaim<'a>>,
    preferred: Option<RouteClaim<'a>>,
    namespace: Option<String>,
    anchored: bool,
    header_partition_policy: bool,
}

impl RouteCandidate<'_> {
    fn owners(&self) -> BTreeSet<RootOwner> {
        self.claims
            .iter()
            .map(|claim| claim.owner.clone())
            .collect()
    }
}

fn route_candidate_conflict(candidate: &RouteCandidate) -> Option<PartitionConflict> {
    if candidate.header_partition_policy {
        return resolve_equivalent_header_route(candidate)
            .err()
            .map(|reason| PartitionConflict {
                name: candidate.source_name.clone(),
                kind: match candidate.kind {
                    OutputKind::Type => PartitionItemKind::Type,
                    OutputKind::Value => PartitionItemKind::Value,
                },
                reason,
                owners: candidate.owners().into_iter().collect(),
            });
    }

    let mut owners = candidate.owners();
    if let Some(namespace) = &candidate.namespace {
        let matching: BTreeSet<_> = owners
            .iter()
            .filter(|owner| owner.namespace == *namespace)
            .cloned()
            .collect();
        if !matching.is_empty() {
            owners = matching;
        }
    }
    let reason = if owners.is_empty() {
        PartitionConflictReason::MissingOwner
    } else {
        let first = owners.first().unwrap();
        let equivalent = if candidate.anchored {
            owners.iter().all(|owner| same_owner_policy(first, owner))
        } else {
            owners
                .iter()
                .all(|owner| same_partition_owner(first, owner))
        };
        if equivalent {
            return None;
        }
        PartitionConflictReason::AmbiguousOwners
    };
    Some(PartitionConflict {
        name: candidate.source_name.clone(),
        kind: match candidate.kind {
            OutputKind::Type => PartitionItemKind::Type,
            OutputKind::Value => PartitionItemKind::Value,
        },
        reason,
        owners: owners.into_iter().collect(),
    })
}

fn resolve_equivalent_header_route(
    candidate: &RouteCandidate<'_>,
) -> Result<RootOwner, PartitionConflictReason> {
    let Some(first) = candidate.claims.first() else {
        return Err(PartitionConflictReason::MissingOwner);
    };
    let namespace = candidate
        .namespace
        .as_deref()
        .unwrap_or(&first.owner.namespace);
    if candidate.claims.iter().any(|claim| {
        candidate
            .namespace
            .as_deref()
            .unwrap_or(&claim.owner.namespace)
            != namespace
            || claim.semantics != first.semantics
    }) {
        return Err(PartitionConflictReason::AmbiguousOwners);
    }
    let Some(preferred) = candidate
        .preferred
        .as_ref()
        .filter(|preferred| candidate.claims.contains(*preferred))
        .or_else(|| (candidate.claims.len() == 1).then_some(first))
    else {
        return Err(PartitionConflictReason::AmbiguousOwners);
    };
    let mut owner = preferred.owner.clone();
    owner.namespace = namespace.to_string();
    Ok(owner)
}

fn route_candidate_error(candidate: &RouteCandidate<'_>, reason: PartitionConflictReason) -> Error {
    let kind = match candidate.kind {
        OutputKind::Type => "type",
        OutputKind::Value => "value",
    };
    match reason {
        PartitionConflictReason::MissingOwner => Error(format!(
            "selected {kind} `{}` has no tagged root owner",
            candidate.name
        )),
        PartitionConflictReason::AmbiguousRootCandidates
        | PartitionConflictReason::AmbiguousOwners
        | PartitionConflictReason::AuthoritySelected => Error(format!(
            "selected {kind} `{}` has ambiguous tagged root owners: {}",
            candidate.name,
            candidate
                .owners()
                .iter()
                .map(|owner| format!("{}:{} -> {}", owner.input, owner.root, owner.namespace))
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

#[cfg(test)]
fn same_typedef_bridge_identity(
    left: &Fact,
    right: &Fact,
    facts_by_origin: &HashMap<&Origin, &Fact>,
    annotations: &BTreeMap<Origin, Rc<RouteAnnotations>>,
) -> bool {
    if !same_source_declaration(left, right) {
        return false;
    }
    let Some(left_scope) = native_parent_scope(left, facts_by_origin) else {
        return false;
    };
    let Some(right_scope) = native_parent_scope(right, facts_by_origin) else {
        return false;
    };
    if left_scope != right_scope {
        return false;
    }
    let left_annotations = annotations
        .get(&left.origin)
        .map_or(&[][..], |annotations| annotations.as_slice());
    let right_annotations = annotations
        .get(&right.origin)
        .map_or(&[][..], |annotations| annotations.as_slice());
    left_annotations == right_annotations
}

#[cfg(test)]
fn native_parent_scope(
    fact: &Fact,
    facts_by_origin: &HashMap<&Origin, &Fact>,
) -> Option<Vec<(FactKind, String, Option<Location>)>> {
    let mut result = vec![];
    let mut parent = fact.parent.as_ref();
    while let Some(origin) = parent {
        let fact = facts_by_origin.get(origin)?;
        result.push((
            fact.kind,
            fact.name.clone(),
            fact.name.is_empty().then(|| fact.spelling.clone()),
        ));
        parent = fact.parent.as_ref();
    }
    result.reverse();
    Some(result)
}

#[derive(Clone, Copy)]
struct TypeProjection<'a> {
    type_names: &'a BTreeMap<String, String>,
    interface_names: &'a BTreeSet<(String, String)>,
    tu: &'a str,
    local_types: &'a BTreeMap<Location, String>,
    routed_types: &'a BTreeMap<String, String>,
    type_choices: &'a BTreeMap<(&'a str, TypeEdge<'a>), TypeChoice<'a>>,
    sal_constant_sizes: Option<&'a SalConstantSizes>,
    namespace: Option<&'a str>,
}

impl<'a> TypeProjection<'a> {
    fn new(
        type_names: &'a BTreeMap<String, String>,
        interface_names: &'a BTreeSet<(String, String)>,
        tu: &'a str,
        local_types: &'a BTreeMap<Location, String>,
        routed_types: &'a BTreeMap<String, String>,
        choices: (
            &'a BTreeMap<(&'a str, TypeEdge<'a>), TypeChoice<'a>>,
            Option<&'a SalConstantSizes>,
        ),
        namespace: Option<&'a str>,
    ) -> Self {
        Self {
            type_names,
            interface_names,
            tu,
            local_types,
            routed_types,
            type_choices: choices.0,
            sal_constant_sizes: choices.1,
            namespace,
        }
    }

    fn name(&self, ty: &TypeRef) -> String {
        self.project(TypeEdge::Type(ty)).name
    }

    fn use_type(&self, ty: &TypeRef, annotations: &[Annotation]) -> ProjectedType {
        self.project(TypeEdge::Use(ty, annotations))
    }

    fn parameter(
        &self,
        param: &Parameter,
        annotations: &[Annotation],
        target: &AnnotationTarget,
    ) -> ProjectedType {
        let resolved = self
            .sal_constant_sizes
            .and_then(|sizes| sizes.get_key_value(target));
        let edge = TypeEdge::Parameter(param, annotations, resolved);
        let mut projected = if param.annotation.com_out_ptr {
            ProjectedType {
                name: "*mut *mut void".to_string(),
                mutable: true,
                annotations: Vec::new(),
            }
        } else {
            self.project(edge)
        };
        if let Some((
            _,
            Ok(SalSize {
                bytes,
                value: SalSizeValue::Constant(count),
            }),
        )) = resolved
        {
            projected.annotations.push(if *bytes {
                Annotation::MemorySizeConst(count.to_string())
            } else {
                Annotation::ArrayCountConst(count.to_string())
            });
        }
        projected
    }

    fn project(&self, edge: TypeEdge<'_>) -> ProjectedType {
        let ty = match self.type_choices.get(&(self.tu, edge)) {
            Some(TypeChoice::Canonical { name, mutable }) => {
                let emitted = self
                    .type_names
                    .get(*name)
                    .cloned()
                    .unwrap_or_else(|| (*name).to_string());
                return ProjectedType {
                    name: qualify_routed_type(&emitted, self.routed_types, self.namespace),
                    mutable: *mutable,
                    annotations: Vec::new(),
                };
            }
            Some(TypeChoice::String {
                name,
                mutable,
                not_null_terminated,
                null_null_terminated,
            }) => {
                let emitted = self.type_names.get(*name).map_or(*name, String::as_str);
                let mut annotations = Vec::new();
                if !mutable {
                    annotations.push(Annotation::Const);
                }
                if *not_null_terminated {
                    annotations.push(Annotation::NotNullTerminated);
                }
                if *null_null_terminated {
                    annotations.push(Annotation::NullNullTerminated);
                }
                return ProjectedType {
                    name: qualify_routed_type(emitted, self.routed_types, self.namespace),
                    mutable: false,
                    annotations,
                };
            }
            Some(TypeChoice::Declared(ty))
                if matches!(edge, TypeEdge::Parameter(..) | TypeEdge::Use(..)) =>
            {
                return self.project(TypeEdge::Type(ty));
            }
            Some(TypeChoice::Declared(ty) | TypeChoice::ComInterface(ty)) => *ty,
            None => match edge {
                TypeEdge::Type(ty) | TypeEdge::Pointee(ty, _) => {
                    assert!(!matches!(ty, TypeRef::Named { .. }));
                    ty
                }
                _ => unreachable!(),
            },
        };
        if matches!(ty, TypeRef::FunctionPointer { .. }) {
            return ProjectedType {
                name: "*mut u8".to_string(),
                mutable: true,
                annotations: Vec::new(),
            };
        }
        if let TypeRef::OpaquePointer { mutable, .. } = ty {
            return ProjectedType {
                name: format!("*{} void", if *mutable { "mut" } else { "const" }),
                mutable: *mutable,
                annotations: Vec::new(),
            };
        }
        if let TypeRef::Reference { mutable, target } = ty {
            if let TypeRef::Named { name, .. } | TypeRef::Generic { name, .. } = target.as_ref()
                && self
                    .interface_names
                    .contains(&(self.tu.to_string(), name.clone()))
            {
                return ProjectedType {
                    name: self.project(TypeEdge::Pointee(target, *mutable)).name,
                    mutable: false,
                    annotations: Vec::new(),
                };
            }
            return self.pointer(target, *mutable, 1);
        }
        if let TypeRef::Array { target, len } = ty {
            return ProjectedType {
                name: format!("[{}; {len}]", self.name(target)),
                mutable: false,
                annotations: Vec::new(),
            };
        }
        if let TypeRef::Generic { name, args, .. } = ty {
            return ProjectedType {
                name: format!(
                    "{}<{}>",
                    rdl_ident(self.type_names.get(name).unwrap_or(name)),
                    args.iter()
                        .map(|arg| self.name(arg))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                mutable: false,
                annotations: Vec::new(),
            };
        }
        let (mutable, depth, target) = pointer_run(ty);
        if depth != 0
            && (matches!(
                self.type_choices
                    .get(&(self.tu, TypeEdge::Pointee(target, mutable))),
                Some(TypeChoice::ComInterface(_))
            ) || matches!(
                target,
                TypeRef::Named { name, .. } | TypeRef::Generic { name, .. }
                    if self.interface_names.contains(&(self.tu.to_string(), name.clone()))
            ))
        {
            return self.pointer(target, mutable, depth - 1);
        }
        if depth != 0 {
            return self.pointer(target, mutable, depth);
        }
        let name = if let TypeRef::Named { name, declaration } = ty
            && let Some(emitted) = self.type_names.get(name)
        {
            qualify_emitted_type(
                declaration,
                emitted,
                self.local_types,
                self.routed_types,
                self.namespace,
            )
        } else if let TypeRef::Named { name, .. } = ty
            && let Some(canonical) = canonical_string_name(name)
        {
            let emitted = self
                .type_names
                .get(canonical)
                .cloned()
                .unwrap_or_else(|| canonical.to_string());
            qualify_routed_type(&emitted, self.routed_types, self.namespace)
        } else {
            planned_type_name(ty, self.type_names, self.local_types, self.namespace)
        };
        ProjectedType {
            name,
            mutable: false,
            annotations: Vec::new(),
        }
    }

    fn pointer(&self, target: &TypeRef, mutable: bool, depth: usize) -> ProjectedType {
        ProjectedType {
            name: format!(
                "{}{}",
                format!("*{} ", if mutable { "mut" } else { "const" }).repeat(depth),
                self.project(TypeEdge::Pointee(target, mutable)).name
            ),
            mutable: depth != 0 && mutable,
            annotations: Vec::new(),
        }
    }
}

struct ProjectedType {
    name: String,
    mutable: bool,
    annotations: Vec<Annotation>,
}

impl ProjectedType {
    fn annotations<'a>(&'a self, existing: &'a [Annotation]) -> Cow<'a, [Annotation]> {
        if self.annotations.is_empty() {
            return Cow::Borrowed(existing);
        }
        let mut result = existing.to_vec();
        for annotation in &self.annotations {
            if !result.contains(annotation) {
                result.push(annotation.clone());
            }
        }
        Cow::Owned(result)
    }

    fn annotated_name(&self, existing: &[Annotation]) -> Result<String, Error> {
        Ok(format!(
            "{}{}",
            annotation_inline(&self.annotations(existing))?,
            self.name
        ))
    }
}

struct ScopedDeclarationIndex {
    exact: BTreeSet<(String, Location, String)>,
    scoped: BTreeMap<(String, Location, String), BTreeSet<String>>,
    by_tu_name: BTreeMap<(String, String), BTreeSet<String>>,
}

impl ScopedDeclarationIndex {
    fn new(facts: &[Fact], collisions: &BTreeSet<String>) -> Self {
        Self {
            exact: facts
                .iter()
                .filter(|fact| partition_collision_symbol(fact) && collisions.contains(&fact.name))
                .map(|fact| {
                    (
                        fact.origin.tu.clone(),
                        fact.spelling.clone(),
                        fact.name.clone(),
                    )
                })
                .collect(),
            scoped: BTreeMap::new(),
            by_tu_name: BTreeMap::new(),
        }
    }

    fn insert(&mut self, fact: &Fact, scoped: String) {
        self.scoped
            .entry((
                fact.origin.tu.clone(),
                fact.spelling.clone(),
                fact.name.clone(),
            ))
            .or_default()
            .insert(scoped.clone());
        self.by_tu_name
            .entry((fact.origin.tu.clone(), fact.name.clone()))
            .or_default()
            .insert(scoped);
    }

    fn get(&self, tu: &str, declaration: &Location, name: &str) -> Option<&String> {
        let key = (tu.to_string(), declaration.clone(), name.to_string());
        if let Some(names) = self.scoped.get(&key) {
            return (names.len() == 1).then(|| names.first().unwrap());
        }
        (!self.exact.contains(&key)).then_some(())?;
        let names = self.by_tu_name.get(&(tu.to_string(), name.to_string()))?;
        (names.len() == 1).then(|| names.first().unwrap())
    }
}

fn partition_collision_symbol(fact: &Fact) -> bool {
    fact.kind != FactKind::Namespace
}

fn remap_fact_types(
    data: &mut FactData,
    tu: &str,
    declarations: &BTreeMap<(String, Location, String), String>,
    roots: &BTreeMap<(String, String), RootOwner>,
) {
    match data {
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            for param in params {
                remap_type_ref(&mut param.ty, tu, declarations, roots);
            }
            remap_type_ref(result, tu, declarations, roots);
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                remap_type_ref(base, tu, declarations, roots);
            }
            for method in methods {
                for param in &mut method.params {
                    remap_type_ref(&mut param.ty, tu, declarations, roots);
                }
                remap_type_ref(&mut method.result, tu, declarations, roots);
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                remap_type_ref(base, tu, declarations, roots);
            }
            for field in fields {
                remap_type_ref(&mut field.ty, tu, declarations, roots);
            }
        }
        FactData::Typedef { target } => remap_type_ref(target, tu, declarations, roots),
        _ => {}
    }
}

type PointerCallbackAliases = BTreeMap<String, BTreeMap<Location, BTreeSet<String>>>;

fn preserve_auto_function_pointer_levels(
    data: &mut FactData,
    names: &BTreeSet<String>,
    tu: &str,
    pointer_callback_aliases: &PointerCallbackAliases,
) {
    match data {
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            for param in params {
                preserve_auto_function_pointer_level(
                    &mut param.ty,
                    names,
                    tu,
                    pointer_callback_aliases,
                );
            }
            preserve_auto_function_pointer_level(result, names, tu, pointer_callback_aliases);
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                preserve_auto_function_pointer_level(base, names, tu, pointer_callback_aliases);
            }
            for method in methods {
                for param in &mut method.params {
                    preserve_auto_function_pointer_level(
                        &mut param.ty,
                        names,
                        tu,
                        pointer_callback_aliases,
                    );
                }
                preserve_auto_function_pointer_level(
                    &mut method.result,
                    names,
                    tu,
                    pointer_callback_aliases,
                );
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                preserve_auto_function_pointer_level(base, names, tu, pointer_callback_aliases);
            }
            for field in fields {
                preserve_auto_function_pointer_level(
                    &mut field.ty,
                    names,
                    tu,
                    pointer_callback_aliases,
                );
            }
        }
        FactData::Typedef { target } => {
            preserve_auto_function_pointer_level(target, names, tu, pointer_callback_aliases);
        }
        _ => {}
    }
}

fn preserve_auto_function_pointer_level(
    ty: &mut TypeRef,
    names: &BTreeSet<String>,
    tu: &str,
    pointer_callback_aliases: &PointerCallbackAliases,
) {
    match ty {
        TypeRef::Named { name, declaration }
            if names.contains(name)
                && !pointer_callback_aliases
                    .get(tu)
                    .and_then(|aliases| aliases.get(declaration))
                    .is_some_and(|aliases| aliases.contains(name)) =>
        {
            *ty = TypeRef::Pointer {
                mutable: true,
                target: Box::new(ty.clone()),
            };
        }
        TypeRef::Generic { args, .. } => {
            for arg in args {
                preserve_auto_function_pointer_level(arg, names, tu, pointer_callback_aliases);
            }
        }
        TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. }
        | TypeRef::Array { target, .. } => {
            preserve_auto_function_pointer_level(target, names, tu, pointer_callback_aliases);
        }
        TypeRef::FunctionPointer { params, result, .. } => {
            for param in params {
                preserve_auto_function_pointer_level(param, names, tu, pointer_callback_aliases);
            }
            preserve_auto_function_pointer_level(result, names, tu, pointer_callback_aliases);
        }
        TypeRef::InlineRecord(record) => {
            if let Some(base) = &mut record.base {
                preserve_auto_function_pointer_level(base, names, tu, pointer_callback_aliases);
            }
            for field in &mut record.fields {
                preserve_auto_function_pointer_level(
                    &mut field.ty,
                    names,
                    tu,
                    pointer_callback_aliases,
                );
            }
        }
        _ => {}
    }
}

fn remap_type_ref(
    ty: &mut TypeRef,
    tu: &str,
    declarations: &BTreeMap<(String, Location, String), String>,
    roots: &BTreeMap<(String, String), RootOwner>,
) {
    match ty {
        TypeRef::Named { name, declaration } => {
            if let Some(target) = declarations
                .get(&(tu.to_string(), declaration.clone(), name.clone()))
                .or_else(|| {
                    roots
                        .get(&(tu.to_string(), declaration.file.clone()))
                        .and_then(|owner| owner.remaps.get(name))
                })
            {
                *name = target.clone();
            }
        }
        TypeRef::Generic {
            name,
            declaration,
            args,
        } => {
            if let Some(target) = declarations
                .get(&(tu.to_string(), declaration.clone(), name.clone()))
                .or_else(|| {
                    roots
                        .get(&(tu.to_string(), declaration.file.clone()))
                        .and_then(|owner| owner.remaps.get(name))
                })
            {
                *name = target.clone();
            }
            for arg in args {
                remap_type_ref(arg, tu, declarations, roots);
            }
        }
        TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. }
        | TypeRef::Array { target, .. } => remap_type_ref(target, tu, declarations, roots),
        TypeRef::FunctionPointer { params, result, .. } => {
            for param in params {
                remap_type_ref(param, tu, declarations, roots);
            }
            remap_type_ref(result, tu, declarations, roots);
        }
        TypeRef::InlineRecord(record) => {
            if let Some(base) = &mut record.base {
                remap_type_ref(base, tu, declarations, roots);
            }
            for field in &mut record.fields {
                remap_type_ref(&mut field.ty, tu, declarations, roots);
            }
        }
        _ => {}
    }
}

fn rename_fact_types(
    data: &mut FactData,
    tu: &str,
    declarations: &ScopedDeclarationIndex,
    collisions: &BTreeSet<String>,
) {
    match data {
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            for param in params {
                rename_type_ref(&mut param.ty, tu, declarations, collisions);
            }
            rename_type_ref(result, tu, declarations, collisions);
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                rename_type_ref(base, tu, declarations, collisions);
            }
            for method in methods {
                for param in &mut method.params {
                    rename_type_ref(&mut param.ty, tu, declarations, collisions);
                }
                rename_type_ref(&mut method.result, tu, declarations, collisions);
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                rename_type_ref(base, tu, declarations, collisions);
            }
            for field in fields {
                rename_type_ref(&mut field.ty, tu, declarations, collisions);
            }
        }
        FactData::Typedef { target } => {
            rename_type_ref(target, tu, declarations, collisions);
        }
        _ => {}
    }
}

fn rename_type_ref(
    ty: &mut TypeRef,
    tu: &str,
    declarations: &ScopedDeclarationIndex,
    collisions: &BTreeSet<String>,
) {
    match ty {
        TypeRef::Named { name, declaration } => {
            if !collisions.contains(name) {
                return;
            }
            if let Some(scoped) = declarations.get(tu, declaration, name) {
                *name = scoped.clone();
            }
        }
        TypeRef::Generic {
            name,
            declaration,
            args,
        } => {
            if collisions.contains(name)
                && let Some(scoped) = declarations.get(tu, declaration, name)
            {
                *name = scoped.clone();
            }
            for arg in args {
                rename_type_ref(arg, tu, declarations, collisions);
            }
        }
        TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. }
        | TypeRef::Array { target, .. } => {
            rename_type_ref(target, tu, declarations, collisions);
        }
        TypeRef::FunctionPointer { params, result, .. } => {
            for param in params {
                rename_type_ref(param, tu, declarations, collisions);
            }
            rename_type_ref(result, tu, declarations, collisions);
        }
        TypeRef::InlineRecord(record) => {
            if let Some(base) = &mut record.base {
                rename_type_ref(base, tu, declarations, collisions);
            }
            for field in &mut record.fields {
                rename_type_ref(&mut field.ty, tu, declarations, collisions);
            }
        }
        _ => {}
    }
}

fn root_owner(tu: &str, root: &str, partition: &RootPartition) -> RootOwner {
    RootOwner {
        input: tu.to_string(),
        root: normalize_name(root),
        partition: partition.partition.clone(),
        namespace: partition.namespace.clone(),
        remaps: partition.remaps.clone(),
        exclusions: partition.exclusions.clone(),
        libraries: partition.libraries.clone(),
        u32_types: partition.u32_types.clone(),
        flags: partition.flags.clone(),
        preserved_auto_function_pointer_levels: partition
            .preserved_auto_function_pointer_levels
            .clone(),
        exclude_empty_records: partition.exclude_empty_records,
    }
}

fn authority_root_owner(partition: &str, namespace: &str, root: &str) -> RootOwner {
    RootOwner {
        input: partition.to_string(),
        root: normalize_name(root),
        partition: partition.to_string(),
        namespace: namespace.to_string(),
        remaps: BTreeMap::new(),
        exclusions: BTreeSet::new(),
        libraries: BTreeMap::new(),
        u32_types: BTreeSet::new(),
        flags: BTreeSet::new(),
        preserved_auto_function_pointer_levels: BTreeSet::new(),
        exclude_empty_records: false,
    }
}

fn fact_header_authority<'a>(
    authorities: &'a BTreeMap<String, HeaderAuthority>,
    fact: &Fact,
) -> Option<&'a HeaderAuthority> {
    authorities
        .get(&normalize_name(&fact.expansion.file).to_ascii_lowercase())
        .or_else(|| authorities.get(&normalize_name(&fact.spelling.file).to_ascii_lowercase()))
}

fn header_fact_owner_candidates(
    lookup: &mut HeaderPartitionLookup<'_>,
    fact: &Fact,
) -> BTreeSet<RootOwner> {
    let candidates = lookup.named_owners(&fact.origin.tu, &fact.expansion.file, &fact.name);
    if candidates.is_empty() && fact.expansion.file != fact.spelling.file {
        lookup.named_owners(&fact.origin.tu, &fact.spelling.file, &fact.name)
    } else {
        candidates
    }
}

fn partition_declaration_location(fact: &Fact) -> &Location {
    if fact.expansion != fact.spelling {
        &fact.expansion
    } else {
        &fact.spelling
    }
}

fn owner_excludes_fact(owner: &RootOwner, fact: &Fact) -> bool {
    owner.exclusions.contains(&fact.name)
        || (owner.exclude_empty_records
            && matches!(&fact.data, FactData::Record { fields, .. } if fields.is_empty()))
}

fn fact_partition_item_kind(fact: &Fact) -> Option<PartitionItemKind> {
    if is_type_fact(fact) {
        Some(PartitionItemKind::Type)
    } else if is_value_fact(fact) || matches!(fact.data, FactData::Function { .. }) {
        Some(PartitionItemKind::Value)
    } else {
        None
    }
}

fn resolve_header_owner(
    name: &str,
    kind: Option<PartitionItemKind>,
    mut owners: BTreeSet<RootOwner>,
    namespace: Option<&str>,
) -> Result<(RootOwner, Option<PartitionConflict>), Error> {
    if let Some(namespace) = namespace {
        let matching: BTreeSet<_> = owners
            .iter()
            .filter(|owner| owner.namespace == namespace)
            .cloned()
            .collect();
        if !matching.is_empty() {
            owners = matching;
        }
    }
    let ambiguous = owners
        .first()
        .is_some_and(|first| owners.iter().any(|owner| !same_owner_policy(first, owner)));
    let candidates = owners.iter().cloned().collect::<Vec<_>>();
    let mut owner = owners
        .pop_first()
        .ok_or_else(|| Error(format!("traversed item `{name}` has no logical owner")))?;
    if let Some(namespace) = namespace
        && !candidates
            .iter()
            .any(|candidate| candidate.namespace == namespace)
    {
        owner.namespace = namespace.to_string();
    }
    validate_namespace(&owner.namespace)?;
    if owner.partition.trim().is_empty() {
        return Err(Error(format!(
            "selected item `{name}` has an empty partition identity"
        )));
    }
    let conflict = if ambiguous {
        kind.map(|kind| PartitionConflict {
            name: name.to_string(),
            kind,
            reason: PartitionConflictReason::AmbiguousRootCandidates,
            owners: candidates,
        })
    } else {
        None
    };
    Ok((owner, conflict))
}

fn same_owner_policy(left: &RootOwner, right: &RootOwner) -> bool {
    left.partition == right.partition
        && left.namespace == right.namespace
        && left.remaps == right.remaps
        && left.exclusions == right.exclusions
        && left.libraries == right.libraries
        && left.u32_types == right.u32_types
        && left.flags == right.flags
        && left.preserved_auto_function_pointer_levels
            == right.preserved_auto_function_pointer_levels
        && left.exclude_empty_records == right.exclude_empty_records
}

fn equivalent_associated_enum_owner_policies(
    name: &str,
    owners: &BTreeSet<RootOwner>,
    namespace: &str,
) -> bool {
    let matching: BTreeSet<_> = owners
        .iter()
        .filter(|owner| owner.namespace == namespace)
        .collect();
    let owners: Vec<_> = if matching.is_empty() {
        owners.iter().collect()
    } else {
        matching.into_iter().collect()
    };
    let Some(first) = owners.first() else {
        return false;
    };
    owners.iter().all(|owner| {
        first.remaps.get(name) == owner.remaps.get(name)
            && first.u32_types.contains(name) == owner.u32_types.contains(name)
            && first.flags.contains(name) == owner.flags.contains(name)
            && first.preserved_auto_function_pointer_levels.contains(name)
                == owner.preserved_auto_function_pointer_levels.contains(name)
    })
}

fn same_partition_owner(left: &RootOwner, right: &RootOwner) -> bool {
    left.partition == right.partition && left.namespace == right.namespace
}

fn unique_owner(
    name: &str,
    kind: OutputKind,
    owners: BTreeSet<RootOwner>,
) -> Result<RootOwner, Error> {
    let kind = match kind {
        OutputKind::Type => "type",
        OutputKind::Value => "value",
    };
    let owners: Vec<_> = owners.into_iter().collect();
    let Some(owner) = owners.first() else {
        return Err(Error(format!(
            "selected {kind} `{name}` has no tagged root owner"
        )));
    };
    if owners.iter().any(|candidate| {
        candidate.partition != owner.partition || candidate.namespace != owner.namespace
    }) {
        return Err(Error(format!(
            "selected {kind} `{name}` has ambiguous tagged root owners: {}",
            owners
                .iter()
                .map(|owner| format!("{}:{} -> {}", owner.input, owner.root, owner.namespace))
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    validate_namespace(&owner.namespace)?;
    if owner.partition.trim().is_empty() {
        Err(Error(format!(
            "selected {kind} `{name}` has an empty partition identity"
        )))
    } else {
        Ok(owner.clone())
    }
}

fn is_type_fact(fact: &Fact) -> bool {
    matches!(
        fact.data,
        FactData::Callback { .. }
            | FactData::Enum { .. }
            | FactData::Interface { .. }
            | FactData::Record { .. }
            | FactData::Typedef { .. }
    )
}

fn is_type_declaration_fact(fact: &Fact) -> bool {
    matches!(
        fact.kind,
        FactKind::Class
            | FactKind::Enum
            | FactKind::EnumFlag
            | FactKind::Struct
            | FactKind::Typedef
            | FactKind::Union
    )
}

fn fact_data_kind(data: &FactData) -> &'static str {
    match data {
        FactData::Callback { .. } => "Callback",
        FactData::Class { .. } => "Class",
        FactData::Enum { .. } => "Enum",
        FactData::EnumFlag { .. } => "EnumFlag",
        FactData::Function { .. } => "Function",
        FactData::NonEmittableFunction { .. } => "NonEmittableFunction",
        FactData::Guid { .. } => "Guid",
        FactData::Interface { .. } => "Interface",
        FactData::Macro { .. } => "Macro",
        FactData::None => "None",
        FactData::PropertyKey { .. } => "PropertyKey",
        FactData::Record { .. } => "Record",
        FactData::Typedef { .. } => "Typedef",
        FactData::Unsupported { .. } => "Unsupported",
    }
}

fn is_value_fact(fact: &Fact) -> bool {
    matches!(
        fact.data,
        FactData::Class { .. } | FactData::Guid { .. } | FactData::PropertyKey { .. }
    )
}

fn is_flat_declaration(
    fact: &Fact,
    facts_by_origin: &HashMap<&Origin, &Fact>,
    references: &BTreeMap<String, TypeReference>,
    root: bool,
) -> bool {
    if references.is_empty() {
        return true;
    }
    let mut namespaces = vec![];
    let mut nested = false;
    let mut parent = fact.parent.as_ref();
    while let Some(origin) = parent {
        let Some(fact) = facts_by_origin.get(origin) else {
            break;
        };
        if fact.kind == FactKind::Namespace {
            namespaces.push(fact.name.as_str());
        } else {
            nested = true;
        }
        parent = fact.parent.as_ref();
    }
    if root && nested {
        return false;
    }
    namespaces.reverse();

    match namespaces.first().copied() {
        None | Some("Windows") => true,
        Some("ABI") => {
            let namespace = namespaces[1..].join(".");
            references
                .get(&fact.name)
                .is_none_or(|reference| reference.namespace != namespace)
        }
        Some(_) => false,
    }
}

fn is_abi_reference_declaration_for_name(
    fact: &Fact,
    facts_by_origin: &HashMap<&Origin, &Fact>,
    references: &BTreeMap<String, TypeReference>,
    name: &str,
) -> bool {
    let Some(reference) = references.get(name) else {
        return false;
    };
    let mut namespaces = vec![];
    let mut parent = fact.parent.as_ref();
    while let Some(origin) = parent {
        let Some(fact) = facts_by_origin.get(origin) else {
            break;
        };
        if fact.kind == FactKind::Namespace {
            namespaces.push(fact.name.as_str());
        }
        parent = fact.parent.as_ref();
    }
    namespaces.reverse();
    namespaces.first().copied() == Some("ABI") && namespaces[1..].join(".") == reference.namespace
}

fn is_native_namespaced_declaration(
    fact: &Fact,
    facts_by_origin: &HashMap<&Origin, &Fact>,
) -> bool {
    let mut namespaces = vec![];
    let mut parent = fact.parent.as_ref();
    while let Some(origin) = parent {
        let Some(fact) = facts_by_origin.get(origin) else {
            break;
        };
        if fact.kind == FactKind::Namespace {
            namespaces.push(fact.name.as_str());
        }
        parent = fact.parent.as_ref();
    }
    namespaces.reverse();
    matches!(
        namespaces.first().copied(),
        Some(namespace) if namespace != "ABI" && namespace != "Windows"
    )
}

fn is_identified_native_interface(
    fact: &Fact,
    source_name: &str,
    facts_by_origin: &HashMap<&Origin, &Fact>,
    rooted_interface_iids: &BTreeSet<(String, String)>,
) -> bool {
    let FactData::Interface { guid, .. } = &fact.data else {
        return false;
    };
    is_native_namespaced_declaration(fact, facts_by_origin)
        && (guid.is_some()
            || rooted_interface_iids.contains(&(fact.origin.tu.clone(), source_name.to_string())))
}

fn defines_local_type(name: &str, fact: &Fact) -> bool {
    match &fact.data {
        FactData::Callback { .. } => true,
        FactData::Enum { .. } | FactData::Interface { .. } | FactData::Record { .. } => {
            fact.definition
        }
        FactData::Typedef {
            target: TypeRef::Named { name: target, .. },
        } => target != name,
        FactData::Typedef { .. } => true,
        _ => false,
    }
}

fn is_root_fact(fact: &Fact) -> bool {
    (is_type_fact(fact) || is_value_fact(fact))
        && !(canonical_named_type(&fact.name).is_some()
            && matches!(fact.data, FactData::Typedef { .. }))
        && (!matches!(
            fact.data,
            FactData::Record { .. } | FactData::Interface { .. }
        ) || fact.definition)
}

fn declaration_kind(
    ty: &TypeRef,
    tu: &str,
    facts_index: &HashMap<&str, Vec<&Fact>>,
    seen: &mut BTreeSet<Location>,
) -> Option<FactKind> {
    let TypeRef::Named { name, declaration } = ty else {
        return None;
    };
    if !seen.insert(declaration.clone()) {
        return None;
    }
    let fact = facts_index
        .get(name.as_str())?
        .iter()
        .find(|fact| fact.origin.tu == tu && fact.spelling == *declaration)?;
    if let FactData::Typedef { target } = &fact.data {
        declaration_kind(target, tu, facts_index, seen)
    } else {
        Some(fact.kind)
    }
}

fn is_tag_declaration(fact: &Fact) -> bool {
    matches!(
        fact.data,
        FactData::Enum { .. } | FactData::Record { .. } | FactData::Interface { .. }
    )
}

fn typedef_declaration<'a>(
    fact: &'a Fact,
    facts_index: &HashMap<&str, Vec<&'a Fact>>,
) -> Option<&'a Fact> {
    typedef_declaration_with_diagnostics(fact, facts_index, None)
}

fn typedef_declaration_with_diagnostics<'a>(
    fact: &'a Fact,
    facts_index: &HashMap<&str, Vec<&'a Fact>>,
    mut diagnostics: Option<(&Snapshot, &mut String)>,
) -> Option<&'a Fact> {
    if !matches!(
        fact.data,
        FactData::Typedef {
            target: TypeRef::Named { .. }
        }
    ) {
        if let Some((_, output)) = &mut diagnostics {
            writeln!(output, "    non-named alias: {:?}", fact.data).unwrap();
        }
        return None;
    }
    let mut current = fact;
    let mut seen = HashSet::new();
    while let FactData::Typedef {
        target: TypeRef::Named { name, declaration },
    } = &current.data
    {
        if !seen.insert(&current.origin) {
            if let Some((_, output)) = &mut diagnostics {
                writeln!(output, "    cycle at {:?}", current.origin).unwrap();
            }
            return None;
        }
        if let Some((_, output)) = &mut diagnostics {
            writeln!(
                output,
                "    alias hop {:?} -> `{name}` at {}:{} in TU {:?}",
                current.origin, declaration.file, declaration.offset, current.origin.tu
            )
            .unwrap();
        }
        let targets = facts_index
            .get(name.as_str())
            .into_iter()
            .flatten()
            .copied()
            .filter(|target| {
                target.origin.tu == current.origin.tu && target.spelling == *declaration
            });
        let mut first = None;
        let mut conflicting = false;
        for candidate in targets {
            if let Some((snapshot, output)) = &mut diagnostics {
                writeln!(
                    output,
                    "      exact bucket {}",
                    root_candidate_context(snapshot, candidate)
                )
                .unwrap();
            }
            let Some(target) = first else {
                first = Some(candidate);
                continue;
            };
            if !same_source_declaration(target, candidate) {
                if let Some((_, output)) = &mut diagnostics {
                    writeln!(
                        output,
                        "      conflicting bucket: {}",
                        source_declaration_mismatch(target, candidate)
                    )
                    .unwrap();
                    conflicting = true;
                } else {
                    return None;
                }
            }
        }
        if conflicting {
            return None;
        }
        let Some(target) = first else {
            if let Some((_, output)) = &mut diagnostics {
                writeln!(output, "      missing exact bucket").unwrap();
            }
            return None;
        };
        current = target;
    }
    if let Some((snapshot, output)) = &mut diagnostics {
        writeln!(
            output,
            "    terminal {} GUID={:?}",
            root_candidate_context(snapshot, current),
            fact_uuid(current)
        )
        .unwrap();
    }
    Some(current)
}

fn typedef_tag_declaration<'a>(
    fact: &'a Fact,
    facts_index: &HashMap<&str, Vec<&'a Fact>>,
) -> Option<&'a Fact> {
    typedef_declaration(fact, facts_index).filter(|target| {
        is_tag_declaration(target)
            || matches!(target.data, FactData::Class { .. }) && !target.definition
    })
}

fn incomplete_declaration_matches_definition(declaration: &Fact, definition: &Fact) -> bool {
    !declaration.definition
        && declaration.kind == definition.kind
        && is_tag_declaration(declaration)
        && is_tag_declaration(definition)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TypeShape(u64, u64);

#[derive(Default)]
struct ShapeCache {
    declarations: HashMap<String, HashMap<Location, TypeShape>>,
    recursive: HashMap<String, HashMap<Location, HashMap<TypeShape, TypeShape>>>,
}

impl ShapeCache {
    fn declaration(&self, tu: &str, declaration: &Location) -> Option<TypeShape> {
        self.declarations.get(tu)?.get(declaration).copied()
    }

    fn insert_declaration(&mut self, tu: &str, declaration: &Location, shape: TypeShape) {
        self.declarations
            .entry(tu.to_string())
            .or_default()
            .insert(declaration.clone(), shape);
    }

    fn is_recursive(&self, tu: &str, declaration: &Location) -> bool {
        self.recursive
            .get(tu)
            .is_some_and(|declarations| declarations.contains_key(declaration))
    }

    fn recursive(&self, tu: &str, declaration: &Location, context: TypeShape) -> Option<TypeShape> {
        self.recursive
            .get(tu)?
            .get(declaration)?
            .get(&context)
            .copied()
    }

    fn insert_recursive(
        &mut self,
        tu: &str,
        declaration: &Location,
        context: TypeShape,
        shape: TypeShape,
    ) {
        self.recursive
            .entry(tu.to_string())
            .or_default()
            .entry(declaration.clone())
            .or_default()
            .insert(context, shape);
    }
}

fn type_shape(value: &str) -> TypeShape {
    use std::hash::{Hash, Hasher};

    // Recursive declarations share compact fingerprints instead of expanded shape strings.
    let mut first = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut first);

    let mut second = 0x9e3779b97f4a7c15u64;
    for byte in value.bytes() {
        second ^= u64::from(byte);
        second = second.wrapping_mul(0x100000001b3);
        second ^= second >> 32;
    }
    TypeShape(first.finish(), second)
}

fn recursion_shape(seen: &BTreeSet<Location>) -> TypeShape {
    use std::hash::{Hash, Hasher};

    let mut first = std::collections::hash_map::DefaultHasher::new();
    seen.hash(&mut first);

    let mut second = std::collections::hash_map::DefaultHasher::new();
    1u8.hash(&mut second);
    seen.hash(&mut second);

    TypeShape(first.finish(), second.finish())
}

fn preferred_fact<'a>(facts: &[&'a Fact]) -> &'a Fact {
    facts
        .iter()
        .copied()
        .min_by_key(|fact| (!fact.root, &fact.origin))
        .unwrap()
}

fn choose_value_root<'a>(name: &str, roots: &[&'a Fact]) -> Result<&'a Fact, Error> {
    let distinct = distinct_source_declarations(roots);
    let Some(first) = distinct.first() else {
        return Err(Error(format!("missing value root `{name}`")));
    };
    if distinct.iter().all(|fact| fact.data == first.data) {
        Ok(preferred_fact(&distinct))
    } else {
        Err(Error(format!("ambiguous value root `{name}`")))
    }
}

fn types_alias_value_class(
    types: &[&Fact],
    values: &[&Fact],
    aliases: &BTreeMap<Origin, CoclassAlias>,
) -> bool {
    types.iter().all(|fact| {
        aliases.get(&fact.origin).is_some_and(|alias| {
            values.iter().any(|value| {
                matches!(value.data, FactData::Class { .. })
                    && alias
                        .guids
                        .iter()
                        .any(|(origin, _)| origin == &value.origin)
            })
        })
    })
}

fn choose_type_root<'a>(
    name: &str,
    roots: &[&'a Fact],
    facts_index: &HashMap<&str, Vec<&'a Fact>>,
) -> Result<&'a Fact, Error> {
    choose_type_root_cached(name, roots, facts_index, &mut ShapeCache::default())
}

fn choose_shared_enum_root<'a>(
    context: &com_pointer::Context<'_, 'a>,
    name: &str,
    roots: &[&'a Fact],
    facts_index: &HashMap<&str, Vec<&'a Fact>>,
    shape_cache: &mut ShapeCache,
    source_names: Option<&PlanningSourceNames>,
    com_bindings: &com_pointer::Bindings<'a>,
) -> Result<Option<&'a Fact>, Error> {
    let Some(names) = source_names.filter(|names| {
        !roots.is_empty()
            && roots
                .iter()
                .all(|fact| names.shared_enum_providers.contains_key(&fact.origin))
    }) else {
        return Ok(None);
    };
    let provider = names.shared_enum_providers[&roots[0].origin]
        .as_ref()
        .map_err(|error| Error(error.message().to_string()))?;
    for fact in roots {
        if names.shared_enum_providers[&fact.origin]
            .as_ref()
            .map_err(|error| Error(error.message().to_string()))?
            != provider
        {
            return Err(Error(format!(
                "conflicting shared enum providers for `{name}`"
            )));
        }
    }
    let fact = facts_index
        .get(name)
        .into_iter()
        .flatten()
        .find(|fact| &fact.origin == provider)
        .copied()
        .ok_or_else(|| Error(format!("missing proved shared enum provider for `{name}`")))?;
    if context.snapshot.ranked_header_partition_policy {
        let authority = com_bindings.authoritative_type(context, fact, facts_index, shape_cache)?;
        if authority.origin != *provider {
            return Err(Error(format!(
                "proved shared enum provider for `{name}` does not retain its exact \
                 authoritative origin: {} -> {}",
                origin(provider),
                origin(&authority.origin),
            )));
        }
    }
    emittable_type(name, fact).map(Some)
}

fn choose_proven_type_root<'a>(
    context: &com_pointer::Context<'_, 'a>,
    name: &str,
    roots: &[&'a Fact],
    facts_index: &HashMap<&str, Vec<&'a Fact>>,
    shape_cache: &mut ShapeCache,
    source_names: Option<&PlanningSourceNames>,
    com_bindings: &com_pointer::Bindings<'a>,
) -> Result<&'a Fact, Error> {
    if context.snapshot.ranked_header_partition_policy
        && roots
            .iter()
            .any(|fact| fact_header_authority(&context.snapshot.header_authorities, fact).is_some())
    {
        let mut selected = Vec::with_capacity(roots.len());
        for fact in roots {
            let authority =
                com_bindings.authoritative_type(context, fact, facts_index, shape_cache)?;
            if !selected
                .iter()
                .any(|previous: &&Fact| previous.origin == authority.origin)
            {
                selected.push(authority);
            }
        }
        let selected = match choose_shared_enum_root(
            context,
            name,
            &selected,
            facts_index,
            shape_cache,
            source_names,
            com_bindings,
        )? {
            Some(provider) => provider,
            None => choose_type_root_cached(name, &selected, facts_index, shape_cache)?,
        };
        if matches!(selected.data, FactData::Interface { .. }) {
            context
                .validate_selected_interface(selected)
                .map_err(Error)?;
        }
        return Ok(selected);
    }
    if roots
        .iter()
        .any(|fact| com_bindings.provider_for(fact, facts_index).is_some())
    {
        let mut proved = Vec::with_capacity(roots.len());
        for fact in roots {
            let provider = com_bindings
                .provider_for(fact, facts_index)
                .transpose()
                .map_err(|failure| Error(failure.message(name)))?;
            let replacement = provider
                .filter(|provider| provider.name == name)
                .unwrap_or(fact);
            proved.push(replacement);
        }
        return choose_type_root_cached(name, &proved, facts_index, shape_cache);
    }
    if let Some(provider) = choose_shared_enum_root(
        context,
        name,
        roots,
        facts_index,
        shape_cache,
        source_names,
        com_bindings,
    )? {
        return Ok(provider);
    }
    let definition = roots
        .iter()
        .copied()
        .find(|fact| fact.definition && matches!(fact.data, FactData::Interface { .. }));
    if let Some(root) = definition {
        let targets: Option<Vec<_>> = roots
            .iter()
            .map(|fact| {
                if matches!(fact.data, FactData::Interface { .. }) {
                    Some(*fact)
                } else {
                    typedef_declaration(fact, facts_index)
                }
            })
            .collect();
        if let Some(targets) = targets
            && targets.iter().any(|fact| fact.origin.tu != root.origin.tu)
            && targets.iter().all(|fact| {
                fact.definition
                    && fact.kind == root.kind
                    && fact.name == root.name
                    && fact.spelling == root.spelling
                    && fact.parent == root.parent
                    && matches!(fact.data, FactData::Interface { .. })
            })
        {
            let failure = targets
                .iter()
                .filter(|fact| fact.origin != root.origin)
                .find_map(|fact| {
                    contextual_interface_matches(context, fact, root).err()
                    .map(|reason| format!(
                        "contextual interface proof {}:{} {:?} against {}:{} {:?}: {reason}",
                        fact.spelling.file, fact.spelling.offset, fact.origin,
                        root.spelling.file, root.spelling.offset, root.origin
                    ))
                });
            let Some(failure) = failure else {
                return Ok(root);
            };
            let legacy = choose_type_root_cached(name, roots, facts_index, shape_cache);
            let error = legacy.err().unwrap_or_else(|| {
                let choices = distinct_source_declarations(roots)
                    .iter()
                    .map(|fact| {
                        format!(
                            "{}:{} {:?}/{} {}",
                            fact.spelling.file,
                            fact.spelling.offset,
                            fact.kind,
                            fact_data_kind(&fact.data),
                            if fact.definition {
                                "definition"
                            } else {
                                "declaration"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                Error(format!("ambiguous type root `{name}`: {choices}"))
            });
            return Err(Error(format!("{error}\n  {failure}")));
        }
    }
    choose_type_root_cached(name, roots, facts_index, shape_cache)
}

fn contextual_interface_matches(
    context: &com_pointer::Context<'_, '_>,
    a: &Fact,
    b: &Fact,
) -> Result<(), String> {
    contextual_interface_matches_seen(context, a, b, &mut BTreeSet::new())
}

fn contextual_interface_matches_seen(
    context: &com_pointer::Context<'_, '_>,
    a: &Fact,
    b: &Fact,
    active: &mut BTreeSet<(Origin, Origin)>,
) -> Result<(), String> {
    let pair = (a.origin.clone(), b.origin.clone());
    if !active.insert(pair.clone()) {
        return Err(format!("cyclic exact interface bases at {pair:?}"));
    }
    let result = contextual_interface_payload(context, a, b, active);
    active.remove(&pair);
    result
}

fn contextual_interface_payload(
    context: &com_pointer::Context<'_, '_>,
    a: &Fact,
    b: &Fact,
    active: &mut BTreeSet<(Origin, Origin)>,
) -> Result<(), String> {
    if a.spelling != b.spelling
        || a.kind != b.kind
        || a.parent != b.parent
        || !a.definition
        || !b.definition
        || context
            .source_names
            .map_or(a.name.as_str(), |names| names.fact_name(a))
            != context
                .source_names
                .map_or(b.name.as_str(), |names| names.fact_name(b))
        || context
            .declarations
            .get(&a.origin.tu, &a.name, &a.spelling)
            .len()
            != 1
        || context
            .declarations
            .get(&b.origin.tu, &b.name, &b.spelling)
            .len()
            != 1
    {
        return Err(
            "interface physical source, kind, name, parent or exact uniqueness differs".into(),
        );
    }
    context.interface_owners_equal(a, b)?;
    let snapshot = context.snapshot;
    if [context.source_annotations, context.effective_annotations]
        .iter()
        .any(|map| map.get(&a.origin) != map.get(&b.origin))
        || snapshot.suppressed_type_origins.contains(&a.origin)
        || snapshot.suppressed_type_origins.contains(&b.origin)
        || snapshot.projected_type_names.get(&a.origin)
            != snapshot.projected_type_names.get(&b.origin)
        || snapshot.fact_namespace_authorities.get(&a.origin)
            != snapshot.fact_namespace_authorities.get(&b.origin)
    {
        return Err(
            "interface annotation, suppression, projection or namespace contract differs".into(),
        );
    }
    let (
        FactData::Interface {
            base: a_base,
            guid: a_guid,
            methods: a_methods,
        },
        FactData::Interface {
            base: b_base,
            guid: b_guid,
            methods: b_methods,
        },
    ) = (&a.data, &b.data)
    else {
        return Err("interface payload missing".into());
    };
    if a_guid != b_guid || a_methods.len() != b_methods.len() {
        return Err("interface GUID or method count differs".into());
    }
    match (a_base, b_base) {
        (None, None) => {}
        (
            Some(TypeRef::Named {
                name: an,
                declaration: ad,
            }),
            Some(TypeRef::Named {
                name: bn,
                declaration: bd,
            }),
        ) => {
            let (Some(a), Some(b)) = (
                context.exact(&a.origin.tu, an, ad),
                context.exact(&b.origin.tu, bn, bd),
            ) else {
                return Err("unresolved exact interface base".into());
            };
            contextual_interface_matches_seen(context, a, b, active)
                .map_err(|reason| format!("base `{an}` -> {reason}"))?;
        }
        _ => return Err("unsupported or differing interface base".into()),
    }
    let (Some(a_contract), Some(b_contract)) = (
        snapshot.native_com_contracts.get(&a.origin),
        snapshot.native_com_contracts.get(&b.origin),
    ) else {
        return Err("missing native interface contract".into());
    };
    if a_contract.guid != b_contract.guid
        || a_contract.pointer_width != b_contract.pointer_width
        || a_contract.bases != b_contract.bases
        || a_contract.slots.len() != b_contract.slots.len()
        || a_contract.has_fields != b_contract.has_fields
    {
        return Err(
            "native interface GUID, width, base, slot count or field contract differs".into(),
        );
    }
    for (index, (left, right)) in a_contract.slots.iter().zip(&b_contract.slots).enumerate() {
        let path = a_methods
            .iter()
            .position(|method| method.name == left.name)
            .map_or_else(
                || format!("native slot[{index}]({})", left.name),
                |method| format!("method[{method}]({})", left.name),
            );
        if left.name != right.name
            || left.convention != right.convention
            || left.params.len() != right.params.len()
        {
            return Err(format!(
                "{path}: ordered name, convention or parameter count differs"
            ));
        }
        let (Some(left_exception), Some(right_exception)) =
            (left.native_exception, right.native_exception)
        else {
            return Err(format!("{path}: missing native exception specification"));
        };
        if left_exception != right_exception
            || !matches!(
                left_exception,
                clang_sys::CXCursor_ExceptionSpecificationKind_None
                    | clang_sys::CXCursor_ExceptionSpecificationKind_BasicNoexcept
            )
        {
            return Err(format!(
                "{path}: differing or unsupported native exception specification \
                {left_exception}/{right_exception}"
            ));
        }
        let (Some(left_native), Some(right_native)) = (&left.native_result, &right.native_result)
        else {
            return Err(format!("{path}.result: missing canonical native evidence"));
        };
        context
            .canonical_types_equal_seen(
                &left.result,
                &a.origin.tu,
                &right.result,
                &b.origin.tu,
                (left_native, right_native),
                active,
            )
            .map_err(|reason| format!("{path}.result: {reason}"))?;
        if left.native_params.len() != left.params.len()
            || right.native_params.len() != right.params.len()
        {
            return Err(format!("{path}: native parameter evidence count differs"));
        }
        for (parameter, (left_param, right_param)) in
            left.params.iter().zip(&right.params).enumerate()
        {
            if left_param.name != right_param.name
                || left_param.annotation != right_param.annotation
            {
                return Err(format!(
                    "{path}.params[{parameter}]: name or annotation differs"
                ));
            }
            let (Some(left_native), Some(right_native)) = (
                &left.native_params[parameter],
                &right.native_params[parameter],
            ) else {
                return Err(format!(
                    "{path}.params[{parameter}]: missing canonical native evidence"
                ));
            };
            context
                .canonical_types_equal_seen(
                    &left_param.ty,
                    &a.origin.tu,
                    &right_param.ty,
                    &b.origin.tu,
                    (left_native, right_native),
                    active,
                )
                .map_err(|reason| format!("{path}.params[{parameter}]: {reason}"))?;
        }
    }
    for (method_index, (a_method, b_method)) in a_methods.iter().zip(b_methods).enumerate() {
        let path = format!("method[{method_index}]({})", a_method.name);
        if a_method.name != b_method.name
            || a_method.special != b_method.special
            || a_method.params.len() != b_method.params.len()
        {
            return Err(format!(
                "{path}: method name, attributes or parameter count differs"
            ));
        }
        let a_slots: Vec<_> = a_contract
            .slots
            .iter()
            .filter(|slot| slot.name == a_method.name)
            .collect();
        let b_slots: Vec<_> = b_contract
            .slots
            .iter()
            .filter(|slot| slot.name == b_method.name)
            .collect();
        let ([a_slot], [b_slot]) = (a_slots.as_slice(), b_slots.as_slice()) else {
            return Err(format!("{path}: missing or ambiguous native slot"));
        };
        if a_slot.convention != b_slot.convention {
            return Err(format!("{path}: native calling convention differs"));
        }
        let (Some(an), Some(bn)) = (&a_slot.native_result, &b_slot.native_result) else {
            return Err(format!("{path}.result: missing canonical native evidence"));
        };
        context
            .canonical_types_equal_seen(
                &a_method.result,
                &a.origin.tu,
                &b_method.result,
                &b.origin.tu,
                (an, bn),
                active,
            )
            .map_err(|reason| format!("{path}.result: {reason}"))?;
        if a_slot.native_params.len() != a_method.params.len()
            || b_slot.native_params.len() != b_method.params.len()
        {
            return Err(format!("{path}: native parameter count differs"));
        }
        for (index, (ap, bp)) in a_method.params.iter().zip(&b_method.params).enumerate() {
            let (Some(an), Some(bn)) = (&a_slot.native_params[index], &b_slot.native_params[index])
            else {
                return Err(format!(
                    "{path}.params[{index}]({}): missing canonical native evidence",
                    ap.name
                ));
            };
            if ap.name != bp.name || ap.annotation != bp.annotation {
                return Err(format!(
                    "{path}.params[{index}]({}): parameter name or annotation differs",
                    ap.name
                ));
            }
            context
                .canonical_types_equal_seen(
                    &ap.ty,
                    &a.origin.tu,
                    &bp.ty,
                    &b.origin.tu,
                    (an, bn),
                    active,
                )
                .map_err(|reason| format!("{path}.params[{index}]({}): {reason}", ap.name))?;
        }
    }
    Ok(())
}

fn choose_type_root_cached<'a>(
    name: &str,
    roots: &[&'a Fact],
    facts_index: &HashMap<&str, Vec<&'a Fact>>,
    shape_cache: &mut ShapeCache,
) -> Result<&'a Fact, Error> {
    let distinct = distinct_source_declarations(roots);
    if let [root] = distinct.as_slice() {
        return emittable_type(name, root);
    }
    if let Some(first) = distinct.first()
        && distinct.iter().all(|fact| {
            fact.origin.tu == first.origin.tu
                && fact.kind == first.kind
                && fact.definition == first.definition
                && fact.data == first.data
        })
    {
        return emittable_type(name, preferred_fact(&distinct));
    }
    let proved_named_typedef_targets = distinct.iter().all(|fact| {
        !matches!(
            fact.data,
            FactData::Typedef {
                target: TypeRef::Named { .. }
            }
        ) || typedef_declaration(fact, facts_index).is_some()
    });
    if proved_named_typedef_targets
        && let Some(target_name) = distinct.first().and_then(|first| match &first.data {
            FactData::Typedef {
                target: TypeRef::Named { name, .. },
            } => Some(name),
            _ => None,
        })
        && distinct.iter().all(|fact| {
            matches!(
                &fact.data,
                FactData::Typedef {
                    target: TypeRef::Named { name, .. }
                } if name == target_name
            )
        })
    {
        let complete: Vec<_> = distinct
            .iter()
            .copied()
            .filter(|fact| {
                typedef_tag_declaration(fact, facts_index)
                    .is_some_and(|target| target.definition && is_tag_declaration(target))
            })
            .collect();
        if let [complete] = complete.as_slice() {
            let FactData::Typedef {
                target: complete_target,
            } = &complete.data
            else {
                unreachable!()
            };
            let complete_kind = declaration_kind(
                complete_target,
                &complete.origin.tu,
                facts_index,
                &mut BTreeSet::new(),
            );
            if complete_kind.is_some()
                && distinct.iter().all(|fact| {
                    let FactData::Typedef { target } = &fact.data else {
                        return false;
                    };
                    declaration_kind(target, &fact.origin.tu, facts_index, &mut BTreeSet::new())
                        == complete_kind
                })
            {
                return emittable_type(name, complete);
            }
        }
    }
    if proved_named_typedef_targets
        && let Some(first) = distinct.first()
        && let FactData::Typedef {
            target: first_target,
        } = &first.data
        && distinct.iter().all(|fact| {
            (fact.spelling == first.spelling
                || (fact.origin.tu == first.origin.tu && fact.parent == first.parent)
                || (fact.parent.is_none() && first.parent.is_none()))
                && matches!(
                    &fact.data,
                    FactData::Typedef { target }
                        if equivalent_type(
                            first_target,
                            &first.origin.tu,
                            target,
                            &fact.origin.tu,
                            facts_index,
                            shape_cache,
                        )
                )
        })
    {
        return emittable_type(name, preferred_fact(&distinct));
    }

    fn resolved_type_shape(
        ty: &TypeRef,
        tu: &str,
        facts_index: &HashMap<&str, Vec<&Fact>>,
        shape_cache: &mut ShapeCache,
    ) -> TypeShape {
        fn write(
            ty: &TypeRef,
            tu: &str,
            facts_index: &HashMap<&str, Vec<&Fact>>,
            seen: &mut BTreeSet<Location>,
            cache: &mut ShapeCache,
            cycle: &mut bool,
        ) -> TypeShape {
            match ty {
                TypeRef::Named { name, declaration } => {
                    if let Some(shape) = cache.declaration(tu, declaration) {
                        return shape;
                    }
                    if cache.is_recursive(tu, declaration) {
                        let context = recursion_shape(seen);
                        if let Some(shape) = cache.recursive(tu, declaration, context) {
                            *cycle = true;
                            return shape;
                        }
                    }
                    if !seen.insert(declaration.clone()) {
                        *cycle = true;
                        return type_shape(&format!(
                            "named:{}",
                            named_type_shape(name).unwrap_or(name)
                        ));
                    }
                    let mut nested_cycle = false;
                    if let Some(target) = facts_index
                        .get(name.as_str())
                        .into_iter()
                        .flatten()
                        .find(|fact| fact.origin.tu == tu && fact.spelling == *declaration)
                    {
                        let result = match &target.data {
                            FactData::Typedef { target } => Some(write(
                                target,
                                tu,
                                facts_index,
                                seen,
                                cache,
                                &mut nested_cycle,
                            )),
                            FactData::Record {
                                base,
                                fields,
                                size,
                                align,
                                packing,
                                alignment,
                                union,
                            } => {
                                let base = base.as_ref().map(|base| {
                                    write(base, tu, facts_index, seen, cache, &mut nested_cycle)
                                });
                                let fields = fields
                                    .iter()
                                    .map(|field| {
                                        format!(
                                            "{}:{}:{}:{}:{:?}:{:?}",
                                            field.name,
                                            field.offset,
                                            field.align,
                                            field.size,
                                            field.bit_width,
                                            write(
                                                &field.ty,
                                                tu,
                                                facts_index,
                                                seen,
                                                cache,
                                                &mut nested_cycle,
                                            )
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",");
                                Some(type_shape(&format!(
                                    "record:{base:?}:{size}:{align}:{packing:?}:{alignment:?}:{union}:{fields}"
                                )))
                            }
                            FactData::Enum {
                                repr,
                                variants,
                                fixed,
                                scoped,
                            } => Some(type_shape(&format!(
                                "enum:{repr:?}:{variants:?}:{fixed}:{scoped}"
                            ))),
                            FactData::Callback {
                                convention,
                                params,
                                result,
                            } => {
                                let params = params
                                    .iter()
                                    .map(|param| {
                                        format!(
                                            "{}:{:?}:{:?}",
                                            param.name,
                                            write(
                                                &param.ty,
                                                tu,
                                                facts_index,
                                                seen,
                                                cache,
                                                &mut nested_cycle,
                                            ),
                                            param.annotation
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",");
                                Some(type_shape(&format!(
                                    "callback:{convention:?}:({params}):{:?}",
                                    write(result, tu, facts_index, seen, cache, &mut nested_cycle,)
                                )))
                            }
                            FactData::Interface {
                                base,
                                guid,
                                methods,
                            } => {
                                let base = base.as_ref().map(|base| {
                                    write(base, tu, facts_index, seen, cache, &mut nested_cycle)
                                });
                                let methods = methods
                                    .iter()
                                    .map(|method| {
                                        let params = method
                                            .params
                                            .iter()
                                            .map(|param| {
                                                format!(
                                                    "{}:{:?}:{:?}",
                                                    param.name,
                                                    write(
                                                        &param.ty,
                                                        tu,
                                                        facts_index,
                                                        seen,
                                                        cache,
                                                        &mut nested_cycle,
                                                    ),
                                                    param.annotation
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                            .join(",");
                                        format!(
                                            "{}:({params}):{:?}:{}",
                                            method.name,
                                            write(
                                                &method.result,
                                                tu,
                                                facts_index,
                                                seen,
                                                cache,
                                                &mut nested_cycle,
                                            ),
                                            method.special
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join(",");
                                Some(type_shape(&format!(
                                    "interface:{base:?}:{guid:?}:{methods}"
                                )))
                            }
                            _ => None,
                        };
                        seen.remove(declaration);
                        *cycle |= nested_cycle;
                        if let Some(result) = result {
                            if !nested_cycle {
                                cache.insert_declaration(tu, declaration, result);
                            } else {
                                cache.insert_recursive(
                                    tu,
                                    declaration,
                                    recursion_shape(seen),
                                    result,
                                );
                            }
                            return result;
                        }
                    }
                    seen.remove(declaration);
                    type_shape(&format!("named:{}", named_type_shape(name).unwrap_or(name)))
                }
                TypeRef::Pointer { mutable, target } => type_shape(&format!(
                    "pointer:{mutable}:{:?}",
                    write(target, tu, facts_index, seen, cache, cycle)
                )),
                TypeRef::Reference { mutable, target } => type_shape(&format!(
                    "reference:{mutable}:{:?}",
                    write(target, tu, facts_index, seen, cache, cycle)
                )),
                TypeRef::FunctionPointer {
                    convention,
                    params,
                    result,
                } => type_shape(&format!(
                    "function:{convention:?}:({:?}):{:?}",
                    params
                        .iter()
                        .map(|param| { write(param, tu, facts_index, seen, cache, cycle) })
                        .collect::<Vec<_>>(),
                    write(result, tu, facts_index, seen, cache, cycle)
                )),
                TypeRef::OpaquePointer { mutable, tag } => {
                    type_shape(&format!("opaque:{mutable}:{tag}"))
                }
                TypeRef::Array { target, len } => type_shape(&format!(
                    "array:{len}:{:?}",
                    write(target, tu, facts_index, seen, cache, cycle)
                )),
                TypeRef::Generic { name, args, .. } => type_shape(&format!(
                    "generic:{name}:{:?}",
                    args.iter()
                        .map(|arg| { write(arg, tu, facts_index, seen, cache, cycle) })
                        .collect::<Vec<_>>()
                )),
                TypeRef::InlineRecord(record) => type_shape(&format!("record:{record:?}")),
                other => type_shape(&format!("{other:?}")),
            }
        }

        write(
            ty,
            tu,
            facts_index,
            &mut BTreeSet::new(),
            shape_cache,
            &mut false,
        )
    }

    fn equivalent_type(
        left: &TypeRef,
        left_tu: &str,
        right: &TypeRef,
        right_tu: &str,
        facts_index: &HashMap<&str, Vec<&Fact>>,
        shape_cache: &mut ShapeCache,
    ) -> bool {
        fn incomplete(
            ty: &TypeRef,
            tu: &str,
            facts_index: &HashMap<&str, Vec<&Fact>>,
            seen: &mut BTreeSet<Location>,
        ) -> bool {
            let TypeRef::Named { name, declaration } = ty else {
                return false;
            };
            if !seen.insert(declaration.clone()) {
                return true;
            }
            let Some(fact) = facts_index
                .get(name.as_str())
                .into_iter()
                .flatten()
                .find(|fact| fact.origin.tu == tu && fact.spelling == *declaration)
            else {
                return true;
            };
            match &fact.data {
                FactData::Typedef { target } => incomplete(target, tu, facts_index, seen),
                FactData::Record { .. } | FactData::Interface { .. } => !fact.definition,
                FactData::Enum { fixed, .. } => !fact.definition && !fixed,
                _ => false,
            }
        }

        fn matching_incomplete_declaration_kind(
            left: &TypeRef,
            left_tu: &str,
            right: &TypeRef,
            right_tu: &str,
            facts_index: &HashMap<&str, Vec<&Fact>>,
        ) -> bool {
            if !incomplete(left, left_tu, facts_index, &mut BTreeSet::new())
                && !incomplete(right, right_tu, facts_index, &mut BTreeSet::new())
            {
                return false;
            }
            let left_kind = declaration_kind(left, left_tu, facts_index, &mut BTreeSet::new());
            left_kind.is_some()
                && left_kind == declaration_kind(right, right_tu, facts_index, &mut BTreeSet::new())
        }

        match (left, right) {
            (
                TypeRef::Named {
                    name: left_name, ..
                },
                TypeRef::Named {
                    name: right_name, ..
                },
            ) if left_name == right_name
                && matching_incomplete_declaration_kind(
                    left,
                    left_tu,
                    right,
                    right_tu,
                    facts_index,
                ) =>
            {
                true
            }
            (
                TypeRef::Pointer {
                    mutable: left_mutable,
                    target: left_target,
                },
                TypeRef::Pointer {
                    mutable: right_mutable,
                    target: right_target,
                },
            )
            | (
                TypeRef::Reference {
                    mutable: left_mutable,
                    target: left_target,
                },
                TypeRef::Reference {
                    mutable: right_mutable,
                    target: right_target,
                },
            ) => {
                left_mutable == right_mutable
                    && equivalent_type(
                        left_target,
                        left_tu,
                        right_target,
                        right_tu,
                        facts_index,
                        shape_cache,
                    )
            }
            (
                TypeRef::Array {
                    target: left_target,
                    len: left_len,
                },
                TypeRef::Array {
                    target: right_target,
                    len: right_len,
                },
            ) => {
                left_len == right_len
                    && equivalent_type(
                        left_target,
                        left_tu,
                        right_target,
                        right_tu,
                        facts_index,
                        shape_cache,
                    )
            }
            _ => {
                resolved_type_shape(left, left_tu, facts_index, shape_cache)
                    == resolved_type_shape(right, right_tu, facts_index, shape_cache)
            }
        }
    }

    fn equivalent_record(
        left: &Fact,
        right: &Fact,
        facts_index: &HashMap<&str, Vec<&Fact>>,
        shape_cache: &mut ShapeCache,
    ) -> bool {
        let (
            FactData::Record {
                base: left_base,
                fields: left_fields,
                size: left_size,
                align: left_align,
                packing: left_packing,
                alignment: left_alignment,
                union: left_union,
            },
            FactData::Record {
                base: right_base,
                fields: right_fields,
                size: right_size,
                align: right_align,
                packing: right_packing,
                alignment: right_alignment,
                union: right_union,
            },
        ) = (&left.data, &right.data)
        else {
            return false;
        };
        left_size == right_size
            && left_align == right_align
            && left_packing == right_packing
            && left_alignment == right_alignment
            && left_union == right_union
            && match (left_base, right_base) {
                (None, None) => true,
                (Some(left_base), Some(right_base)) => equivalent_type(
                    left_base,
                    &left.origin.tu,
                    right_base,
                    &right.origin.tu,
                    facts_index,
                    shape_cache,
                ),
                _ => false,
            }
            && left_fields.len() == right_fields.len()
            && left_fields
                .iter()
                .zip(right_fields)
                .all(|(left_field, right_field)| {
                    left_field.name == right_field.name
                        && left_field.offset == right_field.offset
                        && left_field.align == right_field.align
                        && left_field.size == right_field.size
                        && left_field.bit_width == right_field.bit_width
                        && equivalent_type(
                            &left_field.ty,
                            &left.origin.tu,
                            &right_field.ty,
                            &right.origin.tu,
                            facts_index,
                            shape_cache,
                        )
                })
    }

    let declarations: Vec<_> = distinct
        .iter()
        .copied()
        .filter(|fact| {
            matches!(
                fact.data,
                FactData::Enum { .. } | FactData::Record { .. } | FactData::Interface { .. }
            )
        })
        .collect();
    if let Some(first) = declarations.first()
        && !first.definition
        && declarations.iter().all(|fact| {
            !fact.definition
                && fact.kind == first.kind
                && fact.data == first.data
                && (fact.origin.tu == first.origin.tu || fact.spelling == first.spelling)
        })
        && distinct.iter().all(|fact| {
            declarations.contains(fact)
                || typedef_tag_declaration(fact, facts_index).is_some_and(|target| {
                    declarations
                        .iter()
                        .any(|declaration| same_source_declaration(target, declaration))
                })
        })
    {
        return emittable_type(name, preferred_fact(&declarations));
    }

    let definitions: Vec<_> = distinct
        .iter()
        .copied()
        .filter(|fact| {
            matches!(
                fact.data,
                FactData::Enum { .. } | FactData::Record { .. } | FactData::Interface { .. }
            ) && fact.definition
        })
        .collect();
    if let [root] = definitions.as_slice()
        && let FactData::Interface {
            guid: Some(guid), ..
        } = &root.data
        && distinct.iter().all(|fact| {
            if fact.origin == root.origin {
                return true;
            }
            if fact_uuid(fact) == Some(guid) {
                return matches!(fact.data, FactData::Interface { .. });
            }
            let Some(target) = typedef_tag_declaration(fact, facts_index) else {
                return false;
            };
            fact_uuid(target) == Some(guid)
                && target.kind == root.kind
                && matches!(
                    target.data,
                    FactData::Class { .. } | FactData::Interface { .. }
                )
                && (!target.definition || same_source_declaration(target, root))
        })
    {
        return Ok(root);
    }
    if definitions.len() > 1 {
        let root = preferred_fact(&definitions);
        let equivalent_definitions = definitions.iter().all(|fact| {
            fact.kind == root.kind
                && ((fact.origin.tu == root.origin.tu && fact.data == root.data)
                    || equivalent_record(root, fact, facts_index, shape_cache))
        });
        let definitions_and_aliases = distinct.iter().all(|fact| {
            definitions.contains(fact)
                || typedef_tag_declaration(fact, facts_index).is_some_and(|target| {
                    definitions
                        .iter()
                        .any(|definition| same_source_declaration(target, definition))
                })
        });
        if equivalent_definitions && definitions_and_aliases {
            return Ok(root);
        }
    }
    if let [root] = definitions.as_slice() {
        let aliases_target_root = distinct.iter().all(|fact| {
            fact.origin == root.origin
                || typedef_tag_declaration(fact, facts_index)
                    .is_some_and(|target| same_source_declaration(target, root))
        });
        if aliases_target_root {
            return Ok(root);
        }
        let compatible_declarations_and_aliases = distinct.iter().all(|fact| {
            let linked_nested_declaration = root.parent.is_some()
                && fact.parent.is_none()
                && facts_index.values().flatten().any(|alias| {
                    alias.root
                        && alias.origin.tu == fact.origin.tu
                        && matches!(
                            &alias.data,
                            FactData::Typedef {
                                target: TypeRef::Named { declaration, .. }
                            } if declaration == &fact.spelling
                        )
                });
            fact.origin == root.origin
                || (incomplete_declaration_matches_definition(fact, root)
                    && ((fact.parent.is_none() && root.parent.is_none())
                        || (fact.origin.tu == root.origin.tu
                            && (fact.parent == root.parent || linked_nested_declaration))))
                || typedef_tag_declaration(fact, facts_index).is_some_and(|target| {
                    target.name == name
                        && target.kind == root.kind
                        && (target.origin == root.origin || !target.definition)
                        && target.parent.is_none()
                        && root.parent.is_none()
                })
        });
        if compatible_declarations_and_aliases {
            return Ok(root);
        }
    }
    let choices = distinct
        .iter()
        .map(|fact| {
            format!(
                "{}:{} {:?}/{} {}",
                fact.spelling.file,
                fact.spelling.offset,
                fact.kind,
                fact_data_kind(&fact.data),
                if fact.definition {
                    "definition"
                } else {
                    "declaration"
                }
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Err(Error(format!("ambiguous type root `{name}`: {choices}")))
}

fn constant_types_match(left: &TypeRef, right: &TypeRef) -> bool {
    match (left, right) {
        (TypeRef::Void, TypeRef::Void)
        | (TypeRef::String, TypeRef::String)
        | (TypeRef::Object, TypeRef::Object) => true,
        (TypeRef::Scalar(left), TypeRef::Scalar(right)) => left == right,
        (
            TypeRef::Named {
                name: left_name, ..
            },
            TypeRef::Named {
                name: right_name, ..
            },
        ) => {
            left_name == right_name
                || (named_type_shape(left_name) == named_type_shape(right_name)
                    && named_type_shape(left_name).is_some())
        }
        (
            TypeRef::Pointer {
                mutable: left_mutable,
                target: left,
            },
            TypeRef::Pointer {
                mutable: right_mutable,
                target: right,
            },
        )
        | (
            TypeRef::Reference {
                mutable: left_mutable,
                target: left,
            },
            TypeRef::Reference {
                mutable: right_mutable,
                target: right,
            },
        ) => left_mutable == right_mutable && constant_types_match(left, right),
        (
            TypeRef::FunctionPointer {
                convention: left_convention,
                params: left_params,
                result: left_result,
            },
            TypeRef::FunctionPointer {
                convention: right_convention,
                params: right_params,
                result: right_result,
            },
        ) => {
            left_convention == right_convention
                && left_params.len() == right_params.len()
                && left_params
                    .iter()
                    .zip(right_params)
                    .all(|(left, right)| constant_types_match(left, right))
                && constant_types_match(left_result, right_result)
        }
        (
            TypeRef::OpaquePointer {
                mutable: left_mutable,
                tag: left_tag,
            },
            TypeRef::OpaquePointer {
                mutable: right_mutable,
                tag: right_tag,
            },
        ) => left_mutable == right_mutable && left_tag == right_tag,
        (
            TypeRef::Array {
                target: left,
                len: left_len,
            },
            TypeRef::Array {
                target: right,
                len: right_len,
            },
        ) => left_len == right_len && constant_types_match(left, right),
        (
            TypeRef::Generic {
                name: left_name,
                args: left_args,
                ..
            },
            TypeRef::Generic {
                name: right_name,
                args: right_args,
                ..
            },
        ) => {
            left_name == right_name
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| constant_types_match(left, right))
        }
        (TypeRef::InlineRecord(left), TypeRef::InlineRecord(right)) => left == right,
        _ => false,
    }
}

struct RootSelectionError {
    name: String,
    category: &'static str,
    error: Error,
    candidates: BTreeSet<String>,
}

fn root_candidate_context(snapshot: &Snapshot, fact: &Fact) -> String {
    format!(
        "{} {:?} {:?}/{} spelling={}:{} expansion={}:{} parent={:?} definition={} root={} \
         suppressed={} namespace={:?} owner={:?} source_owner={:?}",
        origin(&fact.origin),
        fact.origin,
        fact.kind,
        fact_data_kind(&fact.data),
        fact.spelling.file,
        fact.spelling.offset,
        fact.expansion.file,
        fact.expansion.offset,
        fact.parent,
        fact.definition,
        fact.root,
        snapshot.suppressed_type_origins.contains(&fact.origin),
        snapshot
            .fact_namespace_authorities
            .get(&fact.origin)
            .or_else(|| snapshot.namespace_authorities.get(&fact.name)),
        snapshot.root_owners.get(&fact.origin),
        snapshot
            .root_partitions
            .get(&(fact.origin.tu.clone(), fact.spelling.file.clone())),
    )
}

fn root_selection_error(
    snapshot: &Snapshot,
    name: &str,
    category: &'static str,
    error: Error,
    facts: &[&Fact],
    constants: &[&Constant],
) -> RootSelectionError {
    let candidates = facts
        .iter()
        .map(|fact| root_candidate_context(snapshot, fact))
        .chain(constants.iter().map(|constant| {
            format!(
                "{} definition={} spelling={}:{} type={:?} value={:?} owner={:?}",
                origin(&constant.root),
                origin(&constant.definition),
                constant.spelling.file,
                constant.spelling.offset,
                constant.ty,
                constant.value,
                snapshot
                    .constant_root_owners
                    .get(&(constant.root.clone(), constant.name.clone())),
            )
        }))
        .collect();
    RootSelectionError {
        name: name.to_string(),
        category,
        error,
        candidates,
    }
}

fn type_root_selection_error(
    snapshot: &Snapshot,
    name: &str,
    phase: &'static str,
    error: Error,
    selected: &[&Fact],
    authority: &[&Fact],
    facts_index: &HashMap<&str, Vec<&Fact>>,
) -> RootSelectionError {
    let mut result = root_selection_error(snapshot, name, "type", error, selected, &[]);
    writeln!(result.error.0, "\n  selection phase={phase}").unwrap();
    if phase == "associated" {
        result.category = "associated enum";
    }
    writeln!(result.error.0, "  authoritative (pre-coalescence):").unwrap();
    for fact in authority {
        writeln!(
            result.error.0,
            "    {}",
            root_candidate_context(snapshot, fact)
        )
        .unwrap();
    }
    let definitions: Vec<_> = distinct_source_declarations(authority)
        .into_iter()
        .filter(|fact| fact.definition && is_tag_declaration(fact))
        .collect();
    for fact in selected {
        if !matches!(fact.data, FactData::Typedef { .. }) {
            continue;
        }
        writeln!(
            result.error.0,
            "\n  alias {:?} authoritative={}",
            fact.origin,
            authority
                .iter()
                .any(|candidate| candidate.origin == fact.origin)
        )
        .unwrap();
        let Some(target) = typedef_declaration_with_diagnostics(
            fact,
            facts_index,
            Some((snapshot, &mut result.error.0)),
        ) else {
            continue;
        };
        if !(is_tag_declaration(target)
            || matches!(target.data, FactData::Class { .. }) && !target.definition)
        {
            writeln!(result.error.0, "    non-tag terminal").unwrap();
            continue;
        }
        for definition in &definitions {
            writeln!(
                result.error.0,
                "    reconciliation against {:?}: GUID candidate={:?} root={:?} kind match={} \
                 parent match={} incomplete-compatible={} full-source match={}",
                definition.origin,
                fact_uuid(target),
                fact_uuid(definition),
                target.kind == definition.kind,
                target.parent == definition.parent,
                incomplete_declaration_matches_definition(target, definition),
                same_source_declaration(target, definition)
            )
            .unwrap();
            if target.definition && !same_source_declaration(target, definition) {
                writeln!(
                    result.error.0,
                    "      representation/source mismatch (not ABI proof): {}",
                    source_declaration_mismatch(target, definition)
                )
                .unwrap();
            }
        }
    }
    result
}

fn source_declaration_mismatch(left: &Fact, right: &Fact) -> String {
    macro_rules! compare {
        ($path:expr, $left:expr, $right:expr) => {
            if $left != $right {
                return format!("{}: left {:?}, right {:?}", $path, $left, $right);
            }
        };
    }
    compare!("kind", left.kind, right.kind);
    compare!("name", left.name, right.name);
    compare!("spelling", left.spelling, right.spelling);
    compare!("definition", left.definition, right.definition);
    if let (
        FactData::Interface {
            guid: left_guid,
            base: left_base,
            methods: left_methods,
        },
        FactData::Interface {
            guid: right_guid,
            base: right_base,
            methods: right_methods,
        },
    ) = (&left.data, &right.data)
    {
        compare!("data.guid", left_guid, right_guid);
        compare!("data.base", left_base, right_base);
        compare!("data.methods.len", left_methods.len(), right_methods.len());
        for (slot, (left, right)) in left_methods.iter().zip(right_methods).enumerate() {
            let path = format!("data.methods[{slot}]({})", left.name);
            compare!(format!("{path}.name"), left.name, right.name);
            compare!(format!("{path}.result"), left.result, right.result);
            compare!(format!("{path}.special"), left.special, right.special);
            compare!(
                format!("{path}.params.len"),
                left.params.len(),
                right.params.len()
            );
            for (index, (left, right)) in left.params.iter().zip(&right.params).enumerate() {
                let path = format!("{path}.params[{index}]({})", left.name);
                compare!(format!("{path}.name"), left.name, right.name);
                compare!(format!("{path}.ty"), left.ty, right.ty);
                compare!(
                    format!("{path}.annotation"),
                    left.annotation,
                    right.annotation
                );
            }
        }
    }
    format!("data: left {:?}, right {:?}", left.data, right.data)
}

fn root_selection_diagnostic(error: RootSelectionError) -> Error {
    if error.candidates.is_empty() {
        return error.error;
    }
    let candidates = error
        .candidates
        .into_iter()
        .collect::<Vec<_>>()
        .join("\n    ");
    Error(format!(
        "{}\n  category={} name=`{}`\n  selected (pre-coalescence):\n    {candidates}",
        error.error, error.category, error.name
    ))
}

fn aggregate_root_selection_errors(mut errors: Vec<RootSelectionError>) -> Error {
    if errors.len() == 1 {
        let error = errors.pop().unwrap();
        return if matches!(error.category, "type" | "associated enum") {
            root_selection_diagnostic(error)
        } else {
            error.error
        };
    }
    if errors.iter().all(|error| error.category == "function") {
        return aggregate_function_root_errors(
            errors.into_iter().map(|error| error.error).collect(),
        );
    }
    errors.sort_by(|left, right| {
        (&left.name, left.category, &left.error.0).cmp(&(
            &right.name,
            right.category,
            &right.error.0,
        ))
    });
    let count = errors.len();
    let messages = errors
        .into_iter()
        .map(|error| {
            let heading = format!("  {} `{}`:", error.category, error.name);
            format!("{heading} {}", root_selection_diagnostic(error))
        })
        .collect::<Vec<_>>()
        .join("\n");
    Error(format!("root selection errors ({count}):\n{messages}"))
}

fn aggregate_function_root_errors(mut errors: Vec<Error>) -> Error {
    if errors.len() == 1 {
        return errors.pop().unwrap();
    }
    let count = errors.len();
    Error(format!(
        "ambiguous function roots ({count}):\n  {}",
        errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("\n  ")
    ))
}

type FunctionParentPath = Vec<(FactKind, String, Option<Location>)>;

fn function_parent_path(
    fact: &Fact,
    facts_by_origin: &HashMap<&Origin, &Fact>,
    source_names: Option<&PlanningSourceNames>,
) -> Option<FunctionParentPath> {
    let mut result = Vec::new();
    let mut parent = fact.parent.as_ref();
    while let Some(origin) = parent {
        let fact = facts_by_origin.get(origin)?;
        let name = source_names.map_or(fact.name.as_str(), |names| names.fact_name(fact));
        result.push((
            fact.kind,
            name.to_string(),
            name.is_empty().then(|| fact.spelling.clone()),
        ));
        parent = fact.parent.as_ref();
    }
    result.reverse();
    Some(result)
}

fn distinct_source_declarations<'a>(roots: &[&'a Fact]) -> Vec<&'a Fact> {
    let mut distinct: Vec<&Fact> = vec![];
    for &root in roots {
        if !distinct
            .iter()
            .any(|existing| same_source_declaration(existing, root))
        {
            distinct.push(root);
        }
    }
    distinct
}

fn same_source_declaration(left: &Fact, right: &Fact) -> bool {
    left.kind == right.kind
        && left.name == right.name
        && left.spelling == right.spelling
        && left.definition == right.definition
        && left.data == right.data
}

fn has_equivalent_source_provider(provider: &Fact, traversed: &[&Fact]) -> bool {
    traversed.iter().any(|candidate| {
        provider.kind == candidate.kind
            && provider.name == candidate.name
            && provider.spelling == candidate.spelling
            && provider.definition == candidate.definition
            && equivalent_associated_enum_provider_data(&provider.data, &candidate.data)
    })
}

fn equivalent_associated_enum_provider_data(left: &FactData, right: &FactData) -> bool {
    match (left, right) {
        (
            FactData::Enum {
                repr: left_repr,
                variants: left_variants,
                fixed: left_fixed,
                scoped: left_scoped,
            },
            FactData::Enum {
                repr: right_repr,
                variants: right_variants,
                fixed: right_fixed,
                scoped: right_scoped,
            },
        ) => {
            left_repr == right_repr
                && left_fixed == right_fixed
                && left_scoped == right_scoped
                && left_variants.len() == right_variants.len()
                && left_variants
                    .iter()
                    .zip(right_variants)
                    .all(|(left, right)| {
                        left.name == right.name
                            && equivalent_enum_value(left.value, right.value, *left_repr)
                    })
        }
        _ => left == right,
    }
}

fn equivalent_enum_value(left: i64, right: i64, repr: Scalar) -> bool {
    match repr {
        Scalar::U8 => left as u8 == right as u8,
        Scalar::U16 => left as u16 == right as u16,
        Scalar::U32 => left as u32 == right as u32,
        Scalar::U64 => left as u64 == right as u64,
        _ => left == right,
    }
}

fn emittable_type<'a>(name: &str, fact: &'a Fact) -> Result<&'a Fact, Error> {
    match fact.data {
        FactData::Callback { .. } | FactData::Typedef { .. } if fact.definition => Ok(fact),
        FactData::Enum { fixed, .. } if fact.definition || fixed => Ok(fact),
        FactData::Class { .. }
        | FactData::Guid { .. }
        | FactData::PropertyKey { .. }
        | FactData::Record { .. }
        | FactData::Interface { .. } => Ok(fact),
        _ => Err(Error(format!("type root `{name}` is not emittable"))),
    }
}

fn underlying_enum_fact<'a>(
    fact: &'a Fact,
    facts_by_declaration: &BTreeMap<(String, Location), &'a Fact>,
    seen: &mut BTreeSet<(String, Location)>,
) -> Option<&'a Fact> {
    match &fact.data {
        FactData::Enum { .. } => Some(fact),
        FactData::Typedef {
            target: TypeRef::Named { declaration, .. },
        } => {
            let key = (fact.origin.tu.clone(), declaration.clone());
            if !seen.insert(key.clone()) {
                return None;
            }
            underlying_enum_fact(*facts_by_declaration.get(&key)?, facts_by_declaration, seen)
        }
        _ => None,
    }
}

struct LayoutContext<'a, 'facts> {
    facts_index: &'a HashMap<&'facts str, Vec<&'facts Fact>>,
    planned_types: &'a BTreeMap<&'facts str, &'facts Fact>,
    pointer_only_class_layouts: &'a BTreeMap<Origin, FactData>,
    embeddable_class_layouts: &'a BTreeSet<Origin>,
    display_names: Option<&'a BTreeMap<String, String>>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum NativeClassValueUse {
    Abi,
    RecordLayout,
}

fn validate_fact_value_abi(fact: &Fact, layout: &LayoutContext<'_, '_>) -> Result<(), Error> {
    match &fact.data {
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            validate_pointer_only_class_value(
                result,
                &fact.origin.tu,
                layout,
                &mut BTreeSet::new(),
                NativeClassValueUse::Abi,
            )?;
            for param in params {
                validate_pointer_only_class_value(
                    &param.ty,
                    &fact.origin.tu,
                    layout,
                    &mut BTreeSet::new(),
                    NativeClassValueUse::Abi,
                )?;
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                validate_pointer_only_class_value(
                    base,
                    &fact.origin.tu,
                    layout,
                    &mut BTreeSet::new(),
                    NativeClassValueUse::RecordLayout,
                )?;
            }
            for field in fields {
                validate_pointer_only_class_value(
                    &field.ty,
                    &fact.origin.tu,
                    layout,
                    &mut BTreeSet::new(),
                    NativeClassValueUse::RecordLayout,
                )?;
            }
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                validate_pointer_only_class_value(
                    base,
                    &fact.origin.tu,
                    layout,
                    &mut BTreeSet::new(),
                    NativeClassValueUse::Abi,
                )?;
            }
            for method in methods {
                validate_pointer_only_class_value(
                    &method.result,
                    &fact.origin.tu,
                    layout,
                    &mut BTreeSet::new(),
                    NativeClassValueUse::Abi,
                )?;
                for param in &method.params {
                    validate_pointer_only_class_value(
                        &param.ty,
                        &fact.origin.tu,
                        layout,
                        &mut BTreeSet::new(),
                        NativeClassValueUse::Abi,
                    )?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_pointer_only_class_value(
    ty: &TypeRef,
    tu: &str,
    layout: &LayoutContext<'_, '_>,
    seen: &mut BTreeSet<Origin>,
    use_kind: NativeClassValueUse,
) -> Result<(), Error> {
    match ty {
        TypeRef::Pointer { .. }
        | TypeRef::Reference { .. }
        | TypeRef::OpaquePointer { .. }
        | TypeRef::Void
        | TypeRef::String
        | TypeRef::Object
        | TypeRef::Scalar(_)
        | TypeRef::Generic { .. } => Ok(()),
        TypeRef::FunctionPointer { params, result, .. } => {
            validate_pointer_only_class_value(result, tu, layout, seen, NativeClassValueUse::Abi)?;
            for param in params {
                validate_pointer_only_class_value(
                    param,
                    tu,
                    layout,
                    seen,
                    NativeClassValueUse::Abi,
                )?;
            }
            Ok(())
        }
        TypeRef::Array { target, .. } => {
            validate_pointer_only_class_value(target, tu, layout, seen, use_kind)
        }
        TypeRef::InlineRecord(record) => {
            if let Some(base) = &record.base {
                validate_pointer_only_class_value(base, tu, layout, seen, use_kind)?;
            }
            for field in &record.fields {
                validate_pointer_only_class_value(&field.ty, tu, layout, seen, use_kind)?;
            }
            Ok(())
        }
        TypeRef::Named { name, declaration } => {
            let matches: Vec<_> = layout
                .facts_index
                .get(name.as_str())
                .into_iter()
                .flatten()
                .copied()
                .filter(|fact| fact.origin.tu == tu && fact.spelling == *declaration)
                .collect();
            let fact = if matches
                .iter()
                .any(|fact| matches!(fact.data, FactData::Typedef { .. }))
            {
                choose_type_root(name, &matches, layout.facts_index)?
            } else if let Some(fact) = layout.planned_types.get(name.as_str()).copied() {
                fact
            } else if !matches.is_empty() {
                choose_type_root(name, &matches, layout.facts_index)?
            } else {
                return Ok(());
            };
            if let Some(pointer_only_layout) = layout.pointer_only_class_layouts.get(&fact.origin)
                && !(use_kind == NativeClassValueUse::RecordLayout
                    && layout.embeddable_class_layouts.contains(&fact.origin))
            {
                let name = layout
                    .display_names
                    .and_then(|names| names.get(&fact.name))
                    .map_or(fact.name.as_str(), String::as_str);
                let kind = if matches!(
                    pointer_only_layout,
                    FactData::Record { fields, .. } if fields.is_empty()
                ) {
                    "native_opaque class"
                } else {
                    "pointer-only native class"
                };
                return Err(Error(format!(
                    "{kind} `{name}` is used by value in translation unit `{}`",
                    fact.origin.tu
                )));
            }
            if !seen.insert(fact.origin.clone()) {
                return Ok(());
            }
            let result = match &fact.data {
                FactData::Record { base, fields, .. } => {
                    if let Some(base) = base {
                        validate_pointer_only_class_value(
                            base,
                            &fact.origin.tu,
                            layout,
                            seen,
                            use_kind,
                        )?;
                    }
                    for field in fields {
                        validate_pointer_only_class_value(
                            &field.ty,
                            &fact.origin.tu,
                            layout,
                            seen,
                            use_kind,
                        )?;
                    }
                    Ok(())
                }
                FactData::Typedef { target } => validate_pointer_only_class_value(
                    target,
                    &fact.origin.tu,
                    layout,
                    seen,
                    use_kind,
                ),
                _ => Ok(()),
            };
            seen.remove(&fact.origin);
            result
        }
    }
}

fn validate_fact_layouts(
    fact: &Fact,
    layout: &LayoutContext<'_, '_>,
    safe_layouts: &mut HashMap<String, HashSet<Location>>,
    validated: &mut HashSet<Origin>,
) -> Result<(), Error> {
    let mut validate = |ty| {
        validate_complete_layout(
            ty,
            &fact.origin.tu,
            layout,
            safe_layouts,
            &mut BTreeSet::new(),
            validated,
        )
    };
    match &fact.data {
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            validate(result)?;
            for param in params {
                validate(&param.ty)?;
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                validate(base)?;
            }
            for field in fields {
                validate(&field.ty)?;
            }
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                validate(base)?;
            }
            for method in methods {
                validate(&method.result)?;
                for param in &method.params {
                    validate(&param.ty)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn known_complete_layout(
    ty: &TypeRef,
    tu: &str,
    safe_layouts: &HashMap<String, HashSet<Location>>,
    references: &BTreeMap<String, TypeReference>,
) -> bool {
    match ty {
        TypeRef::Pointer { .. }
        | TypeRef::Reference { .. }
        | TypeRef::FunctionPointer { .. }
        | TypeRef::OpaquePointer { .. }
        | TypeRef::Void
        | TypeRef::String
        | TypeRef::Object
        | TypeRef::Scalar(_)
        | TypeRef::Generic { .. } => true,
        TypeRef::Array { target, .. } => {
            known_complete_layout(target, tu, safe_layouts, references)
        }
        TypeRef::InlineRecord(record) => {
            record
                .base
                .as_ref()
                .is_none_or(|base| known_complete_layout(base, tu, safe_layouts, references))
                && record
                    .fields
                    .iter()
                    .all(|field| known_complete_layout(&field.ty, tu, safe_layouts, references))
        }
        TypeRef::Named { name, declaration } => {
            references.contains_key(name)
                || safe_layouts
                    .get(tu)
                    .is_some_and(|safe| safe.contains(declaration))
        }
    }
}

fn validate_complete_layout(
    ty: &TypeRef,
    tu: &str,
    layout: &LayoutContext<'_, '_>,
    safe_layouts: &mut HashMap<String, HashSet<Location>>,
    seen: &mut BTreeSet<(String, Location)>,
    validated: &mut HashSet<Origin>,
) -> Result<(), Error> {
    match ty {
        TypeRef::Pointer { .. }
        | TypeRef::Reference { .. }
        | TypeRef::FunctionPointer { .. }
        | TypeRef::OpaquePointer { .. }
        | TypeRef::Void
        | TypeRef::String
        | TypeRef::Object
        | TypeRef::Scalar(_)
        | TypeRef::Generic { .. } => Ok(()),
        TypeRef::Array { target, .. } => {
            validate_complete_layout(target, tu, layout, safe_layouts, seen, validated)
        }
        TypeRef::InlineRecord(record) => {
            if let Some(base) = &record.base {
                validate_complete_layout(base, tu, layout, safe_layouts, seen, validated)?;
            }
            for field in &record.fields {
                validate_complete_layout(&field.ty, tu, layout, safe_layouts, seen, validated)?;
            }
            Ok(())
        }
        TypeRef::Named { name, declaration } => {
            if safe_layouts
                .get(tu)
                .is_some_and(|safe| safe.contains(declaration))
            {
                return Ok(());
            }
            let matches: Vec<_> = layout
                .facts_index
                .get(name.as_str())
                .into_iter()
                .flatten()
                .copied()
                .filter(|fact| fact.origin.tu == tu && fact.spelling == *declaration)
                .collect();
            let fact = if matches
                .iter()
                .any(|fact| matches!(fact.data, FactData::Typedef { .. }))
            {
                choose_type_root(name, &matches, layout.facts_index)?
            } else if let Some(fact) = layout.planned_types.get(name.as_str()).copied() {
                fact
            } else if !matches.is_empty() {
                choose_type_root(name, &matches, layout.facts_index)?
            } else {
                return Ok(());
            };
            if validated.contains(&fact.origin) {
                return Ok(());
            }
            if !seen.insert((fact.origin.tu.clone(), fact.spelling.clone())) {
                return Ok(());
            }
            let result = match &fact.data {
                FactData::Record { .. } if !fact.definition => Err(Error(format!(
                    "incomplete record `{name}` is used by value in translation unit `{tu}`"
                ))),
                FactData::Typedef { target } => validate_complete_layout(
                    target,
                    &fact.origin.tu,
                    layout,
                    safe_layouts,
                    seen,
                    validated,
                ),
                _ => Ok(()),
            };
            if result.is_ok() {
                validated.insert(fact.origin.clone());
                safe_layouts
                    .entry(tu.to_string())
                    .or_default()
                    .insert(declaration.clone());
            }
            result
        }
    }
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum TypeEdge<'a> {
    Type(&'a TypeRef),
    Pointee(&'a TypeRef, bool),
    Use(&'a TypeRef, &'a [Annotation]),
    Parameter(
        &'a Parameter,
        &'a [Annotation],
        Option<(&'a AnnotationTarget, &'a Result<SalSize, SalCountError>)>,
    ),
    Projected(&'a str),
}

#[derive(Clone, Copy)]
enum TypeChoice<'a> {
    Declared(&'a TypeRef),
    ComInterface(&'a TypeRef),
    Canonical {
        name: &'static str,
        mutable: bool,
    },
    String {
        name: &'static str,
        mutable: bool,
        not_null_terminated: bool,
        null_null_terminated: bool,
    },
}

impl<'a> TypeChoice<'a> {
    fn edge(self) -> TypeEdge<'a> {
        match self {
            Self::Declared(ty) | Self::ComInterface(ty) => TypeEdge::Type(ty),
            Self::Canonical { name, .. } | Self::String { name, .. } => TypeEdge::Projected(name),
        }
    }
}

type PendingTypeEdge<'a> = (&'a str, TypeEdge<'a>, DependencyNode<'a>);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DependencyRoot<'a> {
    kind: &'static str,
    name: &'a str,
    tu: &'a str,
    source: &'a Location,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum DependencyNode<'a> {
    Root(usize),
    Fact(&'a Origin),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct DependencyReference<'a> {
    name: &'a str,
    tu: &'a str,
    declaration: Option<&'a Location>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DependencyBlocker<'a> {
    reference: DependencyReference<'a>,
    reason: String,
}

#[derive(Default)]
struct DependencyClosureDiagnostics<'a> {
    enabled: bool,
    roots: Vec<DependencyRoot<'a>>,
    reverse_edges: HashMap<DependencyNode<'a>, HashSet<DependencyNode<'a>>>,
    processed: HashSet<DependencyReference<'a>>,
    resolved: HashSet<DependencyReference<'a>>,
    blockers: BTreeMap<DependencyBlocker<'a>, HashSet<DependencyNode<'a>>>,
    enum_blockers: BTreeMap<DependencyBlocker<'a>, EnumOmissionCause>,
    root_origins: HashMap<usize, &'a Origin>,
}

impl<'a> DependencyClosureDiagnostics<'a> {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    fn add_root(
        &mut self,
        kind: &'static str,
        name: &'a str,
        tu: &'a str,
        source: &'a Location,
    ) -> DependencyNode<'a> {
        if !self.enabled {
            return DependencyNode::Root(usize::MAX);
        }
        let index = self.roots.len();
        self.roots.push(DependencyRoot {
            kind,
            name,
            tu,
            source,
        });
        DependencyNode::Root(index)
    }

    fn add_edge(&mut self, source: DependencyNode<'a>, target: DependencyNode<'a>) {
        if !self.enabled {
            return;
        }
        self.reverse_edges.entry(target).or_default().insert(source);
    }

    fn process(&mut self, reference: DependencyReference<'a>) {
        if !self.enabled {
            return;
        }
        self.processed.insert(reference);
    }

    fn resolve(&mut self, reference: DependencyReference<'a>) {
        if !self.enabled {
            return;
        }
        self.resolved.insert(reference);
    }

    fn block(
        &mut self,
        reference: DependencyReference<'a>,
        reason: impl Into<String>,
        source: DependencyNode<'a>,
    ) {
        if !self.enabled {
            return;
        }
        self.blockers
            .entry(DependencyBlocker {
                reference,
                reason: reason.into(),
            })
            .or_default()
            .insert(source);
    }

    fn is_blocked(&self) -> bool {
        !self.blockers.is_empty()
    }

    fn selected_roots(&self) -> usize {
        self.roots.len()
    }

    fn processed_references(&self) -> usize {
        self.processed.len()
    }

    fn resolved_references(&self) -> usize {
        let blocked: HashSet<_> = self
            .blockers
            .keys()
            .map(|blocker| &blocker.reference)
            .collect();
        self.resolved
            .iter()
            .filter(|reference| !blocked.contains(reference))
            .count()
    }

    fn unique_blockers(&self) -> usize {
        self.blockers.len()
    }

    fn referencing_roots(
        &self,
        sources: &HashSet<DependencyNode<'a>>,
    ) -> BTreeSet<DependencyRoot<'a>> {
        let mut roots = BTreeSet::new();
        let mut seen = HashSet::new();
        let mut queue: Vec<_> = sources.iter().copied().collect();
        while let Some(node) = queue.pop() {
            if !seen.insert(node) {
                continue;
            }
            match node {
                DependencyNode::Root(index) => {
                    roots.insert(self.roots[index]);
                }
                DependencyNode::Fact(_) => {
                    queue.extend(self.reverse_edges.get(&node).into_iter().flatten().copied());
                }
            }
        }
        roots
    }

    fn referencing_fact_roots(&self, fact: &'a Fact) -> BTreeSet<DependencyRoot<'a>> {
        self.referencing_roots(&HashSet::from([DependencyNode::Fact(&fact.origin)]))
    }

    fn error(&self) -> Error {
        let mut result = format!(
            "header partition dependency closure found {} blocker(s)\n\
             coverage: selected_roots={} processed_unique_dependencies={} \
             resolved_dependencies={} unique_blockers={}",
            self.unique_blockers(),
            self.selected_roots(),
            self.processed_references(),
            self.resolved_references(),
            self.unique_blockers(),
        );
        for (index, (blocker, sources)) in self.blockers.iter().enumerate() {
            let declaration = blocker.reference.declaration.as_ref().map_or_else(
                || "projected dependency".to_string(),
                |declaration| format!("{}:{}", declaration.file, declaration.offset),
            );
            write!(
                result,
                "\n{}. `{}` in translation unit `{}` at {}\n   reason: ",
                index + 1,
                blocker.reference.name,
                blocker.reference.tu,
                declaration,
            )
            .unwrap();
            for (line, reason) in blocker.reason.lines().enumerate() {
                if line > 0 {
                    result.push_str("\n           ");
                }
                result.push_str(reason);
            }
            result.push_str("\n   referenced by:");
            let roots = self.referencing_roots(sources);
            if roots.is_empty() {
                result.push_str("\n     - no selected root provenance");
            } else {
                for root in roots {
                    write!(
                        result,
                        "\n     - {} `{}` in translation unit `{}` at {}:{}",
                        root.kind, root.name, root.tu, root.source.file, root.source.offset,
                    )
                    .unwrap();
                }
            }
        }
        result.push_str(
            "\nlimitation: dependencies beneath a missing, ambiguous, or unsupported type \
             cannot be inspected until that blocker is resolved; later layout, ownership, and RDL \
             validation has not run for this failed plan",
        );
        Error(result)
    }
}

fn owner_exclusion_error(
    snapshot: &Snapshot,
    diagnostics: &DependencyClosureDiagnostics<'_>,
    blockers: &[&Fact],
    display_names: Option<&BTreeMap<String, String>>,
) -> Error {
    let mut result = format!(
        "header partition owner validation found {} blocker(s)",
        blockers.len()
    );
    for (index, fact) in blockers.iter().enumerate() {
        let name = display_names
            .and_then(|names| names.get(&fact.name))
            .map_or(fact.name.as_str(), String::as_str);
        if let Some(owner) = snapshot.root_owners.get(&fact.origin) {
            write!(
                result,
                "\n{}. owner-excluded local type `{}` in partition `{}` namespace `{}` is \
                 required without a retained public alias",
                index + 1,
                name,
                owner.partition,
                owner.namespace,
            )
            .unwrap();
        } else {
            write!(
                result,
                "\n{}. owner-excluded local type `{}` in translation unit `{}` is required \
                 without a retained public alias or logical owner",
                index + 1,
                name,
                fact.origin.tu,
            )
            .unwrap();
        }
        write!(
            result,
            "\n   declaration: {}:{}\n   referenced by:",
            fact.spelling.file, fact.spelling.offset,
        )
        .unwrap();
        let roots = diagnostics.referencing_fact_roots(fact);
        if roots.is_empty() {
            result.push_str("\n     - no selected root provenance");
        } else {
            for root in roots {
                write!(
                    result,
                    "\n     - {} `{}` in translation unit `{}` at {}:{}",
                    root.kind, root.name, root.tu, root.source.file, root.source.offset,
                )
                .unwrap();
            }
        }
    }
    result.push_str("\nno RDL was emitted");
    Error(result)
}

type PointerAliasDeclarations<'a> = BTreeSet<(&'a str, &'a Location, &'a str)>;

fn collect_fact_pointer_alias_candidates<'a>(
    fact: &'a Fact,
    callback_requirements: &PointerAliasDeclarations<'_>,
    candidates: &mut PointerAliasDeclarations<'a>,
) {
    let alias_typedef = matches!(
        fact.data,
        FactData::Typedef {
            target: TypeRef::Pointer { .. } | TypeRef::Reference { .. }
        }
    );
    visit_fact_pointer_alias_references(fact, &mut |tu, declaration, name, parent_mutable| {
        if pointer_alias_reference_is_retained(
            tu,
            declaration,
            name,
            parent_mutable,
            alias_typedef,
            callback_requirements,
        ) {
            candidates.insert((tu, declaration, name));
        }
    });
}

fn pointer_alias_root_is_selected(fact: &Fact, options: &EmitOptions<'_>) -> bool {
    if !fact.root {
        return false;
    }
    let FactData::Function { link_name, .. } = &fact.data else {
        return true;
    };
    if options
        .excluded_functions
        .or(options.excluded)
        .is_some_and(|excluded| excluded.contains(&fact.name))
    {
        return false;
    }
    options
        .functions
        .is_none_or(|functions| functions.contains(link_name))
}

fn callback_dependency_pointer_alias_requirements<'a>(
    declarations: &DeclarationIndex<'a>,
    functions: impl IntoIterator<Item = &'a Fact>,
) -> PointerAliasDeclarations<'a> {
    let mut requirements = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for function in functions {
        let FactData::Function { params, result, .. } = &function.data else {
            continue;
        };
        collect_callback_dependency_pointer_aliases(
            result,
            function.origin.tu.as_str(),
            declarations,
            &mut seen,
            &mut requirements,
        );
        for param in params {
            collect_callback_dependency_pointer_aliases(
                &param.ty,
                function.origin.tu.as_str(),
                declarations,
                &mut seen,
                &mut requirements,
            );
        }
    }
    requirements
}

fn collect_callback_dependency_pointer_aliases<'a>(
    ty: &'a TypeRef,
    tu: &'a str,
    declarations: &DeclarationIndex<'a>,
    seen: &mut BTreeSet<(String, String, Location)>,
    requirements: &mut PointerAliasDeclarations<'a>,
) {
    match ty {
        TypeRef::Named { name, declaration } => {
            let key = (tu.to_string(), name.clone(), declaration.clone());
            if !seen.insert(key) {
                return;
            }
            for fact in declarations.get(tu, name, declaration) {
                match &fact.data {
                    FactData::Callback { params, result, .. } => {
                        let tu = fact.origin.tu.as_str();
                        for ty in
                            std::iter::once(result).chain(params.iter().map(|param| &param.ty))
                        {
                            if let TypeRef::Named { name, declaration } = ty
                                && canonical_raw_pointer_name(name)
                            {
                                requirements.insert((tu, declaration, name));
                            }
                        }
                    }
                    FactData::Typedef { target } => collect_callback_dependency_pointer_aliases(
                        target,
                        fact.origin.tu.as_str(),
                        declarations,
                        seen,
                        requirements,
                    ),
                    _ => {}
                }
            }
        }
        TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. }
        | TypeRef::Array { target, .. } => collect_callback_dependency_pointer_aliases(
            target,
            tu,
            declarations,
            seen,
            requirements,
        ),
        TypeRef::Generic { args, .. } => {
            for arg in args {
                collect_callback_dependency_pointer_aliases(
                    arg,
                    tu,
                    declarations,
                    seen,
                    requirements,
                );
            }
        }
        _ => {}
    }
}

fn pointer_alias_reference_is_retained(
    tu: &str,
    declaration: &Location,
    name: &str,
    parent_mutable: bool,
    alias_typedef: bool,
    callback_requirements: &PointerAliasDeclarations<'_>,
) -> bool {
    canonical_raw_pointer_mutability(name).is_none_or(|alias_mutable| {
        alias_typedef
            || alias_mutable != parent_mutable
            || callback_requirements.contains(&(tu, declaration, name))
    })
}

fn visit_fact_pointer_alias_references<'a, F>(fact: &'a Fact, visit: &mut F)
where
    F: FnMut(&'a str, &'a Location, &'a str, bool),
{
    let tu = fact.origin.tu.as_str();
    match &fact.data {
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            visit_pointer_alias_references(result, tu, None, visit);
            for param in params {
                visit_pointer_alias_references(&param.ty, tu, None, visit);
            }
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                visit_pointer_alias_references(base, tu, None, visit);
            }
            for method in methods {
                visit_pointer_alias_references(&method.result, tu, None, visit);
                for param in &method.params {
                    visit_pointer_alias_references(&param.ty, tu, None, visit);
                }
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                visit_pointer_alias_references(base, tu, None, visit);
            }
            for field in fields {
                visit_pointer_alias_references(&field.ty, tu, None, visit);
            }
        }
        FactData::Typedef { target } => {
            visit_pointer_alias_references(target, tu, None, visit);
        }
        _ => {}
    }
}

fn collect_pointer_alias_candidates<'a>(
    ty: &'a TypeRef,
    tu: &'a str,
    callback_requirements: &PointerAliasDeclarations<'_>,
    candidates: &mut PointerAliasDeclarations<'a>,
) {
    visit_pointer_alias_references(
        ty,
        tu,
        None,
        &mut |tu, declaration, name, parent_mutable| {
            if pointer_alias_reference_is_retained(
                tu,
                declaration,
                name,
                parent_mutable,
                false,
                callback_requirements,
            ) {
                candidates.insert((tu, declaration, name));
            }
        },
    );
}

fn visit_pointer_alias_references<'a, F>(
    ty: &'a TypeRef,
    tu: &'a str,
    pointer_parent: Option<bool>,
    visit: &mut F,
) where
    F: FnMut(&'a str, &'a Location, &'a str, bool),
{
    match ty {
        TypeRef::Named { name, declaration } => {
            if let Some(parent_mutable) = pointer_parent {
                visit(tu, declaration, name, parent_mutable);
            }
        }
        TypeRef::Pointer { mutable, target } | TypeRef::Reference { mutable, target } => {
            visit_pointer_alias_references(target, tu, Some(*mutable), visit);
        }
        TypeRef::FunctionPointer { params, result, .. } => {
            visit_pointer_alias_references(result, tu, None, visit);
            for param in params {
                visit_pointer_alias_references(param, tu, None, visit);
            }
        }
        TypeRef::Array { target, .. } => {
            visit_pointer_alias_references(target, tu, None, visit);
        }
        TypeRef::Generic { args, .. } => {
            for arg in args {
                visit_pointer_alias_references(arg, tu, None, visit);
            }
        }
        TypeRef::InlineRecord(record) => {
            if let Some(base) = &record.base {
                visit_pointer_alias_references(base, tu, None, visit);
            }
            for field in &record.fields {
                visit_pointer_alias_references(&field.ty, tu, None, visit);
            }
        }
        _ => {}
    }
}

fn is_pointer_alias_fact(
    fact: &Fact,
    facts_by_declaration: &BTreeMap<(String, Location), &Fact>,
    seen: &mut BTreeSet<Origin>,
) -> bool {
    if !seen.insert(fact.origin.clone()) {
        return false;
    }
    let FactData::Typedef { target } = &fact.data else {
        return false;
    };
    match target {
        TypeRef::Pointer { .. }
        | TypeRef::Reference { .. }
        | TypeRef::FunctionPointer { .. }
        | TypeRef::OpaquePointer { .. } => true,
        TypeRef::Named { name, declaration } => {
            canonical_pointer_alias_name(name)
                || facts_by_declaration
                    .get(&(fact.origin.tu.clone(), declaration.clone()))
                    .is_some_and(|target| is_pointer_alias_fact(target, facts_by_declaration, seen))
        }
        _ => false,
    }
}

fn canonical_pointer_alias_name(name: &str) -> bool {
    canonical_string_name(name).is_some() || canonical_raw_pointer_name(name)
}

fn is_string_alias_fact(
    fact: &Fact,
    facts_by_declaration: &BTreeMap<(String, Location), &Fact>,
    seen: &mut BTreeSet<Origin>,
) -> bool {
    if canonical_string_name(&fact.name).is_some() {
        return true;
    }
    if !seen.insert(fact.origin.clone()) {
        return false;
    }
    let FactData::Typedef {
        target: TypeRef::Named { name, declaration },
    } = &fact.data
    else {
        return false;
    };
    canonical_string_name(name).is_some()
        || facts_by_declaration
            .get(&(fact.origin.tu.clone(), declaration.clone()))
            .is_some_and(|target| is_string_alias_fact(target, facts_by_declaration, seen))
}

fn canonical_raw_pointer_name(name: &str) -> bool {
    canonical_raw_pointer_mutability(name).is_some()
}

fn canonical_raw_pointer_mutability(name: &str) -> Option<bool> {
    let target = canonical_named_type(name)?;
    if target.starts_with("*mut ") {
        Some(true)
    } else if target.starts_with("*const ") {
        Some(false)
    } else {
        None
    }
}

fn type_ref_uses_alias(ty: &TypeRef, aliases: &BTreeSet<&str>) -> bool {
    match ty {
        TypeRef::Named { name, .. } => {
            aliases.contains(canonical_string_name(name).unwrap_or(name))
        }
        TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. }
        | TypeRef::Array { target, .. } => type_ref_uses_alias(target, aliases),
        TypeRef::FunctionPointer { params, result, .. } => {
            type_ref_uses_alias(result, aliases)
                || params
                    .iter()
                    .any(|param| type_ref_uses_alias(param, aliases))
        }
        TypeRef::Generic { args, .. } => args.iter().any(|arg| type_ref_uses_alias(arg, aliases)),
        TypeRef::InlineRecord(record) => {
            record
                .base
                .as_ref()
                .is_some_and(|base| type_ref_uses_alias(base, aliases))
                || record
                    .fields
                    .iter()
                    .any(|field| type_ref_uses_alias(&field.ty, aliases))
        }
        _ => false,
    }
}

fn queue_fact_type_refs(data: &FactData, tu: &str, queue: &mut Vec<(String, TypeRef)>) {
    let mut push = |ty: &TypeRef| queue.push((tu.to_string(), ty.clone()));
    match data {
        FactData::Typedef { target } => push(target),
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            push(result);
            for param in params {
                push(&param.ty);
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                push(base);
            }
            for field in fields {
                push(&field.ty);
            }
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                push(base);
            }
            for method in methods {
                push(&method.result);
                for param in &method.params {
                    push(&param.ty);
                }
            }
        }
        _ => {}
    }
}

fn queue_type_edges<'a>(
    fact: &'a Fact,
    projected_type_names: &'a BTreeMap<Origin, String>,
    annotations: &'a BTreeMap<AnnotationTarget, Vec<Annotation>>,
    sal_constant_sizes: Option<&'a SalConstantSizes>,
    source: DependencyNode<'a>,
    queue: &mut Vec<PendingTypeEdge<'a>>,
) {
    let tu = fact.origin.tu.as_str();
    let mut push = |edge| queue.push((tu, edge, source));
    match &fact.data {
        FactData::Typedef {
            target: TypeRef::InlineRecord(record),
        } => {
            queue_record_edges(
                &record.fields,
                annotations,
                &fact.origin,
                &[],
                source,
                queue,
            );
        }
        FactData::Typedef { target } => push(TypeEdge::Type(target)),
        FactData::Callback { params, result, .. } => {
            push(TypeEdge::Use(
                result,
                annotations_for(annotations, &AnnotationTarget::Return(fact.origin.clone())),
            ));
            for (index, param) in params.iter().enumerate() {
                let target = AnnotationTarget::Parameter {
                    declaration: fact.origin.clone(),
                    index,
                };
                push(TypeEdge::Parameter(
                    param,
                    annotations_for(annotations, &target),
                    sal_constant_sizes.and_then(|sizes| sizes.get_key_value(&target)),
                ));
            }
        }
        FactData::PropertyKey { ty, .. } => push(TypeEdge::Projected(
            projected_type_names
                .get(&fact.origin)
                .map_or(*ty, String::as_str),
        )),
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                push(TypeEdge::Type(base));
            }
            queue_record_edges(fields, annotations, &fact.origin, &[], source, queue);
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                push(TypeEdge::Type(base));
            }
            for (index, method) in methods.iter().enumerate() {
                push(TypeEdge::Use(
                    &method.result,
                    annotations_for(
                        annotations,
                        &AnnotationTarget::MethodReturn {
                            declaration: fact.origin.clone(),
                            index,
                        },
                    ),
                ));
                for (parameter, param) in method.params.iter().enumerate() {
                    let target = AnnotationTarget::MethodParameter {
                        declaration: fact.origin.clone(),
                        method: index,
                        parameter,
                    };
                    push(TypeEdge::Parameter(
                        param,
                        annotations_for(annotations, &target),
                        sal_constant_sizes.and_then(|sizes| sizes.get_key_value(&target)),
                    ));
                }
            }
        }
        _ => {}
    }
}

fn queue_record_edges<'a>(
    fields: &'a [Field],
    annotations: &'a BTreeMap<AnnotationTarget, Vec<Annotation>>,
    origin: &'a Origin,
    path: &[usize],
    source: DependencyNode<'a>,
    queue: &mut Vec<PendingTypeEdge<'a>>,
) {
    for (index, field) in fields.iter().enumerate() {
        let mut nested_path = path.to_vec();
        nested_path.push(index);
        if let TypeRef::InlineRecord(record) = &field.ty {
            if let Some(base) = &record.base {
                queue.push((origin.tu.as_str(), TypeEdge::Type(base), source));
            }
            queue_record_edges(
                &record.fields,
                annotations,
                origin,
                &nested_path,
                source,
                queue,
            );
        } else {
            let target = if path.is_empty() {
                AnnotationTarget::Field {
                    declaration: origin.clone(),
                    index,
                }
            } else {
                AnnotationTarget::NestedField {
                    declaration: origin.clone(),
                    path: nested_path,
                }
            };
            queue.push((
                origin.tu.as_str(),
                TypeEdge::Use(&field.ty, annotations_for(annotations, &target)),
                source,
            ));
        }
    }
}

fn queue_function_edges<'a>(
    fact: &'a Fact,
    annotations: &'a BTreeMap<AnnotationTarget, Vec<Annotation>>,
    sal_constant_sizes: Option<&'a SalConstantSizes>,
    source: DependencyNode<'a>,
    queue: &mut Vec<PendingTypeEdge<'a>>,
) {
    if let FactData::Function { params, result, .. } = &fact.data {
        let tu = fact.origin.tu.as_str();
        let mut push = |edge| queue.push((tu, edge, source));
        push(TypeEdge::Use(
            result,
            annotations_for(annotations, &AnnotationTarget::Return(fact.origin.clone())),
        ));
        for (index, param) in params.iter().enumerate() {
            let target = AnnotationTarget::Parameter {
                declaration: fact.origin.clone(),
                index,
            };
            push(TypeEdge::Parameter(
                param,
                annotations_for(annotations, &target),
                sal_constant_sizes.and_then(|sizes| sizes.get_key_value(&target)),
            ));
        }
    }
}

#[derive(Debug)]
pub struct Error(String);

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for Error {}

struct BitfieldGroup {
    start: usize,
    end: usize,
    offset: i64,
}

fn bitfield_groups(fields: &[Field]) -> Result<Vec<BitfieldGroup>, Error> {
    let mut groups = vec![];
    let mut index = 0;
    while index < fields.len() {
        let field = &fields[index];
        let Some(width) = field.bit_width else {
            index += 1;
            continue;
        };
        if width == 0 {
            index += 1;
            continue;
        }
        let start = index;
        let offset = field.offset;
        let limit = offset + field.size * 8;
        index += 1;
        while index < fields.len() {
            let next = &fields[index];
            let Some(width) = next.bit_width else {
                break;
            };
            if width == 0 || next.size != field.size || next.offset + i64::from(width) > limit {
                break;
            }
            index += 1;
        }
        groups.push(BitfieldGroup {
            start,
            end: index,
            offset,
        });
    }
    Ok(groups)
}

fn annotations_for<'a>(
    annotations: &'a BTreeMap<AnnotationTarget, Vec<Annotation>>,
    target: &AnnotationTarget,
) -> &'a [Annotation] {
    annotations.get(target).map_or(&[], Vec::as_slice)
}

fn annotation_lines(annotations: &[Annotation], indent: &str) -> Result<String, Error> {
    let mut result = String::new();
    for annotation in annotations {
        if let Some(value) = annotation_rdl(annotation)? {
            result.push_str(indent);
            result.push_str(&value);
            result.push('\n');
        }
    }
    Ok(result)
}

fn annotation_inline(annotations: &[Annotation]) -> Result<String, Error> {
    let mut result = String::new();
    for annotation in annotations {
        if let Some(value) = annotation_rdl(annotation)? {
            result.push_str(&value);
            result.push(' ');
        }
    }
    Ok(result)
}

fn annotation_rdl(annotation: &Annotation) -> Result<Option<String>, Error> {
    Ok(Some(match annotation {
        Annotation::SetLastError | Annotation::ImportLibrary(_) => return Ok(None),
        Annotation::PreserveResult => "#[preserve_sig]".to_string(),
        Annotation::RaiiFree(name) => format!("#[raii_free({name:?})]"),
        Annotation::InvalidHandle(value) => {
            let value = value
                .parse::<i64>()
                .map_err(|_| Error(format!("invalid handle value `{value}` was not resolved")))?;
            format!("#[invalid_handle({value})]")
        }
        Annotation::FreeWith(name) => format!("#[free_with({name:?})]"),
        Annotation::DoNotRelease => "#[do_not_release]".to_string(),
        Annotation::NotNullTerminated => "#[not_null_terminated]".to_string(),
        Annotation::NullNullTerminated => "#[null_null_terminated]".to_string(),
        Annotation::ArrayCountParam(value) => {
            let value = value.parse::<i16>().map_err(|_| {
                Error(format!(
                    "array-count parameter index `{value}` is not an i16"
                ))
            })?;
            format!("#[len_param({value})]")
        }
        Annotation::ArrayCountConst(value) => {
            let value = value
                .parse::<i32>()
                .map_err(|_| Error(format!("array count `{value}` is not an i32")))?;
            format!("#[len_const({value})]")
        }
        Annotation::ArrayCountField(name) => format!("#[len_field({name:?})]"),
        Annotation::MemorySizeParam(value) => {
            let value = value.parse::<i16>().map_err(|_| {
                Error(format!(
                    "memory-size parameter index `{value}` is not an i16"
                ))
            })?;
            format!("#[size_param({value})]")
        }
        Annotation::MemorySizeConst(value) => {
            let value = value
                .parse::<i32>()
                .map_err(|_| Error(format!("memory size `{value}` is not an i32")))?;
            format!("#[size_const({value})]")
        }
        Annotation::CanReturnErrorsAsSuccess => "#[errors_as_success]".to_string(),
        Annotation::CanReturnMultipleSuccessValues => "#[multiple_success_values]".to_string(),
        Annotation::Retained => "#[retained]".to_string(),
        Annotation::IgnoreIfReturn(value) => format!("#[ignore_if_return({value:?})]"),
        Annotation::AlsoUsableFor(name) => format!("#[also_usable_for({name:?})]"),
        Annotation::AssociatedEnum(name) => format!("#[associated_enum({name:?})]"),
        Annotation::AssociatedConstant(name) => format!("#[associated_constant({name:?})]"),
        Annotation::NativeInheritance(name) => format!("#[native_inheritance({name:?})]"),
        Annotation::StructSizeField(name) => format!("#[struct_size_field({name:?})]"),
        Annotation::NativeEncoding(name) => format!("#[encoding({name:?})]"),
        Annotation::Ansi => "#[ansi]".to_string(),
        Annotation::Unicode => "#[unicode]".to_string(),
        Annotation::Agile => "#[agile]".to_string(),
        Annotation::Const => "#[native_const]".to_string(),
        Annotation::StaticLibrary(name) => format!("#[static_library({name:?})]"),
        Annotation::SupportedOs(platform) => format!("#[supported_os({platform:?})]"),
        Annotation::In => "#[in]".to_string(),
        Annotation::Out => "#[out]".to_string(),
        Annotation::Optional => "#[opt]".to_string(),
        Annotation::Reserved => "#[reserved]".to_string(),
        Annotation::ComOutPtr => "#[iid_is]".to_string(),
        Annotation::Retval => "#[retval]".to_string(),
    }))
}

fn write_callback(
    name: &str,
    convention: CallingConvention,
    params: &[Parameter],
    result: &TypeRef,
    projection: &TypeProjection,
    annotations: &BTreeMap<AnnotationTarget, Vec<Annotation>>,
    origin: &Origin,
) -> Result<String, Error> {
    let params = write_params(
        params,
        projection,
        annotations,
        CallableTarget::Callback(origin),
    )?
    .join(", ");
    let result = if *result == TypeRef::Void {
        String::new()
    } else {
        let annotations = annotations_for(annotations, &AnnotationTarget::Return(origin.clone()));
        format!(
            " -> {}",
            projection
                .use_type(result, annotations)
                .annotated_name(annotations)?
        )
    };
    Ok(format!(
        "{}    extern{} fn {}({params}){result};\n",
        annotation_lines(
            annotations_for(annotations, &AnnotationTarget::Declaration(origin.clone()),),
            "    ",
        )?,
        calling_convention(convention),
        rdl_ident(name)
    ))
}

#[derive(Clone, Copy)]
enum CallableTarget<'a> {
    Function(&'a Origin),
    Callback(&'a Origin),
    Method(&'a Origin, usize),
}

fn write_params(
    params: &[Parameter],
    projection: &TypeProjection<'_>,
    annotations: &BTreeMap<AnnotationTarget, Vec<Annotation>>,
    target: CallableTarget<'_>,
) -> Result<Vec<String>, Error> {
    params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let target = match target {
                CallableTarget::Function(origin) | CallableTarget::Callback(origin) => {
                    AnnotationTarget::Parameter {
                        declaration: origin.clone(),
                        index,
                    }
                }
                CallableTarget::Method(origin, method) => AnnotationTarget::MethodParameter {
                    declaration: origin.clone(),
                    method,
                    parameter: index,
                },
            };
            let metadata_annotations = annotations_for(annotations, &target);
            let ty = projection.parameter(param, metadata_annotations, &target);
            Ok(format!(
                "{}{}: {}",
                param_attributes(
                    param,
                    params,
                    ty.mutable,
                    &ty.annotations(metadata_annotations),
                )?,
                rdl_ident(&param.name),
                ty.name
            ))
        })
        .collect()
}

fn param_attributes(
    param: &Parameter,
    params: &[Parameter],
    emitted_mutable: bool,
    metadata_annotations: &[Annotation],
) -> Result<String, Error> {
    let annotation = &param.annotation;
    if let Some(reason) = &annotation.unsupported {
        return Err(Error(reason.clone()));
    }
    let mut result = String::new();
    if let Some(size) = &annotation.size {
        match &size.value {
            SalSizeValue::Constant(value) if !size.bytes => {
                result.push_str(&format!("#[len_const({value})] "));
            }
            SalSizeValue::Parameter(name) | SalSizeValue::IndirectParameter(name) => {
                let index = params
                    .iter()
                    .position(|param| param.name == *name)
                    .ok_or_else(|| Error(format!("unresolved SAL size parameter `{name}`")))?;
                let attr = if size.bytes {
                    "size_param"
                } else {
                    "len_param"
                };
                result.push_str(&format!("#[{attr}({index})] "));
            }
            SalSizeValue::Constant(value) => {
                result.push_str(&format!("#[size_const({value})] "));
            }
            SalSizeValue::Expression(_) => {}
        }
    }
    if annotation.reserved && !metadata_annotations.contains(&Annotation::Reserved) {
        result.push_str("#[reserved] ");
    }
    if annotation.com_out_ptr && !metadata_annotations.contains(&Annotation::ComOutPtr) {
        result.push_str("#[iid_is] ");
    }
    if annotation.input
        && (annotation.output || emitted_mutable)
        && !metadata_annotations.contains(&Annotation::In)
    {
        result.push_str("#[in] ");
    }
    if annotation.output
        && (annotation.input || !emitted_mutable)
        && !metadata_annotations.contains(&Annotation::Out)
    {
        result.push_str("#[out] ");
    }
    if annotation.optional && !metadata_annotations.contains(&Annotation::Optional) {
        result.push_str("#[opt] ");
    }
    if annotation.null_null_terminated
        && !metadata_annotations.contains(&Annotation::NullNullTerminated)
    {
        result.push_str("#[null_null_terminated] ");
    }
    if annotation.retval && !metadata_annotations.contains(&Annotation::Retval) {
        result.push_str("#[retval] ");
    }
    result.push_str(&annotation_inline(metadata_annotations)?);
    Ok(result)
}

fn write_interface(
    name: &str,
    base: Option<&TypeRef>,
    guid: Option<&str>,
    methods: &[Method],
    projection: &TypeProjection,
    annotations: &BTreeMap<AnnotationTarget, Vec<Annotation>>,
    origin: &Origin,
) -> Result<String, Error> {
    let base = base.map_or_else(String::new, |base| format!(": {}", projection.name(base)));
    let attribute = guid.map_or_else(
        || "#[no_guid]".to_string(),
        |guid| format!("#[guid({})]", rdl_uuid(guid)),
    );
    let mut result = format!(
        "{}    {attribute}\n    interface {name}{base} {{\n",
        annotation_lines(
            annotations_for(annotations, &AnnotationTarget::Declaration(origin.clone()),),
            "    ",
        )?
    );
    let mut start = 0;
    while start < methods.len() {
        let mut end = start + 1;
        while end < methods.len() && methods[end].name == methods[start].name {
            end += 1;
        }
        for method_index in (start..end).rev() {
            let method = &methods[method_index];
            let params = write_params(
                &method.params,
                projection,
                annotations,
                CallableTarget::Method(origin, method_index),
            )?
            .join(", ");
            let return_type = if method.result == TypeRef::Void {
                String::new()
            } else {
                let annotations = annotations_for(
                    annotations,
                    &AnnotationTarget::MethodReturn {
                        declaration: origin.clone(),
                        index: method_index,
                    },
                );
                format!(
                    " -> {}",
                    projection
                        .use_type(&method.result, annotations)
                        .annotated_name(annotations)?
                )
            };
            result.push_str(&format!(
                "{}        {}fn {}(&self{}{}){return_type};\n",
                annotation_lines(
                    annotations_for(
                        annotations,
                        &AnnotationTarget::Method {
                            declaration: origin.clone(),
                            index: method_index,
                        },
                    ),
                    "        ",
                )?,
                if method.special { "#[special] " } else { "" },
                rdl_ident(&method.name),
                if params.is_empty() { "" } else { ", " },
                params
            ));
        }
        start = end;
    }
    result.push_str("    }\n");
    Ok(result)
}

fn rdl_uuid(guid: &str) -> String {
    let hex = guid.replace('-', "");
    format!(
        "0x{}_{}_{}_{}_{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

fn parameter_string_name(param: &Parameter) -> Option<&'static str> {
    match &param.ty {
        TypeRef::Pointer { mutable, target } if param.annotation.null_terminated => {
            let shape = match target.as_ref() {
                TypeRef::Scalar(Scalar::I8 | Scalar::U8) => "i8",
                TypeRef::Scalar(Scalar::U16) => "u16",
                TypeRef::Named { name, .. } => canonical_named_type(name)?,
                _ => return None,
            };
            match (mutable, shape) {
                (false, "i8" | "u8") => Some("PCSTR"),
                (true, "i8" | "u8") => Some("PSTR"),
                (false, "u16") => Some("PCWSTR"),
                (true, "u16") => Some("PWSTR"),
                _ => None,
            }
        }
        TypeRef::Named { name, .. } => {
            if param.annotation.input && !param.annotation.output {
                match name.as_str() {
                    "LPSTR" => return Some("PCSTR"),
                    "LPWSTR" => return Some("PCWSTR"),
                    _ => {}
                }
            }
            canonical_string_name(name)
        }
        _ => None,
    }
}

struct NativeStringType {
    name: &'static str,
    mutable: bool,
    terminated: bool,
    counted: bool,
    double_terminated: bool,
}

fn native_string_type<'a>(
    mut ty: &'a TypeRef,
    tu: &str,
    facts: &BTreeMap<(String, Location), &'a Fact>,
    source_names: Option<&PlanningSourceNames>,
) -> Option<NativeStringType> {
    let mut seen = BTreeSet::new();
    let mut terminated = false;
    let mut counted = false;
    let mut double_terminated = false;
    while let TypeRef::Named { declaration, .. } = ty {
        let fact = facts.get(&(tu.to_string(), declaration.clone()))?;
        if !seen.insert(&fact.origin) {
            return None;
        }
        if seen.len() == 1 {
            let name = source_names.map_or(fact.name.as_str(), |names| names.fact_name(fact));
            terminated = canonical_string_name(name).is_some();
            counted = matches!(
                name,
                "PCH" | "LPCH" | "PCCH" | "LPCCH" | "PWCH" | "LPWCH" | "PCWCH" | "LPCWCH"
            );
            double_terminated = matches!(name, "PZZSTR" | "PCZZSTR" | "PZZWSTR" | "PCZZWSTR");
        }
        let FactData::Typedef { target } = &fact.data else {
            return None;
        };
        ty = target;
    }
    let TypeRef::Pointer { mutable, target } = ty else {
        return None;
    };
    let mut target = target.as_ref();
    while let TypeRef::Named { declaration, .. } = target {
        let fact = facts.get(&(tu.to_string(), declaration.clone()))?;
        if !seen.insert(&fact.origin) {
            return None;
        }
        let FactData::Typedef { target: next } = &fact.data else {
            return None;
        };
        target = next;
    }
    let name = match target {
        TypeRef::Scalar(Scalar::I8 | Scalar::U8) => "PSTR",
        TypeRef::Scalar(Scalar::U16) => "PWSTR",
        _ => return None,
    };
    Some(NativeStringType {
        name,
        mutable: *mutable,
        terminated,
        counted,
        double_terminated,
    })
}

fn string_type_choice<'a>(
    ty: &'a TypeRef,
    tu: &str,
    param: Option<&ParamAnnotation>,
    annotations: &[Annotation],
    facts: &BTreeMap<(String, Location), &'a Fact>,
    source_names: Option<&PlanningSourceNames>,
) -> Result<Option<TypeChoice<'a>>, Error> {
    let Some(native) = native_string_type(ty, tu, facts, source_names) else {
        return Ok(None);
    };
    if matches!(ty, TypeRef::Named { .. })
        && !native.terminated
        && !native.counted
        && !native.double_terminated
    {
        return Ok(None);
    }
    let terminated = native.double_terminated
        || param.is_some_and(|param| param.null_terminated || param.null_null_terminated)
        || annotations.contains(&Annotation::NullNullTerminated);
    if !native.terminated && !native.counted && !terminated {
        return Ok(None);
    }
    let counted = param.is_some_and(|param| param.size.is_some())
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
    Ok(Some(TypeChoice::String {
        name: native.name,
        mutable: native.mutable,
        not_null_terminated: !terminated && (native.counted || counted),
        null_null_terminated: native.double_terminated,
    }))
}

fn parameter_use_error(
    error: Error,
    param: &Parameter,
    source: DependencyNode<'_>,
    facts: &HashMap<&Origin, &Fact>,
) -> Error {
    if let DependencyNode::Fact(origin) = source
        && let Some(fact) = facts.get(origin)
    {
        Error(format!(
            "{}:{}: `{}` parameter `{}` in translation unit `{}`: {error}",
            fact.spelling.file, fact.spelling.offset, fact.name, param.name, fact.origin.tu,
        ))
    } else {
        error
    }
}

fn planned_emitted_type_name(ty: &TypeRef, projection: &TypeProjection<'_>) -> String {
    projection.name(ty)
}

fn internal_alias_target_resolved(ty: &TypeRef, aliases: &BTreeSet<String>) -> bool {
    match ty {
        TypeRef::Scalar(_) | TypeRef::OpaquePointer { .. } => true,
        TypeRef::Named { name, .. } => {
            aliases.contains(name) || canonical_named_type(name).is_some()
        }
        TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. }
        | TypeRef::Array { target, .. } => internal_alias_target_resolved(target, aliases),
        _ => false,
    }
}

fn canonical_named_type(name: &str) -> Option<&'static str> {
    if let Some(name) = canonical_string_name(name) {
        return Some(name);
    }
    Some(match name {
        "boolean" | "BYTE" | "UCHAR" | "UINT8" | "uint8_t" => "u8",
        "WORD" | "USHORT" | "WCHAR" | "UINT16" | "uint16_t" => "u16",
        "DWORD" | "UINT" | "ULONG" | "DWORD32" | "UINT32" | "ULONG32" | "uint32_t" => "u32",
        "QWORD" | "ULONGLONG" | "DWORD64" | "UINT64" | "ULONG64" | "uint64_t" => "u64",
        "CHAR" | "INT8" | "int8_t" => "i8",
        "SHORT" | "INT16" | "int16_t" => "i16",
        "INT" | "LONG" | "INT32" | "LONG32" | "int32_t" => "i32",
        "LONGLONG" | "INT64" | "LONG64" | "int64_t" => "i64",
        "FLOAT" => "f32",
        "DOUBLE" => "f64",
        "UINT_PTR" | "ULONG_PTR" | "DWORD_PTR" | "SIZE_T" | "size_t" | "rsize_t" | "uintptr_t" => {
            "usize"
        }
        "INT_PTR" | "LONG_PTR" | "SSIZE_T" | "intptr_t" | "ptrdiff_t" => "isize",
        "HNSTIME" => "i64",
        "SCRIPTTHREADID"
        | "mdToken"
        | "mdModule"
        | "mdTypeRef"
        | "mdTypeDef"
        | "mdFieldDef"
        | "mdMethodDef"
        | "mdParamDef"
        | "mdInterfaceImpl"
        | "mdMemberRef"
        | "mdCustomAttribute"
        | "mdPermission"
        | "mdSignature"
        | "mdEvent"
        | "mdProperty"
        | "mdModuleRef"
        | "mdAssembly"
        | "mdAssemblyRef"
        | "mdFile"
        | "mdExportedType"
        | "mdManifestResource"
        | "mdTypeSpec"
        | "mdGenericParam"
        | "mdMethodSpec"
        | "mdGenericParamConstraint"
        | "mdString"
        | "mdCPToken" => "u32",
        "COR_SIGNATURE" => "u8",
        "PCOR_SIGNATURE" => "*mut u8",
        "PCCOR_SIGNATURE" => "*const u8",
        "HCORENUM" => "*mut void",
        "MDUTF8CSTR" => "*const i8",
        "LPUNKNOWN" => "IUnknown",
        "PVOID" | "LPVOID" => "*mut void",
        "IID" | "CLSID" | "FMTID" | "UUID" => "GUID",
        "HRESULT" => "HRESULT",
        _ => return None,
    })
}

fn canonical_string_name(name: &str) -> Option<&'static str> {
    match name {
        "PCSTR" | "LPCSTR" => Some("PCSTR"),
        "PSTR" | "LPSTR" => Some("PSTR"),
        "PCWSTR" | "LPCWSTR" => Some("PCWSTR"),
        "PWSTR" | "LPWSTR" => Some("PWSTR"),
        _ => None,
    }
}

fn named_type_shape(name: &str) -> Option<&'static str> {
    match name {
        "NTSTATUS" => Some("i32"),
        _ => canonical_named_type(name),
    }
}

fn calling_convention(convention: CallingConvention) -> &'static str {
    match convention {
        CallingConvention::Platform => "",
        CallingConvention::C => " \"C\"",
    }
}

fn write_named_record(
    name: &str,
    fields: &[Field],
    packing: Option<i64>,
    alignment: Option<i64>,
    union: bool,
    projection: &TypeProjection<'_>,
    field_annotations: Option<(&BTreeMap<AnnotationTarget, Vec<Annotation>>, &Origin)>,
) -> Result<String, Error> {
    let keyword = if union { "union" } else { "struct" };
    let mut result = String::new();
    if let Some(packing) = packing {
        result.push_str(&format!("    #[packed({packing})]\n"));
    }
    if let Some(alignment) = alignment {
        result.push_str(&format!("    #[align({alignment})]\n"));
    }
    result.push_str(&format!("    {keyword} {name} {{\n"));
    result.push_str(&write_record_fields(
        fields,
        projection,
        8,
        field_annotations.map(|(annotations, origin)| (annotations, origin, Vec::new())),
    )?);
    result.push_str("    }\n");
    result.push_str(&write_nested_records(fields, projection)?);
    Ok(result)
}

fn write_nested_records(
    fields: &[Field],
    projection: &TypeProjection<'_>,
) -> Result<String, Error> {
    let mut result = String::new();
    for field in fields {
        result.push_str(&write_nested_type(&field.ty, projection)?);
    }
    Ok(result)
}

fn write_nested_type(ty: &TypeRef, projection: &TypeProjection<'_>) -> Result<String, Error> {
    match ty {
        TypeRef::Array { target, .. }
        | TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. } => write_nested_type(target, projection),
        TypeRef::InlineRecord(record) => {
            if let Some(name) = &record.name {
                write_named_record(
                    &rdl_ident(name),
                    &record.fields,
                    record.packing,
                    record.alignment,
                    record.union,
                    projection,
                    None,
                )
            } else {
                write_nested_records(&record.fields, projection)
            }
        }
        _ => Ok(String::new()),
    }
}

fn write_record_fields(
    fields: &[Field],
    projection: &TypeProjection<'_>,
    indent: usize,
    field_annotations: Option<(
        &BTreeMap<AnnotationTarget, Vec<Annotation>>,
        &Origin,
        Vec<usize>,
    )>,
) -> Result<String, Error> {
    let mut result = String::new();
    let spaces = " ".repeat(indent);
    let bitfield_groups = bitfield_groups(fields)?;
    let mut group_index = 0;
    let mut index = 0;
    while index < fields.len() {
        let field = &fields[index];
        let metadata_annotations = if let Some((annotations, origin, path)) = &field_annotations {
            let target = if path.is_empty() {
                AnnotationTarget::Field {
                    declaration: (*origin).clone(),
                    index,
                }
            } else {
                let mut path = path.clone();
                path.push(index);
                AnnotationTarget::NestedField {
                    declaration: (*origin).clone(),
                    path,
                }
            };
            annotations_for(annotations, &target)
        } else {
            &[]
        };
        let ty = if matches!(field.ty, TypeRef::InlineRecord(_)) {
            None
        } else {
            Some(projection.use_type(&field.ty, metadata_annotations))
        };
        let attributes = annotation_inline(
            &ty.as_ref()
                .map_or(Cow::Borrowed(metadata_annotations), |ty| {
                    ty.annotations(metadata_annotations)
                }),
        )?;
        if field.bit_width == Some(0) {
            index += 1;
            continue;
        }
        if let TypeRef::InlineRecord(record) = &field.ty {
            let keyword = if record.union { "union" } else { "struct" };
            result.push_str(&format!("{spaces}{attributes}{}: ", rdl_ident(&field.name)));
            if let Some(packing) = record.packing {
                result.push_str(&format!("#[packed({packing})] "));
            }
            if let Some(alignment) = record.alignment {
                result.push_str(&format!("#[align({alignment})] "));
            }
            result.push_str(&format!("{keyword} {{\n"));
            result.push_str(&write_record_fields(
                &record.fields,
                projection,
                indent + 4,
                field_annotations
                    .as_ref()
                    .map(|(annotations, origin, path)| {
                        let mut path = path.clone();
                        path.push(index);
                        (*annotations, *origin, path)
                    }),
            )?);
            result.push_str(&format!("{spaces}}},\n"));
            index += 1;
            continue;
        }
        if field.bit_width.is_none() {
            result.push_str(&format!(
                "{spaces}{attributes}{}: {},\n",
                rdl_ident(&field.name),
                ty.as_ref().unwrap().name
            ));
            index += 1;
            continue;
        }
        let group = &bitfield_groups[group_index];
        group_index += 1;
        let backing = if bitfield_groups.len() == 1 {
            "_bitfield".to_string()
        } else {
            format!("_bitfield{group_index}")
        };
        result.push_str(&format!(
            "{spaces}{attributes}{backing}: {} {{\n",
            ty.as_ref().unwrap().name
        ));
        let mut cursor = group.offset;
        for member in &fields[group.start..group.end] {
            if member.offset > cursor {
                result.push_str(&format!(
                    "{}_: {},\n",
                    " ".repeat(indent + 4),
                    member.offset - cursor
                ));
            }
            let width = member.bit_width.unwrap();
            let name = if member.name.is_empty() {
                "_".to_string()
            } else {
                rdl_ident(&member.name)
            };
            result.push_str(&format!("{}{name}: {width},\n", " ".repeat(indent + 4)));
            cursor = member.offset + i64::from(width);
        }
        result.push_str(&format!("{spaces}}},\n"));
        index = group.end;
    }
    Ok(result)
}

fn record_layout(
    fields: &[Field],
    size: i64,
    align: i64,
    union: bool,
) -> Result<(Option<i64>, Option<i64>), String> {
    if size < 0 || align <= 0 {
        return Err(format!(
            "invalid record layout: size {size}, alignment {align}"
        ));
    }
    if let Some(field) = fields
        .iter()
        .find(|field| field.offset < 0 || field.align <= 0 || field.size < 0)
    {
        return Err(format!(
            "invalid layout for field `{}`: offset {}, size {}, alignment {}",
            field.name, field.offset, field.size, field.align
        ));
    }

    let groups = bitfield_groups(fields).map_err(|error| error.0)?;
    let mut best = None;
    for packing in [None, Some(1), Some(2), Some(4), Some(8), Some(16)] {
        let mut cursor = 0;
        let mut natural_align = 1;
        let mut matches = true;
        let mut group_index = 0;
        let mut index = 0;
        while index < fields.len() {
            let field = &fields[index];
            let field_align = packing.map_or(field.align, |packing| packing.min(field.align));
            natural_align = natural_align.max(field_align);
            let mut offset = if union {
                0
            } else {
                align_up(cursor, field_align)
            };
            if offset * 8 != field.offset {
                let explicit_align = [2, 4, 8, 16]
                    .into_iter()
                    .filter(|candidate| *candidate > field_align && *candidate <= align)
                    .find(|candidate| align_up(cursor, *candidate) * 8 == field.offset);
                let Some(explicit_align) = explicit_align else {
                    matches = false;
                    break;
                };
                offset = align_up(cursor, explicit_align);
            }
            let expected = offset * 8;
            if field.bit_width == Some(0) {
                index += 1;
                continue;
            }
            if field.bit_width.is_some() {
                let group = &groups[group_index];
                group_index += 1;
                if fields[group.start..group.end].iter().any(|member| {
                    member.offset < expected
                        || member.offset + i64::from(member.bit_width.unwrap())
                            > expected + field.size * 8
                }) {
                    matches = false;
                    break;
                }
                index = group.end;
            } else {
                index += 1;
            }
            if union {
                cursor = cursor.max(field.size);
            } else {
                cursor = offset + field.size;
            }
        }
        if !matches || align < natural_align {
            continue;
        }
        let alignment = (align > natural_align).then_some(align);
        let content_size = if fields.is_empty() {
            size
        } else if cursor == 0 {
            1
        } else {
            cursor
        };
        if (cursor == 0 && size == 1) || align_up(content_size, align) == size {
            let score = (alignment.is_none(), natural_align);
            if best.is_none_or(|(_, _, best_score)| score > best_score) {
                best = Some((packing, alignment, score));
            }
        }
    }
    best.map(|(packing, alignment, _)| (packing, alignment))
        .ok_or_else(|| "record fields cannot reproduce Clang's layout".to_string())
}

fn align_up(value: i64, align: i64) -> i64 {
    (value + align - 1) / align * align
}

fn scalar_name(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Bool => "bool",
        Scalar::F32 => "f32",
        Scalar::F64 => "f64",
        Scalar::I8 => "i8",
        Scalar::U8 => "u8",
        Scalar::I16 => "i16",
        Scalar::U16 => "u16",
        Scalar::I32 => "i32",
        Scalar::U32 => "u32",
        Scalar::I64 => "i64",
        Scalar::U64 => "u64",
    }
}

fn unsigned_scalar(scalar: Scalar) -> Scalar {
    match scalar {
        Scalar::I8 => Scalar::U8,
        Scalar::I16 => Scalar::U16,
        Scalar::I32 => Scalar::U32,
        Scalar::I64 => Scalar::U64,
        _ => scalar,
    }
}

fn enum_value(value: i64, repr: Scalar) -> String {
    match repr {
        Scalar::U8 => (value as u8).to_string(),
        Scalar::U16 => (value as u16).to_string(),
        Scalar::U32 => (value as u32).to_string(),
        Scalar::U64 => (value as u64).to_string(),
        _ => value.to_string(),
    }
}

fn planned_type_name(
    ty: &TypeRef,
    type_names: &BTreeMap<String, String>,
    local_types: &BTreeMap<Location, String>,
    namespace: Option<&str>,
) -> String {
    match ty {
        TypeRef::Void => "void".to_string(),
        TypeRef::String => "String".to_string(),
        TypeRef::Object => "Object".to_string(),
        TypeRef::Scalar(scalar) => scalar_name(*scalar).to_string(),
        TypeRef::Named { name, declaration } => qualify_local_type(
            declaration,
            type_names.get(name).unwrap_or(name),
            local_types,
            namespace,
        ),
        TypeRef::Generic {
            name,
            declaration,
            args,
        } => format!(
            "{}<{}>",
            qualify_local_type(
                declaration,
                type_names.get(name).unwrap_or(name),
                local_types,
                namespace,
            ),
            args.iter()
                .map(|arg| planned_type_name(arg, type_names, local_types, namespace))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeRef::Pointer { .. } => {
            let (mutable, depth, target) = pointer_run(ty);
            format!(
                "{}{}",
                format!("*{} ", if mutable { "mut" } else { "const" }).repeat(depth),
                planned_type_name(target, type_names, local_types, namespace)
            )
        }
        TypeRef::Reference { mutable, target } => format!(
            "*{} {}",
            if *mutable { "mut" } else { "const" },
            planned_type_name(target, type_names, local_types, namespace)
        ),
        TypeRef::FunctionPointer { .. } => "*mut u8".to_string(),
        TypeRef::OpaquePointer { mutable, .. } => {
            format!("*{} void", if *mutable { "mut" } else { "const" })
        }
        TypeRef::Array { target, len } => {
            format!(
                "[{}; {len}]",
                planned_type_name(target, type_names, local_types, namespace)
            )
        }

        TypeRef::InlineRecord(record) => record
            .name
            .clone()
            .unwrap_or_else(|| "<inline record>".to_string()),
    }
}

fn qualify_local_type(
    declaration: &Location,
    fallback_name: &str,
    local_types: &BTreeMap<Location, String>,
    namespace: Option<&str>,
) -> String {
    let Some(target_namespace) = local_types.get(declaration) else {
        return rdl_ident(fallback_name);
    };
    if namespace == Some(target_namespace.as_str()) {
        rdl_ident(fallback_name)
    } else {
        format!(
            "{}::{}",
            target_namespace.replace('.', "::"),
            rdl_ident(fallback_name)
        )
    }
}

fn qualify_emitted_type(
    declaration: &Location,
    fallback_name: &str,
    local_types: &BTreeMap<Location, String>,
    routed_types: &BTreeMap<String, String>,
    namespace: Option<&str>,
) -> String {
    if local_types.contains_key(declaration) {
        qualify_local_type(declaration, fallback_name, local_types, namespace)
    } else {
        qualify_routed_type(fallback_name, routed_types, namespace)
    }
}

fn qualify_routed_type(
    fallback_name: &str,
    routed_types: &BTreeMap<String, String>,
    namespace: Option<&str>,
) -> String {
    qualify_routed_type_as(fallback_name, fallback_name, routed_types, namespace)
}

fn qualify_routed_type_as(
    route_name: &str,
    fallback_name: &str,
    routed_types: &BTreeMap<String, String>,
    namespace: Option<&str>,
) -> String {
    let Some(target_namespace) = routed_types.get(route_name) else {
        return rdl_ident(fallback_name);
    };
    if namespace == Some(target_namespace.as_str()) {
        rdl_ident(fallback_name)
    } else {
        format!(
            "{}::{}",
            target_namespace.replace('.', "::"),
            rdl_ident(fallback_name)
        )
    }
}

fn pointer_run(mut ty: &TypeRef) -> (bool, usize, &TypeRef) {
    let mut mutable = true;
    let mut depth = 0;
    while let TypeRef::Pointer {
        mutable: level_mutable,
        target,
    } = ty
    {
        mutable &= *level_mutable;
        depth += 1;
        ty = target;
    }
    (mutable, depth, ty)
}

fn constant_type_name(
    ty: &TypeRef,
    pointer_interface_aliases: &BTreeMap<String, String>,
    projection: &TypeProjection<'_>,
) -> Option<String> {
    let name = match ty {
        TypeRef::Scalar(Scalar::Bool) => "u32".to_string(),
        TypeRef::Named { name, .. } if pointer_interface_aliases.contains_key(name) => {
            return None;
        }
        TypeRef::Named { name, .. } | TypeRef::Generic { name, .. }
            if projection
                .interface_names
                .contains(&(projection.tu.to_string(), name.clone())) =>
        {
            return None;
        }
        TypeRef::Void
        | TypeRef::Object
        | TypeRef::Generic { .. }
        | TypeRef::Array { .. }
        | TypeRef::InlineRecord(_) => return None,
        TypeRef::Pointer { .. } => {
            let (mutable, depth, target) = pointer_run(ty);
            projection.pointer(target, mutable, depth).name
        }
        TypeRef::Reference { mutable, target } => projection.pointer(target, *mutable, 1).name,
        _ => projection.name(ty),
    };
    Some(name)
}

fn value_name(value: &Value) -> String {
    match value {
        Value::F32(value) => float_name(f32::from_bits(*value) as f64),
        Value::F64(value) => float_name(f64::from_bits(*value)),
        Value::Signed(value) => value.to_string(),
        Value::Unsigned(value) => value.to_string(),
        Value::Utf8(value) | Value::Utf16(value) => format!("{value:?}"),
    }
}

fn float_name(value: f64) -> String {
    let value = value.to_string();
    if value.contains(['.', 'e', 'E']) {
        value
    } else {
        format!("{value}.0")
    }
}

fn rdl_ident(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do",
        "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in",
        "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
        "return", "static", "struct", "trait", "true", "try", "type", "typeof", "union", "unsafe",
        "unsized", "use", "virtual", "where", "while", "yield",
    ];
    if name == "_" {
        "__".to_string()
    } else if ["crate", "self", "Self", "super"].contains(&name) {
        format!("{name}_")
    } else if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

fn normalize_name(name: &str) -> String {
    name.replace('\\', "/")
}

fn source_path_matches(configured: &str, extracted: &str) -> bool {
    let configured = normalize_name(configured).to_ascii_lowercase();
    let extracted = normalize_name(extracted).to_ascii_lowercase();
    configured == extracted
        || extracted
            .strip_suffix(&configured)
            .is_some_and(|prefix| prefix.ends_with('/'))
        || configured
            .strip_suffix(&extracted)
            .is_some_and(|prefix| prefix.ends_with('/'))
}

fn format_owner(root: &str, owner: &RootOwner) -> String {
    format!(
        "input={:?}, root={:?}, normalized_root={:?}, partition={:?}, namespace={:?}",
        owner.input,
        root,
        normalize_name(root),
        owner.partition,
        owner.namespace,
    )
}

fn fact_uuid(fact: &Fact) -> Option<&str> {
    match &fact.data {
        FactData::Class { guid } => Some(guid),
        FactData::Interface {
            guid: Some(guid), ..
        } => Some(guid),
        _ => None,
    }
}

fn origin(origin: &Origin) -> String {
    format!("{}#{}", origin.tu, origin.local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sal_constant_probe_isolates_unknown_siblings_and_tu_values() {
        helpers::ensure_libclang();
        let source = r#"
            typedef char* PSTR;
            extern "C" void Mixed(
                __attribute__((annotate("_In_reads_(LIMIT)"))) PSTR valid,
                __attribute__((annotate("_In_reads_(UNKNOWN_COUNT)"))) PSTR unknown,
                __attribute__((annotate("_In_reads_(LIMIT + 2)"))) PSTR sibling);
        "#;
        let snapshot = extract(
            [
                Input::new("first.hpp", format!("#define LIMIT 8\n{source}")),
                Input::new("second.hpp", format!("#define LIMIT 12\n{source}")),
                Input::new("no-count.hpp", "typedef int VALUE;"),
            ],
            &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
        )
        .unwrap();
        for fact in snapshot.facts.iter().filter(|fact| fact.name == "Mixed") {
            for (index, expected) in [
                (0, if fact.origin.tu == "first.hpp" { 8 } else { 12 }),
                (
                    2,
                    if fact.origin.tu == "first.hpp" {
                        10
                    } else {
                        14
                    },
                ),
            ] {
                assert_eq!(
                    snapshot.sal_constant_sizes[&AnnotationTarget::Parameter {
                        declaration: fact.origin.clone(),
                        index,
                    }],
                    Ok(SalSize {
                        bytes: false,
                        value: SalSizeValue::Constant(expected)
                    }),
                );
            }
            let unknown = &snapshot.sal_constant_sizes[&AnnotationTarget::Parameter {
                declaration: fact.origin.clone(),
                index: 1,
            }];
            assert!(
                unknown
                    .as_ref()
                    .unwrap_err()
                    .to_string()
                    .contains("UNKNOWN_COUNT")
            );
            let FactData::Function { params, .. } = &fact.data else {
                panic!("Mixed is not a function");
            };
            assert_eq!(
                params[0].annotation.size.as_ref().unwrap().value,
                SalSizeValue::Expression("LIMIT".to_string())
            );
        }
        let references = BTreeMap::new();
        let excluded_constants = BTreeSet::from(["LIMIT".to_string()]);
        let mut options = EmitOptions::new("Test", &references);
        options.excluded_constants = Some(&excluded_constants);
        options.library = Some("test.dll");
        snapshot.emit_with_options(&options).unwrap();
        assert!(!snapshot.sal_count_diagnostics(&options).unwrap().is_empty());
        options.mutable_string_aliases = true;
        assert!(snapshot.emit_with_options(&options).is_err());
    }

    mod com_pointer;
    mod native_class_layouts;
    mod planner_lookups;
    mod producer_corrections;
    mod ranked_types;
    mod shared_enum_authority_control;

    fn test_fact(
        local: u32,
        parent: Option<u32>,
        kind: FactKind,
        name: &str,
        spelling: Location,
        data: FactData,
    ) -> Fact {
        test_fact_in_tu(
            "tu",
            local,
            parent.map(|local| Origin {
                tu: "tu".to_string(),
                local,
            }),
            kind,
            name,
            spelling,
            data,
        )
    }

    fn test_fact_in_tu(
        tu: &str,
        local: u32,
        parent: Option<Origin>,
        kind: FactKind,
        name: &str,
        spelling: Location,
        data: FactData,
    ) -> Fact {
        Fact {
            origin: Origin {
                tu: tu.to_string(),
                local,
            },
            parent,
            kind,
            name: name.to_string(),
            spelling: spelling.clone(),
            expansion: spelling,
            definition: true,
            main_file: false,
            root: true,
            system: false,
            data,
        }
    }

    #[test]
    fn typedef_bridge_lookups_ignore_unrelated_facts() {
        const PLANNED_ALIASES: usize = 256;
        const UNRELATED_TYPEDEFS: usize = 20_000;
        let declaration = Location {
            file: "common.h".to_string(),
            offset: 32,
        };
        let pointer = FactData::Typedef {
            target: TypeRef::Pointer {
                mutable: true,
                target: Box::new(TypeRef::Void),
            },
        };
        let mut facts = vec![
            test_fact_in_tu(
                "foundation",
                0,
                None,
                FactKind::Typedef,
                "LPVOID",
                declaration.clone(),
                pointer.clone(),
            ),
            test_fact_in_tu(
                "consumer",
                0,
                None,
                FactKind::Typedef,
                "LPVOID",
                declaration.clone(),
                pointer.clone(),
            ),
            test_fact_in_tu(
                "consumer",
                1,
                None,
                FactKind::Typedef,
                "LPVOID",
                declaration.clone(),
                pointer,
            ),
        ];
        for index in 0..PLANNED_ALIASES {
            facts.push(test_fact_in_tu(
                "consumer",
                1000 + index as u32,
                None,
                FactKind::Typedef,
                &format!("ALIAS_{index}"),
                Location {
                    file: "consumer.h".to_string(),
                    offset: index as u32,
                },
                FactData::Typedef {
                    target: TypeRef::Named {
                        name: "LPVOID".to_string(),
                        declaration: declaration.clone(),
                    },
                },
            ));
        }
        for index in 0..UNRELATED_TYPEDEFS {
            facts.push(test_fact_in_tu(
                "noise",
                index as u32,
                None,
                FactKind::Typedef,
                &format!("NOISE_{index}"),
                Location {
                    file: "noise.h".to_string(),
                    offset: index as u32,
                },
                FactData::Typedef {
                    target: TypeRef::Scalar(Scalar::U32),
                },
            ));
        }

        let planned: Vec<_> = facts
            .iter()
            .filter(|fact| {
                (fact.origin.tu == "foundation" && fact.name == "LPVOID")
                    || fact.name.starts_with("ALIAS_")
            })
            .collect();
        let declarations = TypedefDeclarationIndex::new(&facts);
        let facts_by_origin = facts
            .iter()
            .map(|fact| (&fact.origin, fact))
            .collect::<HashMap<_, _>>();
        for fact in planned {
            if let FactData::Typedef {
                target: TypeRef::Named { name, declaration },
            } = &fact.data
            {
                let local = declarations.get(&fact.origin.tu, name, declaration);
                assert_eq!(local.len(), 2);
                assert!(local.iter().all(|candidate| same_typedef_bridge_identity(
                    &facts[0],
                    candidate,
                    &facts_by_origin,
                    &BTreeMap::new(),
                )));
            }
        }

        assert_eq!(
            declarations.metrics(),
            (facts.len(), PLANNED_ALIASES, PLANNED_ALIASES * 2)
        );
    }

    #[test]
    fn typedef_bridge_requires_every_exact_candidate_equivalent() {
        let declaration = Location {
            file: "common.h".to_string(),
            offset: 32,
        };
        let pointer = |target| FactData::Typedef {
            target: TypeRef::Pointer {
                mutable: true,
                target: Box::new(target),
            },
        };
        let facts = vec![
            test_fact_in_tu(
                "foundation",
                0,
                None,
                FactKind::Typedef,
                "LPVOID",
                declaration.clone(),
                pointer(TypeRef::Void),
            ),
            test_fact_in_tu(
                "consumer",
                0,
                None,
                FactKind::Typedef,
                "LPVOID",
                declaration.clone(),
                pointer(TypeRef::Void),
            ),
            test_fact_in_tu(
                "consumer",
                1,
                None,
                FactKind::Typedef,
                "LPVOID",
                declaration.clone(),
                pointer(TypeRef::Scalar(Scalar::U32)),
            ),
            test_fact_in_tu(
                "consumer",
                2,
                None,
                FactKind::Typedef,
                "ALIAS",
                Location {
                    file: "consumer.h".to_string(),
                    offset: 16,
                },
                FactData::Typedef {
                    target: TypeRef::Named {
                        name: "LPVOID".to_string(),
                        declaration,
                    },
                },
            ),
        ];
        let declarations = TypedefDeclarationIndex::new(&facts);
        let facts_by_origin = facts
            .iter()
            .map(|fact| (&fact.origin, fact))
            .collect::<HashMap<_, _>>();
        let local = declarations.get("consumer", "LPVOID", &facts[0].spelling);
        assert!(!local.iter().all(|candidate| same_typedef_bridge_identity(
            &facts[0],
            candidate,
            &facts_by_origin,
            &BTreeMap::new(),
        )));

        assert_eq!(declarations.metrics(), (facts.len(), 1, 2));
    }

    #[test]
    fn shared_pointer_alias_requires_exact_source_location() {
        let left = test_fact_in_tu(
            "left",
            1,
            None,
            FactKind::Typedef,
            "LPVOID",
            Location {
                file: "shared.h".to_string(),
                offset: 32,
            },
            FactData::Typedef {
                target: TypeRef::Pointer {
                    mutable: true,
                    target: Box::new(TypeRef::Void),
                },
            },
        );
        let mut right = left.clone();
        right.origin.tu = "right".to_string();
        for different_location in [false, true] {
            right.spelling.offset = if different_location { 33 } else { 32 };
            let facts = [left.clone(), right.clone()];
            let facts_by_origin = facts
                .iter()
                .map(|fact| (&fact.origin, fact))
                .collect::<HashMap<_, _>>();
            assert_eq!(
                same_typedef_bridge_identity(
                    &facts[0],
                    &facts[1],
                    &facts_by_origin,
                    &BTreeMap::new(),
                ),
                !different_location
            );
        }
    }

    #[test]
    fn typedef_bridge_identity_includes_semantic_annotations() {
        let declaration = Location {
            file: "common.h".to_string(),
            offset: 32,
        };
        let data = FactData::Typedef {
            target: TypeRef::Pointer {
                mutable: true,
                target: Box::new(TypeRef::Void),
            },
        };
        let left = test_fact_in_tu(
            "left",
            1,
            None,
            FactKind::Typedef,
            "LPVOID",
            declaration.clone(),
            data.clone(),
        );
        let right = test_fact_in_tu(
            "right",
            1,
            None,
            FactKind::Typedef,
            "LPVOID",
            declaration,
            data,
        );
        let facts = [&left, &right];
        let facts_by_origin = facts
            .into_iter()
            .map(|fact| (&fact.origin, fact))
            .collect::<HashMap<_, _>>();
        let shared = Rc::new(vec![(
            RouteAnnotationTarget::Declaration,
            vec![Annotation::Const],
        )]);
        let mut annotations = BTreeMap::from([
            (left.origin.clone(), shared.clone()),
            (right.origin.clone(), shared),
        ]);
        assert!(same_typedef_bridge_identity(
            &left,
            &right,
            &facts_by_origin,
            &annotations,
        ));

        annotations.insert(
            right.origin.clone(),
            Rc::new(vec![(
                RouteAnnotationTarget::Declaration,
                vec![Annotation::Optional],
            )]),
        );
        assert!(!same_typedef_bridge_identity(
            &left,
            &right,
            &facts_by_origin,
            &annotations,
        ));
        let facts = [left, right];
        let facts_by_origin = facts
            .iter()
            .map(|fact| (&fact.origin, fact))
            .collect::<HashMap<_, _>>();
        assert!(!same_typedef_bridge_identity(
            &facts[0],
            &facts[1],
            &facts_by_origin,
            &annotations,
        ));
    }

    #[test]
    fn same_location_typedefs_require_equivalent_resolved_targets() {
        let typedef_location = Location {
            file: "dciddi.h".to_string(),
            offset: 9754,
        };
        let first_record_location = Location {
            file: "dciddi.h".to_string(),
            offset: 9000,
        };
        let second_record_location = Location {
            file: "dciddi.h".to_string(),
            offset: 9001,
        };
        let record = |local, location: Location, scalar| {
            test_fact(
                local,
                None,
                FactKind::Struct,
                "_DCIENUMINPUT",
                location,
                FactData::Record {
                    base: None,
                    fields: vec![Field {
                        name: "value".to_string(),
                        ty: TypeRef::Scalar(scalar),
                        offset: 0,
                        align: 4,
                        size: 4,
                        bit_width: None,
                    }],
                    size: 4,
                    align: 4,
                    packing: None,
                    alignment: None,
                    union: false,
                },
            )
        };
        let first_record = record(1, first_record_location.clone(), Scalar::U32);
        let equivalent_record = record(2, second_record_location.clone(), Scalar::U32);
        let different_record = record(3, second_record_location.clone(), Scalar::U64);
        let typedef = |local, parent, declaration| {
            test_fact(
                local,
                Some(parent),
                FactKind::Typedef,
                "DCIENUMINPUT",
                typedef_location.clone(),
                FactData::Typedef {
                    target: TypeRef::Named {
                        name: "_DCIENUMINPUT".to_string(),
                        declaration,
                    },
                },
            )
        };
        let first = typedef(4, 10, first_record_location);
        let equivalent = typedef(5, 11, second_record_location.clone());
        let different = typedef(6, 12, second_record_location);
        let equivalent_targets = vec![&first_record, &equivalent_record];
        let equivalent_index = HashMap::from([("_DCIENUMINPUT", equivalent_targets)]);

        assert!(
            choose_type_root("DCIENUMINPUT", &[&first, &equivalent], &equivalent_index).is_ok()
        );

        let different_targets = vec![&first_record, &different_record];
        let different_index = HashMap::from([("_DCIENUMINPUT", different_targets)]);
        assert!(choose_type_root("DCIENUMINPUT", &[&first, &different], &different_index).is_err());
    }

    #[test]
    fn traversed_provider_equivalence_uses_definition_shape_and_unsigned_width() {
        let location = Location {
            file: "shared.h".to_string(),
            offset: 42,
        };
        let enumeration = |local, value, file: &str| {
            test_fact(
                local,
                None,
                FactKind::Enum,
                "SHARED_ASSOCIATED",
                Location {
                    file: file.to_string(),
                    ..location.clone()
                },
                FactData::Enum {
                    repr: Scalar::U32,
                    variants: vec![Variant {
                        name: "SHARED_VALUE".to_string(),
                        value,
                    }],
                    fixed: true,
                    scoped: false,
                },
            )
        };
        let provider = enumeration(1, 2_147_483_648, "shared.h");
        let equivalent = enumeration(2, -2_147_483_648, "shared.h");
        let different_definition = enumeration(3, 2, "shared.h");
        let different_header = enumeration(4, 1, "other.h");
        let mut different_repr = equivalent.clone();
        let FactData::Enum { repr, .. } = &mut different_repr.data else {
            unreachable!();
        };
        *repr = Scalar::I32;
        let alias_location = Location {
            file: "shared.h".to_string(),
            offset: 84,
        };
        let alias = |local| {
            test_fact(
                local,
                None,
                FactKind::Typedef,
                "PUBLIC_ASSOCIATED",
                alias_location.clone(),
                FactData::Typedef {
                    target: TypeRef::Named {
                        name: "SHARED_ASSOCIATED".to_string(),
                        declaration: location.clone(),
                    },
                },
            )
        };
        let provider_alias = alias(5);
        let equivalent_alias = alias(6);

        assert!(has_equivalent_source_provider(&provider, &[&equivalent]));
        assert!(!has_equivalent_source_provider(
            &provider,
            &[&different_definition]
        ));
        assert!(!has_equivalent_source_provider(
            &provider,
            &[&different_header]
        ));
        assert!(!has_equivalent_source_provider(
            &provider,
            &[&different_repr]
        ));
        assert!(has_equivalent_source_provider(
            &equivalent_alias,
            &[&provider_alias, &provider]
        ));
        assert!(!has_equivalent_source_provider(
            &different_definition,
            &[&provider_alias, &provider]
        ));
    }

    #[test]
    fn constant_type_identity_ignores_named_declaration_location() {
        let first = TypeRef::Named {
            name: "LCID".to_string(),
            declaration: Location {
                file: "ntdef.h".to_string(),
                offset: 3202,
            },
        };
        let second = TypeRef::Named {
            name: "LCID".to_string(),
            declaration: Location {
                file: "winnt.h".to_string(),
                offset: 2381,
            },
        };

        assert!(constant_types_match(&first, &second));
        assert!(!constant_types_match(
            &TypeRef::Scalar(Scalar::U32),
            &TypeRef::Scalar(Scalar::U64)
        ));
    }

    #[test]
    fn recursive_shape_cache_is_context_specific() {
        let declaration = Location {
            file: "recursive.hpp".to_string(),
            offset: 7,
        };
        let first_context = TypeShape(1, 2);
        let second_context = TypeShape(3, 4);
        let first_shape = TypeShape(5, 6);
        let second_shape = TypeShape(7, 8);
        let mut cache = ShapeCache::default();

        cache.insert_recursive("tu", &declaration, first_context, first_shape);
        cache.insert_recursive("tu", &declaration, second_context, second_shape);

        assert!(cache.is_recursive("tu", &declaration));
        assert_eq!(
            cache.recursive("tu", &declaration, first_context),
            Some(first_shape)
        );
        assert_eq!(
            cache.recursive("tu", &declaration, second_context),
            Some(second_shape)
        );
        assert_eq!(
            cache.recursive("other-tu", &declaration, first_context),
            None
        );
    }

    #[test]
    fn canonical_string_aliases_share_one_mapping() {
        for (alias, canonical) in [
            ("PCSTR", "PCSTR"),
            ("LPCSTR", "PCSTR"),
            ("PSTR", "PSTR"),
            ("LPSTR", "PSTR"),
            ("PCWSTR", "PCWSTR"),
            ("LPCWSTR", "PCWSTR"),
            ("PWSTR", "PWSTR"),
            ("LPWSTR", "PWSTR"),
        ] {
            assert_eq!(canonical_string_name(alias), Some(canonical));
            assert_eq!(canonical_named_type(alias), Some(canonical));
        }
    }

    #[test]
    fn canonical_floating_aliases_use_rdl_primitives() {
        assert_eq!(canonical_named_type("FLOAT"), Some("f32"));
        assert_eq!(canonical_named_type("DOUBLE"), Some("f64"));
    }
}
