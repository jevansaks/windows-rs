## windows-rdl

The [windows-rdl](https://crates.io/crates/windows-rdl) crate compiles **RDL** (Rust Definition
Language) - a Rust-like text format for describing Windows APIs - into ECMA-335 `.winmd` metadata,
and back again.

* [Getting
  started](https://github.com/microsoft/windows-rs/blob/master/docs/crates/windows-rdl.md)

Start by adding the following to your Cargo.toml file:

```toml
[dependencies.windows-rdl]
version = "0.100"
```

Use the `reader` to compile `.rdl` source into a `.winmd`, and the `writer` to regenerate canonical
`.rdl` from a `.winmd`:

```rust,no_run
windows_rdl::reader()
    .input("example.rdl")
    .output("example.winmd")
    .write()
    .unwrap();

windows_rdl::writer()
    .input("example.winmd")
    .output("example.rdl")
    .write()
    .unwrap();
```

`item_names(path, namespace)` returns names declared directly in one exact namespace.
`qualified_item_names(path)` parses the file once and returns typed namespace/name identities for
every namespace in the file.

Use `writer().partition(map)` for metadata whose item names are unique in a flat namespace. Use
`writer().partition_qualified(map)` when the same short name can occur in different namespaces.
The flat and qualified partition modes cannot be combined on one writer.

Inline structs and unions compile to nested TypeDefs. Their field signatures retain the full
enclosing TypeRef chain, and the writer uses that identity when reconstructing inline RDL.

Reference resolution keeps exact generic arity and nested ownership. Local RDL definitions win
over same-named reference definitions, including forward references. Compiler-generated const
modifiers and callback metadata use the core-library identity rather than a module-scoped fallback.
Those core references remain distinct from local types with the same qualified name, and callback
constructor signatures retain the exact core TypeRef selected by the compiler.

`implib::read_contracts` reads COFF short-import records without collapsing the native linker
symbol, DLL, raw machine value, import kind, and ordinal or name mode. Callers can reject contracts
from a different architecture before emitting metadata. Archives can contain records for more than
one machine, so selection filters each record by the exact target instead of requiring a
single-machine archive. `ImportContract::resolve_entry_point` returns an `ImportEntryPoint` ordinal
or applies the PE/COFF name transformation while leaving the source contract unchanged. A hint on a
named import is not an ordinal, and an empty transformed name is an error. The older `implib::read`
symbol-to-DLL view remains available for callers that do not emit native entry-point metadata.
Callers must report a selected function with no exact-machine code contract instead of borrowing
another machine's record or silently omitting the function.
