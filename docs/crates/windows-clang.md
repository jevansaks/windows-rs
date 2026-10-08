# windows-clang

> Generates RDL from C/C++ headers using libclang.

- [crates.io](https://crates.io/crates/windows-clang)
- [docs.rs](https://docs.rs/windows-clang)
- [Getting started](../../crates/libs/clang/readme.md)
- [Source](https://github.com/microsoft/windows-rs/tree/master/crates/libs/clang)

`windows-clang` is the header-facing stage of the Windows metadata pipeline:

```text
headers -> windows-clang -> RDL -> windows-rdl -> WinMD
```

It extracts declarations and source annotations into immutable facts, then plans and emits RDL
from the completed fact graph. It does not generate Rust bindings or provision libclang.

## High-level generation

`clang()` returns a `Clang` builder for the common case of extracting one set of headers and
writing RDL directly:

```rust,no_run
windows_clang::clang()
    .input("Example.h")
    .args(["-x", "c++", "--target=x86_64-pc-windows-msvc"])
    .reference_default()
    .namespace("Example")
    .library("example.dll")
    .output("Example.rdl")
    .write()
    .unwrap();
```

The builder is an adapter over the extraction and emission APIs described below. `input` and
`input_text` construct `Input` values, `filter` selects included headers by path suffix,
`reference_default` supplies the Windows metadata references, and `write` calls `extract` and
emits with `EmitOptions`. It does not have a separate parser or projection path.

`parallelism` bounds concurrent parsing of original translation units. Zero and one select serial
parsing. `exclude_path` omits declarations spelled beneath a directory, such as Clang's resource
headers, without hiding that the files were visited.

Use the lower-level API when a generator must inspect facts, compare architectures, merge
snapshots, assign libraries per function, or control output promotion.

## Extraction

Each `Input` contains:

| Field | Purpose |
| --- | --- |
| `name` | Translation-unit name used for diagnostics and default ownership. |
| `source` | C or C++ source passed to libclang. |
| `roots` | Header paths whose declarations may become output roots. |
| `root_dirs` | Directory prefixes whose declarations may become output roots. |
| `root_suffixes` | Header path suffixes whose declarations may become output roots. |
| `excluded_roots` | Header paths excluded from output ownership. |

`Input::new(name, source)` treats `name` as a root. `with_roots`, `with_root_dirs`,
`with_root_suffixes`, and `with_excluded_roots` extend that policy.
`with_excluded_source_dirs` excludes every declaration spelled beneath a directory while retaining
the file in inclusion provenance. The caller supplies all compiler arguments to `extract`,
including the language, target, include paths, defines, forced includes, and extensions.

```rust,no_run
let input = windows_clang::Input::new(
    "Example.h",
    std::fs::read_to_string("Example.h").unwrap(),
);
let snapshot = windows_clang::extract(
    [input],
    &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
)
.unwrap();
```

Use `extract_with_options` or `extract_partitioned_with_options` to parse independent original
translation units concurrently:

```rust,no_run
let options = windows_clang::ExtractionOptions::new().with_parallelism(4);
let snapshot = windows_clang::extract_with_options(
    [windows_clang::Input::new("aggregate.cpp", "#include \"all.h\"")],
    &["-x", "c++"],
    &options,
)
.unwrap();
```

The worker count is bounded by the configured value and input count. Each concurrent translation
unit owns a separate libclang index until the translation unit is dropped. Results and errors are
collected in input order. Only original extraction translation units use this setting; synthetic
constant-probe translation units keep their existing bounded probe scheduling.

The resulting `Snapshot` owns translation-unit-local facts and constants. `facts`, `constants`,
`unsupported`, and `dump` expose the extraction result for diagnostics and validation.
`included_files` returns `IncludedFile` records for the physical files visited in each original
translation unit, including the main input and headers that produced no fact or constant. The
`input` field is the normalized `Input::name` used by `Origin::tu`; `path` is the slash-normalized
filename reported by libclang.

Inclusion records are grouped by extraction input order, then sorted by ASCII-case-insensitive
path with original spelling as the tie-break. Duplicate path spellings within an input are
collapsed case-insensitively. The records do not include synthetic translation units used to
evaluate constants, skipped conditional includes, or headers supplied by a precompiled header.
Paths are not filesystem-canonicalized, so symlinks and junctions may retain different spellings.
These records are extraction provenance only and do not change root selection or emission.

### Aggregate header partition planning

`HeaderPartitionPolicy` separates logical output partitions from extraction translation units. A
caller can compile one aggregate source, map each explicitly traversed physical header to one or
more `RootPartition` candidates, and emit RDL after an internal complete ownership audit:

```rust,no_run
use std::collections::BTreeMap;
use windows_clang::{
    EmitOptions, HeaderPartitionPolicy, Input, NamespaceAuthorities, RootPartition, extract,
};

let policy = HeaderPartitionPolicy::new()
    .with_traversed_header(
        "sdk/first.h",
        RootPartition::new("first", "Example.First"),
    )
    .with_traversed_header(
        "sdk/second.h",
        RootPartition::new("second", "Example.Second"),
    );
let roots = policy
    .traversed_header_paths()
    .map(str::to_string)
    .collect::<Vec<_>>();
let snapshot = extract(
    [Input::new("aggregate.cpp", "#include \"sdk/all.h\"").with_roots(roots)],
    &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
)
.unwrap();
let references = BTreeMap::new();
let options = EmitOptions::new("Example.Common", &references);
let plan = snapshot
    .into_header_partition_plan(&policy, &NamespaceAuthorities::new())
    .unwrap();
let partitions = plan.emit_with_options(&options).unwrap();
```

`into_header_partition_plan` moves the extracted snapshot into the plan, so SDK-scale callers that
are finished inspecting extraction results do not retain the original allocation beside a cloned
planning snapshot. `plan_header_partitions` preserves the borrowed API for callers that still need
the original snapshot; it clones the snapshot and otherwise produces the same plan.

The extraction roots must cover every traversed header whose functions, GUIDs, macros, or deferred
record bodies are needed. They control extraction coverage only for this API; the
`HeaderPartitionPolicy` supplies logical root ownership after extraction. Included headers that
are not in the policy remain available for layout and type closure. If a referenced dependency
needs emission, it is routed to the namespace in `EmitOptions`. An explicit traversed-header
mapping or namespace authority takes precedence. Exact declaration-path dependencies may come from
C++ namespaces that are not eligible as public roots; unrelated declarations from those headers
remain excluded. A native free function in such a namespace becomes eligible only when its
traversed-header declaration has an explicit owner and its link name is selected by
`EmitOptions::functions`. This scoped selection does not promote other namespaced declarations or
change non-header emission.

An `associated_enum` annotation on a selected root adds the matching enum provider from its
annotation-compatible redeclarations to dependency closure. Those redeclarations may come from
other extraction inputs in the aggregate snapshot. A provider that has no traversed-header owner
inherits the annotated root's owner settings. Exclusions and namespace authority resolve multiple
candidates; otherwise the audit reports the owner conflict. An inherited owner remap updates both
the enum name and the annotation. A provider already owned by a traversed header in any extraction
input keeps that route when its physical source declaration and extracted definition match. A
signed Clang presentation of an unsigned enum value is compared at the declared representation
width, matching RDL emission. A same-name declaration from another header or with different
definition data remains independent.
When an exact or wildcard authority supplies a destination namespace for an otherwise unowned
provider, unrelated owner settings do not create a conflict. A differing remap, integer override,
flags setting, or pointer-level override for that enum still fails the audit. Unrelated included
declarations remain excluded.

If a selected function reaches a non-POD native class, header planning can retain its field layout
as a pointer-only dependency. The class must have uniformly public or uniformly protected instance
fields, no base classes, and no virtual member functions. Constructors, non-virtual methods, and
private copy operations are not emitted. The extracted fact remains unsupported until this selected
dependency path is planned, so legacy and unselected emission keep their prior behavior. Pointer
and reference uses are allowed. A protected-only class layout may also be used as storage inside an
emitted record, but direct by-value ABI uses still fail preflight. A containing record passed by
value also fails transitively. Public-field classes and `native_opaque` classes remain invalid
through typedefs, arrays, callback signatures, or containing records. An exact native declaration
also takes precedence over an unrelated external metadata type with the same leaf name; a
nonrepresentable native declaration reports a dependency blocker rather than binding the external
type. Partition exclusions still take precedence and report the normal owner-excluded dependency
diagnostic.

The valueless `win32metadata:native_opaque` annotation marks a named C++ class definition whose
source name is part of the native API but whose implementation is not metadata. The class emits as
an empty nominal type only when used through pointers or references. Its fields, methods, base
classes, size, alignment, and native inheritance are not projected. Direct and indirect by-value
uses fail preflight. Unannotated non-data classes keep their opaque `void` pointer projection.

Use `with_traversed_header_for_input(input, header, partition)` when one physical header is parsed
in several extraction inputs with different compile definitions. The input selector matches
`Fact::origin.tu` and limits which compile variant is a public root; it does not supply the logical
partition identity. Unqualified entries still apply to every matching extraction input.

Use `with_traversed_header_override(header, name, partition)` to route a named declaration away
from the header's default candidates without listing every declaration that remains with the
default owner. `with_traversed_header_override_for_input` adds the same rule for one extraction
input. Override names match extracted declaration or constant names before remaps. If at least one
override matches a name, its candidates replace the default header candidates for that name.
Multiple override candidates still use exclusions and namespace authority, and remain an audit
conflict when those policies do not select one owner.

Input identities are normalized and matched case-insensitively. Header selectors are also
case-insensitive and may be full paths or path-component suffixes. Once a header matches, root
settings and emitted `RdlPartition::header` values use the extracted physical path. This keeps
remaps and other exact source lookups consistent when policy spelling differs by case or uses a
relative suffix.

Source provenance follows these rules:

| Source item | Traversed-header path |
| --- | --- |
| Ordinary declaration, definition, or typedef | Expansion path, normally equal to spelling. |
| Macro-generated declaration | Expansion path at the invocation. |
| Fact with no matching expansion path | Spelling path as a fallback. |
| Object-like macro constant | The public root macro fact, not its evaluated definition origin. |
| Anonymous-enum or constant-variable value | The constant spelling path when no root fact exists. |
| Associated constant | The owner of the annotated declaration that retains the constant. |

The expansion-first rule keeps a declaration generated by a helper macro with the header that
invoked it. Following the public root of an evaluated constant prevents a helper macro definition
from becoming public merely because a traversed macro expands to it.

One header may have several candidate partitions. Per-candidate exclusions are applied before
ownership resolution. Namespace authorities then select a matching namespace. The selected
candidate's remaps, type overrides, flags, pointer-level overrides, and library mappings are
applied by the existing partition emitter. If candidates still differ, `PartitionAudit` reports
all independent conflicts in deterministic name order and emission fails without selecting one.
For equivalent compile variants, a type owner is omitted when its typedef or pointer-alias
projection reaches an owner-excluded type and another unsuppressed projection exists. The excluded
facts remain available for layout and dependency closure. If every projection is suppressed, the
owners are retained so existing exclusion diagnostics and single-partition alias behavior remain
unchanged.

Equivalent declarations from different traversed headers can retain several logical owners after
root selection. Header-plan routing merges those claims only when an exact or wildcard authority,
or the owners themselves, gives one destination namespace and the transformed declaration has the
same emitted annotations, UUID, flags treatment, and effective import library. Unrelated entries in
an owner's remap, exclusion, override, or library maps do not create a conflict. The canonical
planned declaration supplies the output partition and physical header, while every candidate remains
available in an ambiguity report.

When equivalent roots resolve to different namespaces, header planning keeps one public identity per
namespace if each exact source declaration belongs to one effective namespace. Ordinary declarations
use their spelling location for this classification. Macro-generated declarations use their
expansion location, so repeated invocations of one helper macro can coalesce within a destination
and retain separate identities across destinations. Type references tied to an exact spelling
location follow that declaration's scoped identity when the match is unique, so APIs from separate
header families keep their own aliases. Duplicate GUID macro facts follow the same rule. Namespace
qualification uses a separate spelling-location index because Clang type references carry
declaration spelling provenance. A spelling location is indexed only when every retained route
agrees on one namespace. This qualifies an included-only macro alias in the default namespace
without selecting an owner when independent macro expansions resolve to different namespaces.
Namespace authorities are applied before this classification, so declarations routed to one
destination still coalesce under the semantic comparison above. When several physical declarations
share one destination, a stable policy-and-source ordering selects its output partition
independently of declaration order. For a colliding public name, a source declaration claimed by
several namespaces or a reference tied to such an irreconcilable declaration remains a conflict
rather than selecting a lexical owner across destinations. Unowned dependencies continue to use the
default namespace. These rules apply to `HeaderPartitionPlan`; legacy partitioned emission keeps
its existing owner selection behavior.

Namespace collision scoping uses internal planner names only. External references and exclusions
continue to match the public post-remap name, and owner diagnostics do not expose scoped names.
Reference rewriting first matches the exact translation unit, public name, and spelling location.
The single-scoped-candidate fallback for a translation unit is used only when the snapshot has no
declaration at the referenced location. An exact included-only declaration therefore stays
unscoped, so ordinary dependency projection can default-route or inline it even when later
redeclarations of the same name have logical owners in that translation unit. Several scoped macro
expansions may share one spelling key; the index retains all of them and rewrites a reference only
when that exact key has one scoped identity.

`DEFINE_PROPERTYKEY` and `DEFINE_DEVPROPKEY` facts carry a projected key type name rather than a
Clang declaration location. Header planning tracks that name through remaps and collision scoping,
uses it for dependency closure, and qualifies the emitted constant with the resolved type route.
An included-only key type therefore uses the default namespace, while a separately owned key type
uses its logical namespace. Multiple unresolved key-type routes remain dependency blockers.

Typedef variants are compared after following exact local typedef chains only for collision
classification. If an explicitly owner-excluded declaration refers to several retained declarations
that are all equivalent, its exact references keep the deterministic surviving public owner that
planning selected before namespace scoping. If any retained declaration differs, the normal
owner-exclusion diagnostic remains.

Canonical typedefs are not promoted to public identities merely because equivalent declarations
have logical owners. If dependency closure retains a canonical raw-pointer alias such as `PVOID`
for a nested pointer boundary, its route comes from the selected declaration. A direct typedef in
another extraction input may preserve that nominal target when its local canonical alias has the
same physical spelling location, identical extracted declaration data, matching native parent
qualification, and the same semantic annotations. Emission builds one exact
`(input, name, spelling) -> declarations` typedef index and requires every declaration in the
matching bucket to satisfy those rules. This source-identity bridge does not promote the other copy
or change its owner. Direct non-typedef uses, declarations from another physical header, and raw
pointers still project to the raw pointer and do not create nominal aliases in their logical or
default namespaces.

Equivalent canonical raw-pointer declarations are scoped only for exact declarations that need a
nominal identity: another pointer typedef points through the declaration, a pointer boundary changes
mutability, or a selected function reaches a named callback that directly uses the same declaration.
Those namespaces retain the authored alias and exact references keep that identity. Standalone
callbacks, direct uses, and other representable same-mutability function or field pointer chains
keep the canonical projection behavior above. Function references participate only when the
function passes the emission function selection and exclusion filters, so an unselected declaration
cannot force a nominal alias into its header partition.

The extractor records direct `DECLARE_HANDLE(name)` macro invocations and validates the exact
`name__ { int unused; }` plus `typedef name__ *name` expansion. When a header partition suppresses
that precise private record origin but retains the public typedef, planning emits the public handle
as a named native typedef over `*mut void`. Aliases and pointer aliases keep their authored names
and pointer depth. A non-excluded handle, a lookalike declaration, or a different macro expansion
keeps the existing record-backed representation. No cleanup or invalid-handle policy is inferred.

Planning indexes declarations by exact translation unit, name, and spelling location for authority
and typedef-projection lookups. Each bucket retains snapshot fact order, including duplicate
declarations. The index is local to each planning or routing operation and is built after owner
settings and remaps have been applied.

An authority-routed dependency uses the partition identity set by `with_authority_partition`; if
none is set, its namespace is the deterministic fallback partition. Its header remains the
physical declaration or expansion path rather than the aggregate translation-unit name.

`HeaderPartitionPlan::emit_with_options` consumes the prepared plan. It transforms, plans, audits,
routes, and formats once, avoiding repeated planning for SDK-scale snapshots. Use
`HeaderPartitionPlan::audit` only for report-only callers that will not emit the same plan.
Calling `audit` before `emit_with_options` repeats planning and clones the snapshot.

Header-plan dependency closure collects independent missing, ambiguous, and unsupported dependency
edges before returning an error. The report groups each blocker by translation unit, declaration
location, name, and reason, then lists every selected root that reaches it through the resolved
dependency graph. No partial RDL is returned. Legacy single-namespace and partitioned emission keep
their existing first-error behavior.

After successful dependency closure, header planning also collects every required local type whose
owner excluded it without retaining a public alias. That deterministic report lists the partition,
namespace, declaration location, and every selected root that reaches each excluded type. Audit and
emission return the same report, and emission produces no partial RDL.

The closure report and the `plan-dependencies` timing line use these counters:

| Counter | Meaning |
| --- | --- |
| `selected_roots` | Selected type, value, function, and constant roots that started the walk. |
| `processed_unique_dependencies` | Distinct named or projected dependency references examined. |
| `resolved_dependencies` | Examined references satisfied without a blocker. |
| `unique_blockers` | Distinct declaration/name/reason groups in the report. |

A blocked declaration has no trustworthy child graph, so dependencies beneath it cannot be
reported until it is fixed. Layout, owner routing, and RDL validation also run after a successful
closure and are not represented by these counters. The counters therefore describe closure
coverage, not an overall completion percentage.

Set `WINDOWS_CLANG_TIMINGS=1` before extraction to write phase timings and counts to stderr. The
structured lines cover initial parsing, cursor traversal and fact extraction, macro and constant
probe batches and worker bounds, planning, item construction, and RDL formatting. Timing is
disabled by default and does not change the generated RDL. The legacy `WINDOWS_CLANG_TIMING`
spelling is also accepted.

## Emission

Use `Snapshot::emit(namespace)` for one namespace, `emit_with_library` to attach one DLL to all
functions, or `emit_with_options` for generator policy. `emit_by_header_with_options` returns a
map of defining-header names to RDL partitions.

`EmitOptions` controls:

| Option | Purpose |
| --- | --- |
| `namespace` | RDL namespace. |
| `library` / `libraries` | Default or per-function DLL mappings. |
| `native_imports` | Per-linker-symbol DLL plus exact exported name or ordinal contracts. |
| `references` | External types and enum members available to the projection. |
| `excluded_types` | Types omitted from the local output. |
| `excluded_functions` | Functions omitted from the local output. |
| `excluded_constants` | Constants omitted from the local output. |
| `functions` | Optional free-function allowlist. |

`NativeImports` rejects two different contracts for one linker symbol. A contract may name an
export or use `NativeImport::ordinal` to emit an ordinal entry point such as `#660`. If the same
symbol also has a `library`, `libraries`, source annotation, or partition-library mapping, the DLL
names must agree. The native entry point is independent of both the linker symbol and the projected
metadata method name. Callers reading SDK libraries can preserve that distinction with
`windows_rdl::implib::read_contracts`. They must select contracts whose raw COFF machine matches
the target architecture before constructing `NativeImports`; data and const import objects are not
function contracts.

References are explicit `TypeReference` values classified as `Type`, `Interface`, or `Enum`.
Referenced enum member names let an overlay emit an enum only when it adds members to the base.
`MetadataReferences` builds that map from WinMD files and records existing type, function, and
constant names for exclusion. `apply_reference_exclusions` excludes only types with an
unambiguous external reference, while `apply_exclusions` is available for overlays that must omit
every item from a known base. Both the high-level builder and repository generators use this
indexing path.

## Architecture

The implementation has four stages:

1. Parse each translation unit and record immutable facts, source locations, annotations, and
   constants.
2. Select roots and compute the dependency closure from the complete fact graph.
3. Resolve equivalent declarations, external references, and unsupported constructs.
4. Project the plan to RDL without mutating or discovering declarations during emission.

This separation keeps extraction order out of ownership and dependency decisions. Facts retain
translation-unit identity, while equivalent declarations are resolved during planning.

Source declarations control type identity and pointer mutability. SAL supplies direction,
optionality, size relationships, return-value markers, and interface-selection metadata; it does
not rewrite the declared C type. Explicit string and pointer typedefs therefore survive parameter
annotations.

Callback calling conventions come from the callback declarator. A pointer alias to a bare function
typedef can inherit the convention from that exact function type when Clang canonicalizes the
pointee. Referenced return, parameter, and record types do not contribute a convention. An
unannotated function pointer therefore remains C even when its return type contains `WINAPI`
members, while aliases of an explicitly annotated callback keep the annotation on every target
architecture.

`ParamAnnotation::null_terminated` records ordinary `_z_` string contracts.
`ParamAnnotation::null_null_terminated` records the captured `_NullNull_terminated_` multistring
contract and emits `#[null_null_terminated]`. The two flags are independent, and neither is
inferred for an unannotated binary buffer.

### Win32 metadata annotations

Headers can add metadata policy with Clang `annotate` attributes whose payload begins with
`win32metadata:`. Emitted annotation values are stored in the `Snapshot` sidecar by declaration
origin and member slot, merged across compatible redeclarations, and emitted with the selected
owning declaration. Extraction-only annotations may instead control fact classification.
Annotation collection does not change defining-header ownership.

The primary vocabulary controls:

| Contract | Annotation |
| --- | --- |
| Function import policy | `set_last_error`, `import_library`, `static_library` |
| HRESULT projection | `preserve_result` |
| Handle lifetime | `raii_free`, `invalid_handle`, `free_with`, `do_not_release` |
| Relationships | `retained`, `also_usable_for`, `associated_enum`, `associated_constant` |
| Parameter projection | `in`, `out`, `optional`, `reserved`, `retval`, `com_out_ptr` |
| Parameter buffer sizes | `array_count_param`, `array_count_const`, `memory_size_param` |
| Field buffer sizes | `array_count_field` |
| String and value metadata | `ansi`, `unicode`, `native_encoding`, `const` |
| Native opaque classes | `native_opaque` |
| Type and result policy | `agile`, `native_inheritance`, `struct_size_field`, `supported_os` |

`raii_free` may list one cleanup provider followed by numeric or object-like macro invalid-handle
sentinels. Symbolic sentinels are resolved in the source macro environment. `associated_constant`
adds only the named provider constant to the dependency closure. `supported_os` is repeatable.
Compatible redeclarations union repeatable annotations. Singleton conflicts are errors except for
`import_library`, which uses deterministic source-order first-wins behavior.

`native_opaque` is valueless and valid only on a named, non-COM C++ class definition without a
UUID. Place the attribute after the `class` keyword and before the name. It is an extraction
control and does not produce an RDL attribute. Clang-propagated copies on redeclarations are
accepted, but spelling the marker on a forward declaration is an error.

Unknown keys, missing or unexpected values, invalid declaration targets, unresolved symbolic
sentinels, and missing associated-constant providers are extraction errors. A consumer that
activates annotations through a forced include should also pass `-DWIN32METADATA=1`; direct
`Input::source` text containing `win32metadata:` enables the same strict validation.

### RDL type identity policy

RDL is the authoritative description produced from the headers. It preserves the type named by a
declaration unless that name belongs to a small, explicit canonical vocabulary. Canonicalization is
name-keyed rather than structural: two typedefs with the same ABI representation are not assumed to
have the same meaning.

| Source category | RDL policy |
| --- | --- |
| Scalar vocabulary (`BYTE`, `DWORD`, `FLOAT`, `DOUBLE`) | Use the corresponding RDL primitive. |
| MIDL predefined scalars (`boolean`) | Use the corresponding RDL primitive (`u8`). |
| Pointer-sized vocabulary (`SIZE_T`, `ULONG_PTR`, `LONG_PTR`) | Use `usize` or `isize`. |
| ABI aliases (`HNSTIME`, CLR tokens, signatures, and handles) | Use their fixed ABI shapes. |
| String aliases (`LPCWSTR`, `LPWSTR`) | Use the canonical RDL string vocabulary. |
| GUID aliases (`IID`, `CLSID`, `UUID`) | Use `GUID`. |
| Generic void pointers (`PVOID`, `LPVOID`) | Use the corresponding raw pointer. |
| Interface pointer typedefs | Project to the RDL interface type with encoded pointer semantics. |
| Other typedefs, including pointer typedefs | Preserve the name and emit its definition. |

Lowercase `boolean` is part of MIDL's predefined type vocabulary and has an unsigned 8-bit
representation, so it becomes `u8`, not RDL `bool`. Uppercase `BOOLEAN` is a named Windows API
typedef and remains `type BOOLEAN = u8`; references to it retain the `BOOLEAN` name. This preserves
the distinction between canonical language vocabulary and an API-authored typedef without treating
arbitrary byte values as Rust booleans.

Explicit null-terminated SAL on a direct character pointer selects the same string vocabulary after
applying the existing scalar canonicalization to its pointee. For example, `_In_z_ const WCHAR *`
becomes `PCWSTR`. Without the null-terminated fact, `WCHAR *` keeps its raw pointer shape.

For example, the headers declare `PBYTE`, `PDWORD`, and `PORHKEY` in API signatures. RDL retains
those names and separately records their representations:

```rdl
type PBYTE = *mut u8;
type PDWORD = *mut u32;
type ORHKEY = *mut void;
type PORHKEY = *mut ORHKEY;
```

This keeps both the authored API vocabulary and the ABI shape. A parameter declared as
`_Out_opt_ PDWORD` is emitted as `#[out] #[opt] PDWORD`, not `*mut u32`. A parameter written
directly as `ORHKEY *` remains `*mut ORHKEY`; it is not renamed to `PORHKEY`.

Do not flatten typedefs in RDL to accommodate a binding projection. A downstream generator can
resolve or collapse an alias when needed, while recovering a discarded source name is unreliable.
Likewise, do not use SAL direction to change `P*` aliases or mutable pointers into const pointers.
RDL records the declared C type and the SAL contract as separate facts.

Header-partition emission retains dependency pointer typedefs when they provide a nested pointer
boundary. WinMD can represent `LPCWSTR *` as an outer mutable pointer to the `PCWSTR` typedef, but
not as one raw pointer chain with different constness at each level. Required aliases and aliases
that depend on them use the configured default namespace. Direct canonical string-alias uses retain
their canonical `PCSTR`, `PSTR`, `PCWSTR`, or `PWSTR` alias when references do not supply it. The
alias is qualified when emitted in another logical namespace and remains unqualified inside that
namespace. A supplied metadata reference takes precedence over the local canonical typedef. The RDL
compiler continues to reject a mixed raw `*mut`/`*const` chain when the source provides no named
boundary.

Incomplete records are valid when used through pointers and rejected when a complete by-value
layout is required. Fixed-underlying forward enums can be represented by their declared integer
type. Unfixed forward enums are rejected rather than assigned a guessed representation.

An incomplete declaration may resolve to a complete declaration from another translation unit when
their public names and C/C++ declaration kinds match. The completed projection may differ from the
placeholder representation: for example, an incomplete `struct` is initially a record but may
resolve to a COM interface once another translation unit supplies its virtual definition.
Incompatible declaration kinds remain ambiguous.

Defined POD C++ classes with public instance fields and no inheritance, methods, constructors,
destructors, conversions, or function templates use the checked record-layout path. Other
defined non-interface C++ classes remain opaque. A definition marked `native_opaque` instead uses
the named empty-record representation for pointer and reference types while discarding its C++
implementation details. A forward-only non-UUID class uses the same named representation without
an annotation, so pointer parameters, return values, and typedef aliases retain their source type
identity. If an unannotated definition is available, it still controls classification: public
data-only definitions become records, while behavioral definitions remain opaque. Neither empty
projection infers fields, packing, alignment, or inheritance, and by-value uses fail validation.

UUID attributes do not by themselves make a declaration a coclass. Defined UUID-bearing structs
and public data-only classes use the record-layout path and retain the UUID as a `GuidAttribute` on
the emitted WinMD type. Structurally recognized COM interfaces remain interfaces. An incomplete
UUID-bearing class or struct with no data or interface definition remains a coclass GUID value,
which preserves the SDK forward-declaration convention.

Record layout inference keeps member packing and forced record alignment separate. When more than
one representation matches Clang's size, alignment, and field offsets, it prefers one without
forced alignment and then the least restrictive packing. This distinguishes `#pragma pack(N)` from
an explicitly over-aligned record and permits records that require both.

C++ reference data members use the target pointer size and alignment for record layout, including
references hidden behind typedefs. Their declared reference type and constness are preserved, so
RDL represents them as mutable or const unmanaged pointers. Function and callback parameter
references continue to use their existing parameter conversion independently of record storage.

MIDL-generated headers assign `__MIDL...` tags to anonymous IDL declarations and `_NAME` backing
enum tags to some public scalar typedefs named `NAME`. An unreferenced generated enum is emitted as
loose constants, matching the IDL API identity. When it is followed by a scalar typedef in the
source header, those constants use that public alias. The `_NAME` case additionally requires the
MIDL compiler marker in the same physical header and an exact adjacent `NAME` alias. Generated
records used only to define an opaque pointer typedef are likewise represented by the public
pointer alias. A generated declaration referenced directly by another ABI type keeps its generated
name and layout.

Windows `DECLARE_HANDLE` uses a private dummy record to make unrelated handles distinct in C++.
Header-partition emission may replace that record dependency with `*mut void` only for an exact
recorded macro expansion whose private record origin is excluded by its owner. The public handle
remains a named typedef, so aliases such as a second handle name and pointer-to-handle typedefs
retain their metadata identities. Other handles keep their record-backed output.

An object-like macro defined after a global unscoped enum member may replace that member's value
when it has one same-name candidate in the same translation unit and physical header, and its type
resolves directly, through scalar typedefs, or through an enum typedef to an integer domain. This
preserves the identifier that C callers see without emitting two items into the header's shared RDL
value namespace.

A macro defined before the member is treated as the same source symbol only when its integer type
resolves directly or through scalar typedefs to the enum's width and their mathematical values are
equal. Signed positive values may match unsigned values of the same width, but negative
reinterpretation and width changes do not match. Explicitly associated constants remain separate
providers. Scoped enums, C++ namespace members, ambiguous candidates, typed pointer or handle
constants, and declarations from different headers or translation units also remain separate.

RDL can encode a type definition and a member of the namespace's `Apis` class with the same
projected name. The extractor therefore preserves a type and object-like macro with the same public
name, whether they come from one header or are combined across translation units. This does not
impose namespace and type-name uniqueness on WinMD; tagged architecture inputs may contain more
than one matching `TypeDef`.

GUID, property-key, and coclass facts are also planned as values. They honor constant exclusions
while their referenced types still participate in dependency closure.

Native NaN and infinity constants are omitted because RDL and ECMA metadata cannot represent them.
This includes `f64` values that become non-finite when narrowed to their declared `f32` type.
Integer-valued pointer constants remain supported. If a typedef chain resolves to an
interface-pointer alias that is projected as the interface itself, constants declared with that
typedef are omitted because ECMA metadata cannot encode an interface-valued constant. An explicit
pointer to the same interface remains a pointer and is emitted.

Object-like macro probes accept a value only when Clang parses the complete generated expression
without an error diagnostic. `KeepGoing` may otherwise expose an evaluable cursor for the valid
prefix of a malformed macro. Error locations in generated declarations reject only the associated
macro, while diagnostics already accepted from the original translation unit do not invalidate
the synthetic probe. An error that cannot be localized still uses the recovery, isolation, and
singleton tiers. The probe uses `const` declarations rather than `constexpr`, which preserves
integer-valued pointer casts while syntax errors still reject the malformed macro.

### Bit-field member scraping

RDL and WinMD cannot encode C bit-field syntax directly. A consecutive run of bit fields is emitted
as an integer backing field with `NativeBitfieldAttribute` entries that preserve each member's name,
offset, and width. Bindgen uses those attributes to generate accessors over the backing field.

## Generator responsibilities

The consuming tool owns concerns outside header extraction:

- libclang provisioning and version checks;
- SDK, WDK, or component package restoration;
- compiler language mode, target, and include arguments;
- import-library parsing and function-to-DLL policy;
- architecture-specific extraction and merge;
- transactional promotion of generated RDL;
- RDL compilation and downstream binding generation.

The repository's generator tools share these facilities through `crates/tools/helpers`.
`tool-win32` supplies the Win32 and WDK policy, while `tool-webview` supplies the WebView2 policy.

## Known limits

- RDL cannot represent mixed pointer-chain mutability.
- Coverage is limited to declarations reachable from configured roots.
- The flat Win32 namespace cannot preserve distinct declarations that differ only by curated
  namespace placement.
- Header extraction cannot infer policy that is absent from SAL or `win32metadata:` annotations.
- Stable Rust has no function-pointer ABI corresponding to every native calling convention.

## Testing

The crate's integration tests cover fact isolation, constants, layouts, interfaces, annotations,
dependency closure, header ownership, external references, incomplete declarations, macros,
callbacks, arrays, variadics, recursion, and cross-translation-unit resolution. Declarative
input/output cases live in `test_clang`: each `input/<name>.h` fixture generates
`expected/<name>.rdl`, which is parsed with `windows-rdl` before the golden is updated. CI rejects
any uncommitted output change.

Fixtures default to C++, the `Test` namespace, and `test.dll`. Leading `//!` lines may override the
setup:

| Directive | Effect |
| --- | --- |
| `namespace <name>` | Sets the emitted namespace. |
| `library <name>` | Sets the import library. |
| `args <arguments>` | Replaces the libclang arguments. |
| `reference-default` | Resolves extraction types against the default metadata. |

```text
cargo test -p windows-clang
cargo test -p test_clang
```

The ignored `planner_lookup_scaling` unit test measures planning and partition emission with 1,000
typedef roots and 8,000 or 32,000 unrelated facts. Run it with
`cargo test -p windows-clang --release --lib planner_lookup_scaling -- --ignored --nocapture`.
Set `WINDOWS_CLANG_LOOKUP_EVIDENCE` to an existing directory to save the ordered partition output
and conflict diagnostic for byte-for-byte comparisons. This fixture does not parse headers or
measure SDK extraction.

The tests need a loadable compatible libclang. Repository CI obtains the pinned runtime with
`cargo run -q -p tool-clang -- path` and exports `LIBCLANG_PATH` before running the workspace.
