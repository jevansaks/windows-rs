# windows-rdl

> A parser for RDL (Rust Definition Language) and an ECMA-335 metadata generator.

- 📦 [crates.io](https://crates.io/crates/windows-rdl)
- 📖 [docs.rs](https://docs.rs/windows-rdl)
- 🚀 [Getting started](../../crates/libs/rdl/readme.md)
- 📁 [Source](https://github.com/microsoft/windows-rs/tree/master/crates/libs/rdl)

`windows-rdl` is the front of the metadata authoring pipeline. It parses RDL (Rust Definition
Language), a small Rust-like syntax for Windows APIs. It emits ECMA-335 `.winmd` metadata for
[`windows-bindgen`](windows-bindgen.md). It also writes canonical RDL from `.winmd` files.

Use `windows-rdl` when an API needs metadata first. You can write RDL by hand. You can also generate
RDL from C or C++ headers with [`windows-clang`](windows-clang.md). Then pass the `.winmd` output to
`windows-bindgen`.

## Getting started

Add `windows-rdl` as a build dependency. It usually runs from a codegen tool or `build.rs`. It is
not a runtime dependency.

```toml
[build-dependencies]
windows-rdl = "0.100"
```

The crate exposes two builders:

- `reader()` compiles RDL source to `.winmd` metadata.
- `writer()` writes canonical RDL source from `.winmd` metadata.

Input, reference, and output paths accept strings, `Path`, or `PathBuf`, so build scripts can pass
paths without converting them to UTF-8 strings. `.input_text(..)` and `.input_texts(..)` compile RDL
source already in memory.

Use `.input_text_named(name, source)` or `.input_texts_named(sources)` for in-memory sources whose
names should appear in diagnostics. `Diagnostic` carries a severity, optional code, source labels,
notes, and help. `DiagnosticReport` assigns each input a stable `SourceId` and stores its original
text, so duplicate display names cannot attach labels to the wrong source. `Error` is a small owned
wrapper that dereferences to its `Diagnostic`.

### RDL to winmd, and back

Use `reader` to compile `.rdl` into `.winmd`. Use `writer` to regenerate canonical `.rdl` from
`.winmd`.

```rust,no_run
// RDL source -> winmd metadata.
windows_rdl::reader()
    .input("example.rdl")
    .output("example.winmd")
    .write()
    .unwrap();

// winmd metadata -> canonical RDL source.
windows_rdl::writer()
    .input("example.winmd")
    .output("example.rdl")
    .write()
    .unwrap();
```

RDL can reference types it does not define. Examples include `HRESULT` and
`Windows::Win32::System::Com::IUnknown`. Add the standard metadata so those references resolve.

```rust,no_run
windows_rdl::reader()
    .input("example.rdl")
    .reference_default()
    .output("example.winmd")
    .write()
    .unwrap();
```

The reader treats the default metadata as references while compiling the input RDL. Add other
reference metadata with `.reference(path)`, `.references(paths)`, `.reference_bytes(bytes)`, or
`.reference_byte_sets(byte_sets)`. The writer has the corresponding `.input`, `.inputs`,
`.input_bytes`, and `.input_byte_sets` methods and treats default metadata as input to render.

### C/C++ headers to RDL

Use [`windows-clang`](windows-clang.md) when an API ships only a C or C++ header. The `clang()`
builder parses the header into RDL. Then `reader()` compiles that RDL to metadata.

Each header is parsed as its own translation unit. The scraper emits only that header's top-level
declarations. It does not emit declarations from `#include` files. List each header you need as a
separate input.

```rust,no_run
windows_clang::clang()
    .args(["-x", "c++", "--target=x86_64-pc-windows-msvc"])
    .input("Example.h")
    .reference_default()
    .output("example.rdl")
    .namespace("Example")
    .library("Example.dll")
    .write()
    .unwrap();
```

## RDL syntax

RDL looks like a small Rust module. A top-level `mod` is a metadata namespace. Tag it `#[winrt]` or
`#[win32]` to select the type system. Attributes map to metadata attributes. Item keywords mirror
metadata kinds.

```text
#[win32]
mod Example {
    #[repr(i32)]
    enum Color {
        Red = 1,
        Green = 2,
        Blue = 3,
    }

    struct Point {
        x: i32,
        y: i32,
    }

    const MAX: u32 = 42;

    #[library("example.dll")]
    extern fn GetPoint() -> Point;

    #[guid(0x00000001_0002_0003_0004_000000000005)]
    interface ICustom : Windows::Win32::System::Com::IUnknown {
        fn Method(&self, value: i32) -> i32;
    }
}
```

Most attributes name a metadata attribute type directly. Some attributes use short pseudo-attribute
names. The reader expands those names to full metadata attributes. See `PSEUDO_ATTRS` in
`windows-rdl`.

Struct bit fields use their own syntax. A run of bit fields packed into one backing integer is
written as a C-like block on that field. Each member uses `Name: width`. Anonymous padding uses
`_: width`.

```text
struct D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
    _bitfield: u32 {
        Usage: 1,
        RGB_Range: 1,
        YCbCr_Matrix: 1,
        YCbCr_xvYCC: 1,
        Nominal_Range: 2,
        Reserved: 26,
    },
}
```

Member offsets are implicit. Each offset is the total width of earlier members, including padding.
The reader writes one `Windows.Win32.Metadata.NativeBitfieldAttribute(name, offset, width)` custom
attribute per named member. The writer renders it back to block form.

See [`windows-clang`](windows-clang.md#bit-field-member-scraping) for how the scraper emits bit
fields. See [`windows-bindgen`](windows-bindgen.md#generating-bit-field-accessors) for the accessors
they drive.

WinRT types use the `#[winrt]` namespace flavor. They also add runtime-class and property syntax.

```text
#[winrt]
mod Robotics {
    #[Activatable(1)]
    class Robot {
        IRobot,
    }

    #[ExclusiveTo(Robot)]
    interface IRobot {
        fn Speak(&self, message: String);
        Name: String;
    }
}
```

The `crates/tests/libs/rdl/input` directory has focused `.rdl` files for syntax examples. It covers
structs, flags, delegates, generic interfaces, unions, and more.

## How it fits with windows-bindgen

`windows-rdl` and `windows-bindgen` are two stages in one pipeline.

```text
C/C++ headers -- clang() --> .rdl -- reader() --> .winmd -- bindgen() --> bindings.rs
 (windows-clang)             (windows-rdl)        (windows-bindgen)
```

Skip `windows-rdl` when metadata already exists. Use it when you need to create metadata first. You
can write RDL by hand or lift declarations from a header.

Two in-repo tools show both uses:

- `tool_webview` runs the full path. WebView2 ships only a C/C++ header. `clang()` produces
  `WebView2.rdl`. `reader()` compiles it to `WebView2.winmd`. Then `windows_bindgen::bindgen`
  generates bindings for [`windows-webview`](windows-webview.md).
- `tool_reactor` hand-writes COM interfaces and bootstrap functions in
  `crates/tools/reactor/src/extras.rdl`. These declarations fill gaps in the WinUI and Windows App
  SDK metadata. The tool compiles them with the standard Win32 winmd into `extras.winmd`. Then it
  feeds that winmd to `windows_bindgen::bindgen` for [`windows-reactor`](windows-reactor.md).

In both tools, `reader` also gets the standard metadata as references. That lets RDL names resolve
against the standard definitions.

---

## Internal documentation

The rest of this page covers how the crate is built and maintained. It is for contributors and is
not needed to use `windows-rdl`.

### How it's built

The RDL grammar uses `syn`, `quote`, and `proc-macro2`. It reuses Rust's tokenizer so the syntax
stays Rust-shaped. The `reader` lowers the syntax tree to ECMA-335 and emits `.winmd` through
[`windows-metadata`](windows-metadata.md). The `writer` reads metadata through the same crate and
writes canonical RDL.

The `clang` path uses `clang-sys` to parse C or C++ translation units. It projects the declarations
into the RDL syntax tree. The header path and hand-authored RDL path share the same lowering code.
The `formatter` module pretty-prints generated RDL.

### Testing

Dedicated test crates cover the crate:

- `test_rdl` covers RDL to winmd round trips with `input/*.rdl` fixtures.
- `test_rdl` also checks representative Win32 and WinRT output with .NET
  `System.Reflection.Metadata`, then projects the WinRT output with the Windows SDK C++/WinRT
  projection tool.
- `test_clang` covers header to RDL output with `expected/*.rdl` goldens.
- `tool_roundtrip` re-derives committed RDL files from committed winmd files. The `gen` workflow
  enforces a clean `git diff`.
- `test_bindgen` covers the `.winmd` to Rust step that consumes this crate's output.

Run targeted tests with:

```sh
cargo test -p test_rdl
cargo test -p test_clang
```

### Performance benchmarks

Run the RDL benchmark tool with an optimized build:

```sh
cargo run -p tool_rdl_bench --release
cargo run -p tool_rdl_bench --release -- --samples 10
```

The tool preloads the committed WinRT RDL and winmd inputs, runs one unmeasured warmup, and reports
the median, minimum, and maximum time. It measures the complete RDL check pipeline, strict winmd
reading and WinRT validation, formatting every WinRT RDL file, winmd-to-split-RDL dumping, and
building the dumped RDL back into a winmd. Dump output stays under `target/rdl-bench`.

The initial five-sample reference was recorded on 2026-08-08 with a 12th Gen Intel Core i9-12900K,
Windows x64, and the workspace release profile:

| Workload | Input | Median |
| --- | ---: | ---: |
| Check WinRT RDL | 343 files, 9,904,180 bytes | 1.879 s |
| Validate `Windows.winmd` | 6,171,136 bytes | 0.420 s |
| Format WinRT RDL | 343 files, 9,904,180 bytes | 0.486 s |
| Dump `Windows.winmd` | 6,171,136 bytes | 1.309 s |
| Build dumped WinRT RDL | 343 files, 9,904,180 bytes | 1.890 s |

These values are same-host references, not portable CI limits. Do not treat one slower run as a
regression. Re-run at least three five-sample measurements on the same idle machine and review the
profile before adding or changing a performance gate.

An initial phase profile found:

| Workload | Main measured costs |
| --- | --- |
| Check/build | Class and interface projection dominate metadata row encoding; parsing is secondary |
| Validation | Attribute decoding, type/member checks, and WinRT policy checks |
| Formatting | Parsing plus concrete-syntax rendering |
| Dumping | Metadata-to-token projection, formatting, and file writes |

The first profile found repeated `AttributeUsageAttribute` decoding in WinRT target validation.
Caching each attribute definition's decoded usage mask reduced that phase from about 0.10-0.11
seconds to 0.06-0.07 seconds on the reference machine. The cache is local to one validation run and
does not change diagnostics. Further encoding or dump changes need a call-stack or allocation
profile; the broad phase timings alone do not justify a larger refactor.

### Default metadata files

`windows-rdl` builds the default metadata files used by the in-repo generators and library crates.

| File | Source | Writer |
|------|--------|--------|
| `crates/libs/default/Windows.winmd` | SDK Contracts winmds, merged and written through RDL | `tool_winrt` |
| `crates/libs/default/Windows.Win32.winmd` | Windows SDK + WDK headers scraped to RDL, um + km merged | `tool_win32` |

The committed RDL files are the reviewable source for these metadata files:

- `metadata/winrt/*.rdl` is partitioned by namespace.
- `metadata/win32/*.rdl` is partitioned by defining header.
- `metadata/wdk/*.rdl` is partitioned by defining header.

The binary winmd files are derived artifacts. Generation is deterministic. The metadata writer
stages tables in `BTreeMap`s and uses a fixed zero GUID for the module MVID.

Every maintained crate that needs Win32 metadata resolves against the in-repo `Windows.Win32.winmd`.
Minimal-binding crates and `windows-reactor` use it directly. The `windows` and `windows-sys` crates
use it through `tool_package`.

### Multi-arch merge

`tool_win32` scrapes x64, arm64, and x86 into separate RDL sets. Then `merge_arch_rdl` combines them
into one winmd. A type with the same shape on every architecture is emitted once. A type that
differs by architecture is split into per-architecture copies tagged `#[arch(X86|X64|Arm64)]`.

The merge compares type structure through [`windows-metadata`](windows-metadata.md).
`merge_arch_rdl` handles orchestration. It reads each architecture's RDL, runs the merge, and writes
the combined output. `ArchInput` stores its RDL directory and winmd as `PathBuf`.

### Published crates and namespace remap

The in-repo Win32 and WDK metadata lives in flat namespaces. Published `windows` and `windows-sys`
APIs are partitioned behind many Cargo features.

`tool_package` remaps the flat metadata into header-stem namespaces under `target/package/`. It uses
the committed `metadata/win32` RDL directory as the routing signal. Then it runs `windows-bindgen`
over that partition. `tool_features` uses the same remap so feature search reports the same header
stems.

The in-repo WinRT `Windows.winmd` is projected with the remapped Win32 and WDK metadata.

### Round-trip rules

RDL is the reviewable source for WinRT, Win32, and WDK metadata. The `.winmd` files are derived
artifacts. The `gen` workflow runs the generators and `tool_roundtrip`. It fails when regeneration
changes tracked files.

| Family | External source | RDL layout | Winmd build path |
|--------|-----------------|------------|------------------|
| WinRT | SDK Contracts winmds | `metadata/winrt`, per namespace | merge SDK winmds, write RDL, read RDL to winmd |
| Win32 | SDK headers | `metadata/win32`, per header | scrape headers to RDL, merge architectures, read RDL to winmd |
| WDK | WDK headers | `metadata/wdk`, per header | scrape headers to RDL, merge architectures, read RDL to winmd |

`tool_roundtrip` validates the reverse direction:

- WinRT uses `writer(Windows.winmd).split()` to write `metadata/winrt`.
- Win32 and WDK cannot recover header files from flat winmd alone. The tool reads the committed RDL
  layout to map type names back to header stems. Then it writes `metadata/win32` or `metadata/wdk`
  with `writer(winmd).partition(map)`.

### Current normalization rules

Some metadata forms have canonical RDL spellings. These rules are intentional. They keep generated
RDL stable.

| Form | RDL spelling | Reason |
|------|--------------|--------|
| WinRT `System.Char` | `Char16` | It stays distinct from `u16` and maps to `metadata::Type::Char`. |
| Property setter parameter names | `value` or `#[set_name(name)]` | Nonstandard names stay explicit. |
| Event add parameter names | `handler` or `#[add_name(name)]` | Nonstandard names stay explicit. |
| Event remove parameter names | `token` or `#[remove_name(name)]` | Nonstandard names stay explicit. |
| Property and event accessors | Property or event shorthand | The writer tracks consumed `get_`, `put_`, `add_`, and `remove_` methods. |
| Unconsumed interface methods | Full method form | Methods that are not part of shorthand stay explicit. |
| Input direction | `#[in]` | The reader accepts `#[r#in]`; the formatter emits `#[in]`. |
| Raw identifiers, GUID constants, and delegate ABI spelling | Canonical writer output | Text can differ while metadata stays equivalent. |

The reader rejects unsupported forms with errors. It does not silently drop them. Examples include
unsupported types, constants, callback ABIs, variadic callback parameters, and function ABIs.

### Method parameter rows

The winmd writer uses `MethodDef::params_by_sequence` from
[`windows-metadata`](windows-metadata.md). Sequence 0 supplies return-type attributes. Nonzero rows
are matched to one-based signature positions, so sparse or out-of-order metadata cannot rename or
reflag another parameter. Every signature parameter is emitted; a missing row uses `pN` with no
explicit direction or optional marker, which the RDL reader lowers with its existing type-based
direction default. Duplicate and out-of-range sequences stop the write with a diagnostic.

Direction and optionality come from `MethodParam::direction()` and `is_optional()`. Reserved,
retval, and count attributes remain independent pseudos or custom attributes. The writer applies
RDL's format boundary only when spelling them: In+Out emits both markers, while Unspecified emits
the input spelling because RDL has no literal unspecified-direction syntax.

`test_rdl::method_params` authors sparse and out-of-order rows directly, checks parameter and
return pseudos in the generated RDL, and compiles the RDL back to dense metadata with the same
associations. It also checks that every non-return `Param` row in the committed WinRT, Win32, and
WDK metadata has at least one representable direction flag.

### Lossless round-trip limits

- **Direction flags:** RDL has no spelling for a `Param` row with neither In nor Out. Omitting both
  invokes the type-based default, so a metadata row with neither flag reads back as In.
- **Void return rows:** Return attributes are written after `->`. A void method has no return type
  to carry them, so attributes on its sequence 0 row are not represented.
- **Count relationships:** `#[len_param(N)]` and `#[size_param(N)]` store raw zero-based signature
  positions. Reordering parameters without updating `N` changes the relationship.
- **Pointer constness:** `metadata::Type` stores one constness bit with a pointer depth. Uniform
  chains such as `*mut *mut T` and `*const *const T` round-trip. Mixed chains are rejected before
  metadata is written, including chains nested inside a reference.

## Future development

RDL should remain an explicit description of the API and its ABI. Concision is useful when it
removes repetitive syntax without hiding interface boundaries, factory calls, allocations,
marshaling, versioning, compatibility obligations, or other costs that API authors need to review.
MIDL3 syntax is therefore an input for comparison, not a feature checklist.

The first development work should focus on correctness, lossless conversion, diagnostics, and
source-level usability. Runtime-class shorthand should be evaluated later and adopted only where
the lowering remains visible and predictable.

### Semantic validation

RDL should lower into ECMA-335 metadata as soon as parsing, imports, and enough name resolution
have succeeded. Metadata is already the interchange format used by `tool_win32`, `tool_winrt`,
`tool_roundtrip`, merge, remap, and bindgen. Maintaining a second declaration model in
`windows-rdl` would duplicate that schema and allow the representations to drift.

```text
source -> syntax tree -> metadata builder + source origins -> validation -> optional winmd
```

Syntax-only checks remain in `windows-rdl`: parsing, imports, unsupported syntax, malformed control
attributes, and failures where a type cannot be classified well enough to encode a signature.
Everything represented by ECMA-335 should be checked by a shared `windows-metadata` validator. The
RDL compiler now records an external origin map from stable metadata row IDs to compact source IDs
and positions, then translates shared validation failures back to source labels. The map is not
stored in the syntax tree or emitted as private attributes in the winmd.

`windows-metadata` currently has an append-only `writer::File` and a queryable byte-backed
`reader::Index`. Improve that boundary in stages:

1. Add a standalone validator over `reader::Index`.
2. Add an in-memory writer-to-reader handoff, initially permitting internal serialization.
3. Refactor merge and remap to validate their output before writing files.
4. Add a queryable finalized metadata image if avoiding serialization makes the pipeline simpler
   or measurably faster.
5. Refactor RDL to lower directly into the metadata builder plus an origin map.

This order validates the metadata design independently of RDL and gives the rest of the toolchain a
useful validator even if some source-side facts must remain during lowering.

Validation rules should be grouped by profile:

| Profile | Examples |
|---------|----------|
| Common | Names, attributes, generic arity, overloads |
| Win32 | Calling conventions, libraries, pointers, arrays, architecture, layout |
| WinRT | Type graph, classes, factories, contracts, versions, overloads |
| Round trip | Source or metadata forms that cannot be represented |

Duplicate handling needs an explicit symbol model rather than a blanket uniqueness check. A scope
may contain several declarations with the same source name only when the declaration kind permits
it and the variants are distinguishable:

- Method overloads must have distinct signatures and valid overload metadata.
- Architecture variants must have nonconflicting architecture masks.
- Repeated partial declarations must follow a defined merge rule.
- Properties, events, fields, enum members, generic parameters, and ordinary types must be unique.
- A function, constant, and type sharing a metadata scope must be checked against the output
  representation rather than accepted because they live in separate internal maps.

Each collision diagnostic should identify the new declaration and the earlier declaration. This
work should resolve the rules tracked by
[`windows-rdl` duplicate symbols](https://github.com/microsoft/windows-rs/issues/4186).

The reader now runs an initial duplicate-symbol pass after indexing and before metadata emission.
`RDL0001` diagnostics label both declarations. The pass rejects collisions between top-level
types, functions, and constants; duplicate members and parameters; overlapping architecture
variants; and method overloads with the same parameter types. Distinct method signatures,
disjoint architecture variants, and matching split get/set properties remain valid.

Top-level symbols, enum variants, attribute properties/constructors, and cross-kind interface
member conflicts remain source checks. Those concepts either participate in source resolution or
lower to less-specific metadata rows, so moving them would weaken diagnostics. Field, method,
property, event, and implemented-interface identities are validated on finalized metadata rows, so
aliases and qualified paths share one identity. Return types do not distinguish methods or
non-indexed properties; complementary property/event accessor rows are valid only when their
associated types agree. `RDL0005` rejects missing or extra generic arguments.

The winmd writer now preflights Property and Event rows before reconstructing shorthand. Custom
attributes, nonzero flags, property constants, unsupported or duplicate semantics, missing
accessors, mismatched accessor names, non-special accessors, and accessor custom attributes are
rejected instead of dropping the row or synthesizing a different one. This keeps MethodSemantics
authoritative while RDL lacks syntax for attributes on property/event rows or their accessors.
The writer also scans every custom-attribute row before writing output and rejects attributes on
TypeRef, MemberRef, and TypeSpec parents, which are not represented by RDL declarations.
FieldLayout rows are preserved by metadata merge and remap. RDL unions represent only explicit
layouts where every instance field has offset zero; missing or nonzero offsets are rejected rather
than normalized to overlapping fields.

The same pre-emission pass now rejects accepted syntax that the encoder cannot represent.
`RDL0002` covers attributes on event shorthand, generic functions/callbacks/interface methods,
variadic callbacks/delegates/interface methods, generic bounds/defaults/attributes, attribute
constructors with returns or variadic parameters, and enum variants with payload fields.

Where metadata has a direct representation, the conversion now preserves it instead of rejecting
it. Custom attributes on typedefs, enum variants, GUID constants, and property-key constants
round-trip in both directions. The writer rejects equivalent winmd states that have no RDL
spelling, including generic methods, variadic non-function methods, generic parameter flags,
attributes on synthetic typedef, enum, or attribute-property fields. Callback and delegate
`Invoke` attributes use the explicit `#[invoke(Attribute(...))]` wrapper and round-trip without
moving the attribute to the generated type.

Metadata-to-metadata merge and remap copy custom-attribute value blobs without decoding them.
Named arguments therefore retain their field/property tag and exact serialized form. RDL still
decodes attributes when producing source because its syntax needs semantic values rather than raw
blobs.

Initial validation work:

1. Done: define initial syntax-level symbol keys and legal duplicate categories.
2. Done: add negative fixtures for duplicate properties, events, fields, methods, types,
   constants, functions, architecture variants, and parameter names.
3. Replaced: keep shared name resolution, but lower resolved declarations into the metadata builder
   rather than constructing a parallel resolved declaration tree.
4. Add structural custom-attribute validation. Do not enforce `AttributeUsageAttribute` target
   masks as base metadata validity: the committed Windows metadata uses API-contract attributes
   outside their declared masks. Target policy requires an explicit Windows profile.
5. Done: add checks for parsed syntax that is currently ignored or not represented, including
   attributes on event shorthand, method generics, and variadic interface methods.
6. Done: run the validator over the committed WinRT, Win32, and WDK RDL as a compatibility
   baseline.

### Lossless metadata conversion

RDL cannot serve as the reviewable source for arbitrary winmd files while metadata tables are
silently discarded. `windows-metadata` now reads Property, PropertyMap, Event, EventMap, and
MethodSemantics rows and preserves them through merge and namespace remapping. Those copy paths
also retain WinRT runtime-class methods, property constants, flags, signatures, and custom
attributes on properties and events.

The desired rule is:

> Every metadata fact is preserved, represented explicitly, or rejected with a diagnostic.

Initial losslessness work:

1. Done: add reader row types and traversal APIs for Property, PropertyMap, Event, EventMap, and
   MethodSemantics.
2. Done: preserve those tables and WinRT class methods through `windows-metadata` merge and remap.
   FieldLayout rows are also read and preserved through both paths.
3. Done: add focused winmd -> winmd tests that compare methods, property and event rows, flags,
   signatures, constants, custom attributes, and accessor semantics before and after conversion.
4. Add winmd -> RDL -> winmd tests for properties, events, class methods, return rows, and custom
   attributes on every supported parent. Property/Event row states without a lossless shorthand are
   now rejected and covered by negative tests. TypeRef, MemberRef, and TypeSpec attributes are also
   rejected before output because RDL has no declaration site for them.
5. Inventory every ECMA-335 table that the reader skips and classify it as preserved, irrelevant
   to Windows metadata, or unsupported with an error.
6. Replace known silent losses with errors until a lossless spelling or copy path exists.

The existing round-trip limits above should become machine-readable capabilities so the writer can
report the exact unrepresentable row rather than relying only on documentation.

### Diagnostics

Diagnostics should read naturally in a terminal and follow the useful parts of `rustc` output:

```text
error[RDL0001]: duplicate property `Name`
  --> src/widget.rdl:18:9
   |
12 |         Name: String;
   |         ------------ first declared here
...
18 |         Name: String;
   |         ^^^^ duplicate property
   |
   = help: remove one declaration or use distinct property names
```

A diagnostic should contain:

- Stable code, severity, message, and primary span.
- Zero or more labeled secondary spans.
- Notes and actionable help.
- Source name and source text supplied independently, including named in-memory inputs.
- A rendering API separate from the diagnostic data model.

The library should return structured diagnostics and leave color, terminal width, and final
rendering to the caller. The default renderer should support color auto-detection, `--color`, short
and human-readable formats, and one final error count. JSON output should be available for editors
and build systems.

Parser recovery reports several useful errors from one file without accepting an invalid tree for
metadata emission. It synchronizes at top-level and nested module-item boundaries, retains
successfully parsed items, and permits their semantic and metadata validation to finish. Any syntax
diagnostic still suppresses output. Finer recovery inside a malformed declaration can be added when
it produces useful diagnostics without guessing at the intended item shape.

`Reader::check_all` now returns a `DiagnosticReport`. Parsing continues across input files, and
validation collects independent errors across declarations and namespaces. Resolved method and
attribute-constructor signatures, unresolved types, import ambiguity, and generic arity are checked
before encoding. The report retains source text for named in-memory inputs and files, so terminal
rendering does not need to re-read them. Labels carry stable source IDs; name-based lookup remains
available only for an unambiguous name, while ID lookup resolves duplicate names exactly. Recovered
syntax diagnostics do not block validation of unaffected items, but they still prevent metadata
output.

### `riddle` command-line tool

`riddle` is a small binary crate built on the library APIs rather than the removed bindgen
argument forwarder. The binary contains argument parsing, terminal rendering, standard input, and
exit-code policy; parsing, validation, resolution, and metadata encoding remain library code.

The initial implementation provides `riddle check` and `riddle build`. Both accept repeated file
or directory inputs, repeated winmd references, standard input, and the default Windows metadata.
`Reader::check` runs the same pipeline as `Reader::write` without creating a winmd. `riddle check`
uses `Reader::check_all`, rendering every independent diagnostic with source locations, labeled
lines, notes, help, and a final error count. Invalid RDL uses exit code 1, while invalid command
lines use exit code 2.

An initial command set:

| Command | Purpose |
|---------|---------|
| `riddle check` | Parse, resolve, and validate RDL without writing a winmd |
| `riddle build` | Validate and compile RDL to winmd |
| `riddle fmt` | Format files, with `--check` for CI |
| `riddle dump` | Write canonical RDL from winmd |
| `riddle validate` | Validate an existing winmd and report unsupported or malformed metadata |

Future commands should use the same input and diagnostic behavior. Response files can be added if
Windows command-line limits become relevant.

### Formatting

The formatter validates a complete RDL file before formatting and returns a named diagnostic for
invalid input. A comment-aware lexer builds a lossless concrete syntax tree whose recursive group,
token, and comment nodes preserve author spellings and trivia. Layout consumes that tree directly,
so comments are not represented as fake identifiers during formatting. Formatting is idempotent
across comments between attributes, declarations, members, parameters, and closing delimiters.

`riddle fmt` formats files or directories in place and writes formatted standard input to standard
output. `riddle fmt --check` reports files that differ without changing them. All inputs are read,
parsed, and formatted before any file is replaced, so one invalid file cannot cause a partial
update.

The committed RDL corpus is a formatter gate: every file must parse, format, and produce identical
output on a second pass. Range formatting is intentionally deferred until editor integration can
reuse parsed source and define how comments at a range boundary are owned.

### Imports and name resolution

Imports are an authoring convenience for types and attributes. They are scoped to one RDL file,
including every metadata namespace declared by that file. Import paths therefore name absolute
metadata namespaces rather than paths relative to a module.

```rust
use Windows::Foundation::Point;
use Windows::Foundation::Collections as Collections;
use Windows::Foundation::{Point, Size as Extent};
use Windows::Foundation::Metadata::*;
```

Named imports, aliases, grouped imports, `self` within a group, and namespace globs are supported.
An imported name can identify a type or a namespace according to where it is used, so a namespace
alias such as `Collections` resolves `Collections::IIterable`. The same model applies to
attributes. Importing `Marker` resolves the metadata type `MarkerAttribute` when `#[Marker]` is
used.

Names resolve in this order:

1. Generic parameters, primitive types, and core spellings.
2. A declaration in the current namespace.
3. An explicit named or aliased import.
4. Namespace glob imports.
5. Core aliases such as `Type`, `GUID`, and `HRESULT`.

An explicit import can disambiguate competing globs. Multiple glob matches produce `RDL0004` with
a label for each candidate instead of selecting the first match. `RDL0003` reports an import whose
target is not a known namespace, type, or source-spelled attribute. Reusing one local import name
for different targets also produces `RDL0004`; repeating the same import is accepted.

Leading `crate`, `self`, and `super` imports are rejected because their meaning would depend on
which namespace in the file used them. `self` remains valid within a group such as
`use Windows::Foundation::{self as Foundation, Point};`.

The writer does not infer or emit imports. Canonical winmd -> RDL output uses qualified or
namespace-relative paths, so metadata round trips do not depend on import style. `RDL0006` reports
an import that never wins resolution. `RDL0007` reports a named import that a local declaration,
generic, or built-in shadows when the name is used. Warnings do not prevent checking or metadata
output. `#[allow(unused_imports)]` and `#[allow(shadowed_imports)]` suppress the corresponding
warning for one use declaration, including every member of a grouped use.

### Overload authoring

Overloads are a suitable convenience because the metadata already carries the distinction and the
author must still write every ABI method signature. RDL uses the public projected name in the
function declaration and requires the metadata ABI name in `#[overload(...)]`:

```rust
#[overload(Get)]
#[default_overload]
fn Get(&self, value: i32);

#[overload(GetWithString)]
fn Get(&self, value: String);
```

This spelling keeps all four relevant facts visible:

- The public projected name.
- The exact metadata method name.
- The full signature used to distinguish overloads.
- The selected default overload.

The reader lowers these pseudos directly to `OverloadAttribute` and
`DefaultOverloadAttribute`. The writer restores the same explicit spelling. Shared metadata
validation rejects duplicate projected signatures, more than one default, and a default without
overload metadata. Metadata names may repeat when their signatures differ; corpus validation
showed that this is common WinRT metadata rather than an error. The committed Windows metadata
corpus and `riddle check` exercise the rules. Automatic metadata-name generation remains deferred
because canonical output must expose any generated name before such a convenience is safe. This
work addresses
[`windows-rdl` overload attribute should be supported directly][rdl-overloads].

### Runtime-class authoring

MIDL3 runtime-class bodies are much shorter because the compiler synthesizes default, factory,
static, and composable interfaces. Those interfaces are real ABI and versioning boundaries. RDL
should not copy this design without showing authors what is generated and how it changes.

Investigate class conveniences with these constraints:

- No hidden interface is added without a stable, inspectable name.
- Constructor and static-member lowering is available through `riddle dump` or another expansion
  view.
- Interface assignment and method order are deterministic.
- Adding a constructor or member cannot silently change an existing interface ABI.
- Version and contract placement is explicit or derived by a documented, reviewable rule.
- Authors can always write the fully lowered interface form.

Compare three designs before implementation:

1. Keep classes explicit and add only diagnostics and templates for common factory patterns.
2. Add class-body syntax that requires authors to name the target interface for each member group.
3. Add MIDL3-like synthesis, but require an expansion manifest that is committed and checked for
   ABI changes.

Use the remaining MIDLRT-backed activation, constructor, overload, composable, `noexcept`, and
reference-parameter tests as study cases. Replacing MIDLRT in those tests is useful only when the
resulting RDL makes the ABI at least as reviewable as the current explicit interface form.

#### Findings from the MIDLRT metadata

`riddle dump` makes the generated shape visible. The remaining test cases establish these facts:

| Source construct | Generated metadata |
| --- | --- |
| Instance members | Exclusive interface plus duplicate methods and properties on the class |
| Parameterless constructor | `ActivatableAttribute` plus a class `.ctor` row |
| Parameterized constructor | Named factory interface, activation attribute, and class `.ctor` |
| Static member | Named static interface, `StaticAttribute`, and a static class method |
| Unsealed class | Non-sealed class, composable attribute, and a factory interface |
| Derived unsealed class | May receive an empty factory interface even without constructors |
| `noexcept` property | Attribute copied to both interface and class accessor methods |
| Reference parameter | Ordinary signature facts such as `&mut T`; no class convenience needed |
| Automatic overload | Generated projected names can collide across exclusive interfaces |

The automatic-overload case confirms that inferred names are not reliable ABI design. MIDLRT gives
both interfaces of one class projected names `Method` and `Method2`, producing class-level
collisions. Explicit RDL overload names avoid this.

Current RDL can state the important interface and factory boundaries directly, but class authoring
originally had three concrete weaknesses:

1. Every class was emitted sealed. `#[unsealed]` and `#[static_only]` now preserve all SDK class
   shapes.
2. The first listed interface became the default implicitly. `#[default]` now records the choice
   on the interface entry, while the first-entry fallback remains for older RDL.
3. Members from generic interfaces were not reconstructed. Concrete generic substitution now
   covers implemented instance interfaces.

#### Direction

Do not add MIDL3-style class member bodies. Keep instance, static, activation, and composable
interfaces explicit and named. This preserves method order, factory signatures, GUID boundaries,
versions, contracts, and overload names in reviewable source.

Add only conveniences that expose metadata facts rather than inventing interfaces:

1. Done: add explicit sealed, unsealed, and static-only class forms that map directly to type flags.
2. Done: add `#[default]` on class interface entries and stop treating list position as the only
   signal in canonical output.
3. Done: permit representable custom attributes on `InterfaceImpl` entries, including overridable
   metadata, instead of rejecting every interface-entry attribute.
4. Done: define and test deterministic reconstruction of redundant class projection rows from
   explicit interfaces and activation/static/composable attributes. `riddle expand` shows the
   reconstructed rows.
5. Add WinRT-profile validation for factory signatures and attribute/interface consistency.
6. Consider templates that emit the fully explicit interfaces and attributes, but do not make
   templates part of the language.

Projection-row reconstruction now covers methods, properties, events, semantics, parameter rows,
method attributes, overload names, and concrete generic substitution from local interfaces listed
by the class and interfaces named by `StaticAttribute`. It emits the MIDLRT public-final,
protected-final, protected-overridable, and static method flags, implementation flags, and calling
conventions. All 38,012 runtime-class MethodDef rows now match the merged SDK corpus. Property and
event maps are shared across projected interfaces on a class. All 17,211 class Property rows and
1,383 class Event rows also match. The committed WinRT RDL corpus is a compatibility gate, and a
process-level `riddle expand` test checks that the reconstructed ABI is visible.

Activation and composition attributes now reconstruct class `.ctor` rows from their explicit
factory interfaces. Activatable parameters are copied directly. Composable factories must end with
`Object, &mut Object`; those aggregation parameters are removed from the class constructor.
`CompositionType::Protected` produces a family constructor while public composition and activation
produce public constructors. A dump comparison confirms all 1,588 runtime-class constructors in
the generated Windows metadata match the merged SDK contracts.

Class shape and implementation metadata are also corpus-checked. All 4,516 runtime-class flags,
4,259 default-interface selections, and 6,014 InterfaceImpl attributes match the merged SDK
contracts. `riddle dump` prints attributes under each implemented interface.

Property, event, and accessor attributes now have lossless wrappers:
`#[property(Attribute)]`, `#[get(Attribute)]`, `#[set(Attribute)]`, `#[event(Attribute)]`,
`#[add(Attribute)]`, and `#[remove(Attribute)]`. Regenerating the canonical Windows RDL snapshot
found 673 accessor wrappers across 54 files that now state their target explicitly. Generic event
handler types and enum-field default flags are also preserved. `#[set_name(name)]`,
`#[add_name(name)]`, and `#[remove_name(name)]` retain accessor parameter names that differ from
the shorthand defaults.

Class Property and Event rows do not follow one derivable projection rule. Some SDK classes omit
association rows for their own projected accessors, while others carry rows associated with
inherited methods. Canonical output therefore uses `#[no_property(Name)]` and `#[no_event(Name)]`
on implemented interfaces, plus `#[no_static_property(Interface, Name)]` and
`#[no_static_event(Interface, Name)]` on classes, to suppress only the absent rows while retaining
the accessor MethodDef rows. `#[class_property(Name: Type, ...)]` and
`#[class_event(Name: Type, ...)]` preserve rows whose semantics reference raw, static, or inherited
accessors. Accessor paths name the owning class when the method is inherited. The writer emitted 87
explicit class associations across the SDK corpus. The merged SDK metadata is exactly the union of
the individual contract ABI facts, and the RDL round trip now matches every runtime-class method,
property, and event fact. `#[interface_event(Name: Type, ...)]` preserves the corresponding
interface Event row when remove precedes add and shorthand would change method order. All 34,219
Property and 2,785 Event rows across classes and interfaces now match.

The remaining member-method differences were separate from class projection. Delegates now
reconstruct their private runtime constructor and use an instance `Invoke` signature.
`#[invoke_no_new_slot]` preserves the five generic async delegates whose `Invoke` method omits
`NewSlot`. `#[runtime]` preserves runtime MethodImpl flags on foundation interfaces, and WinRT
attribute constructors use instance signatures with runtime implementation flags. All 72,109
MethodDef rows now match, so every MethodDef, Property, and Event fact in the merged SDK metadata
round-trips through RDL. Runtime-class projections also regenerate the MethodImpl mapping from
each projected class method to its interface declaration. The writer accepts only mappings that
this rule can reproduce and rejects other MethodImpl shapes.

`tool_winrt` enforces this comparison in-process after compiling the canonical RDL. It compares
qualified owners, method signatures and parameter names, method and implementation flags, calling
conventions, property signatures, event types, association semantics, and MethodImpl body and
declaration identities. Generation fails with missing and extra fact samples if the round trip
changes any member row.

Class association inference uses the same projection-shape model for implemented and static
interfaces. Matching includes concrete generic substitutions, property signatures, static versus
instance calling conventions, event types, accessor semantics, and accessor ownership. A property
with the same name but an incompatible signature is suppressed independently rather than being
mistaken for the class row.

### Initial implementation order

1. Done: introduce structured diagnostics and named source inputs without changing parser behavior.
2. Done: add duplicate-symbol validation and negative diagnostic fixtures.
3. Done: reject syntax and metadata states that are currently ignored or silently lost.
4. Done: add Property/Event/MethodSemantics reading and preserve those tables through merge and
   remap.
5. Done: restore a minimal `riddle check` and `riddle build` on the new library APIs.
6. Done: replace the formatter's silent parse fallback, preserve comments, and add `riddle fmt`.
7. Done: add named imports, aliases, grouped imports, and ambiguity diagnostics.
8. Done: implement explicit overload authoring after the semantic foundation described below.
9. Done: evaluate runtime-class conveniences after overload lowering is explicit and reviewable.

### Review after the initial implementation

Steps 1-7 removed several silent-loss paths and made the existing compiler usable from a terminal.
They also made the next architectural limit clearer: validation and resolution still run partly
inside `Encoder`, return the first error, and use syntax spellings for some semantic comparisons.
Adding overload authoring directly to that design would make the coupling worse.

The main findings are:

- **Validation:** Most passes stop at the first error. Collect independent semantic diagnostics
  before encoding.
- **Resolution:** Type and attribute lookup now share `Resolver` and produce canonical
  `metadata::Type` identities. Encode those facts directly rather than retaining a second
  declaration tree.
- **Duplicate checks:** Field, method, property, event, and implemented-interface identities use
  finalized metadata. Attribute-constructor signatures use resolved types. Top-level and
  source-only symbols remain syntax checks where metadata would lose the author-facing concept.
- **Encoding:** Some unresolved or invalid states are found while the winmd writer is being
  mutated. Make the metadata builder queryable and validate its finalized rows before output.
- **Diagnostics:** The data model supports labels, but source text is external and `riddle` renders
  one label at a time. Add a source registry, diagnostic collections, color/short/JSON rendering,
  and a final count.
- **Formatting:** Layout uses a lossless concrete syntax tree after semantic RDL validation.
  Recursive delimiter and comment nodes preserve grouped imports, comments, and author spellings.
- **Losslessness:** Known losses are rejected or documented, but the ECMA table and parent inventory
  is incomplete. Finish a machine-checked support inventory before claiming arbitrary winmd round
  trips.

Grouped imports are now formatted inline rather than as ordinary brace blocks. This is a useful
example of why each new authoring feature needs parser, resolver, diagnostic, formatter, CLI, and
round-trip coverage rather than parser coverage alone.

The next phase should proceed in this order:

1. Done: introduce a source registry, canonical type identities, and a shared name resolver.
   `DiagnosticReport` and `Resolver` provide these layers while preserving the current
   `Result<T, Error>` APIs as convenience wrappers.
2. Done: add row identities and a standalone validator to `windows-metadata`.
3. Done: add an in-memory writer-to-reader handoff and validate merge/remap output.
4. Done: lower RDL directly into metadata while recording row-to-source origins. Declaration,
   field, method, property, event, map, layout, and generated accessor rows are mapped and shared
   validation runs after encoding. Sorted rows use their mapped association as the diagnostic
   location because their row positions are assigned during finalization.
5. Done for the common validation baseline: move duplicate, custom-attribute structure, signature,
   layout, association, and overload checks onto the shared metadata validator where ECMA-335
   represents the fact. Attribute multiplicity
   is now checked for definitions with explicit usage contracts. `Validator` carries authored and
   reference indexes without merging them and is the boundary for future explicit profiles; target
   masks remain a profile decision. Constructor shape, calling convention, and the value-blob
   structure are now checked through one offset-reporting decoder shared by `Attribute::value` and
   validation; merge/remap remain on the raw blob path. `Char` is represented as its UTF-16 code
   unit. Null values use a typed `Value::Null` representation and round-trip through the RDL
   `null` spelling. Boxed values preserve their embedded serialization type as
   `Value::Boxed` and use the explicit RDL `boxed(type, value)` spelling. Typed arrays use
   `Value::Array`, including empty, null, named, and boxed arrays, and use ordinary RDL array
   literals. Enum decoding now resolves non-`i32` backing types through the authored or reference
   metadata index.
   Constructor parameter types are checked against the ECMA custom-attribute serialization types
   before the value blob is decoded. `Attribute::try_args` retains named field/property tags, so
   validation can check member existence, types, and duplicate named arguments against local or
   referenced attribute definitions. Named fields must be public writable instance fields; named
   properties require a matching public instance setter.
   Common signature validation now rejects illegal `void` field, parameter, property, and array
   element types and rejects `HASTHIS` on static methods. It does not require `HASTHIS` on instance
   methods because canonical WinRT metadata commonly omits it. RDL global functions now encode a
   static signature rather than inheriting the instance default. Win32 native-typedef wrappers
   retain their established `Value: void` representation. Profile-specific policy remains
   deferred until a profile is selected explicitly.
6. Done: implement explicit overload authoring as transparent metadata lowering. RDL keeps the
   projected name in the declaration, requires the metadata name in `#[overload(...)]`, preserves
   the spelling through winmd-to-RDL output, and validates group identities in shared metadata.
7. Done: split shared validation into focused attribute, member, association, method, and layout
   modules behind one private context. The public `Validator` API and metadata-first boundary remain
   unchanged.
8. Done: upgrade `riddle` metadata inspection. `riddle validate` applies
   the shared validator directly to existing winmd files and directories while keeping references
   separate. `riddle expand` compiles RDL in memory and prints finalized types, signatures, flags,
   properties, events, layouts, attributes, and overload names so lowered ABI is inspectable before
   runtime-class conveniences are considered.
9. Done: move formatting to a lossless concrete syntax tree and preserve comments as syntax nodes.
   Range formatting remains deferred until editor use justifies boundary and cache semantics.
10. Done: evaluate runtime-class conveniences using `riddle dump` on the remaining MIDLRT-backed
    activation, constructor, static, composable, overload, `noexcept`, and reference-parameter
    cases. Keep interfaces explicit; address class flags, default-interface spelling, interface
    attributes, and projection-row fidelity before adding any class-body convenience.
11. Done: projection-row reconstruction covers explicit implemented interfaces plus
    `StaticAttribute` interfaces, including concrete generic substitution, methods, properties,
    events, semantics, parameter names, attributes, flags, and overload names. All 38,012 class
    MethodDef, 17,211 Property, and 1,383 Event rows match. Activatable/composable factory
    transformation matches all 1,588 SDK runtime-class constructors. Class flags, default
    interfaces, and InterfaceImpl attributes also match the full SDK corpus.
12. Done: preserve the remaining non-class member rows. Delegate constructors and calling
    conventions, runtime interface MethodImpl flags, interface events with reverse accessor order,
    and WinRT attribute constructors now round-trip. All 72,109 MethodDef, 34,219 Property, and
    2,785 Event rows match the merged SDK metadata.

### Maturity and refactoring plan

The initial implementation established metadata fidelity and the authoring model. The next work
should reduce implementation complexity, finish the supported metadata boundary, improve
diagnostics, and establish performance gates before adding more language conveniences.

#### Metadata completeness

1. Done: add one exhaustive ECMA-335 table registry that classifies every table as preserved,
   regenerated, unsupported, or irrelevant to Windows API metadata. The metadata reader consults
   this registry, and tests require all 45 standard table IDs to remain classified.
2. Done: strict metadata reads reject every present unsupported table with a structured diagnostic
   that names the table and row count. `MethodImpl`, found in MIDLRT and Windows App SDK metadata,
   is read, written, and preserved through merge/remap. RDL regenerates runtime-class projection
   mappings from implemented interfaces and rejects mappings outside that model. A test scans every
   committed winmd and requires its tables to pass the strict inventory.
3. Done: `ValidationProfile` separates Common, Win32, WinRT, and combined Windows
   validation. Common remains the standalone metadata default. RDL compilation infers Win32/WinRT
   from the explicit source profile, and `riddle validate --profile` selects policy for existing
   metadata. WinRT rules check type flags, interface shape, conflicting default interfaces, and
   activation/composition/static factory types. They also enforce `AttributeUsageAttribute` target
   masks while accepting metadata propagation from WinRT types to owned rows and between
   properties/events and their accessor methods. API contracts must be structs with one contract
   version; contract versions must be nonzero and resolve type or string names to an API contract.
   A member or interface implementation cannot precede its owning type when both name the same API
   contract. Win32 rules check type flags and require `PInvokeImpl` and `ImplMap` to agree.
   P/Invoke methods must be static, use `platformapi`, `cdecl`, or `fastcall`, and use `cdecl` for
   variadic signatures. Native structs must select sequential or explicit layout. Explicit unions
   may use the SDK's all-implicit-zero convention or a complete set of field-layout rows, but not a
   partial set.
4. Done: complete the custom-attribute value model. Typed null, boxed, and array values round-trip
   through winmd and RDL without dropping the serialized type.
5. Done: generic static-interface projection substitutes concrete arguments through methods,
   properties, and events. Generic `System.Type` custom-attribute values retain their recursive
   runtime type names in the blob and use Rust turbofish syntax in RDL attribute expressions.
   Generic arity mismatches and unresolved open arguments produce source diagnostics.

#### Refactoring

1. Done: split source loading and profile propagation into `reader/source.rs`, the public compile
   facade and pipeline orchestration into `reader/compile.rs`, source-origin diagnostics into
   `reader/origin.rs`, and shared parser helpers into `reader/syntax.rs`. Import validation now
   lives with the other source validators in `reader/validate.rs`; indexing and resolution remain
   in their dedicated modules. `reader/mod.rs` retains the shared lowering `Encoder` because the
   item-specific lowerers extend it directly, avoiding wider field visibility or a forwarding
   layer. `Reader` remains the stable public facade.
2. Done: `reader/projection.rs` owns member ownership flags, projection maps, projected-property
   state, class projection inputs, generic substitution, exclusive-interface filtering, and the
   common projected-method emitter. Instance and static class reconstruction now build a small
   `ClassProjection` and use one lowering path; method, property, event, and association rows still
   flow through the shared interface-member lowering code.
3. Done: `windows-metadata` finalization produces canonical table, string, and blob streams once.
   RDL validates that queryable image directly with its encoding references, then packages the same
   streams as winmd bytes only at the output boundary. Existing metadata and RDL round-trip suites
   prove the serialized output and row identities remain unchanged.
4. Done: formatting consumes a lossless concrete syntax tree after semantic RDL validation.
   Comments are first-class nodes and author-significant token spellings remain intact. A committed
   corpus test requires parseability and idempotence. Range formatting remains deferred until
   editor integration needs it.
5. Done: the `windows-metadata` validator is organized by metadata fact, with Win32 and WinRT
   policy layered over Common validation. WinRT overload-name and default-overload rules now live
   in the WinRT profile rather than common method validation. Common validation deliberately keeps
   the `NativeTypedefAttribute` `Value: void` encoding exception because standalone validation of
   Windows metadata must accept that type representation. Boundary tests prove Common ignores
   WinRT overload policy while the WinRT profile enforces it.

#### Diagnostics and tooling

1. Done: every registered input receives a stable `SourceId`, and diagnostics and labels retain it
   through parsing, source validation, resolution, and metadata-origin mapping.
   `DiagnosticReport::source_by_id` resolves duplicate display names without guessing. The parser
   recovers at top-level and nested module-item boundaries; recovered syntax errors allow
   unaffected items to complete semantic and metadata validation while still suppressing output.
2. Done: `riddle` uses one renderer for RDL, metadata-validation, formatting, and single-error
   diagnostics. `--format human`, `short`, or `json` selects labeled source output, one-line
   diagnostics, or a machine-readable document with error/warning counts. `--color auto`,
   `always`, or `never` controls ANSI output; auto mode honors `NO_COLOR`, and JSON is always
   uncolored. Error and warning severities share one final summary.
3. Done: import candidates record usage at the point type or attribute resolution selects them.
   `RDL0006` warns about unused named, namespace, and glob imports. `RDL0007` distinguishes a named
   import that cannot win a used name because local, generic, or built-in resolution takes
   precedence. Warnings do not block output and may be suppressed per use declaration with
   `#[allow(unused_imports)]` or `#[allow(shadowed_imports)]`.
4. Deferred after an ownership audit: parsed metadata references are owned and could be cached,
   but the RDL `Index<'a>` stores direct `&File` and `&Item` references. Retaining that merged index
   while replacing one source would require a self-referential workspace or stale references.
   Editor work first needs owned `FileId`/`ItemId` handles, per-file symbol indexes, and a separate
   merged lookup view. Until then, `Reader::check_all` remains batch analysis and is not presented
   as incremental.

#### Verification and performance

1. Done: `crates/libs/rdl/fuzz` contains `cargo-fuzz` targets for full RDL checking, formatter
   validity/idempotence, strict metadata reading plus validation, and fallible custom-attribute
   decoding. Text targets have representative RDL seeds. Metadata targets generate a valid
   attributed winmd in memory and apply deterministic input mutations, while non-seed inputs still
   exercise arbitrary bytes. Minimized failures are committed to the target corpus and promoted to
   normal regression tests when a stable higher-level spelling exists.
2. Done: `test_rdl` generates representative Win32 and WinRT metadata and passes both files through
   .NET `System.Reflection.Metadata`. The test compares stable type, field, and method identities
   rather than tool-specific text. It also passes the WinRT file through the Windows SDK C++/WinRT
   projection tool and requires the projected interface and runtime class identities. The C# build
   and projection output stay under Cargo's output directory, so the test does not modify the
   source tree.
3. Done: `tool_rdl_bench` measures full WinRT RDL checking, existing-winmd reading and validation,
   formatting, dumping, and rebuilding with one warmup and configurable repeated samples. The
   contributor documentation records exact input sizes and the initial same-host medians. The
   benchmark is not an automatic cross-machine CI gate; a regression must repeat across at least
   three same-host runs and be profiled before a reviewed limit changes.
4. Done for the current baseline: phase profiling identified class/interface projection as the
   dominant check/build cost, attribute/type/WinRT policy checks as the validation cost, and
   projection/formatting/file writes as the dump cost. WinRT attribute-target validation now caches
   each definition's decoded usage mask, reducing that phase by about 35-40% on the reference
   machine. The serialize-and-reparse validation boundary was removed earlier. Further encoding or
   dump optimization needs call-stack or allocation evidence, and editor performance still requires
   incremental parsing and validation.

Maturity requires all metadata accepted by the reader to be preserved, regenerated by a documented
rule, or rejected explicitly; every validator rule reachable from RDL to have an end-to-end
diagnostic test; the committed corpora and external consumers to accept generated metadata; and
performance benchmarks to remain within reviewed limits.

Validation testing follows the layered strategy documented for [`riddle`](riddle.md): synthetic
metadata for unauthorable states, committed-corpus compatibility, RDL lowering tests, and
`riddle check` process tests for source-visible diagnostics. Every validator rule that accepted RDL
can trigger should have an end-to-end test rather than only a table-level fixture.

### Maturity closure

The implementation and maturity roadmap above is complete. Close the branch with these final
verification steps:

1. Done: every shared validator rule reachable from authored RDL has source-level diagnostic
   coverage. This includes duplicate definitions and members, invalid field/method/property types,
   duplicate fixed and named attribute arguments, non-multiple attributes, WinRT overload and
   contract policy, attribute target masks, contract version ordering, and variadic Win32 calling
   conventions. Rules for malformed maps, layouts, semantics, parameter sequences, flags, custom
   attribute constructor metadata, named properties, missing references, and inconsistent
   P/Invoke rows remain synthetic `test_metadata` cases because RDL parsing, resolution, or
   lowering cannot emit those states.
2. Done: the complete WinRT, Win32, WDK, and round-trip generation pipelines pass. WinRT generation
   now preserves the SDK MethodImpl mappings and compares them as semantic facts. Exclusion
   attributes use canonical ordering so merged and rebuilt winmd inputs produce identical RDL.
3. Done: stale MethodImpl and generic static-interface limitations were removed, and the design
   description now matches the implemented projection and validation boundaries.
4. Done: record the final readiness assessment below.

### MIDL replacement readiness

`windows-rdl` is ready to replace MIDL for the Windows metadata generation paths exercised by this
repository. WinRT, Win32, and WDK sources compile and round-trip through the full generators. The
WinRT pipeline compares every method, property, event, association, and MethodImpl fact against the
merged SDK contracts. Representative Win32 and WinRT outputs are also accepted by .NET
`System.Reflection.Metadata`, and WinRT output is accepted by the Windows SDK C++/WinRT tool.

The supported boundary is explicit. Metadata is either preserved, regenerated by a documented
projection rule, or rejected with a diagnostic. RDL-authored validator failures have end-to-end
source tests; malformed metadata states that RDL cannot emit remain covered by synthetic metadata
tests. Generic interfaces, custom attributes, runtime-class projections, overloads, contracts,
properties, events, semantics, and projected MethodImpl mappings are covered by round-trip or
diagnostic tests.

The architecture remains a direct source -> syntax -> metadata -> validation pipeline with shared
metadata validation and source-origin diagnostics. Release benchmarks on the reference machine
remain about 1.9 seconds for checking or rebuilding the 343-file WinRT corpus, 0.5 seconds for
formatting, and 1.3 seconds for dumping. These are same-host references rather than portable CI
limits. Profiling found no release-blocking cost; further optimization should follow measured call
stacks or allocations.

There is no known metadata-fidelity blocker for replacing the repository's current MIDL generation
work. Incremental parsing and validation for editor workloads remain deferred. They affect IDE
latency and language-server design, not deterministic command-line generation or metadata
correctness.

[rdl-overloads]: https://github.com/microsoft/windows-rs/issues/4166
