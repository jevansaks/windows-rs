## windows-metadata

The [windows-metadata](https://crates.io/crates/windows-metadata) crate reads and writes the
ECMA-335 metadata format used by .NET, WinRT, and Win32 metadata.

* [Getting
  started](https://github.com/microsoft/windows-rs/blob/master/docs/crates/windows-metadata.md)

Start by adding the following to your Cargo.toml file:

```toml
[dependencies.windows-metadata]
version = "0.100"
```

Query a type with the metadata reader:

```rust,no_run
use windows_metadata::*;

let index = reader::Index::read("Windows.winmd").unwrap();

let def = index.expect("Windows.Foundation", "Point");
assert_eq!(def.namespace(), "Windows.Foundation");
assert_eq!(def.name(), "Point");

let extends = def.extends().unwrap();
assert_eq!(extends.namespace(), "System");
assert_eq!(extends.name(), "ValueType");

let fields: Vec<_> = def.fields().collect();
assert_eq!(fields.len(), 2);
assert_eq!(fields[0].name(), "X");
assert_eq!(fields[1].name(), "Y");
assert_eq!(fields[0].ty(), Type::F32);
assert_eq!(fields[1].ty(), Type::F32);
```

`TypeDef::qualified_name` and `TypeRef::qualified_name` preserve nested identity as a root
namespace plus a slash-separated enclosing path. The physical `namespace` and `name` accessors
continue to return the exact row values.

`writer::File::TypeRef` resolves supplied reference definitions by their exact metadata name,
including generic arity and nested paths. Local definitions keep module scope, including forward
definitions completed before `into_stream`. Use `CoreTypeRef` only for compiler-known core-library
types; it prefers an exact supplied definition and otherwise uses the legacy `mscorlib` identity.
Core and inferred references use separate physical rows, so a local type with the same qualified
name cannot be retargeted by later core emission. `MemberRefWithTypeRefs` pins exact physical
TypeRef rows in compiler-authored member signatures without changing ordinary name resolution.
