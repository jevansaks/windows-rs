## RDL parser library and ECMA-335 generator

The [windows-rdl](https://crates.io/crates/windows-rdl) crate compiles **RDL** (Rust Definition
Language) - a Rust-like text format for describing Windows APIs - into ECMA-335 `.winmd` metadata,
and back again.

* [Samples](https://github.com/microsoft/windows-rs/tree/master/crates/samples)
* [Releases](https://github.com/microsoft/windows-rs/releases)

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

Use `.check()` to run the same parse, validation, resolution, and encoding pipeline without
writing a `.winmd`. Use `.check_all()` to collect independent diagnostics from every input.
Use `.bytes("assembly-name")` to retrieve the finalized metadata in memory.

Use `.reference("dependency.winmd")` when the RDL refers to types defined by another metadata file.
Use `.input_text(source)` or `.input_texts(sources)` for RDL already in memory. Use
`.reference_default()` for the standard Windows metadata.

Use `.input_text_named("schema.rdl", source)` or `.input_texts_named(sources)` when in-memory source
names should appear in diagnostics. `Diagnostic` stores a severity, optional code, source labels,
notes, and help. Each registered input receives a `SourceId`, so duplicate display names still map
labels to the correct text through `DiagnosticReport::source_by_id`. The parser recovers at module
item boundaries and continues validating unaffected items. `Error` is a small owned wrapper that
dereferences to its `Diagnostic`.

The reader reports `RDL0001` for duplicate symbols, `RDL0002` for accepted syntax that cannot be
represented in metadata, `RDL0003`/`RDL0004` for import failures, and `RDL0005` for generic-arity
errors. `RDL0006` warns about unused imports, and `RDL0007` warns when local resolution shadows a
named import. Suppress one use declaration with `#[allow(unused_imports)]` or
`#[allow(shadowed_imports)]`. The writer likewise rejects metadata forms that have no lossless RDL
spelling rather than emitting incomplete source.

The `riddle` binary provides the same operations from a terminal:

```

Parser, formatter, metadata-reader, and custom-attribute decoder fuzz targets live in `fuzz/`.
Run them with `cargo fuzz`; minimized failures belong in the corresponding committed corpus
directory and should also receive a normal regression test when possible.text
riddle check example.rdl
riddle build example.rdl --out example.winmd
riddle fmt example.rdl
riddle fmt --check example.rdl
```

`formatter::format` and `formatter::format_named` validate complete RDL source and return a
diagnostic on invalid input. A lossless concrete syntax tree preserves regular comments,
documentation comments, and author-significant token spellings.

The winmd writer matches `Param` rows by ECMA-335 `Param.Sequence`, not table order. Sparse methods
still emit every signature parameter, using `pN` and the reader's type-based default direction when
a row is absent. Sequence 0 return attributes are emitted on the return type. Duplicate and
out-of-range sequences are errors.

The writer reads raw direction and optionality through `MethodParam::direction()` and
`is_optional()`. Reserved, retval, and count attributes remain separate pseudos/custom attributes;
the metadata layer does not merge them with projection policy.

Canonical output spells the input direction as `#[in]`; the reader also accepts Rust's raw
identifier spelling, `#[r#in]`.

WinRT overloads keep the projected name in the function declaration and spell the metadata method
name explicitly:

```rust
#[overload(Get)]
#[default_overload]
fn Get(&self, value: i32);

#[overload(GetWithString)]
fn Get(&self, value: String);
```

This emits `OverloadAttribute("Get")` on both methods and `DefaultOverloadAttribute` on the first.
Canonical winmd-to-RDL output retains both pseudo-attributes, so the metadata names remain visible
in review.

Runtime classes reconstruct the redundant class MethodDef, Property, Event, and MethodSemantics
rows from explicitly listed local interfaces, including concrete generic arguments. Interfaces
named by `StaticAttribute` produce static rows and substitute concrete generic arguments. RDL does
not invent either interface.
`ActivatableAttribute` and
`ComposableAttribute` factory methods produce the redundant class constructors. Composable
factories must end with the explicit `Object, &mut Object` aggregation parameters; those two
parameters are not part of the projected constructor.

Classes are sealed by default. `#[unsealed]` emits a non-sealed runtime class, while
`#[static_only]` emits the abstract-and-sealed shape used by static-only classes. Class interface
entries use `#[default]` to state the default interface explicitly; other attributes on the entry
are emitted on the InterfaceImpl row. `OverridableAttribute` and `ProtectedAttribute` also select
the corresponding protected class method flags.

Property and event metadata can carry attributes at three different levels. Wrapper attributes keep
the target visible: `#[property(Attribute)]`, `#[get(Attribute)]`, `#[set(Attribute)]`,
`#[event(Attribute)]`, `#[add(Attribute)]`, and `#[remove(Attribute)]`. Marker `#[get]` and
`#[set]` still select a get-only or set-only property. `#[set_name(name)]`,
`#[add_name(name)]`, and `#[remove_name(name)]` preserve nonstandard accessor parameter names.
Canonical output uses `#[no_property(Name)]`, `#[no_event(Name)]`,
`#[no_static_property(Interface, Name)]`, and `#[no_static_event(Interface, Name)]` when the class
has projected accessor methods without the corresponding Property or Event association row.
`#[class_property(Name: Type, get = Owner::method, set = Owner::method)]` and
`#[class_event(Name: Type, add = method, remove = method)]` preserve association rows that use raw,
static, or inherited accessor methods. `class_property` accepts `static` before the property name.
`#[interface_event(Name: Type, add = method, remove = method)]` preserves an interface Event row
when accessor order prevents event shorthand from representing the method order.

`#[runtime]` on an interface preserves runtime MethodImpl flags for interfaces such as the
foundation collection contracts. `#[invoke_no_new_slot]` preserves delegates whose `Invoke` method
omits `NewSlot`. Delegate constructors, delegate instance calling conventions, and WinRT attribute
constructors are reconstructed from their source forms.

Some metadata states do not have a lossless RDL spelling. Parameter direction cannot be neither
In nor Out because an omitted direction is inferred. Attributes on a void return row cannot be
written because there is no return type to carry them. `#[len_param(N)]` and `#[size_param(N)]`
store raw parameter positions, so reordering parameters also requires updating `N`. Pointer chains
must use one constness throughout, such as `*mut *mut T` or `*const *const T`; mixed chains are
rejected. Explicit-layout types can be written as RDL unions only when every instance field has
offset zero.
