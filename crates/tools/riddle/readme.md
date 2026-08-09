# Riddle

Riddle checks, compiles, and formats RDL API descriptions and validates existing metadata.

```text
riddle check api.rdl
riddle build api.rdl --out api.winmd
riddle expand api.rdl
riddle dump api.winmd
riddle validate api.winmd
riddle validate --profile winrt api.winmd
riddle check --format short --color never api.rdl
riddle check --format json api.rdl
riddle fmt api.rdl
riddle fmt --check api.rdl
```

RDL inputs may be files, directories containing `.rdl` files, or `-` for standard input.
`riddle expand` prints the lowered metadata types, members, signatures, flags, properties, events,
and custom-attribute identities. `riddle dump` prints the same view for existing `.winmd` files.
`riddle validate` accepts `.winmd` files and directories. Use `--reference <path>` for additional
metadata references. The default Windows metadata is available for type and attribute resolution
unless `--no-default` is specified. Validation profiles are `common`, `win32`, `winrt`, and
`windows`; common is the default.

Diagnostic formats are `human`, `short`, and `json`. Human output includes labeled source lines;
short output emits one location line per diagnostic; JSON emits one document with `diagnostics`
and an error/warning `summary`. Color policy is `auto`, `always`, or `never`. JSON output never
contains ANSI escapes. `riddle check` prints import warnings without returning a failure status.
