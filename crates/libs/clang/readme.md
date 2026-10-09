## windows-clang

The [windows-clang](https://crates.io/crates/windows-clang) crate extracts declarations from C and
C++ source with libclang and emits [RDL](https://crates.io/crates/windows-rdl). It is the
header-facing stage of the Windows metadata pipeline:

```text
headers -> windows-clang -> RDL -> windows-rdl -> WinMD
```

Add the crate to your Cargo.toml:

```toml
[dependencies.windows-clang]
version = "0.100"
```

For ordinary header-to-RDL generation, configure and run the high-level builder:

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

The builder reads inputs and references, invokes the extractor, emits RDL, and writes the output.
Use [`Input`][input], `extract`, and `EmitOptions` directly when a generator needs to inspect or
combine immutable snapshots before emission.

`EmitOptions::native_imports` accepts typed DLL entry-point contracts keyed by the native C linker
symbol. It keeps an ordinal such as `#660` or an exact exported name separate from the projected
metadata function name. The existing `library` and `libraries` options remain available for
symbol-to-DLL-only callers.

`Clang::parallelism` bounds concurrent parsing of original translation units. The lower-level
`extract_with_options` and `extract_partitioned_with_options` functions accept the same setting
through `ExtractionOptions`. Results, diagnostics, and inclusion records retain extraction input
order. Zero and one select serial parsing.

`Input::with_excluded_source_dirs` and `Clang::exclude_path` omit declarations spelled beneath
toolchain resource directories while retaining those files in inclusion provenance.

`Snapshot::included_files` returns the normalized physical files visited in each original
translation unit, including headers that produced no extracted fact or constant. Records retain
the corresponding `Input::name` identity and have deterministic per-input ordering.

An aggregate translation unit can be routed after extraction with `HeaderPartitionPolicy` and
`Snapshot::into_header_partition_plan`. This consuming path moves the extracted snapshot into the
plan and avoids retaining a cloned copy during planning. `Snapshot::plan_header_partitions` remains
available when the caller must keep using the original snapshot. Both paths produce the same plan.
The policy maps traversed physical headers to logical `RootPartition` candidates. Input-qualified
entries support the same physical header parsed under different compile definitions without making
the extraction input a logical partition. Header selectors are case-insensitive path suffixes;
emitted partition headers and owner settings use the resolved physical source path. Named header
overrides replace the default candidates for selected declarations, including input-qualified
compile variants. Call the consuming `emit_with_options` for normal generation; it audits
internally and refuses to emit a dirty plan. Use `audit` only when a caller needs the report without
emission because auditing before emission repeats planning and clones the snapshot.
`RootPartition::with_preserved_auto_function_pointer_level` restores the implicit pointer level for
uses of a bare function typedef. A pointer typedef promoted to a delegate already contains that
source pointer, so its uses keep only their authored outer pointer depth.
Dependency declarations remain available for closure without becoming public roots. When
equivalent compile variants exist, an owner whose typedef projection reaches an excluded type is
omitted if another unsuppressed projection can own that output.
An `associated_enum` annotation on a selected root adds its matching enum provider from compatible
redeclarations to closure, including redeclarations from another aggregate extraction input.
Dependency-only providers inherit that root's owner settings; providers already owned by a
traversed header keep their existing route.

Headers may transport metadata policy with Clang `annotate` attributes whose payload begins with
`win32metadata:`. The extractor validates this vocabulary and carries it through `Snapshot`
planning into RDL and WinMD attributes. Unknown, malformed, or misplaced annotations are errors.
Callback conventions come from the callback declarator. Pointer aliases inherit from the exact
bare function typedef they alias, not from referenced return, parameter, or record types.
The valueless `native_opaque` marker may appear on a named C++ class definition. It preserves only
the class's nominal identity as an empty type for pointer and reference use. Fields, methods, base
classes, layout, and native inheritance are not projected, and any by-value use is an error.
Captured SAL keeps ordinary NUL-terminated strings and double-NUL multistrings as separate
parameter facts through `ParamAnnotation`.

Pointer projection is chosen per use rather than by header ownership. Ordinary `LPVOID` and `PVOID`
uses become raw pointers; nested pointers retain a native alias when WinMD needs its const boundary.
Noncanonical pointer aliases such as `HANDLE` stay named. Explicit SAL directions and buffer counts
survive lowering, while an unannotated mutable pointer uses the raw-pointer output default.

Set `WINDOWS_CLANG_TIMINGS=1` to write structured extraction, planning, and emission measurements
to stderr without changing the generated RDL.

The caller owns libclang installation, compiler arguments, package versions, import-library
discovery, architecture merging, output promotion, and RDL-to-WinMD compilation. See the [crate
documentation][docs] for both APIs and the extraction model.

[input]: https://docs.rs/windows-clang/latest/windows_clang/struct.Input.html
[docs]: https://github.com/microsoft/windows-rs/blob/master/docs/crates/windows-clang.md
