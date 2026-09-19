use super::*;

type SourceLocation = (String, usize, usize);

struct Provider {
    source: SourceLocation,
    stem: String,
    constant: Const,
}

impl Clang {
    pub(super) fn resolve_associated_constants(
        &self,
        parsed: &ParsedInputs,
        root: &str,
        reference: &metadata::reader::Index,
        collectors: &mut BTreeMap<String, Collector>,
    ) -> Result<(), Error> {
        let mut required = BTreeMap::<String, String>::new();
        let mut enums = HashSet::new();
        for collector in collectors.values() {
            for item in collector.values() {
                let Item::Enum(item) = item else {
                    continue;
                };
                for annotation in &item.annotations {
                    if !annotation.is("associated_constant") {
                        continue;
                    }
                    let name = annotation.value.as_deref().unwrap_or_default();
                    if !is_identifier(name) {
                        return Err(Error::new(
                            &format!("associated constant `{name}` is not a native identifier"),
                            "",
                            0,
                            0,
                        ));
                    }
                    required.insert(name.to_string(), format!("{root}.{}", item.name));
                    enums.insert(item.name.clone());
                }
            }
        }
        if required.is_empty() {
            return Ok(());
        }
        if root.is_empty() {
            return Err(Error::new(
                "associated constants require an explicit enum namespace",
                "",
                0,
                0,
            ));
        }

        for (namespace, name, item) in reference.iter_items() {
            if required.contains_key(name) && matches!(item, metadata::reader::Item::Const(_)) {
                return Err(Error::new(
                    &format!(
                        "associated constant `{name}` for `{}` conflicts with reference provider \
                         `{namespace}.{name}`",
                        required[name]
                    ),
                    "",
                    0,
                    0,
                ));
            }
        }

        let args: Vec<_> = parsed.args.iter().map(String::as_str).collect();
        let mut providers = BTreeMap::<String, Provider>::new();
        let mut enum_owners = BTreeMap::<String, (SourceLocation, Enum)>::new();
        for (source, tu) in parsed
            .h_tus
            .iter()
            .map(|(path, tu)| (MacroSource::File(path), tu))
            .chain(
                parsed
                    .str_tus
                    .iter()
                    .map(|(text, tu)| (MacroSource::Str(text), tu)),
            )
        {
            let mut declarations = vec![];
            flatten_decls(tu.cursor(), false, false, None, None, &mut declarations);
            let tag_rename = build_tag_rename_map(tu);
            let mut candidates = BTreeMap::<String, (SourceLocation, String)>::new();
            for (cursor, _) in declarations {
                let Some(path) = header_path_of(&cursor) else {
                    continue;
                };
                let Some(stem) = header_stem_of(&cursor) else {
                    continue;
                };
                if self.exclude_headers.contains(&stem)
                    || self
                        .exclude_paths
                        .iter()
                        .any(|root| path_is_under(&path, root))
                {
                    continue;
                }
                let name = cursor.name();
                let location = cursor.source_location();
                if cursor.kind() == CXCursor_EnumDecl && cursor.is_definition() {
                    let name = tag_rename.get(&name).cloned().unwrap_or(name);
                    if !enums.contains(&name) {
                        continue;
                    }
                    let item = Enum::parse(cursor)?;
                    if let Some((previous, existing)) = enum_owners.get(&name) {
                        if previous != &location
                            || existing.repr != item.repr
                            || existing.variants != item.variants
                            || existing.annotations != item.annotations
                        {
                            return Err(source_error(
                                &format!(
                                    "conflicting source contexts for associated enum `{root}.{name}`"
                                ),
                                &location,
                            ));
                        }
                    } else {
                        enum_owners.insert(name, (location, item));
                    }
                    continue;
                }
                if !required.contains_key(&name)
                    || !(cursor.kind() == CXCursor_MacroDefinition
                        && !cursor.is_macro_builtin()
                        && !cursor.is_macro_function_like()
                        || cursor.kind() == CXCursor_VarDecl
                            && cursor.is_definition()
                            && cursor.ty().is_const())
                {
                    continue;
                }
                if let Some((previous, _)) = candidates.get(&name)
                    && previous != &location
                {
                    return Err(source_error(
                        &format!("associated constant `{name}` has multiple source owners"),
                        &location,
                    ));
                }
                candidates.insert(name, (location, stem));
            }
            let names: Vec<_> = candidates.keys().cloned().collect();
            let evaluated = Const::evaluate_dependencies(source, &names, &parsed.index, &args)?;
            let mut evaluated: BTreeMap<_, _> = evaluated
                .into_iter()
                .map(|constant| (constant.name.clone(), constant))
                .collect();
            for (name, (location, stem)) in candidates {
                let constant = evaluated.remove(&name).ok_or_else(|| {
                    source_error(
                        &format!("associated constant `{name}` is not a native integer constant"),
                        &location,
                    )
                })?;
                if let Some(existing) = providers.get(&name) {
                    if existing.source != location {
                        return Err(source_error(
                            &format!("associated constant `{name}` has multiple source owners"),
                            &location,
                        ));
                    }
                    if existing.constant.ty != constant.ty
                        || existing.constant.value != constant.value
                    {
                        return Err(source_error(
                            &format!(
                                "associated constant `{name}` has conflicting native values or types"
                            ),
                            &location,
                        ));
                    }
                } else {
                    providers.insert(
                        name,
                        Provider {
                            source: location,
                            stem,
                            constant,
                        },
                    );
                }
            }
        }
        for name in &enums {
            if !enum_owners.contains_key(name) {
                return Err(Error::new(
                    &format!("associated enum `{root}.{name}` has no source owner"),
                    "",
                    0,
                    0,
                ));
            }
        }
        for (name, consumer) in &required {
            if !providers.contains_key(name) {
                return Err(Error::new(
                    &format!(
                        "associated constant `{name}` for `{consumer}` has no source provider"
                    ),
                    "",
                    0,
                    0,
                ));
            }
        }
        // Resolve after the ordinary sweep so dependency-only headers contribute no other roots.
        for collector in collectors.values_mut() {
            collector.retain_items(|_, item| {
                !matches!(item, Item::Const(constant) if required.contains_key(&constant.name))
            });
        }
        for provider in providers.into_values() {
            collectors
                .entry(provider.stem)
                .or_default()
                .insert(Item::Const(provider.constant));
        }
        Ok(())
    }
}

fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && bytes.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn source_error(message: &str, (file, line, column): &SourceLocation) -> Error {
    Error::new(message, file, *line, *column)
}
