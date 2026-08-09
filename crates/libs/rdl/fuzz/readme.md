# windows-rdl fuzzing

Install `cargo-fuzz`, then run one target from this directory:

```text
cargo install cargo-fuzz
cargo fuzz run rdl_parser
cargo fuzz run rdl_formatter
cargo fuzz run metadata_reader
cargo fuzz run attribute_decoder
```

The metadata targets treat the committed `seed` corpus input as an unmodified valid winmd and
interpret bytes after that prefix as deterministic mutations. Other inputs are parsed directly or
applied as mutations, so both arbitrary and near-valid metadata are covered.

When a failure is found, minimize it with `cargo fuzz tmin`, copy the minimized input to the
target's `corpus/<target>/regression-*` path, and add a normal unit or integration test when the
failure has a stable higher-level spelling.
