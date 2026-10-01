#![allow(non_upper_case_globals)]
#![doc = include_str!("../readme.md")]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::{Display, Formatter};

fn timings_enabled() -> bool {
    ["WINDOWS_CLANG_TIMINGS", "WINDOWS_CLANG_TIMING"]
        .into_iter()
        .filter_map(std::env::var_os)
        .any(|value| value != "0")
}

fn declaration_uuid<'a>(
    snapshot: &'a Snapshot,
    name: &str,
    tu: &str,
    declaration: &Location,
) -> Option<&'a str> {
    let guids: BTreeSet<_> = snapshot
        .facts
        .iter()
        .filter(|fact| fact.name == name && fact.origin.tu == tu && fact.spelling == *declaration)
        .filter_map(fact_uuid)
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

mod extract;
pub use extract::{extract, extract_partitioned};

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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RdlPartition {
    pub partition: String,
    pub namespace: String,
    pub header: String,
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

pub struct EmitOptions<'a> {
    pub namespace: &'a str,
    pub library: Option<&'a str>,
    pub libraries: Option<&'a BTreeMap<String, String>>,
    pub references: &'a BTreeMap<String, TypeReference>,
    pub excluded: Option<&'a BTreeSet<String>>,
    pub excluded_types: Option<&'a BTreeSet<String>>,
    pub excluded_functions: Option<&'a BTreeSet<String>>,
    pub excluded_constants: Option<&'a BTreeSet<String>>,
    pub functions: Option<&'a BTreeSet<String>>,
}

