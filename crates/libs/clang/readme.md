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

An aggregate translation unit can be routed after extraction with `HeaderPartitionPolicy` and
`Snapshot::plan_header_partitions`. The policy maps traversed physical headers to logical
`RootPartition` candidates. Input-qualified entries support the same physical header parsed under
different compile definitions without making the extraction input a logical partition. Header
selectors are case-insensitive path suffixes; emitted partition headers and owner settings use the
resolved physical source path. Call the consuming `emit_with_options` for normal generation; it
audits internally and refuses to emit a dirty plan. Use `audit` only when a caller needs the report
without emission because auditing before emission repeats planning and clones the snapshot.
Dependency declarations remain available for closure without becoming public roots. When
equivalent compile variants exist, an owner whose typedef projection reaches an excluded type is
omitted if another unsuppressed projection can own that output.

Headers may transport metadata policy with Clang `annotate` attributes whose payload begins with
`win32metadata:`. The extractor validates this vocabulary and carries it through `Snapshot`
planning into RDL and WinMD attributes. Unknown, malformed, or misplaced annotations are errors.

Set `WINDOWS_CLANG_TIMINGS=1` to write structured extraction, planning, and emission measurements
to stderr without changing the generated RDL.

The caller owns libclang installation, compiler arguments, package versions, import-library
discovery, architecture merging, output promotion, and RDL-to-WinMD compilation. See the [crate
documentation][docs] for both APIs and the extraction model.

[input]: https://docs.rs/windows-clang/latest/windows_clang/struct.Input.html
[docs]: https://github.com/microsoft/windows-rs/blob/master/docs/crates/windows-clang.md
