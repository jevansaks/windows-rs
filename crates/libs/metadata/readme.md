## Low-level metadata library for ECMA-335

The [windows-metadata](https://crates.io/crates/windows-metadata) crate reads and writes the
ECMA-335 metadata format used by .NET, WinRT, and Win32 metadata.

* [Getting started](https://github.com/microsoft/windows-rs/blob/master/docs/readme.md)
* [Samples](https://github.com/microsoft/windows-rs/tree/master/crates/samples)
* [Releases](https://github.com/microsoft/windows-rs/releases)

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

`reader::File::read` and `reader::File::new` retain the compact `Option` API. Use `try_read` or
`try_new` when an error must distinguish malformed metadata, I/O failure, and an unsupported table.
Unsupported-table errors include the table name and row count.

`Field::layout()` returns the optional ECMA-335 `FieldLayout` row and its byte offset.

ECMA-335 associates a method's `Param` rows with signature positions through the one-based
`Param.Sequence` column. Use `MethodDef::params_by_sequence(signature.types.len())` for semantic
association. The result keeps a separate Sequence 0 return row and one `Option<MethodParam>` per
signature parameter, so absent and sparse rows do not truncate the signature. Duplicate or
out-of-range sequences are reported as errors. `MethodDef::params()` remains physical table-order
iteration for lossless metadata copying.

`MethodParam::direction()` reports the literal `In`/`Out` combination as `ParamDirection`,
including `Unspecified`. `is_optional`, `is_reserved`, and `is_retval_attribute` expose separate
facts. They do not infer direction from the type or treat a reserved parameter as optional.

Use `validator::validate(&index)` to check duplicate metadata identities and malformed table
associations. Each error carries a `reader::RowId`, which metadata producers can map to their own
source locations.

Use `validator::Validator::profile` to add Win32, WinRT, or combined Windows policy checks.
`ValidationProfile::infer` selects a profile from authored type flags. WinRT policy includes
attribute target masks, the target propagation used by projected properties and events, and API
contract/version consistency, including owner/member version ordering within one contract.
Win32 policy checks P/Invoke mappings and native calling conventions.
It also checks sequential/explicit native struct layout and rejects partial explicit offset tables.

`Value::Null(Type)`, `Value::Boxed`, and `Value::Array` preserve typed custom-attribute arguments.

`writer::File::finalize()` produces canonical metadata streams that can be validated directly and
then packaged as a winmd with `FinalizedFile::into_stream()`. `finish()` and `into_index()` retain
the reader-facing convenience APIs. All paths avoid temporary files.
