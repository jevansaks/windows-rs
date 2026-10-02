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
    .plan_header_partitions(&policy, &NamespaceAuthorities::new())
    .unwrap();
let partitions = plan.emit_with_options(&options).unwrap();
```

The extraction roots must cover every traversed header whose functions, GUIDs, macros, or deferred
record bodies are needed. They control extraction coverage only for this API; the
`HeaderPartitionPolicy` supplies logical root ownership after extraction. Included headers that
are not in the policy remain available for layout and type closure. If a referenced dependency
needs emission, it is routed to the namespace in `EmitOptions`. An explicit traversed-header
mapping or namespace authority takes precedence. Exact declaration-path dependencies may come from
C++ namespaces that are not eligible as public roots; unrelated declarations from those headers
remain excluded.

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

An authority-routed dependency uses the partition identity set by `with_authority_partition`; if
none is set, its namespace is the deterministic fallback partition. Its header remains the
physical declaration or expansion path rather than the aggregate translation-unit name.

`HeaderPartitionPlan::emit_with_options` consumes the prepared plan. It transforms, plans, audits,
routes, and formats once, avoiding repeated planning for SDK-scale snapshots. Use
`HeaderPartitionPlan::audit` only for report-only callers that will not emit the same plan.
Calling `audit` before `emit_with_options` repeats planning and clones the snapshot.

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
| `references` | External types and enum members available to the projection. |
| `excluded_types` | Types omitted from the local output. |
| `excluded_functions` | Functions omitted from the local output. |
| `excluded_constants` | Constants omitted from the local output. |
| `functions` | Optional free-function allowlist. |

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

### Win32 metadata annotations

Headers can add metadata policy with Clang `annotate` attributes whose payload begins with
`win32metadata:`. Annotation values are stored in the `Snapshot` sidecar by declaration origin and
member slot, merged across compatible redeclarations, and emitted with the selected owning
declaration. Annotation collection does not change defining-header ownership.

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
| Type and result policy | `agile`, `native_inheritance`, `struct_size_field`, `supported_os` |

`raii_free` may list one cleanup provider followed by numeric or object-like macro invalid-handle
sentinels. Symbolic sentinels are resolved in the source macro environment. `associated_constant`
adds only the named provider constant to the dependency closure. `supported_os` is repeatable.
Compatible redeclarations union repeatable annotations. Singleton conflicts are errors except for
`import_library`, which uses deterministic source-order first-wins behavior.

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
non-interface C++ classes remain opaque.

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

When an active object-like macro shadows an enum member declared by the same header, the member
takes the macro's effective value. This preserves the identifier that C callers see without
emitting two items into the header's shared RDL value namespace. Same-named declarations owned by
different headers remain separate.

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

The tests need a loadable compatible libclang. Repository CI obtains the pinned runtime with
`cargo run -q -p tool-clang -- path` and exports `LIBCLANG_PATH` before running the workspace.