impl<'a> EmitOptions<'a> {
    pub fn new(namespace: &'a str, references: &'a BTreeMap<String, TypeReference>) -> Self {
        Self {
            namespace,
            library: None,
            libraries: None,
            references,
            excluded: None,
            excluded_types: None,
            excluded_functions: None,
            excluded_constants: None,
            functions: None,
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

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct ParamAnnotation {
    pub input: bool,
    pub output: bool,
    pub optional: bool,
    pub reserved: bool,
    pub com_out_ptr: bool,
    pub retval: bool,
    pub null_terminated: bool,
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

#[derive(Clone, Debug)]
pub struct Snapshot {
    facts: Vec<Fact>,
    constants: Vec<Constant>,
    annotations: BTreeMap<AnnotationTarget, Vec<Annotation>>,
    root_owners: BTreeMap<Origin, RootOwner>,
    root_partitions: BTreeMap<(String, String), RootOwner>,
    partition_exclusions: Vec<ExcludedPartitionDeclaration>,
    forced_flags: BTreeSet<Origin>,
    suppressed_type_origins: BTreeSet<Origin>,
    namespace_authorities: BTreeMap<String, String>,
    fact_namespace_authorities: BTreeMap<Origin, String>,
    constant_namespace_authorities: BTreeMap<Origin, String>,
    timing_target: Option<String>,
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
            && self.annotations == other.annotations
            && self.root_owners == other.root_owners
            && self.root_partitions == other.root_partitions
            && self.partition_exclusions == other.partition_exclusions
            && self.forced_flags == other.forced_flags
            && self.suppressed_type_origins == other.suppressed_type_origins
            && self.namespace_authorities == other.namespace_authorities
            && self.fact_namespace_authorities == other.fact_namespace_authorities
            && self.constant_namespace_authorities == other.constant_namespace_authorities
    }
}

impl Eq for Snapshot {}

impl Snapshot {
    pub fn facts(&self) -> &[Fact] {
        &self.facts
    }

    pub fn constants(&self) -> &[Constant] {
        &self.constants
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
        let result = write_rdl(
            options.namespace,
            items.values().map(|(_, item)| item.as_str()),
        )?;
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
                    .map(|namespace| (constant.definition.clone(), namespace.clone()))
            })
            .collect();
        let timing = self.timing_target.is_some();
        let target = self.timing_target.clone();
        let (snapshot, display_names) = self.into_partitioned_planning_snapshot();
        let plan = snapshot.plan(
            options.references,
            options.excluded_types.or(options.excluded),
            options.excluded_functions.or(options.excluded),
            options.excluded_constants.or(options.excluded),
            options.functions,
            timing,
        )?;
        let routes = snapshot.partition_routes(&plan)?;
        let format_time = timing.then(std::time::Instant::now);
        let items = snapshot.format_items(plan, options, Some(&routes), Some(&display_names))?;
        let mut partitions: BTreeMap<RdlPartition, Vec<String>> = BTreeMap::new();
        for (key, (_, item)) in items {
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
        let result: BTreeMap<RdlPartition, String> = partitions
            .into_iter()
            .map(|(partition, items)| {
                Ok((
                    partition.clone(),
                    write_rdl_strict(&partition.namespace, items.iter().map(String::as_str))?,
                ))
            })
            .collect::<Result<_, Error>>()?;
        if timing {
            eprintln!(
                "windows-clang timing phase=rdl-format target={} mode=partitioned partitions={} bytes={} elapsed_ms={:.3}",
                target.as_deref().unwrap(),
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
        let plan = self.plan(
            options.references,
            options.excluded_types.or(options.excluded),
            options.excluded_functions.or(options.excluded),
            options.excluded_constants.or(options.excluded),
            options.functions,
            timing,
        )?;
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

    fn format_items(
        &self,
        mut plan: Plan<'_>,
        options: &EmitOptions<'_>,
        routes: Option<&BTreeMap<(String, OutputKind), RootOwner>>,
        display_names: Option<&BTreeMap<String, String>>,
    ) -> Result<BTreeMap<(String, OutputKind), (String, String)>, Error> {
        let timing = self.timing_target.is_some();
        let target = self.timing_target.as_deref().unwrap_or("default");
        let emission_time = timing.then(std::time::Instant::now);
        let mut local_types = BTreeMap::new();
        if let Some(routes) = routes {
            let type_namespaces: BTreeMap<_, _> = plan
                .types
                .iter()
                .map(|planned| {
                    (
                        planned.name.as_str(),
                        routes[&(planned.name.clone(), OutputKind::Type)]
                            .namespace
                            .as_str(),
                    )
                })
                .collect();
            for fact in self.facts.iter().filter(|fact| is_type_fact(fact)) {
                let emitted_name = plan
                    .type_names
                    .get(&fact.name)
                    .map_or(fact.name.as_str(), String::as_str);
                let Some(namespace) = type_namespaces.get(emitted_name) else {
                    continue;
                };
                if let Some(previous) =
                    local_types.insert(fact.spelling.clone(), (*namespace).to_string())
                    && previous != *namespace
                {
                    return Err(Error(format!(
                        "local declaration `{}` has conflicting namespaces `{previous}` and `{namespace}`",
                        fact.name
                    )));
                }
            }
            let mut uuid_namespaces: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
            for planned in &plan.types {
                let Some(guid) = fact_uuid(planned.fact) else {
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
                let Some(guid) = fact_uuid(fact) else {
                    continue;
                };
                let Some(namespaces) = uuid_namespaces.get(&(fact.name.as_str(), guid)) else {
                    continue;
                };
                if namespaces.len() == 1 {
                    local_types.insert(
                        fact.spelling.clone(),
                        (*namespaces.first().unwrap()).to_string(),
                    );
                }
            }
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
            let fact = planned.fact;
            let output_name = display_names
                .and_then(|names| names.get(&planned.name))
                .unwrap_or(&planned.name);
            let namespace = routes
                .and_then(|routes| routes.get(&(planned.name.clone(), kind)))
                .map(|owner| owner.namespace.as_str());
            let item = match &fact.data {
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
                    format!(
                        "    #[guid({})]\n    const {}: {} = {pid};\n",
                        rdl_uuid(guid),
                        rdl_ident(output_name),
                        rdl_ident(ty)
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
                        planned_emitted_type_name(
                            target,
                            &plan.type_names,
                            &plan.interface_names,
                            &fact.origin.tu,
                            &local_types,
                            namespace,
                        )
                    )
                }
                FactData::Enum {
                    repr,
                    variants,
                    scoped,
                    ..
                } => {
                    let flags = plan
                        .flag_enums
                        .contains(&(fact.origin.tu.clone(), planned.name.clone()));
                    let repr = if flags { unsigned_scalar(*repr) } else { *repr };
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
        for function in plan.functions {
            let output_name = display_names
                .and_then(|names| names.get(&function.name))
                .unwrap_or(&function.name);
            let FactData::Function {
                link_name,
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
                .and_then(|routes| routes.get(&(function.name.clone(), OutputKind::Value)))
                .map(|owner| owner.namespace.as_str());
            let projection = TypeProjection::new(
                &plan.type_names,
                &plan.interface_names,
                &function.origin.tu,
                &local_types,
                namespace,
            );
            let callable = CallableTarget::Function(&function.origin);
            let mut params = write_params(params, &projection, &self.annotations, callable)?;
            if *variadic {
                params.push("...".to_string());
            }
            let params = params.join(", ");
            let result = if *result == TypeRef::Void {
                String::new()
            } else {
                format!(
                    " -> {}{}",
                    annotation_inline(annotations_for(
                        &self.annotations,
                        &AnnotationTarget::Return(function.origin.clone()),
                    ),)?,
                    planned_emitted_type_name(
                        result,
                        &plan.type_names,
                        &plan.interface_names,
                        &function.origin.tu,
                        &local_types,
                        namespace,
                    )
                )
            };
            let declaration_annotations = annotations_for(
                &self.annotations,
                &AnnotationTarget::Declaration(function.origin.clone()),
            );
            let route =
                routes.and_then(|routes| routes.get(&(function.name.clone(), OutputKind::Value)));
            let library = declaration_annotations
                .iter()
                .find_map(|annotation| match annotation {
                    Annotation::ImportLibrary(library) => Some(library.as_str()),
                    _ => None,
                })
                .or_else(|| {
                    route.and_then(|owner| owner.libraries.get(link_name).map(String::as_str))
                })
                .or_else(|| {
                    options
                        .libraries
                        .and_then(|libraries| libraries.get(link_name).map(String::as_str))
                })
                .or(options.library)
                .ok_or_else(|| {
                    Error(format!(
                        "function `{}` requires an import library",
                        function.name
                    ))
                })?;
            let abi = calling_convention(*convention);
            let set_last_error = declaration_annotations.contains(&Annotation::SetLastError);
            let library = if function.name == *link_name {
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
                    "#[library({library:?}, import = {link_name:?}{})]",
                    if set_last_error {
                        ", set_last_error"
                    } else {
                        ""
                    }
                )
            };
            let item = format!(
                "{}{}    {library}\n    extern{abi} fn {}({params}){result};\n",
                annotation_lines(declaration_annotations, "    ")?,
                if *noreturn { "    #[noreturn]\n" } else { "" },
                rdl_ident(output_name),
            );
            if items
                .insert(
                    (function.name.clone(), OutputKind::Value),
                    (function.spelling.file.clone(), item),
                )
                .is_some()
            {
                return Err(Error(format!("duplicate planned name `{}`", function.name)));
            }
        }
        for planned in plan.constants {
            let constant = planned.constant;
            let output_name = display_names
                .and_then(|names| names.get(&constant.name))
                .unwrap_or(&constant.name);
            let namespace = routes
                .and_then(|routes| routes.get(&(constant.name.clone(), OutputKind::Value)))
                .map(|owner| owner.namespace.as_str());
            let ty = if routes.is_some()
                && !matches!(constant.value, Value::Utf8(_) | Value::Utf16(_))
            {
                constant_type_name(
                    &constant.ty,
                    &plan.type_names,
                    &plan.interface_names,
                    &plan.pointer_interface_aliases,
                    &constant.root.tu,
                    &local_types,
                    namespace,
                )
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

    fn into_partitioned_planning_snapshot(mut self) -> (Self, BTreeMap<String, String>) {
        self.apply_partition_type_settings();
        self.apply_partition_exclusions();
        self.apply_partition_remaps();
        let mut variants: BTreeMap<&str, BTreeMap<&str, BTreeSet<&FactData>>> = BTreeMap::new();
        for fact in self.facts.iter().filter(|fact| fact.root) {
            let Some(owner) = self.root_owners.get(&fact.origin) else {
                continue;
            };
            variants
                .entry(&fact.name)
                .or_default()
                .entry(&owner.namespace)
                .or_default()
                .insert(&fact.data);
        }
        let collisions: BTreeSet<_> = variants
            .into_iter()
            .filter_map(|(name, namespaces)| {
                let distinct: BTreeSet<_> = namespaces.values().flatten().copied().collect();
                (namespaces.len() > 1 && distinct.len() > 1).then_some(name.to_string())
            })
            .collect();
        if collisions.is_empty() {
            return (self, BTreeMap::new());
        }

        let mut source_namespaces: BTreeMap<(Location, String), BTreeSet<String>> = BTreeMap::new();
        for fact in self
            .facts
            .iter()
            .filter(|fact| collisions.contains(&fact.name))
        {
            if let Some(owner) = self.root_owners.get(&fact.origin) {
                source_namespaces
                    .entry((fact.spelling.clone(), fact.name.clone()))
                    .or_default()
                    .insert(owner.namespace.clone());
            }
        }
        let fact_namespaces: BTreeMap<_, _> = self
            .facts
            .iter()
            .filter(|fact| collisions.contains(&fact.name))
            .filter_map(|fact| {
                let namespace = self
                    .root_owners
                    .get(&fact.origin)
                    .map(|owner| owner.namespace.clone())
                    .or_else(|| {
                        let namespaces =
                            source_namespaces.get(&(fact.spelling.clone(), fact.name.clone()))?;
                        (namespaces.len() == 1).then(|| namespaces.first().unwrap().clone())
                    })?;
                Some((fact.origin.clone(), namespace))
            })
            .collect();
        let mut scoped_names = BTreeMap::new();
        let mut display_names = BTreeMap::new();
        for (index, (name, namespace)) in self
            .facts
            .iter()
            .filter(|fact| collisions.contains(&fact.name))
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

        let mut declarations = BTreeMap::new();
        let mut tu_declarations: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
        for fact in &self.facts {
            let Some(namespace) = fact_namespaces.get(&fact.origin) else {
                continue;
            };
            let Some(scoped) = scoped_names.get(&(fact.name.clone(), namespace.clone())) else {
                continue;
            };
            declarations.insert(
                (
                    fact.origin.tu.clone(),
                    fact.spelling.clone(),
                    fact.name.clone(),
                ),
                scoped.clone(),
            );
            tu_declarations
                .entry((fact.origin.tu.clone(), fact.name.clone()))
                .or_default()
                .insert(scoped.clone());
        }

        for fact in &mut self.facts {
            let original = fact.name.clone();
            if let Some(namespace) = fact_namespaces.get(&fact.origin)
                && let Some(scoped) = scoped_names.get(&(original.clone(), namespace.clone()))
            {
                fact.name = scoped.clone();
            }
            rename_fact_types(
                &mut fact.data,
                &fact.origin.tu,
                &declarations,
                &tu_declarations,
                &collisions,
            );
        }
        for constant in &mut self.constants {
            let original = constant.name.clone();
            if let Some(owner) = self.root_owners.get(&constant.root)
                && let Some(scoped) = scoped_names.get(&(original, owner.namespace.clone()))
            {
                constant.name = scoped.clone();
            }
            rename_type_ref(
                &mut constant.ty,
                &constant.root.tu,
                &declarations,
                &tu_declarations,
                &collisions,
            );
        }
        (self, display_names)
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
            if let Some(owner) = self.root_owners.get(&constant.root)
                && let Some(target) = owner.remaps.get(&constant.name)
            {
                constant.name = target.clone();
            }
        }
    }

    fn apply_partition_exclusions(&mut self) {
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
                    uuid: fact_uuid(fact).map(str::to_string),
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
            self.root_owners
                .get(&constant.root)
                .or_else(|| {
                    self.root_partitions
                        .get(&(constant.root.tu.clone(), constant.spelling.file.clone()))
                })
                .is_none_or(|owner| !owner.exclusions.contains(&constant.name))
        });
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
                );
            }
        }

        for constant in &mut self.constants {
            if let Some(owner) = self.root_owners.get(&constant.root).or_else(|| {
                self.root_partitions
                    .get(&(constant.root.tu.clone(), constant.spelling.file.clone()))
            }) {
                preserve_auto_function_pointer_level(
                    &mut constant.ty,
                    &owner.preserved_auto_function_pointer_levels,
                );
            }
        }
    }

    fn authority_candidates<'a>(&self, facts: &[&'a Fact]) -> Vec<&'a Fact> {
        let Some(namespace) = facts
            .iter()
            .find_map(|fact| self.fact_authority_namespace(fact))
        else {
            let projected: Vec<_> = facts
                .iter()
                .copied()
                .filter(|fact| !self.type_projection_suppressed(fact, &mut BTreeSet::new()))
                .collect();
            return if projected.is_empty() {
                facts.to_vec()
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
            facts.to_vec()
        } else {
            matching
        }
    }

    fn fact_authority_namespace<'a>(&'a self, fact: &'a Fact) -> Option<&'a String> {
        self.fact_namespace_authorities
            .get(&fact.origin)
            .or_else(|| {
                let FactData::Typedef {
                    target: TypeRef::Named { name, declaration },
                } = &fact.data
                else {
                    return None;
                };
                self.facts
                    .iter()
                    .filter(|target| {
                        target.name == *name
                            && target.origin.tu == fact.origin.tu
                            && target.spelling == *declaration
                    })
                    .find_map(|target| self.fact_namespace_authorities.get(&target.origin))
            })
    }

    fn type_projection_suppressed(
        &self,
        fact: &Fact,
        seen: &mut BTreeSet<(String, Location)>,
    ) -> bool {
        if self.suppressed_type_origins.contains(&fact.origin) {
            return true;
        }
        let FactData::Typedef { target } = &fact.data else {
            return false;
        };
        self.type_ref_projection_suppressed(target, &fact.origin.tu, seen)
    }

    fn type_ref_projection_suppressed(
        &self,
        ty: &TypeRef,
        tu: &str,
        seen: &mut BTreeSet<(String, Location)>,
    ) -> bool {
        match ty {
            TypeRef::Named { name, declaration } => {
                if !seen.insert((tu.to_string(), declaration.clone())) {
                    return false;
                }
                self.facts
                    .iter()
                    .filter(|fact| {
                        fact.name == *name && fact.origin.tu == tu && fact.spelling == *declaration
                    })
                    .any(|fact| self.type_projection_suppressed(fact, seen))
            }
            TypeRef::Pointer { target, .. }
            | TypeRef::Reference { target, .. }
            | TypeRef::Array { target, .. } => {
                self.type_ref_projection_suppressed(target, tu, seen)
            }
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
            let rejection = if !is_type_fact(fact) {
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
                fact_uuid(fact).unwrap_or("none"),
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

    fn partition_routes(
        &self,
        plan: &Plan<'_>,
    ) -> Result<BTreeMap<(String, OutputKind), RootOwner>, Error> {
        let mut fact_owners: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for fact in &self.facts {
            if let Some(owner) = self.root_owners.get(&fact.origin) {
                fact_owners
                    .entry((&fact.name, fact.kind, fact.definition, &fact.data))
                    .or_default()
                    .insert(owner.clone());
            }
        }
        let mut constant_owners: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for constant in &self.constants {
            if let Some(owner) = self.root_owners.get(&constant.root) {
                constant_owners
                    .entry((&constant.name, &constant.ty, &constant.value))
                    .or_default()
                    .insert(owner.clone());
            }
        }
        let mut result = BTreeMap::new();
        for planned in &plan.types {
            let owners = fact_owners
                .get(&(
                    &planned.fact.name,
                    planned.fact.kind,
                    planned.fact.definition,
                    &planned.fact.data,
                ))
                .cloned()
                .unwrap_or_default();
            result.insert(
                (planned.name.clone(), OutputKind::Type),
                self.authoritative_owner(
                    &planned.name,
                    OutputKind::Type,
                    owners,
                    self.fact_authority_namespace(planned.fact)
                        .map(String::as_str),
                )?,
            );
        }
        for planned in &plan.values {
            let owners = fact_owners
                .get(&(
                    &planned.fact.name,
                    planned.fact.kind,
                    planned.fact.definition,
                    &planned.fact.data,
                ))
                .cloned()
                .unwrap_or_default();
            result.insert(
                (planned.name.clone(), OutputKind::Value),
                self.authoritative_owner(
                    &planned.name,
                    OutputKind::Value,
                    owners,
                    self.fact_authority_namespace(planned.fact)
                        .map(String::as_str),
                )?,
            );
        }
        for function in &plan.functions {
            let owners = fact_owners
                .get(&(
                    &function.name,
                    function.kind,
                    function.definition,
                    &function.data,
                ))
                .cloned()
                .unwrap_or_default();
            result.insert(
                (function.name.clone(), OutputKind::Value),
                self.authoritative_owner(
                    &function.name,
                    OutputKind::Value,
                    owners,
                    self.fact_authority_namespace(function).map(String::as_str),
                )?,
            );
        }
        for planned in &plan.constants {
            let constant = planned.constant;
            result.insert(
                (constant.name.clone(), OutputKind::Value),
                self.authoritative_owner(
                    &constant.name,
                    OutputKind::Value,
                    constant_owners
                        .get(&(&constant.name, &constant.ty, &constant.value))
                        .cloned()
                        .unwrap_or_default(),
                    self.constant_namespace_authorities
                        .get(&constant.definition)
                        .map(String::as_str),
                )?,
            );
        }
        Ok(result)
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

    fn plan(
        &self,
        references: &BTreeMap<String, TypeReference>,
        excluded_types: Option<&BTreeSet<String>>,
        excluded_functions: Option<&BTreeSet<String>>,
        excluded_constants: Option<&BTreeSet<String>>,
        selected_functions: Option<&BTreeSet<String>>,
        timing: bool,
    ) -> Result<Plan<'_>, Error> {
        let target = self.timing_target.as_deref().unwrap_or("default");
        let mut phase_time = timing.then(std::time::Instant::now);
        #[derive(Default)]
        struct Roots<'a> {
            types: Vec<&'a Fact>,
            values: Vec<&'a Fact>,
            functions: Vec<&'a Fact>,
            constants: Vec<&'a Constant>,
        }

        let facts_by_origin: HashMap<_, _> =
            self.facts.iter().map(|fact| (&fact.origin, fact)).collect();
        let is_flat_root = |fact| is_flat_declaration(fact, &facts_by_origin, references, true);
        let is_flat_dependency =
            |fact| is_flat_declaration(fact, &facts_by_origin, references, false);
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
            let Some(interface) = fact.name.strip_prefix("IID_") else {
                continue;
            };
            if !interfaces.contains(interface) || declared_interface_guids.contains_key(interface) {
                continue;
            }
            let FactData::Guid { value } = &fact.data else {
                unreachable!()
            };
            if let Some(previous) = interface_guids.insert(interface.to_string(), value.clone())
                && previous != *value
            {
                return Err(Error(format!(
                    "interface `{interface}` has conflicting IID declarations"
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
                let Some(interface) = fact.name.strip_prefix("IID_") else {
                    return true;
                };
                let FactData::Guid { value } = &fact.data else {
                    return true;
                };
                if references
                    .get(interface)
                    .is_some_and(|reference| reference.kind == TypeReferenceKind::Interface)
                {
                    return false;
                }
                if !interfaces.contains(interface) {
                    return true;
                }
                declared_interface_guids
                    .get(interface)
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
                fact.root && matches!(fact.data, FactData::Function { .. }) && is_flat_root(fact)
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
        for fact in self.facts.iter().filter(|fact| is_flat_dependency(fact)) {
            facts_index.entry(&fact.name).or_default().push(fact);
        }
        let facts_by_declaration: BTreeMap<_, _> = self
            .facts
            .iter()
            .map(|fact| ((fact.origin.tu.clone(), fact.spelling.clone()), fact))
            .collect();
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
        let mut constants = vec![];
        let mut root_names = BTreeSet::new();
        let mut shape_cache = ShapeCache::default();

        for (name, roots) in roots {
            if !roots.types.is_empty() && !roots.functions.is_empty() {
                return Err(Error(format!(
                    "type and function roots collide on `{name}`"
                )));
            }
            let value = if roots.values.is_empty()
                || excluded_constants.is_some_and(|excluded| excluded.contains(name))
            {
                None
            } else {
                Some(choose_value_root(name, &roots.values)?)
            };
            let constant = if roots.constants.is_empty() {
                None
            } else {
                Some(choose_constant_root(name, &roots.constants)?)
            };
            let types_alias_value_class =
                types_alias_value_class(name, &roots.types, &roots.values);
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
                    let authority = self.authority_candidates(&roots.types);
                    let root =
                        choose_type_root_cached(name, &authority, &facts_index, &mut shape_cache)?;
                    root_names.insert(name.to_string());
                    type_roots.push(root);
                }
            } else if !roots.functions.is_empty() {
                functions.push(choose_function_root(name, &roots.functions)?);
            }
            if let Some(value) = value {
                value_roots.push(value);
            }
            if let Some(constant) = constant {
                constants.push(constant);
            }
        }
        if let Some(selected) = selected_functions {
            let found: BTreeSet<_> = functions
                .iter()
                .filter_map(|function| match &function.data {
                    FactData::Function { link_name, .. } => Some(link_name.as_str()),
                    _ => None,
                })
                .collect();
            if let Some(missing) = selected.iter().find(|name| !found.contains(name.as_str())) {
                return Err(Error(format!(
                    "selected function `{missing}` was not found"
                )));
            }
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-roots target={target} elapsed_ms={:.3}",
                elapsed_ms(phase_time)
            );
            phase_time = Some(std::time::Instant::now());
        }

        let facts_by_name = loop {
            let mut facts = BTreeSet::new();
            let mut queue = vec![];
            for root in &type_roots {
                if facts.insert(root.origin.clone()) {
                    queue_type_edges(root, &mut queue);
                }
            }
            for root in &value_roots {
                queue_type_edges(root, &mut queue);
            }
            for constant in &constants {
                queue.push((constant.root.tu.as_str(), TypeEdge::Type(&constant.ty)));
            }
            for function in &functions {
                queue_function_edges(function, &mut queue);
            }

            while let Some((tu, edge)) = queue.pop() {
                let ty = match edge {
                    TypeEdge::Type(ty) => ty,
                    TypeEdge::Projected(name) => {
                        if references.contains_key(name) && !root_names.contains(name) {
                            continue;
                        }
                        let matches: Vec<_> = facts_index
                            .get(name)
                            .into_iter()
                            .flatten()
                            .copied()
                            .filter(|fact| fact.origin.tu == tu && is_type_fact(fact))
                            .collect();
                        let fact = choose_type_root_cached(
                            name,
                            &matches,
                            &facts_index,
                            &mut shape_cache,
                        )?;
                        if facts.insert(fact.origin.clone()) {
                            queue_type_edges(fact, &mut queue);
                        }
                        continue;
                    }
                };
                let (name, declaration) = match ty {
                    TypeRef::Pointer { target, .. } | TypeRef::Reference { target, .. } => {
                        queue.push((tu, TypeEdge::Type(target)));
                        continue;
                    }
                    TypeRef::FunctionPointer { .. } | TypeRef::OpaquePointer { .. } => continue,
                    TypeRef::Array { target, .. } => {
                        queue.push((tu, TypeEdge::Type(target)));
                        continue;
                    }
                    TypeRef::InlineRecord(record) => {
                        for field in &record.fields {
                            queue.push((tu, TypeEdge::Type(&field.ty)));
                        }
                        continue;
                    }
                    TypeRef::Named { name, declaration } => (name, declaration),
                    _ => continue,
                };
                if excluded_local_names.contains(&(tu.to_string(), name.clone())) {
                    continue;
                }
                if references.contains_key(name) && !root_names.contains(name) {
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
                let mut fact = match matches.as_slice() {
                    [fact] => *fact,
                    [] => {
                        return Err(self.unresolved_local_type_error(name, tu, declaration));
                    }
                    choices => {
                        choose_type_root_cached(name, choices, &facts_index, &mut shape_cache)?
                    }
                };
                if self.root_owners.get(&fact.origin).is_none()
                    && let Some(guid) = declaration_uuid(self, name, tu, declaration)
                {
                    let owned: Vec<_> = facts_index
                        .get(name.as_str())
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(|candidate| is_type_fact(candidate))
                        .filter(|candidate| fact_uuid(candidate) == Some(guid))
                        .filter(|candidate| self.root_owners.contains_key(&candidate.origin))
                        .collect();
                    let owners: BTreeSet<_> = owned
                        .iter()
                        .filter_map(|candidate| self.root_owners.get(&candidate.origin))
                        .map(|owner| (&owner.partition, &owner.namespace))
                        .collect();
                    if owners.len() > 1 {
                        return Err(Error(format!(
                            "local type `{name}` at {}:{} matches UUID `{guid}` in multiple tagged owners",
                            declaration.file, declaration.offset
                        )));
                    }
                    if !owned.is_empty() {
                        fact =
                            choose_type_root_cached(name, &owned, &facts_index, &mut shape_cache)?;
                    }
                }
                if let FactData::Unsupported { reason } = &fact.data {
                    return Err(Error(format!(
                        "unsupported type `{name}` in translation unit `{tu}`: {reason}"
                    )));
                }
                if !is_type_fact(fact) {
                    return Err(self.unresolved_local_type_error(name, tu, declaration));
                };
                if canonical_named_type(name).is_some()
                    && matches!(fact.data, FactData::Typedef { .. })
                {
                    continue;
                }
                if facts.insert(fact.origin.clone()) {
                    queue_type_edges(fact, &mut queue);
                }
            }

            let mut grouped: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
            for fact in facts.into_iter().map(|origin| facts_by_origin[&origin]) {
                grouped.entry(&fact.name).or_default().push(fact);
            }

            let collisions: Vec<_> = constants
                .iter()
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
                    )?;
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
            for (name, choices) in grouped {
                let authority = self.authority_candidates(&choices);
                let selected =
                    choose_type_root_cached(name, &authority, &facts_index, &mut shape_cache)?;
                facts_by_name.insert(name, selected);
            }
            break facts_by_name;
        };
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
        };
        let validation_time = timing.then(std::time::Instant::now);
        for fact in facts_by_name.values() {
            validate_fact_layouts(fact, &layout, &mut safe_layouts, &mut validated_layouts)?;
        }
        for fact in &value_roots {
            validate_fact_layouts(fact, &layout, &mut safe_layouts, &mut validated_layouts)?;
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-validate-types target={target} elapsed_ms={:.3}",
                elapsed_ms(validation_time)
            );
        }
        let validation_time = timing.then(std::time::Instant::now);
        for function in &functions {
            validate_fact_layouts(function, &layout, &mut safe_layouts, &mut validated_layouts)?;
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=plan-validate-functions target={target} elapsed_ms={:.3}",
                elapsed_ms(validation_time)
            );
        }
        let validation_time = timing.then(std::time::Instant::now);
        for constant in &constants {
            validate_complete_layout(
                &constant.ty,
                &constant.root.tu,
                &layout,
                &mut safe_layouts,
                &mut BTreeSet::new(),
                &mut validated_layouts,
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

        let local_roots = root_names.clone();
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
        let mut types: Vec<_> = facts_by_name
            .into_iter()
            .filter(|(name, _)| required.contains(*name))
            .filter(|(name, _)| !alias_names.contains(*name))
            .map(|(_, fact)| {
                let name = type_names
                    .get(fact.name.as_str())
                    .cloned()
                    .unwrap_or_else(|| fact.name.clone());
                if self.suppressed_type_origins.contains(&fact.origin)
                    && name == fact.name
                    && self.fact_authority_namespace(fact).is_none()
                {
                    let owner = self.root_owners.get(&fact.origin).unwrap();
                    return Err(Error(format!(
                        "owner-excluded local type `{}` in partition `{}` namespace `{}` is \
                         required without a retained public alias",
                        fact.name, owner.partition, owner.namespace
                    )));
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
                target: TypeRef::Pointer { target, .. },
            } = &fact.data
            else {
                continue;
            };
            let (TypeRef::Named { name: target, .. } | TypeRef::Generic { name: target, .. }) =
                target.as_ref()
            else {
                continue;
            };
            if !interface_names.contains(&(fact.origin.tu.clone(), target.clone())) {
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
        let mut constants: Vec<_> = constants
            .into_iter()
            .filter_map(|constant| {
                let ty = match &constant.value {
                    Value::Utf8(_) | Value::Utf16(_) => Some("String".to_string()),
                    _ => constant_type_name(
                        &constant.ty,
                        &type_names,
                        &interface_names,
                        &pointer_interface_aliases,
                        &constant.root.tu,
                        &BTreeMap::new(),
                        None,
                    ),
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
        Ok(Plan {
            types,
            values,
            functions,
            constants,
            type_names,
            interface_names,
            pointer_interface_aliases,
            interface_guids,
            flag_enums,
        })
    }
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

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum OutputKind {
    Value,
    Type,
}

struct Plan<'a> {
    types: Vec<PlannedFact<'a>>,
    values: Vec<PlannedFact<'a>>,
    functions: Vec<&'a Fact>,
    constants: Vec<PlannedConstant<'a>>,
    type_names: BTreeMap<String, String>,
    interface_names: BTreeSet<(String, String)>,
    pointer_interface_aliases: BTreeMap<String, String>,
    interface_guids: BTreeMap<String, String>,
    flag_enums: BTreeSet<(String, String)>,
}

struct TypeProjection<'a> {
    type_names: &'a BTreeMap<String, String>,
    interface_names: &'a BTreeSet<(String, String)>,
    tu: &'a str,
    local_types: &'a BTreeMap<Location, String>,
    namespace: Option<&'a str>,
}

impl<'a> TypeProjection<'a> {
    fn new(
        type_names: &'a BTreeMap<String, String>,
        interface_names: &'a BTreeSet<(String, String)>,
        tu: &'a str,
        local_types: &'a BTreeMap<Location, String>,
        namespace: Option<&'a str>,
    ) -> Self {
        Self {
            type_names,
            interface_names,
            tu,
            local_types,
            namespace,
        }
    }

    fn name(&self, ty: &TypeRef) -> String {
        planned_emitted_type_name(
            ty,
            self.type_names,
            self.interface_names,
            self.tu,
            self.local_types,
            self.namespace,
        )
    }
}

fn scoped_declaration_name<'a>(
    tu: &str,
    declaration: &Location,
    name: &str,
    declarations: &'a BTreeMap<(String, Location, String), String>,
    tu_declarations: &'a BTreeMap<(String, String), BTreeSet<String>>,
) -> Option<&'a String> {
    declarations
        .get(&(tu.to_string(), declaration.clone(), name.to_string()))
        .or_else(|| {
            let names = tu_declarations.get(&(tu.to_string(), name.to_string()))?;
            (names.len() == 1).then(|| names.first().unwrap())
        })
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

fn preserve_auto_function_pointer_levels(data: &mut FactData, names: &BTreeSet<String>) {
    match data {
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            for param in params {
                preserve_auto_function_pointer_level(&mut param.ty, names);
            }
            preserve_auto_function_pointer_level(result, names);
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                preserve_auto_function_pointer_level(base, names);
            }
            for method in methods {
                for param in &mut method.params {
                    preserve_auto_function_pointer_level(&mut param.ty, names);
                }
                preserve_auto_function_pointer_level(&mut method.result, names);
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                preserve_auto_function_pointer_level(base, names);
            }
            for field in fields {
                preserve_auto_function_pointer_level(&mut field.ty, names);
            }
        }
        FactData::Typedef { target } => preserve_auto_function_pointer_level(target, names),
        _ => {}
    }
}

fn preserve_auto_function_pointer_level(ty: &mut TypeRef, names: &BTreeSet<String>) {
    match ty {
        TypeRef::Named { name, .. } if names.contains(name) => {
            *ty = TypeRef::Pointer {
                mutable: true,
                target: Box::new(ty.clone()),
            };
        }
        TypeRef::Generic { args, .. } => {
            for arg in args {
                preserve_auto_function_pointer_level(arg, names);
            }
        }
        TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. }
        | TypeRef::Array { target, .. } => preserve_auto_function_pointer_level(target, names),
        TypeRef::FunctionPointer { params, result, .. } => {
            for param in params {
                preserve_auto_function_pointer_level(param, names);
            }
            preserve_auto_function_pointer_level(result, names);
        }
        TypeRef::InlineRecord(record) => {
            if let Some(base) = &mut record.base {
                preserve_auto_function_pointer_level(base, names);
            }
            for field in &mut record.fields {
                preserve_auto_function_pointer_level(&mut field.ty, names);
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
    declarations: &BTreeMap<(String, Location, String), String>,
    tu_declarations: &BTreeMap<(String, String), BTreeSet<String>>,
    collisions: &BTreeSet<String>,
) {
    match data {
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            for param in params {
                rename_type_ref(&mut param.ty, tu, declarations, tu_declarations, collisions);
            }
            rename_type_ref(result, tu, declarations, tu_declarations, collisions);
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                rename_type_ref(base, tu, declarations, tu_declarations, collisions);
            }
            for method in methods {
                for param in &mut method.params {
                    rename_type_ref(&mut param.ty, tu, declarations, tu_declarations, collisions);
                }
                rename_type_ref(
                    &mut method.result,
                    tu,
                    declarations,
                    tu_declarations,
                    collisions,
                );
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                rename_type_ref(base, tu, declarations, tu_declarations, collisions);
            }
            for field in fields {
                rename_type_ref(&mut field.ty, tu, declarations, tu_declarations, collisions);
            }
        }
        FactData::Typedef { target } => {
            rename_type_ref(target, tu, declarations, tu_declarations, collisions)
        }
        _ => {}
    }
}

fn rename_type_ref(
    ty: &mut TypeRef,
    tu: &str,
    declarations: &BTreeMap<(String, Location, String), String>,
    tu_declarations: &BTreeMap<(String, String), BTreeSet<String>>,
    collisions: &BTreeSet<String>,
) {
    match ty {
        TypeRef::Named { name, declaration } => {
            if !collisions.contains(name) {
                return;
            }
            if let Some(scoped) =
                scoped_declaration_name(tu, declaration, name, declarations, tu_declarations)
            {
                *name = scoped.clone();
            }
        }
        TypeRef::Generic {
            name,
            declaration,
            args,
        } => {
            if collisions.contains(name) {
                if let Some(scoped) =
                    scoped_declaration_name(tu, declaration, name, declarations, tu_declarations)
                {
                    *name = scoped.clone();
                }
            }
            for arg in args {
                rename_type_ref(arg, tu, declarations, tu_declarations, collisions);
            }
        }
        TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. }
        | TypeRef::Array { target, .. } => {
            rename_type_ref(target, tu, declarations, tu_declarations, collisions)
        }
        TypeRef::FunctionPointer { params, result, .. } => {
            for param in params {
                rename_type_ref(param, tu, declarations, tu_declarations, collisions);
            }
            rename_type_ref(result, tu, declarations, tu_declarations, collisions);
        }
        TypeRef::InlineRecord(record) => {
            if let Some(base) = &mut record.base {
                rename_type_ref(base, tu, declarations, tu_declarations, collisions);
            }
            for field in &mut record.fields {
                rename_type_ref(&mut field.ty, tu, declarations, tu_declarations, collisions);
            }
        }
        _ => {}
    }
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

fn fact_data_kind(data: &FactData) -> &'static str {
    match data {
        FactData::Callback { .. } => "Callback",
        FactData::Class { .. } => "Class",
        FactData::Enum { .. } => "Enum",
        FactData::EnumFlag { .. } => "EnumFlag",
        FactData::Function { .. } => "Function",
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

fn types_alias_value_class(name: &str, types: &[&Fact], values: &[&Fact]) -> bool {
    types.iter().all(|fact| {
        values
            .iter()
            .copied()
            .filter(|value| matches!(value.data, FactData::Class { .. }))
            .any(|value| {
                fact.origin == value.origin
                    || (fact.origin.tu == value.origin.tu
                        && fact.parent == value.parent
                        && matches!(
                            &fact.data,
                            FactData::Typedef {
                                target: TypeRef::Named { name: target, .. }
                            } if target == name
                        ))
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
    if let Some(target_name) = distinct.first().and_then(|first| match &first.data {
        FactData::Typedef {
            target: TypeRef::Named { name, .. },
        } => Some(name),
        _ => None,
    }) && distinct.iter().all(|fact| {
        matches!(
            &fact.data,
            FactData::Typedef {
                target: TypeRef::Named { name, .. }
            } if name == target_name
        )
    }) {
        let complete: Vec<_> = distinct
            .iter()
            .copied()
            .filter(|fact| {
                let FactData::Typedef {
                    target: TypeRef::Named { name, declaration },
                } = &fact.data
                else {
                    return false;
                };
                facts_index
                    .get(name.as_str())
                    .into_iter()
                    .flatten()
                    .any(|target| {
                        target.origin.tu == fact.origin.tu
                            && target.spelling == *declaration
                            && target.definition
                            && matches!(
                                target.data,
                                FactData::Enum { .. }
                                    | FactData::Record { .. }
                                    | FactData::Interface { .. }
                            )
                    })
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
    if let Some(first) = distinct.first()
        && let FactData::Typedef {
            target: first_target,
        } = &first.data
        && distinct.iter().all(|fact| {
            ((fact.origin.tu == first.origin.tu && fact.parent == first.parent)
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
                || matches!(
                    &fact.data,
                    FactData::Typedef {
                        target: TypeRef::Named { declaration, .. }
                    } if declarations.iter().any(|target| {
                        target.origin.tu == fact.origin.tu && target.spelling == *declaration
                    })
                )
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
            let FactData::Typedef {
                target: TypeRef::Named { name, declaration },
            } = &fact.data
            else {
                return false;
            };
            facts_index
                .get(name.as_str())
                .into_iter()
                .flatten()
                .any(|target| {
                    target.origin.tu == fact.origin.tu
                        && target.spelling == *declaration
                        && fact_uuid(target) == Some(guid)
                        && target.kind == root.kind
                        && matches!(
                            target.data,
                            FactData::Class { .. } | FactData::Interface { .. }
                        )
                })
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
                || matches!(
                    &fact.data,
                    FactData::Typedef {
                        target: TypeRef::Named { declaration, .. }
                    } if definitions.iter().any(|definition| definition.spelling == *declaration)
                )
        });
        if equivalent_definitions && definitions_and_aliases {
            return Ok(root);
        }
    }
    if let [root] = definitions.as_slice() {
        let aliases_target_root = distinct.iter().all(|fact| {
            fact.origin == root.origin
                || matches!(
                    &fact.data,
                    FactData::Typedef {
                        target: TypeRef::Named { declaration, .. }
                    } if declaration == &root.spelling
                )
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
                || matches!(
                    &fact.data,
                    FactData::Typedef {
                        target: TypeRef::Named { declaration, .. }
                    } if facts_index
                        .get(name)
                        .into_iter()
                        .flatten()
                        .any(|target| {
                            target.origin.tu == fact.origin.tu
                                && target.spelling == *declaration
                                && target.kind == root.kind
                                && (target.origin == root.origin || !target.definition)
                                && target.parent.is_none()
                                && root.parent.is_none()
                        })
                )
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

fn choose_constant_root<'a>(name: &str, roots: &[&'a Constant]) -> Result<&'a Constant, Error> {
    let Some(first) = roots.first() else {
        return Err(Error(format!("missing constant root `{name}`")));
    };
    if roots.iter().all(|constant| {
        constant_types_match(&constant.ty, &first.ty) && constant.value == first.value
    }) {
        Ok(roots
            .iter()
            .min_by_key(|constant| &constant.spelling)
            .copied()
            .unwrap())
    } else {
        Err(Error(format!("ambiguous constant root `{name}`")))
    }
}

fn constant_types_match(left: &TypeRef, right: &TypeRef) -> bool {
    left == right
        || matches!(
            (left, right),
            (
                TypeRef::Named {
                    name: left_name, ..
                },
                TypeRef::Named {
                    name: right_name, ..
                },
            ) if named_type_shape(left_name) == named_type_shape(right_name)
                && named_type_shape(left_name).is_some()
        )
}

fn choose_function_root<'a>(name: &str, roots: &[&'a Fact]) -> Result<&'a Fact, Error> {
    let distinct = distinct_source_declarations(roots);
    if let [root] = distinct.as_slice() {
        return Ok(root);
    }
    if let Some(first) = distinct.first()
        && let FactData::Function {
            link_name: first_link_name,
            ..
        } = &first.data
        && distinct.iter().all(|fact| {
            fact.origin.tu == first.origin.tu
                && fact.parent == first.parent
                && matches!(
                    &fact.data,
                    FactData::Function { link_name, .. } if link_name == first_link_name
                )
        })
    {
        return Ok(distinct
            .iter()
            .min_by_key(|fact| &fact.spelling)
            .copied()
            .unwrap());
    }
    let choices = distinct
        .iter()
        .map(|fact| {
            format!(
                "{}:{} {:?}",
                fact.spelling.file, fact.spelling.offset, fact.data
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Err(Error(format!(
        "ambiguous function root `{name}`: {choices}"
    )))
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

enum TypeEdge<'a> {
    Type(&'a TypeRef),
    Projected(&'static str),
}

fn queue_type_edges<'a>(fact: &'a Fact, queue: &mut Vec<(&'a str, TypeEdge<'a>)>) {
    if let FactData::Typedef { target } = &fact.data {
        queue.push((fact.origin.tu.as_str(), TypeEdge::Type(target)));
    } else if let FactData::Callback { params, result, .. } = &fact.data {
        queue.push((fact.origin.tu.as_str(), TypeEdge::Type(result)));
        for param in params {
            queue.push((fact.origin.tu.as_str(), TypeEdge::Type(&param.ty)));
        }
    } else if let FactData::PropertyKey { ty, .. } = &fact.data {
        queue.push((fact.origin.tu.as_str(), TypeEdge::Projected(ty)));
    } else if let FactData::Record { base, fields, .. } = &fact.data {
        if let Some(base) = base {
            queue.push((fact.origin.tu.as_str(), TypeEdge::Type(base)));
        }
        for field in fields {
            queue.push((fact.origin.tu.as_str(), TypeEdge::Type(&field.ty)));
        }
    } else if let FactData::Interface { base, methods, .. } = &fact.data {
        if let Some(base) = base {
            queue.push((fact.origin.tu.as_str(), TypeEdge::Type(base)));
        }
        for method in methods {
            queue.push((fact.origin.tu.as_str(), TypeEdge::Type(&method.result)));
            for param in &method.params {
                queue.push((fact.origin.tu.as_str(), TypeEdge::Type(&param.ty)));
                if let Some(name) = parameter_string_name(param) {
                    queue.push((fact.origin.tu.as_str(), TypeEdge::Projected(name)));
                }
            }
        }
    }
}

fn queue_function_edges<'a>(fact: &'a Fact, queue: &mut Vec<(&'a str, TypeEdge<'a>)>) {
    if let FactData::Function { params, result, .. } = &fact.data {
        queue.push((fact.origin.tu.as_str(), TypeEdge::Type(result)));
        for param in params {
            queue.push((fact.origin.tu.as_str(), TypeEdge::Type(&param.ty)));
            if let Some(name) = parameter_string_name(param) {
                queue.push((fact.origin.tu.as_str(), TypeEdge::Projected(name)));
            }
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
        format!(
            " -> {}{}",
            annotation_inline(annotations_for(
                annotations,
                &AnnotationTarget::Return(origin.clone()),
            ))?,
            projection.name(result)
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
            Ok(format!(
                "{}{}: {}",
                param_attributes(
                    param,
                    params,
                    emitted_pointer_is_mutable(param, projection.interface_names, projection.tu),
                    annotations_for(annotations, &target),
                )?,
                rdl_ident(&param.name),
                planned_param_type_name(
                    param,
                    projection.type_names,
                    projection.interface_names,
                    projection.tu,
                    projection.local_types,
                    projection.namespace,
                )
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
            SalSizeValue::Constant(_) => {
                return Err(Error(
                    "constant byte-size SAL annotations are unsupported".to_string(),
                ));
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
    if annotation.retval {
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
                format!(
                    " -> {}{}",
                    annotation_inline(annotations_for(
                        annotations,
                        &AnnotationTarget::MethodReturn {
                            declaration: origin.clone(),
                            index: method_index,
                        },
                    ))?,
                    projection.name(&method.result)
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

fn planned_param_type_name(
    param: &Parameter,
    type_names: &BTreeMap<String, String>,
    interface_names: &BTreeSet<(String, String)>,
    tu: &str,
    local_types: &BTreeMap<Location, String>,
    namespace: Option<&str>,
) -> String {
    if param.annotation.com_out_ptr {
        return "*mut *mut void".to_string();
    }
    if let Some(name) = parameter_string_name(param) {
        return type_names
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string());
    }
    planned_emitted_type_name(
        &param.ty,
        type_names,
        interface_names,
        tu,
        local_types,
        namespace,
    )
}

fn emitted_pointer_is_mutable(
    param: &Parameter,
    interface_names: &BTreeSet<(String, String)>,
    tu: &str,
) -> bool {
    if param.annotation.com_out_ptr {
        return true;
    }
    if parameter_string_name(param).is_some() {
        return false;
    }
    match &param.ty {
        TypeRef::Pointer { .. } => {
            let (mutable, depth, target) = pointer_run(&param.ty);
            if depth == 1
                && matches!(
                    target,
                    TypeRef::Named { name, .. } | TypeRef::Generic { name, .. }
                        if interface_names.contains(&(tu.to_string(), name.clone()))
                )
            {
                false
            } else {
                mutable
            }
        }
        TypeRef::Reference { mutable, target } => {
            !matches!(
                target.as_ref(),
                TypeRef::Named { name, .. } | TypeRef::Generic { name, .. }
                    if interface_names.contains(&(tu.to_string(), name.clone()))
            ) && *mutable
        }
        TypeRef::FunctionPointer { .. } => true,
        TypeRef::OpaquePointer { mutable, .. } => *mutable,
        TypeRef::Named { name, .. } if matches!(name.as_str(), "PVOID" | "LPVOID") => true,
        _ => false,
    }
}

fn parameter_string_name(param: &Parameter) -> Option<&'static str> {
    match &param.ty {
        TypeRef::Pointer { mutable, target } if param.annotation.null_terminated => {
            match (mutable, target.as_ref()) {
                (false, TypeRef::Scalar(Scalar::I8 | Scalar::U8)) => Some("PCSTR"),
                (true, TypeRef::Scalar(Scalar::I8 | Scalar::U8)) => Some("PSTR"),
                (false, TypeRef::Scalar(Scalar::U16)) => Some("PCWSTR"),
                (true, TypeRef::Scalar(Scalar::U16)) => Some("PWSTR"),
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

fn planned_emitted_type_name(
    ty: &TypeRef,
    type_names: &BTreeMap<String, String>,
    interface_names: &BTreeSet<(String, String)>,
    tu: &str,
    local_types: &BTreeMap<Location, String>,
    namespace: Option<&str>,
) -> String {
    if matches!(ty, TypeRef::FunctionPointer { .. }) {
        return "*mut u8".to_string();
    }
    if let TypeRef::OpaquePointer { mutable, .. } = ty {
        return format!("*{} void", if *mutable { "mut" } else { "const" });
    }
    if let TypeRef::Named { name, declaration } = ty
        && let Some(name) = type_names.get(name)
    {
        return qualify_local_type(declaration, name, local_types, namespace);
    }
    if let TypeRef::Named { name, .. } = ty
        && let Some(name) = canonical_named_type(name)
    {
        return type_names
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string());
    }
    if let TypeRef::Reference { mutable, target } = ty {
        if let TypeRef::Named { name, .. } | TypeRef::Generic { name, .. } = target.as_ref()
            && interface_names.contains(&(tu.to_string(), name.clone()))
        {
            return planned_type_name(target, type_names, local_types, namespace);
        }
        return format!(
            "*{} {}",
            if *mutable { "mut" } else { "const" },
            planned_emitted_type_name(
                target,
                type_names,
                interface_names,
                tu,
                local_types,
                namespace,
            )
        );
    }
    if let TypeRef::Array { target, len } = ty {
        return format!(
            "[{}; {len}]",
            planned_emitted_type_name(
                target,
                type_names,
                interface_names,
                tu,
                local_types,
                namespace,
            )
        );
    }
    let (mutable, depth, target) = pointer_run(ty);
    if depth != 0
        && let TypeRef::Named { name, .. } | TypeRef::Generic { name, .. } = target
        && interface_names.contains(&(tu.to_string(), name.clone()))
    {
        return format!(
            "{}{}",
            format!("*{} ", if mutable { "mut" } else { "const" }).repeat(depth - 1),
            planned_emitted_type_name(
                target,
                type_names,
                interface_names,
                tu,
                local_types,
                namespace,
            )
        );
    }
    if depth != 0 {
        return format!(
            "{}{}",
            format!("*{} ", if mutable { "mut" } else { "const" }).repeat(depth),
            planned_emitted_type_name(
                target,
                type_names,
                interface_names,
                tu,
                local_types,
                namespace,
            )
        );
    }
    planned_type_name(ty, type_names, local_types, namespace)
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
        let attributes = if let Some((annotations, origin, path)) = &field_annotations {
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
            annotation_inline(annotations_for(annotations, &target))?
        } else {
            String::new()
        };
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
                projection.name(&field.ty)
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
            projection.name(&field.ty)
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
    type_names: &BTreeMap<String, String>,
    interface_names: &BTreeSet<(String, String)>,
    pointer_interface_aliases: &BTreeMap<String, String>,
    tu: &str,
    local_types: &BTreeMap<Location, String>,
    namespace: Option<&str>,
) -> Option<String> {
    let name = match ty {
        TypeRef::Scalar(Scalar::Bool) => "u32".to_string(),
        TypeRef::Named { name, .. } if pointer_interface_aliases.contains_key(name) => {
            return None;
        }
        TypeRef::Named { name, .. } | TypeRef::Generic { name, .. }
            if interface_names.contains(&(tu.to_string(), name.clone())) =>
        {
            return None;
        }
        TypeRef::Named { name, .. } if type_names.contains_key(name) => {
            planned_type_name(ty, type_names, local_types, namespace)
        }
        TypeRef::Named { name, .. } => canonical_named_type(name).map_or_else(
            || planned_type_name(ty, type_names, local_types, namespace),
            str::to_string,
        ),
        TypeRef::Void
        | TypeRef::Object
        | TypeRef::Generic { .. }
        | TypeRef::Array { .. }
        | TypeRef::InlineRecord(_) => return None,
        _ => planned_type_name(ty, type_names, local_types, namespace),
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
