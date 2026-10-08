use super::*;
use clang_sys::*;
use std::cell::OnceCell;
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

pub fn extract(inputs: impl IntoIterator<Item = Input>, args: &[&str]) -> Result<Snapshot, Error> {
    extract_with_options(inputs, args, &ExtractionOptions::default())
}

pub fn extract_with_options(
    inputs: impl IntoIterator<Item = Input>,
    args: &[&str],
    options: &ExtractionOptions,
) -> Result<Snapshot, Error> {
    extract_impl(
        inputs.into_iter().collect(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        args,
        options,
    )
}

pub fn extract_partitioned(
    inputs: impl IntoIterator<Item = PartitionedInput>,
    args: &[&str],
) -> Result<Snapshot, Error> {
    extract_partitioned_with_options(inputs, args, &ExtractionOptions::default())
}

pub fn extract_partitioned_with_options(
    inputs: impl IntoIterator<Item = PartitionedInput>,
    args: &[&str],
    options: &ExtractionOptions,
) -> Result<Snapshot, Error> {
    let inputs: Vec<_> = inputs.into_iter().collect();
    let mut identities = BTreeSet::new();
    let mut owners = BTreeMap::new();
    let mut partition_inputs = BTreeMap::new();
    let mut input_arguments = BTreeMap::new();
    for input in &inputs {
        if input.identity.trim().is_empty() {
            return Err(Error("partitioned input identity is empty".to_string()));
        }
        if !identities.insert(input.identity.as_str()) {
            return Err(Error(format!(
                "duplicate partitioned input identity `{}`",
                input.identity
            )));
        }
        partition_inputs.insert(input.input.name.clone(), input.identity.clone());
        for (root, partition) in &input.roots {
            owners.insert(
                (input.input.name.clone(), root.clone()),
                RootOwner {
                    input: input.identity.clone(),
                    root: root.clone(),
                    partition: partition.partition.clone(),
                    namespace: partition.namespace.clone(),
                    remaps: partition.remaps.clone(),
                    exclusions: partition.exclusions.clone(),
                    libraries: partition.libraries.clone(),
                    u32_types: partition.u32_types.clone(),
                    flags: partition.flags.clone(),
                    preserved_auto_function_pointer_levels: partition
                        .preserved_auto_function_pointer_levels
                        .clone(),
                    exclude_empty_records: partition.exclude_empty_records,
                },
            );
        }
        input_arguments.insert(input.input.name.clone(), input.arguments.clone());
    }
    extract_impl(
        inputs.into_iter().map(|input| input.input).collect(),
        owners,
        partition_inputs,
        input_arguments,
        args,
        options,
    )
}

fn extract_impl(
    inputs: Vec<Input>,
    owners: BTreeMap<(String, String), RootOwner>,
    partition_inputs: BTreeMap<String, String>,
    input_arguments: BTreeMap<String, Vec<String>>,
    args: &[&str],
    options: &ExtractionOptions,
) -> Result<Snapshot, Error> {
    let input_order = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| (input.name.clone(), index))
        .collect();
    let mut names = HashSet::new();
    for input in &inputs {
        if !names.insert(input.name.as_str()) {
            return Err(Error(format!("duplicate input name `{}`", input.name)));
        }
    }

    let library = Library::new()?;
    let index = Index::new()?;
    let timing = timings_enabled();
    let validate_annotations = args.contains(&"-DWIN32METADATA=1")
        || inputs
            .iter()
            .any(|input| input.source.contains("win32metadata:"));
    let target = timing.then(|| timing_target(args));
    let total_time = timing.then(std::time::Instant::now);
    let parse_time = timing.then(std::time::Instant::now);
    let parse_context = ParseContext {
        args,
        input_arguments: &input_arguments,
    };
    let translation_units = if options.parallelism() <= 1 || inputs.len() <= 1 {
        inputs
            .iter()
            .map(|input| parse_input(&index, input, &parse_context, timing))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let shared_library = library.shared();
        try_map_ordered_bounded(
            &inputs,
            options.parallelism(),
            || Library::from_shared(shared_library.clone()),
            |_, input| {
                let input_index = Index::new()?;
                parse_input(&input_index, input, &parse_context, timing)
                    .map(|parsed| parsed.with_index(input_index))
            },
        )?
    };
    if timing {
        for parsed in &translation_units {
            eprintln!(
                "windows-clang timing phase=parse-tu target={} tu={:?} source_bytes={} elapsed_ms={:.3}",
                target.as_deref().unwrap(),
                parsed.input,
                parsed.source_bytes,
                parsed.elapsed_ms,
            );
        }
    }
    if timing {
        eprintln!(
            "windows-clang timing phase=parse-total target={} input_tus={} elapsed_ms={:.3}",
            target.as_deref().unwrap(),
            inputs.len(),
            elapsed_ms(parse_time)
        );
    }

    let included_files = translation_units
        .iter()
        .flat_map(|parsed| parsed.translation_unit.included_files(&parsed.input))
        .collect();

    let traversal_time = timing.then(std::time::Instant::now);
    let mut facts = vec![];
    let mut constants = vec![];
    let mut annotations = BTreeMap::new();
    let mut declaration_guids = BTreeMap::new();
    let mut raw_function_link_names = BTreeMap::new();
    let mut canonical_function_origins = BTreeMap::new();
    let mut pointer_callback_aliases = BTreeSet::new();
    let mut pointer_only_class_layouts = BTreeMap::new();
    let mut embeddable_class_layouts = BTreeSet::new();
    let mut clang_flag_enums = BTreeSet::new();
    let mut extracted = vec![];
    let mut traversal_cursors = 0;
    let mut traversal_facts = 0;
    let mut traversal_constants = 0;
    for (input, parsed) in inputs.iter().zip(&translation_units) {
        let mut output = ExtractionState {
            facts: &mut facts,
            constants: &mut constants,
            annotations: &mut annotations,
            declaration_guids: &mut declaration_guids,
            raw_function_link_names: &mut raw_function_link_names,
            canonical_function_origins: &mut canonical_function_origins,
            pointer_callback_aliases: &mut pointer_callback_aliases,
            pointer_only_class_layouts: &mut pointer_only_class_layouts,
            embeddable_class_layouts: &mut embeddable_class_layouts,
            clang_flag_enums: &mut clang_flag_enums,
        };
        let (result, metrics) =
            parsed
                .translation_unit
                .extract(input, &mut output, timing, validate_annotations)?;
        if let Some(metrics) = metrics {
            traversal_cursors += metrics.cursors;
            traversal_facts += metrics.facts;
            traversal_constants += metrics.constants;
            eprintln!(
                "windows-clang timing phase=extract-tu target={} tu={:?} macro_definitions={} macro_expansion_files={} cursors={} facts={} constants={} macro_index_ms={:.3} traversal_ms={:.3} elapsed_ms={:.3}",
                target.as_deref().unwrap(),
                input.name,
                metrics.macro_definitions,
                metrics.macro_expansion_files,
                metrics.cursors,
                metrics.facts,
                metrics.constants,
                metrics.macro_index_ms,
                metrics.traversal_ms,
                metrics.elapsed_ms
            );
        }
        extracted.push(result);
    }
    merge_redeclaration_annotations(&facts, &mut annotations)?;
    let annotation_macros = annotation_macro_names(&annotations);
    let associated_constants = associated_constant_names(&facts, &annotations);
    decode_selected_macro_definitions(&mut facts, &extracted, &annotation_macros);
    if timing {
        eprintln!(
            "windows-clang timing phase=extract-total target={} input_tus={} cursors={} facts={} constants={} elapsed_ms={:.3}",
            target.as_deref().unwrap(),
            inputs.len(),
            traversal_cursors,
            traversal_facts,
            traversal_constants,
            elapsed_ms(traversal_time)
        );
    }
    let constant_time = timing.then(std::time::Instant::now);
    let mut probe_candidates = 0;
    let mut synthetic_tus = 0;
    let mut retry_tus = 0;
    for ((input, parsed), extracted) in inputs.iter().zip(&translation_units).zip(&extracted) {
        let local_arguments = input_arguments.get(&input.name);
        let probe_arguments: Vec<_> = args
            .iter()
            .copied()
            .chain(local_arguments.into_iter().flatten().map(String::as_str))
            .collect();
        let (new_constants, metrics) = evaluate_constants(
            &index,
            input,
            &probe_arguments,
            &facts,
            &ConstantSources {
                macros: &extracted.macros,
                translation_unit: &parsed.translation_unit,
            },
            &associated_constants,
            timing,
        )?;
        if let Some(metrics) = metrics {
            probe_candidates += metrics.candidates;
            synthetic_tus += metrics.synthetic_tus;
            retry_tus += metrics.retry_tus;
            eprintln!(
                "windows-clang timing phase=constant-probes target={} tu={:?} candidates={} string_constants={} evaluated={} constants={} initial_chunk={} initial_batches={} recovery_chunk={} recovery_batches={} isolation_chunk={} isolation_batches={} defined_check_tus={} singleton_probes={} synthetic_tus={} retry_tus={} available_parallelism={} configured_workers={} initial_workers={} recovery_workers={} isolation_workers={} singleton_workers={} elapsed_ms={:.3}",
                target.as_deref().unwrap(),
                input.name,
                metrics.candidates,
                metrics.string_constants,
                metrics.evaluated,
                metrics.constants,
                metrics.initial_chunk,
                metrics.initial_batches,
                metrics.recovery_chunk,
                metrics.recovery_batches,
                metrics.isolation_chunk,
                metrics.isolation_batches,
                metrics.defined_check_tus,
                metrics.singleton_probes,
                metrics.synthetic_tus,
                metrics.retry_tus,
                metrics.available_parallelism,
                metrics.configured_workers,
                metrics.initial_workers,
                metrics.recovery_workers,
                metrics.isolation_workers,
                metrics.singleton_workers,
                metrics.elapsed_ms
            );
        }
        constants.extend(new_constants);
    }
    validate_associated_constants(&associated_constants, &constants)?;
    if timing {
        eprintln!(
            "windows-clang timing phase=constant-probes-total target={} input_tus={} candidates={} synthetic_tus={} retry_tus={} constants={} elapsed_ms={:.3}",
            target.as_deref().unwrap(),
            inputs.len(),
            probe_candidates,
            synthetic_tus,
            retry_tus,
            constants.len(),
            elapsed_ms(constant_time)
        );
    }
    let phase_time = timing.then(std::time::Instant::now);
    decode_reachable_structs(&mut facts, &constants, &extracted);
    if timing {
        eprintln!(
            "windows-clang timing phase=deferred-records target={} elapsed_ms={:.3}",
            target.as_deref().unwrap(),
            elapsed_ms(phase_time)
        );
    }
    let phase_time = timing.then(std::time::Instant::now);
    apply_macro_enum_overrides(&mut facts, &mut constants, &associated_constants);
    if timing {
        eprintln!(
            "windows-clang timing phase=macro-overrides target={} elapsed_ms={:.3}",
            target.as_deref().unwrap(),
            elapsed_ms(phase_time)
        );
    }
    let phase_time = timing.then(std::time::Instant::now);
    recover_midl_artifacts(&inputs, &mut facts, &mut constants);
    if timing {
        eprintln!(
            "windows-clang timing phase=midl-recovery target={} elapsed_ms={:.3}",
            target.as_deref().unwrap(),
            elapsed_ms(phase_time)
        );
    }
    materialize_anonymous_callbacks(&mut facts);
    let declare_handles = identify_declare_handles(&inputs, &facts, &extracted);
    facts.sort();
    for pair in facts.windows(2) {
        if pair[0].origin == pair[1].origin {
            return Err(Error(format!(
                "duplicate fact origin `{}`",
                origin(&pair[0].origin)
            )));
        }
    }
    if let Some(fact) = facts.iter().find(|fact| {
        matches!(fact.data, FactData::Function { .. })
            && !raw_function_link_names.contains_key(&fact.origin)
    }) {
        return Err(Error(format!(
            "function `{}` is missing its raw linker identity",
            origin(&fact.origin)
        )));
    }
    let function_link_name_index =
        FunctionLinkNameIndex::from_facts(&facts, &raw_function_link_names);
    constants.sort();
    if timing {
        let headers = facts
            .iter()
            .filter(|fact| fact.root)
            .map(|fact| fact.spelling.file.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        eprintln!(
            "windows-clang timing phase=extract-summary target={} input_tus={} root_headers={} facts={} constants={} elapsed_ms={:.3}",
            target.as_deref().unwrap(),
            inputs.len(),
            headers,
            facts.len(),
            constants.len(),
            elapsed_ms(total_time)
        );
    }
    let mut root_owners = BTreeMap::new();
    for fact in &facts {
        let mut matches: Vec<_> = owners
            .iter()
            .filter(|((tu, root), _)| {
                tu == &fact.origin.tu && source_path_matches(root, &fact.expansion.file)
            })
            .map(|(_, owner)| owner)
            .collect();
        let mut matched_file = &fact.expansion.file;
        if matches.is_empty() {
            matched_file = &fact.spelling.file;
            matches.extend(
                owners
                    .iter()
                    .filter(|((tu, root), _)| {
                        tu == &fact.origin.tu && source_path_matches(root, &fact.spelling.file)
                    })
                    .map(|(_, owner)| owner),
            );
        }
        let mut matches = matches.into_iter();
        let Some(owner) = matches.next() else {
            continue;
        };
        if let Some(other) = matches.find(|other| *other != owner) {
            return Err(Error(format!(
                "source `{}` in translation unit `{}` matches multiple tagged roots: {}:{} and {}:{}",
                matched_file, fact.origin.tu, owner.input, owner.root, other.input, other.root,
            )));
        }
        root_owners.insert(fact.origin.clone(), owner.clone());
    }
    Ok(Snapshot {
        facts,
        constants,
        included_files,
        raw_function_link_names,
        canonical_function_origins,
        function_link_name_index,
        declare_handles,
        annotations,
        declaration_guids,
        pointer_callback_aliases,
        pointer_only_class_layouts,
        embeddable_class_layouts,
        clang_flag_enums,
        root_owners,
        constant_root_owners: BTreeMap::new(),
        root_partitions: owners,
        partition_inputs,
        input_order,
        partition_exclusions: vec![],
        forced_flags: BTreeSet::new(),
        suppressed_type_origins: BTreeSet::new(),
        projected_type_names: BTreeMap::new(),
        namespace_authorities: BTreeMap::new(),
        fact_namespace_authorities: BTreeMap::new(),
        constant_namespace_authorities: BTreeMap::new(),
        header_partition_policy: false,
        header_authority_partition: None,
        timing_target: target,
    })
}

fn apply_macro_enum_overrides(
    facts: &mut [Fact],
    constants: &mut Vec<Constant>,
    associated_constants: &BTreeSet<String>,
) {
    let macro_origins: HashSet<_> = facts
        .iter()
        .filter_map(|fact| {
            matches!(fact.data, FactData::Macro { .. })
                .then_some((fact.origin.tu.as_str(), fact.origin.local))
        })
        .collect();
    let scalar_aliases: HashMap<_, _> = facts
        .iter()
        .filter_map(|fact| {
            let FactData::Typedef { target } = &fact.data else {
                return None;
            };
            Some((
                (fact.origin.tu.as_str(), fact.name.as_str(), &fact.spelling),
                target,
            ))
        })
        .collect();
    let enum_reprs: HashMap<_, _> = facts
        .iter()
        .filter_map(|fact| {
            let FactData::Enum { repr, .. } = &fact.data else {
                return None;
            };
            Some((
                (fact.origin.tu.as_str(), fact.name.as_str(), &fact.spelling),
                *repr,
            ))
        })
        .collect();
    let windows_metadata_namespaces: HashSet<_> = facts
        .iter()
        .filter(|fact| {
            fact.kind == FactKind::Namespace && fact.name == "Windows" && fact.parent.is_none()
        })
        .map(|fact| fact.origin.clone())
        .collect();
    let mut enum_members: HashMap<_, Vec<_>> = HashMap::new();
    for (fact_index, fact) in facts.iter().enumerate() {
        let FactData::Enum {
            repr,
            variants,
            scoped,
            ..
        } = &fact.data
        else {
            continue;
        };
        let windows_metadata_enum = fact
            .parent
            .as_ref()
            .is_some_and(|parent| windows_metadata_namespaces.contains(parent));
        if *scoped || (fact.parent.is_some() && !windows_metadata_enum) {
            continue;
        }
        for (variant_index, variant) in variants.iter().enumerate() {
            enum_members
                .entry((
                    fact.origin.tu.clone(),
                    source_file_key(&fact.spelling.file),
                    variant.name.clone(),
                ))
                .or_default()
                .push((
                    fact_index,
                    variant_index,
                    *repr,
                    fact.spelling.offset,
                    variant.value,
                    windows_metadata_enum,
                ));
        }
    }

    let mut actions = vec![];
    for constant in constants.iter() {
        if associated_constants.contains(&constant.name)
            || !macro_origins
                .contains(&(constant.definition.tu.as_str(), constant.definition.local))
        {
            continue;
        }
        let Some(
            [(fact_index, variant_index, repr, enum_offset, enum_value, windows_metadata_enum)],
        ) = enum_members
            .get(&(
                constant.root.tu.clone(),
                source_file_key(&constant.spelling.file),
                constant.name.clone(),
            ))
            .map(Vec::as_slice)
        else {
            continue;
        };
        let value = if constant.spelling.offset < *enum_offset {
            macro_before_enum_matches(
                constant,
                *repr,
                *enum_value,
                *windows_metadata_enum,
                &scalar_aliases,
            )
            .then_some(None)
        } else {
            macro_after_enum_value(
                constant,
                *repr,
                &facts[*fact_index].spelling,
                &scalar_aliases,
                &enum_reprs,
            )
            .map(Some)
        };
        if let Some(value) = value {
            actions.push((
                constant.definition.clone(),
                constant.name.clone(),
                *fact_index,
                *variant_index,
                value,
            ));
        }
    }
    drop(enum_members);
    drop(macro_origins);
    drop(scalar_aliases);
    drop(enum_reprs);
    for (_, _, fact_index, variant_index, value) in &actions {
        let Some(value) = value else {
            continue;
        };
        let FactData::Enum { variants, .. } = &mut facts[*fact_index].data else {
            unreachable!()
        };
        variants[*variant_index].value = *value;
    }
    let overridden: HashSet<_> = actions
        .iter()
        .map(|(definition, name, ..)| (definition, name.as_str()))
        .collect();
    constants
        .retain(|constant| !overridden.contains(&(&constant.definition, constant.name.as_str())));
}

fn source_file_key(file: &str) -> String {
    normalize_name(file)
}

fn macro_before_enum_matches(
    constant: &Constant,
    repr: Scalar,
    variant_value: i64,
    allow_different_widths: bool,
    scalar_aliases: &HashMap<(&str, &str, &Location), &TypeRef>,
) -> bool {
    let Some(source) = macro_scalar_type(&constant.ty, &constant.root.tu, scalar_aliases, None)
    else {
        return false;
    };
    let Some((source_width, source_value)) = scalar_value(source, &constant.value) else {
        return false;
    };
    let Some((enum_width, enum_value)) = normalized_enum_value(repr, variant_value) else {
        return false;
    };
    source_value == enum_value && (source_width == enum_width || allow_different_widths)
}

fn macro_after_enum_value(
    constant: &Constant,
    repr: Scalar,
    enum_declaration: &Location,
    scalar_aliases: &HashMap<(&str, &str, &Location), &TypeRef>,
    enum_reprs: &HashMap<(&str, &str, &Location), Scalar>,
) -> Option<i64> {
    let source = macro_scalar_type(
        &constant.ty,
        &constant.root.tu,
        scalar_aliases,
        Some((enum_reprs, enum_declaration)),
    )?;
    scalar_value(source, &constant.value)?;
    enum_override_value(&constant.value, repr)
}

fn macro_scalar_type(
    ty: &TypeRef,
    tu: &str,
    scalar_aliases: &HashMap<(&str, &str, &Location), &TypeRef>,
    enum_reprs: Option<(&HashMap<(&str, &str, &Location), Scalar>, &Location)>,
) -> Option<Scalar> {
    fn resolve(
        ty: &TypeRef,
        tu: &str,
        scalar_aliases: &HashMap<(&str, &str, &Location), &TypeRef>,
        enum_reprs: Option<(&HashMap<(&str, &str, &Location), Scalar>, &Location)>,
        seen: &mut HashSet<Location>,
    ) -> Option<Scalar> {
        match ty {
            TypeRef::Scalar(scalar) => Some(*scalar),
            TypeRef::Named { name, declaration } if seen.insert(declaration.clone()) => {
                let key = (tu, name.as_str(), declaration);
                if let Some(target) = scalar_aliases.get(&key) {
                    resolve(target, tu, scalar_aliases, enum_reprs, seen)
                } else {
                    let (enum_reprs, target_declaration) = enum_reprs?;
                    if declaration == target_declaration {
                        enum_reprs.get(&key).copied()
                    } else {
                        None
                    }
                }
            }
            _ => None,
        }
    }

    resolve(ty, tu, scalar_aliases, enum_reprs, &mut HashSet::new())
}

fn scalar_value(scalar: Scalar, value: &Value) -> Option<(u8, i128)> {
    match (scalar, value) {
        (Scalar::I8, Value::Signed(value)) => Some((8, i128::from(*value))),
        (Scalar::U8, Value::Unsigned(value)) => Some((8, i128::from(*value))),
        (Scalar::I16, Value::Signed(value)) => Some((16, i128::from(*value))),
        (Scalar::U16, Value::Unsigned(value)) => Some((16, i128::from(*value))),
        (Scalar::I32, Value::Signed(value)) => Some((32, i128::from(*value))),
        (Scalar::U32, Value::Unsigned(value)) => Some((32, i128::from(*value))),
        (Scalar::I64, Value::Signed(value)) => Some((64, i128::from(*value))),
        (Scalar::U64, Value::Unsigned(value)) => Some((64, i128::from(*value))),
        _ => None,
    }
}

fn normalized_enum_value(repr: Scalar, value: i64) -> Option<(u8, i128)> {
    Some(match repr {
        Scalar::I8 => (8, i128::from(value as i8)),
        Scalar::U8 => (8, i128::from(value as u8)),
        Scalar::I16 => (16, i128::from(value as i16)),
        Scalar::U16 => (16, i128::from(value as u16)),
        Scalar::I32 => (32, i128::from(value as i32)),
        Scalar::U32 => (32, i128::from(value as u32)),
        Scalar::I64 => (64, i128::from(value)),
        Scalar::U64 => (64, i128::from(value as u64)),
        Scalar::Bool | Scalar::F32 | Scalar::F64 => return None,
    })
}

fn enum_override_value(value: &Value, repr: Scalar) -> Option<i64> {
    match (value, repr) {
        (Value::Signed(value), Scalar::I8) => i8::try_from(*value).ok().map(i64::from),
        (Value::Signed(value), Scalar::I16) => i16::try_from(*value).ok().map(i64::from),
        (Value::Signed(value), Scalar::I32) => i32::try_from(*value).ok().map(i64::from),
        (Value::Signed(value), Scalar::I64) => Some(*value),
        (Value::Unsigned(value), Scalar::U8) => u8::try_from(*value).ok().map(i64::from),
        (Value::Unsigned(value), Scalar::U16) => u16::try_from(*value).ok().map(i64::from),
        (Value::Unsigned(value), Scalar::U32) => u32::try_from(*value).ok().map(i64::from),
        (Value::Unsigned(value), Scalar::U64) => Some(*value as i64),
        _ => None,
    }
}

fn identify_declare_handles(
    inputs: &[Input],
    facts: &[Fact],
    extracted: &[Extracted<'_>],
) -> Vec<DeclareHandle> {
    if extracted
        .iter()
        .all(|extracted| extracted.declare_handle_expansions.is_empty())
    {
        return Vec::new();
    }
    let mut result = BTreeSet::new();
    let mut facts_by_expansion: HashMap<(&str, &str, &Location), Vec<&Fact>> = HashMap::new();
    for fact in facts {
        facts_by_expansion
            .entry((&fact.origin.tu, &fact.name, &fact.expansion))
            .or_default()
            .push(fact);
    }
    for (input, extracted) in inputs.iter().zip(extracted) {
        let expansions: BTreeSet<_> = extracted.declare_handle_expansions.iter().collect();
        for expansion in expansions {
            let record_name = format!("{}__", expansion.name);
            let aliases: Vec<_> = facts_by_expansion
                .get(&(
                    input.name.as_str(),
                    expansion.name.as_str(),
                    &expansion.location,
                ))
                .into_iter()
                .flatten()
                .copied()
                .filter(|fact| {
                    matches!(
                        &fact.data,
                        FactData::Typedef {
                            target: TypeRef::Pointer {
                                mutable: true,
                                target,
                            },
                        } if matches!(
                            target.as_ref(),
                            TypeRef::Named { name, .. } if name == &record_name
                        )
                    )
                })
                .collect();
            let [alias] = aliases.as_slice() else {
                continue;
            };
            let FactData::Typedef {
                target:
                    TypeRef::Pointer {
                        target: alias_target,
                        ..
                    },
            } = &alias.data
            else {
                unreachable!()
            };
            let TypeRef::Named {
                declaration: record_declaration,
                ..
            } = alias_target.as_ref()
            else {
                unreachable!()
            };
            let records: Vec<_> = facts_by_expansion
                .get(&(
                    input.name.as_str(),
                    record_name.as_str(),
                    &expansion.location,
                ))
                .into_iter()
                .flatten()
                .copied()
                .filter(|fact| fact.spelling == *record_declaration)
                .filter(|fact| {
                    matches!(
                        &fact.data,
                        FactData::Record {
                            base: None,
                            fields,
                            size: 4,
                            align: 4,
                            packing: None,
                            alignment: None,
                            union: false,
                        } if fact.definition
                            && matches!(
                                fields.as_slice(),
                                [Field {
                                    name,
                                    ty: TypeRef::Scalar(Scalar::I32),
                                    offset: 0,
                                    size: 4,
                                    align: 4,
                                    bit_width: None,
                                }] if name == "unused"
                            )
                    )
                })
                .collect();
            let [record] = records.as_slice() else {
                continue;
            };
            result.insert(DeclareHandle {
                alias: alias.origin.clone(),
                record: record.origin.clone(),
            });
        }
    }
    result.into_iter().collect()
}

fn recover_midl_artifacts(inputs: &[Input], facts: &mut [Fact], constants: &mut Vec<Constant>) {
    let mut sources = HashMap::new();
    let reference_counts = reference_counts(facts, constants);
    let mut named_typedefs = HashSet::new();
    let mut typedefs_by_file: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (index, fact) in facts.iter().enumerate() {
        let FactData::Typedef { target } = &fact.data else {
            continue;
        };
        if let TypeRef::Named { name, declaration } = target {
            named_typedefs.insert((fact.origin.tu.clone(), name.clone(), declaration.clone()));
        }
        if fact.root {
            typedefs_by_file
                .entry((fact.origin.tu.clone(), fact.spelling.file.clone()))
                .or_default()
                .push(index);
        }
    }
    for typedefs in typedefs_by_file.values_mut() {
        typedefs.sort_by_key(|index| facts[*index].spelling.offset);
    }

    let mut enum_recoveries = vec![];
    for (enum_index, enum_fact) in facts.iter().enumerate() {
        let FactData::Enum { repr, variants, .. } = &enum_fact.data else {
            continue;
        };
        let generated_name = midl_generated_name(&enum_fact.name);
        let private_alias_name = midl_private_alias_name(&enum_fact.name);
        if !enum_fact.root || (!generated_name && private_alias_name.is_none()) {
            continue;
        }
        if named_typedefs.contains(&(
            enum_fact.origin.tu.clone(),
            enum_fact.name.clone(),
            enum_fact.spelling.clone(),
        )) {
            continue;
        }
        if reference_counts
            .get(&(enum_fact.origin.tu.clone(), enum_fact.name.clone()))
            .copied()
            .unwrap_or_default()
            != 0
        {
            continue;
        }
        let adjacent_alias = typedefs_by_file
            .get(&(enum_fact.origin.tu.clone(), enum_fact.spelling.file.clone()))
            .and_then(|typedefs| {
                typedefs.iter().find_map(|index| {
                    if facts[*index].spelling.offset <= enum_fact.spelling.offset {
                        return None;
                    }
                    scalar_type(&facts[*index].data, &facts[*index].origin.tu, facts)
                        .map(|scalar| (*index, scalar))
                })
            })
            .and_then(|(index, scalar)| {
                let fact = &facts[index];
                (adjacent_midl_enum_alias(inputs, &mut sources, enum_fact, fact)
                    && (generated_name
                        || private_alias_name.is_some_and(|name| {
                            name == fact.name
                                && midl_generated_header(inputs, &sources, &enum_fact.spelling.file)
                        }))
                    && variants.iter().all(|variant| {
                        value_for_midl_scalar(variant.value, *repr, scalar).is_some()
                    }))
                .then_some(index)
            });
        if !generated_name && adjacent_alias.is_none() {
            continue;
        }
        enum_recoveries.push((
            enum_index,
            adjacent_alias,
            *repr,
            variants.clone(),
            enum_fact.origin.clone(),
            enum_fact.spelling.clone(),
        ));
    }
    for (enum_index, alias_index, enum_repr, variants, origin, spelling) in enum_recoveries {
        let (ty, scalar) = if let Some(alias_index) = alias_index {
            (
                TypeRef::Named {
                    name: facts[alias_index].name.clone(),
                    declaration: facts[alias_index].spelling.clone(),
                },
                scalar_type(
                    &facts[alias_index].data,
                    &facts[alias_index].origin.tu,
                    facts,
                )
                .unwrap(),
            )
        } else {
            (TypeRef::Scalar(enum_repr), enum_repr)
        };
        facts[enum_index].root = false;
        constants.extend(variants.into_iter().map(|variant| Constant {
            root: origin.clone(),
            definition: origin.clone(),
            spelling: spelling.clone(),
            name: variant.name,
            ty: ty.clone(),
            value: value_for_midl_scalar(variant.value, enum_repr, scalar).unwrap(),
        }));
    }

    let mut opaque_recoveries = vec![];
    for (alias_index, alias_fact) in facts.iter().enumerate() {
        let FactData::Typedef {
            target:
                TypeRef::Pointer {
                    mutable,
                    target: pointee,
                },
        } = &alias_fact.data
        else {
            continue;
        };
        let TypeRef::Named { name, declaration } = pointee.as_ref() else {
            continue;
        };
        if !alias_fact.root || !midl_generated_name(name) {
            continue;
        }
        let Some((record_index, record_fact)) = facts.iter().enumerate().find(|(_, candidate)| {
            candidate.origin.tu == alias_fact.origin.tu
                && candidate.spelling == *declaration
                && matches!(
                    &candidate.data,
                    FactData::Record { fields, union: false, .. }
                        if matches!(fields.as_slice(), [Field { name, .. }] if name == "_")
                )
        }) else {
            continue;
        };
        let references = reference_counts
            .get(&(alias_fact.origin.tu.clone(), name.clone()))
            .copied()
            .unwrap_or_default();
        if references != 1
            || !midl_record_pointer_alias(inputs, &mut sources, record_fact, alias_fact)
        {
            continue;
        }
        opaque_recoveries.push((alias_index, record_index, *mutable, alias_fact.name.clone()));
    }
    for (alias_index, record_index, mutable, alias) in opaque_recoveries {
        facts[record_index].root = false;
        let FactData::Typedef { target } = &mut facts[alias_index].data else {
            unreachable!()
        };
        *target = TypeRef::OpaquePointer {
            mutable,
            tag: alias,
        };
    }
}

fn midl_generated_name(name: &str) -> bool {
    name.starts_with("__MIDL")
}

fn midl_private_alias_name(name: &str) -> Option<&str> {
    name.strip_prefix('_')
        .filter(|name| !name.is_empty() && !name.starts_with('_'))
}

fn midl_generated_header(inputs: &[Input], sources: &HashMap<String, String>, file: &str) -> bool {
    inputs
        .iter()
        .find(|input| input.name == file)
        .map(|input| input.source.as_str())
        .or_else(|| sources.get(file).map(String::as_str))
        .is_some_and(|source| {
            source
                .lines()
                .take(10)
                .any(|line| line.contains("File created by MIDL compiler version"))
        })
}

fn input_segment(
    inputs: &[Input],
    sources: &mut HashMap<String, String>,
    start: &Location,
    end: &Location,
) -> Option<String> {
    if start.file != end.file || start.offset >= end.offset {
        return None;
    }
    if let Some(input) = inputs.iter().find(|input| input.name == start.file) {
        return input
            .source
            .get(start.offset as usize..end.offset as usize)
            .map(str::to_string);
    }
    if !sources.contains_key(&start.file) {
        sources.insert(
            start.file.clone(),
            std::fs::read_to_string(&start.file).ok()?,
        );
    }
    sources
        .get(&start.file)?
        .get(start.offset as usize..end.offset as usize)
        .map(str::to_string)
}

fn adjacent_midl_enum_alias(
    inputs: &[Input],
    sources: &mut HashMap<String, String>,
    enum_fact: &Fact,
    alias_fact: &Fact,
) -> bool {
    let Some(source) = input_segment(inputs, sources, &enum_fact.spelling, &alias_fact.spelling)
    else {
        return false;
    };
    let compact: String = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let declaration = compact
        .strip_prefix(&enum_fact.name)
        .or_else(|| compact.strip_prefix(&format!("enum{}", enum_fact.name)));
    let Some(declaration) = declaration else {
        return false;
    };
    declaration.starts_with('{')
        && declaration
            .rfind('}')
            .is_some_and(|end| declaration[end..].starts_with("};typedef"))
        && declaration.matches('{').count() == 1
        && declaration.matches('}').count() == 1
}

fn midl_record_pointer_alias(
    inputs: &[Input],
    sources: &mut HashMap<String, String>,
    record: &Fact,
    alias: &Fact,
) -> bool {
    let Some(source) = input_segment(inputs, sources, &record.spelling, &alias.spelling) else {
        return false;
    };
    let compact: String = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let declaration = compact
        .strip_prefix(&record.name)
        .or_else(|| compact.strip_prefix(&format!("struct{}", record.name)));
    declaration.is_some_and(|declaration| {
        declaration.starts_with('{')
            && declaration.ends_with("}*")
            && declaration.matches('{').count() == 1
            && declaration.matches('}').count() == 1
    })
}

fn scalar_type(data: &FactData, tu: &str, facts: &[Fact]) -> Option<Scalar> {
    fn resolve(
        ty: &TypeRef,
        tu: &str,
        facts: &[Fact],
        seen: &mut HashSet<Location>,
    ) -> Option<Scalar> {
        match ty {
            TypeRef::Scalar(scalar) => Some(*scalar),
            TypeRef::Named { declaration, .. } if seen.insert(declaration.clone()) => {
                let fact = facts
                    .iter()
                    .find(|fact| fact.origin.tu == tu && fact.spelling == *declaration)?;
                match &fact.data {
                    FactData::Typedef { target } => resolve(target, tu, facts, seen),
                    FactData::Enum { repr, .. } => Some(*repr),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    let FactData::Typedef { target } = data else {
        return None;
    };
    match resolve(target, tu, facts, &mut HashSet::new())? {
        scalar @ (Scalar::I8
        | Scalar::U8
        | Scalar::I16
        | Scalar::U16
        | Scalar::I32
        | Scalar::U32
        | Scalar::I64
        | Scalar::U64) => Some(scalar),
        Scalar::Bool | Scalar::F32 | Scalar::F64 => None,
    }
}

fn value_for_scalar(value: i64, scalar: Scalar) -> Option<Value> {
    match scalar {
        Scalar::U8 => u8::try_from(value)
            .ok()
            .map(|value| Value::Unsigned(u64::from(value))),
        Scalar::U16 => u16::try_from(value)
            .ok()
            .map(|value| Value::Unsigned(u64::from(value))),
        Scalar::U32 => u32::try_from(value)
            .ok()
            .map(|value| Value::Unsigned(u64::from(value))),
        Scalar::U64 => u64::try_from(value).ok().map(Value::Unsigned),
        Scalar::I8 => i8::try_from(value)
            .ok()
            .map(|value| Value::Signed(i64::from(value))),
        Scalar::I16 => i16::try_from(value)
            .ok()
            .map(|value| Value::Signed(i64::from(value))),
        Scalar::I32 => i32::try_from(value)
            .ok()
            .map(|value| Value::Signed(i64::from(value))),
        Scalar::I64 => Some(Value::Signed(value)),
        Scalar::Bool | Scalar::F32 | Scalar::F64 => None,
    }
}

fn value_for_midl_scalar(value: i64, source: Scalar, target: Scalar) -> Option<Value> {
    value_for_scalar(value, target).or_else(|| match (source, target) {
        (Scalar::I8, Scalar::U8) => Some(Value::Unsigned(u64::from(value as u8))),
        (Scalar::I16, Scalar::U16) => Some(Value::Unsigned(u64::from(value as u16))),
        (Scalar::I32, Scalar::U32) => Some(Value::Unsigned(u64::from(value as u32))),
        (Scalar::I64, Scalar::U64) => Some(Value::Unsigned(value as u64)),
        (Scalar::U8, Scalar::I8) => Some(Value::Signed(i64::from(value as i8))),
        (Scalar::U16, Scalar::I16) => Some(Value::Signed(i64::from(value as i16))),
        (Scalar::U32, Scalar::I32) => Some(Value::Signed(i64::from(value as i32))),
        (Scalar::U64, Scalar::I64) => Some(Value::Signed(value)),
        _ => None,
    })
}

fn reference_counts(facts: &[Fact], constants: &[Constant]) -> HashMap<(String, String), usize> {
    let mut result = HashMap::new();
    for fact in facts {
        let mut names = HashSet::new();
        fact_type_names(&fact.data, &mut names);
        for name in names {
            *result.entry((fact.origin.tu.clone(), name)).or_default() += 1;
        }
    }
    for constant in constants {
        let mut names = HashSet::new();
        type_names(&constant.ty, &mut names);
        for name in names {
            *result.entry((constant.root.tu.clone(), name)).or_default() += 1;
        }
    }
    result
}

struct Library {
    current: Arc<SharedLibrary>,
    previous: Option<Arc<SharedLibrary>>,
}

impl Library {
    fn new() -> Result<Self, Error> {
        let current = match get_library() {
            Some(library) => library,
            None => Arc::new(
                load_manually()
                    .map_err(|error| Error(format!("failed to load libclang: {error}")))?,
            ),
        };
        Ok(Self::from_shared(current))
    }

    fn from_shared(current: Arc<SharedLibrary>) -> Self {
        let previous = set_library(Some(current.clone()));
        Self { current, previous }
    }

    fn shared(&self) -> Arc<SharedLibrary> {
        self.current.clone()
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        set_library(self.previous.take());
    }
}

struct Index(CXIndex);

impl Index {
    fn new() -> Result<Self, Error> {
        let value = unsafe { clang_createIndex(0, 0) };
        if value.is_null() {
            Err(Error("failed to create libclang index".into()))
        } else {
            Ok(Self(value))
        }
    }
}

impl Drop for Index {
    fn drop(&mut self) {
        unsafe { clang_disposeIndex(self.0) };
    }
}

struct TranslationUnit(CXTranslationUnit);

struct OwnedTranslationUnit {
    translation_unit: TranslationUnit,
    _index: Option<Index>,
}

impl OwnedTranslationUnit {
    fn new(translation_unit: TranslationUnit) -> Self {
        Self {
            translation_unit,
            _index: None,
        }
    }

    fn with_index(mut self, index: Index) -> Self {
        self._index = Some(index);
        self
    }
}

impl Deref for OwnedTranslationUnit {
    type Target = TranslationUnit;

    fn deref(&self) -> &Self::Target {
        &self.translation_unit
    }
}

// SAFETY: this private bundle uniquely owns both libclang handles and is moved only after parsing
// finishes. It is never accessed from two threads at once, and its fields drop the translation
// unit before the index that created it.
unsafe impl Send for OwnedTranslationUnit {}

struct ParsedTranslationUnit {
    input: String,
    source_bytes: usize,
    elapsed_ms: f64,
    translation_unit: OwnedTranslationUnit,
}

impl ParsedTranslationUnit {
    fn with_index(mut self, index: Index) -> Self {
        self.translation_unit = self.translation_unit.with_index(index);
        self
    }
}

struct ParseContext<'a> {
    args: &'a [&'a str],
    input_arguments: &'a BTreeMap<String, Vec<String>>,
}

fn parse_input(
    index: &Index,
    input: &Input,
    context: &ParseContext<'_>,
    timing: bool,
) -> Result<ParsedTranslationUnit, Error> {
    let start = timing.then(std::time::Instant::now);
    let local_arguments = context.input_arguments.get(&input.name);
    let arguments: Vec<_> = context
        .args
        .iter()
        .copied()
        .chain(local_arguments.into_iter().flatten().map(String::as_str))
        .collect();
    let translation_unit = TranslationUnit::parse(index, input, &arguments)?;
    Ok(ParsedTranslationUnit {
        input: input.name.clone(),
        source_bytes: input.source.len(),
        elapsed_ms: elapsed_ms(start),
        translation_unit: OwnedTranslationUnit::new(translation_unit),
    })
}

fn try_map_ordered_bounded<T, S, R, E>(
    inputs: &[T],
    parallelism: usize,
    init: impl Fn() -> S + Sync,
    map: impl Fn(&mut S, &T) -> Result<R, E> + Sync,
) -> Result<Vec<R>, E>
where
    T: Sync,
    R: Send,
    E: Send,
{
    if inputs.is_empty() {
        return Ok(vec![]);
    }

    let workers = parallelism.max(1).min(inputs.len());
    if workers == 1 {
        let mut state = init();
        return inputs.iter().map(|input| map(&mut state, input)).collect();
    }

    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        use std::sync::atomic::Ordering;

        let (sender, receiver) = std::sync::mpsc::channel();
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let init = &init;
            let map = &map;
            let sender = sender.clone();
            let next = &next;
            handles.push(scope.spawn(move || {
                let mut state = init();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= inputs.len()
                        || sender
                            .send((index, map(&mut state, &inputs[index])))
                            .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        drop(sender);

        let mut output: Vec<Option<Result<R, E>>> = (0..inputs.len()).map(|_| None).collect();
        for (index, result) in receiver {
            output[index] = Some(result);
        }
        for handle in handles {
            if let Err(payload) = handle.join() {
                std::panic::resume_unwind(payload);
            }
        }
        output.into_iter().map(|result| result.unwrap()).collect()
    })
}

struct Evaluated {
    name: String,
    ty: TypeRef,
    value: Value,
}

struct ExtractionMetrics {
    macro_definitions: usize,
    macro_expansion_files: usize,
    cursors: u32,
    facts: usize,
    constants: usize,
    macro_index_ms: f64,
    traversal_ms: f64,
    elapsed_ms: f64,
}

struct ProbeMetrics {
    candidates: usize,
    string_constants: usize,
    evaluated: usize,
    constants: usize,
    initial_chunk: usize,
    initial_batches: usize,
    recovery_chunk: usize,
    recovery_batches: usize,
    isolation_chunk: usize,
    isolation_batches: usize,
    defined_check_tus: usize,
    singleton_probes: usize,
    synthetic_tus: usize,
    retry_tus: usize,
    available_parallelism: usize,
    configured_workers: usize,
    initial_workers: usize,
    recovery_workers: usize,
    isolation_workers: usize,
    singleton_workers: usize,
    elapsed_ms: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ErrorDiagnostic {
    spelling: String,
    file: String,
    line: u32,
    column: u32,
    offset: u32,
}

struct ConstantSources<'a, 'tu> {
    macros: &'a MacroDefinitions<'tu>,
    translation_unit: &'a TranslationUnit,
}

impl TranslationUnit {
    fn parse(index: &Index, input: &Input, args: &[&str]) -> Result<Self, Error> {
        let name = CString::new(input.name.as_str())
            .map_err(|_| Error(format!("invalid input name `{}`", input.name)))?;
        let source = CString::new(input.source.as_str())
            .map_err(|_| Error(format!("input `{}` contains a null byte", input.name)))?;
        let args: Result<Vec<_>, _> = args
            .iter()
            .map(|arg| CString::new(*arg).map_err(|_| Error(format!("invalid argument `{arg}`"))))
            .collect();
        let args = args?;
        let arg_pointers: Vec<_> = args.iter().map(|arg| arg.as_ptr()).collect();
        let mut unsaved = CXUnsavedFile {
            Filename: name.as_ptr(),
            Contents: source.as_ptr(),
            Length: input.source.len().try_into().unwrap(),
        };
        let value = unsafe {
            clang_parseTranslationUnit(
                index.0,
                name.as_ptr(),
                arg_pointers.as_ptr(),
                arg_pointers.len().try_into().unwrap(),
                &mut unsaved,
                1,
                CXTranslationUnit_DetailedPreprocessingRecord
                    | CXTranslationUnit_SkipFunctionBodies,
            )
        };
        if value.is_null() {
            return Err(Error(format!("failed to parse `{}`", input.name)));
        }

        let result = Self(value);
        let errors = result.errors();
        if errors.is_empty() {
            Ok(result)
        } else {
            Err(Error(errors.join("\n")))
        }
    }

    fn parse_probe(
        index: &Index,
        input: &Input,
        probe: &str,
        args: &[&str],
    ) -> Result<Self, Error> {
        let synthetic_name = format!("{}.__clang_eval.cpp", input.name);
        let synthetic_source = format!("{}\n{probe}", input.source);
        let name = CString::new(synthetic_name.as_str()).unwrap();
        let source = CString::new(synthetic_source).unwrap();
        let mut unsaved = CXUnsavedFile {
            Filename: name.as_ptr(),
            Contents: source.as_ptr(),
            Length: source.as_bytes().len().try_into().unwrap(),
        };
        let mut args: Vec<_> = args.iter().map(|arg| CString::new(*arg).unwrap()).collect();
        args.push(CString::new("-ferror-limit=0").unwrap());
        let arg_pointers: Vec<_> = args.iter().map(|arg| arg.as_ptr()).collect();
        let value = unsafe {
            clang_parseTranslationUnit(
                index.0,
                name.as_ptr(),
                arg_pointers.as_ptr(),
                arg_pointers.len().try_into().unwrap(),
                &mut unsaved,
                1,
                CXTranslationUnit_KeepGoing | CXTranslationUnit_SkipFunctionBodies,
            )
        };
        if value.is_null() {
            Err(Error(format!(
                "failed to evaluate macro in `{}`",
                input.name
            )))
        } else {
            Ok(Self(value))
        }
    }

    fn errors(&self) -> Vec<String> {
        let mut result = vec![];
        let count = unsafe { clang_getNumDiagnostics(self.0) };
        for index in 0..count {
            let diagnostic = unsafe { clang_getDiagnostic(self.0, index) };
            let severity = unsafe { clang_getDiagnosticSeverity(diagnostic) };
            let spelling = cx_string(unsafe { clang_getDiagnosticSpelling(diagnostic) });
            if severity >= CXDiagnostic_Error
                && spelling != "expression is not an integral constant expression"
            {
                result.push(cx_string(unsafe {
                    clang_formatDiagnostic(diagnostic, clang_defaultDiagnosticDisplayOptions())
                }));
            }
            unsafe { clang_disposeDiagnostic(diagnostic) };
        }
        result
    }

    fn error_diagnostics(&self) -> Vec<ErrorDiagnostic> {
        let mut result = vec![];
        let count = unsafe { clang_getNumDiagnostics(self.0) };
        for index in 0..count {
            let diagnostic = unsafe { clang_getDiagnostic(self.0, index) };
            if unsafe { clang_getDiagnosticSeverity(diagnostic) } >= CXDiagnostic_Error {
                let location = unsafe { clang_getDiagnosticLocation(diagnostic) };
                let mut file = std::ptr::null_mut();
                let mut line = 0;
                let mut column = 0;
                let mut offset = 0;
                unsafe {
                    clang_getExpansionLocation(
                        location,
                        &mut file,
                        &mut line,
                        &mut column,
                        &mut offset,
                    );
                }
                result.push(ErrorDiagnostic {
                    spelling: cx_string(unsafe { clang_getDiagnosticSpelling(diagnostic) }),
                    file: if file.is_null() {
                        String::new()
                    } else {
                        normalize_name(&cx_string(unsafe { clang_getFileName(file) }))
                    },
                    line,
                    column,
                    offset,
                });
            }
            unsafe { clang_disposeDiagnostic(diagnostic) };
        }
        result
    }

    fn included_files(&self, input: &str) -> Vec<IncludedFile> {
        struct Visit {
            paths: Vec<String>,
            panic: Option<Box<dyn std::any::Any + Send>>,
        }

        extern "C" fn visit(
            file: CXFile,
            _inclusion_stack: *mut CXSourceLocation,
            _include_len: u32,
            data: CXClientData,
        ) {
            let visit = unsafe { &mut *(data as *mut Visit) };
            if visit.panic.is_some() {
                return;
            }
            if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if !file.is_null() {
                    let path = normalize_name(&cx_string(unsafe { clang_getFileName(file) }));
                    if !path.is_empty() {
                        visit.paths.push(path);
                    }
                }
            })) {
                visit.panic = Some(panic);
            }
        }

        let mut state = Visit {
            paths: vec![],
            panic: None,
        };
        unsafe {
            clang_getInclusions(self.0, visit, &mut state as *mut _ as CXClientData);
        }
        if let Some(panic) = state.panic {
            std::panic::resume_unwind(panic);
        }

        state.paths.sort_by_cached_key(|path| {
            let folded = path.to_ascii_lowercase();
            (folded, path.clone())
        });
        state
            .paths
            .dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        state
            .paths
            .into_iter()
            .map(|path| IncludedFile {
                input: input.to_string(),
                path,
            })
            .collect()
    }

    fn extract<'tu>(
        &'tu self,
        input: &Input,
        output: &mut ExtractionState<'_>,
        timing: bool,
        validate_annotations: bool,
    ) -> Result<(Extracted<'tu>, Option<ExtractionMetrics>), Error> {
        let total_time = timing.then(std::time::Instant::now);
        let phase_time = timing.then(std::time::Instant::now);
        let macros = macro_definitions(self, unsafe { clang_getTranslationUnitCursor(self.0) });
        let macro_index_ms = elapsed_ms(phase_time);
        let phase_time = timing.then(std::time::Instant::now);
        let initial_facts = output.facts.len();
        let initial_constants = output.constants.len();
        let mut traversal = Traversal {
            tu: &input.name,
            roots: &input.roots,
            root_dirs: &input.root_dirs,
            root_suffixes: &input.root_suffixes,
            excluded_roots: &input.excluded_roots,
            next: 0,
            seen: HashMap::new(),
            canonical_functions: HashMap::new(),
            macros: &macros,
            pending_structs: vec![],
            pending_macros: vec![],
            declare_handle_expansions: vec![],
            facts: &mut *output.facts,
            constants: &mut *output.constants,
            annotations: &mut *output.annotations,
            declaration_guids: &mut *output.declaration_guids,
            raw_function_link_names: &mut *output.raw_function_link_names,
            canonical_function_origins: &mut *output.canonical_function_origins,
            pointer_callback_aliases: &mut *output.pointer_callback_aliases,
            pointer_only_class_layouts: &mut *output.pointer_only_class_layouts,
            embeddable_class_layouts: &mut *output.embeddable_class_layouts,
            clang_flag_enums: &mut *output.clang_flag_enums,
            error: None,
            validate_annotations,
        };
        extract_children(
            unsafe { clang_getTranslationUnitCursor(self.0) },
            None,
            &mut traversal,
        );
        let traversal_ms = elapsed_ms(phase_time);
        if let Some(error) = traversal.error.take() {
            return Err(error);
        }
        let metrics = timing.then(|| ExtractionMetrics {
            macro_definitions: macros.definitions.len(),
            macro_expansion_files: macros.expansion_orders.len(),
            cursors: traversal.next,
            facts: traversal.facts.len() - initial_facts,
            constants: traversal.constants.len() - initial_constants,
            macro_index_ms,
            traversal_ms,
            elapsed_ms: elapsed_ms(total_time),
        });
        let pending_structs = std::mem::take(&mut traversal.pending_structs);
        let pending_macros = std::mem::take(&mut traversal.pending_macros);
        let declare_handle_expansions = std::mem::take(&mut traversal.declare_handle_expansions);
        drop(traversal);
        Ok((
            Extracted {
                macros,
                pending_structs,
                pending_macros,
                declare_handle_expansions,
            },
            metrics,
        ))
    }
}

impl Drop for TranslationUnit {
    fn drop(&mut self) {
        unsafe { clang_disposeTranslationUnit(self.0) };
    }
}

struct Traversal<'a> {
    tu: &'a str,
    roots: &'a BTreeSet<String>,
    root_dirs: &'a BTreeSet<String>,
    root_suffixes: &'a BTreeSet<String>,
    excluded_roots: &'a BTreeSet<String>,
    next: u32,
    seen: HashMap<u32, Vec<(CXCursor, Origin)>>,
    canonical_functions: HashMap<u32, Vec<(CXCursor, Origin)>>,
    macros: &'a MacroDefinitions<'a>,
    pending_structs: Vec<(usize, CXCursor)>,
    pending_macros: Vec<(usize, CXCursor)>,
    declare_handle_expansions: Vec<DeclareHandleExpansion>,
    facts: &'a mut Vec<Fact>,
    constants: &'a mut Vec<Constant>,
    annotations: &'a mut BTreeMap<AnnotationTarget, Vec<Annotation>>,
    declaration_guids: &'a mut BTreeMap<Origin, String>,
    raw_function_link_names: &'a mut BTreeMap<Origin, String>,
    canonical_function_origins: &'a mut BTreeMap<Origin, Origin>,
    pointer_callback_aliases: &'a mut BTreeSet<Origin>,
    pointer_only_class_layouts: &'a mut BTreeMap<Origin, FactData>,
    embeddable_class_layouts: &'a mut BTreeSet<Origin>,
    clang_flag_enums: &'a mut BTreeSet<Origin>,
    error: Option<Error>,
    validate_annotations: bool,
}

struct ExtractionState<'a> {
    facts: &'a mut Vec<Fact>,
    constants: &'a mut Vec<Constant>,
    annotations: &'a mut BTreeMap<AnnotationTarget, Vec<Annotation>>,
    declaration_guids: &'a mut BTreeMap<Origin, String>,
    raw_function_link_names: &'a mut BTreeMap<Origin, String>,
    canonical_function_origins: &'a mut BTreeMap<Origin, Origin>,
    pointer_callback_aliases: &'a mut BTreeSet<Origin>,
    pointer_only_class_layouts: &'a mut BTreeMap<Origin, FactData>,
    embeddable_class_layouts: &'a mut BTreeSet<Origin>,
    clang_flag_enums: &'a mut BTreeSet<Origin>,
}

struct Extracted<'tu> {
    macros: MacroDefinitions<'tu>,
    pending_structs: Vec<(usize, CXCursor)>,
    pending_macros: Vec<(usize, CXCursor)>,
    declare_handle_expansions: Vec<DeclareHandleExpansion>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DeclareHandleExpansion {
    name: String,
    location: Location,
}

impl Traversal<'_> {
    fn is_root(&self, file: &str) -> bool {
        is_root_path(
            self.roots,
            self.root_dirs,
            self.root_suffixes,
            self.excluded_roots,
            file,
        )
    }

    fn is_source_excluded(&self, file: &str) -> bool {
        self.excluded_roots.iter().any(|root| {
            root.ends_with('/') && source_path_is_under(file, root.trim_end_matches('/'))
        })
    }
}

fn is_root_path(
    roots: &BTreeSet<String>,
    root_dirs: &BTreeSet<String>,
    root_suffixes: &BTreeSet<String>,
    excluded_roots: &BTreeSet<String>,
    file: &str,
) -> bool {
    !excluded_roots.iter().any(|root| {
        if root.ends_with('/') {
            source_path_is_under(file, root.trim_end_matches('/'))
        } else {
            source_path_matches(root, file)
        }
    }) && (roots.iter().any(|root| source_path_matches(root, file))
        || root_dirs
            .iter()
            .any(|root| source_path_is_under(file, root.trim_end_matches('/')))
        || root_suffixes
            .iter()
            .any(|root| source_path_matches(root, file)))
}

fn source_path_is_under(path: &str, root: &str) -> bool {
    let path = normalize_name(path).to_ascii_lowercase();
    let root = normalize_name(root)
        .trim_end_matches('/')
        .to_ascii_lowercase();
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn extract_children(cursor: CXCursor, parent: Option<&Origin>, traversal: &mut Traversal<'_>) {
    struct Visit<'parent, 'traversal, 'facts> {
        parent: Option<&'parent Origin>,
        traversal: &'traversal mut Traversal<'facts>,
        panic: Option<Box<dyn std::any::Any + Send>>,
    }

    extern "C" fn visit(
        cursor: CXCursor,
        parent_cursor: CXCursor,
        data: CXClientData,
    ) -> CXChildVisitResult {
        let visit = unsafe { &mut *(data as *mut Visit<'_, '_, '_>) };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            extract_child(cursor, parent_cursor, visit.parent, visit.traversal);
        })) {
            Ok(()) if visit.traversal.error.is_none() => CXChildVisit_Continue,
            Ok(()) => CXChildVisit_Break,
            Err(panic) => {
                visit.panic = Some(panic);
                CXChildVisit_Break
            }
        }
    }

    let mut state = Visit {
        parent,
        traversal,
        panic: None,
    };
    unsafe {
        clang_visitChildren(cursor, visit, &mut state as *mut _ as CXClientData);
    }
    if let Some(panic) = state.panic {
        std::panic::resume_unwind(panic);
    }
}

fn extract_child(
    child: CXCursor,
    cursor_parent: CXCursor,
    parent: Option<&Origin>,
    traversal: &mut Traversal<'_>,
) {
    if let Some((spelling, _, _, _)) = cursor_locations(child)
        && traversal.is_source_excluded(&spelling.file)
    {
        return;
    }
    let local = traversal.next;
    traversal.next += 1;
    let kind = unsafe { clang_getCursorKind(child) };
    if kind == CXCursor_AnnotateAttr {
        if !traversal.validate_annotations {
            return;
        }
        let spelling = cx_string(unsafe { clang_getCursorSpelling(child) });
        if spelling.starts_with("win32metadata:")
            && let Err(error) = validate_win32metadata_annotation(cursor_parent, child, &spelling)
        {
            traversal.error = Some(error);
        }
        return;
    }
    let mut child_parent = None;
    let mut repeated = false;

    let mut name = if matches!(
        kind,
        CXCursor_FunctionDecl
            | CXCursor_VarDecl
            | CXCursor_MacroExpansion
            | CXCursor_ClassDecl
            | CXCursor_EnumDecl
            | CXCursor_MacroDefinition
            | CXCursor_Namespace
            | CXCursor_StructDecl
            | CXCursor_TypedefDecl
            | CXCursor_UnionDecl
    ) {
        cx_string(unsafe { clang_getCursorSpelling(child) })
    } else {
        String::new()
    };
    if kind == CXCursor_FunctionDecl
        && let Some(source_name) = source_function_name(child, &name, traversal.macros)
    {
        name = source_name;
    }
    if kind == CXCursor_MacroExpansion
        && name == "DECLARE_HANDLE"
        && let Some(handle) = declare_handle_name(&cursor_tokens(child))
        && let Some((_, expansion, _, _)) = cursor_locations(child)
    {
        traversal
            .declare_handle_expansions
            .push(DeclareHandleExpansion {
                name: handle.to_string(),
                location: expansion,
            });
    }
    if kind == CXCursor_VarDecl
        && !name.is_empty()
        && let Some((spelling, _, _, _)) = cursor_locations(child)
        && traversal.is_root(&spelling.file)
        && variable_is_global(child)
    {
        let ty = unsafe { clang_getCursorType(child) };
        if let Some((ty, value)) = evaluate_variable_constant(child, ty) {
            let origin = Origin {
                tu: traversal.tu.to_string(),
                local,
            };
            let annotations = match annotation_values(child, traversal.macros) {
                Ok(annotations) => annotations,
                Err(error) => {
                    traversal.error = Some(error);
                    return;
                }
            };
            traversal.constants.push(Constant {
                root: origin.clone(),
                definition: origin.clone(),
                spelling,
                name: name.clone(),
                ty,
                value,
            });
            insert_annotations(
                traversal.annotations,
                AnnotationTarget::Declaration(origin),
                annotations,
            );
        }
    }
    let anonymous_enum =
        kind == CXCursor_EnumDecl && unsafe { clang_Cursor_isAnonymous(child) } != 0;
    if anonymous_enum
        && let Some((spelling, _, _, _)) = cursor_locations(child)
        && traversal.is_root(&spelling.file)
    {
        let ty = unsafe { clang_getEnumDeclIntegerType(child) };
        if let Some(repr) = scalar(ty) {
            let origin = Origin {
                tu: traversal.tu.to_string(),
                local,
            };
            for constant in cursor_children(child).into_iter().filter(|cursor| unsafe {
                clang_getCursorKind(*cursor) == CXCursor_EnumConstantDecl
            }) {
                let value = if matches!(repr, Scalar::U8 | Scalar::U16 | Scalar::U32 | Scalar::U64)
                {
                    Value::Unsigned(unsafe { clang_getEnumConstantDeclUnsignedValue(constant) })
                } else {
                    Value::Signed(unsafe { clang_getEnumConstantDeclValue(constant) })
                };
                traversal.constants.push(Constant {
                    root: origin.clone(),
                    definition: origin.clone(),
                    spelling: spelling.clone(),
                    name: cx_string(unsafe { clang_getCursorSpelling(constant) }),
                    ty: TypeRef::Scalar(repr),
                    value,
                });
            }
        }
    }
    let fact_kind = if kind == CXCursor_MacroExpansion && name == "DEFINE_ENUM_FLAG_OPERATORS" {
        Some(FactKind::EnumFlag)
    } else if kind == CXCursor_MacroExpansion
        && matches!(
            name.as_str(),
            "DEFINE_GUID" | "DEFINE_OLEGUID" | "DEFINE_KNOWN_FOLDER"
        )
    {
        let ole = name == "DEFINE_OLEGUID";
        parse_define_guid_tokens(&cursor_tokens(child), ole).map(|(guid_name, _)| {
            name = guid_name;
            FactKind::Guid
        })
    } else if kind == CXCursor_MacroExpansion
        && matches!(name.as_str(), "DEFINE_PROPERTYKEY" | "DEFINE_DEVPROPKEY")
    {
        parse_property_key_tokens(&cursor_tokens(child)).map(|(key_name, _, _)| {
            name = key_name;
            FactKind::Guid
        })
    } else if kind == CXCursor_VarDecl
        && variable_is_global(child)
        && variable_guid(child).is_some()
    {
        Some(FactKind::Guid)
    } else if anonymous_enum {
        None
    } else {
        fact_kind(kind)
    };
    if let Some(fact_kind) = fact_kind {
        let clang_flag_enum = fact_kind == FactKind::Enum && enum_has_flag_attribute(child);
        let anonymous_record = matches!(kind, CXCursor_StructDecl | CXCursor_UnionDecl)
            && unsafe { clang_Cursor_isAnonymous(child) } != 0;
        if !name.is_empty() && !anonymous_record {
            let hash = unsafe { clang_hashCursor(child) };
            let seen = traversal.seen.entry(hash).or_default();
            if let Some((_, origin)) = seen
                .iter()
                .find(|(cursor, _)| unsafe { clang_equalCursors(*cursor, child) } != 0)
            {
                child_parent = Some(origin.clone());
                repeated = true;
                if clang_flag_enum {
                    traversal.clang_flag_enums.insert(origin.clone());
                }
            } else if let Some((spelling, expansion, _, system)) = cursor_locations(child) {
                let main_file = spelling.file == traversal.tu;
                let root = is_root_path(
                    traversal.roots,
                    traversal.root_dirs,
                    traversal.root_suffixes,
                    traversal.excluded_roots,
                    &spelling.file,
                );
                let origin = Origin {
                    tu: traversal.tu.to_string(),
                    local,
                };
                seen.push((child, origin.clone()));
                child_parent = Some(origin.clone());
                let annotated_function =
                    fact_kind == FactKind::Function && has_win32metadata_annotation(child);
                if root
                    || !matches!(fact_kind, FactKind::Function | FactKind::Guid)
                    || annotated_function
                {
                    if fact_kind == FactKind::Macro
                        && unsafe { clang_Cursor_isMacroFunctionLike(child) } != 0
                    {
                        return;
                    }
                    let deferred_struct = !root && fact_kind == FactKind::Struct;
                    let deferred_macro = !root && fact_kind == FactKind::Macro;
                    let data = if deferred_struct || deferred_macro {
                        FactData::None
                    } else {
                        fact_data(child, fact_kind, traversal.macros)
                    };
                    let raw_function_link_name =
                        (fact_kind == FactKind::Function).then(|| raw_external_link_name(child));
                    if fact_kind == FactKind::Typedef
                        && matches!(data, FactData::Callback { .. })
                        && typedef_is_function_pointer_alias(child)
                    {
                        traversal.pointer_callback_aliases.insert(origin.clone());
                    }
                    if fact_kind == FactKind::Class {
                        if let Some(layout) = native_opaque_class_layout(child) {
                            traversal
                                .pointer_only_class_layouts
                                .insert(origin.clone(), layout);
                        } else if matches!(data, FactData::Unsupported { .. })
                            && let Some((layout, embeddable)) =
                                pointer_only_class_layout(child, traversal.macros)
                        {
                            traversal
                                .pointer_only_class_layouts
                                .insert(origin.clone(), layout);
                            if embeddable {
                                traversal.embeddable_class_layouts.insert(origin.clone());
                            }
                        }
                    }
                    let index = traversal.facts.len();
                    let declaration_guid = matches!(fact_kind, FactKind::Class | FactKind::Struct)
                        .then(|| cursor_uuid(child))
                        .flatten();
                    traversal.facts.push(Fact {
                        origin: origin.clone(),
                        parent: parent.cloned(),
                        kind: fact_kind,
                        name,
                        spelling,
                        expansion,
                        definition: unsafe { clang_isCursorDefinition(child) } != 0,
                        main_file,
                        root,
                        system,
                        data,
                    });
                    if let Some(raw_function_link_name) = raw_function_link_name {
                        traversal
                            .raw_function_link_names
                            .insert(origin.clone(), raw_function_link_name);
                    }
                    if fact_kind == FactKind::Function {
                        let canonical = unsafe { clang_getCanonicalCursor(child) };
                        let hash = unsafe { clang_hashCursor(canonical) };
                        let declarations = traversal.canonical_functions.entry(hash).or_default();
                        let canonical_origin = if let Some((_, canonical_origin)) =
                            declarations.iter().find(|(declaration, _)| unsafe {
                                clang_equalCursors(*declaration, canonical) != 0
                            }) {
                            canonical_origin.clone()
                        } else {
                            declarations.push((canonical, origin.clone()));
                            origin.clone()
                        };
                        traversal
                            .canonical_function_origins
                            .insert(origin.clone(), canonical_origin);
                    }
                    if clang_flag_enum {
                        traversal.clang_flag_enums.insert(origin.clone());
                    }
                    if let Some(guid) = declaration_guid {
                        traversal.declaration_guids.insert(origin.clone(), guid);
                    }
                    if let Err(error) = collect_fact_annotations(
                        child,
                        fact_kind,
                        &origin,
                        traversal.macros,
                        traversal.annotations,
                    ) {
                        traversal.error = Some(error);
                        return;
                    }
                    if deferred_struct {
                        traversal.pending_structs.push((index, child));
                    }
                    if deferred_macro {
                        traversal.pending_macros.push((index, child));
                    }
                }
            }
        }
    }

    if !repeated {
        extract_children(child, child_parent.as_ref().or(parent), traversal);
    }
}

fn has_win32metadata_annotation(cursor: CXCursor) -> bool {
    cursor_children(cursor).into_iter().any(|child| {
        (unsafe { clang_getCursorKind(child) }) == CXCursor_AnnotateAttr
            && cx_string(unsafe { clang_getCursorSpelling(child) }).starts_with("win32metadata:")
    })
}

fn enum_has_flag_attribute(cursor: CXCursor) -> bool {
    cursor_children(cursor)
        .into_iter()
        .any(|child| unsafe { clang_getCursorKind(child) } == CXCursor_FlagEnum)
}

fn materialize_anonymous_callbacks(facts: &mut Vec<Fact>) {
    let mut used: BTreeSet<_> = facts.iter().map(|fact| fact.name.clone()).collect();
    let mut next_local: HashMap<_, u32> = HashMap::new();
    for fact in facts.iter() {
        next_local
            .entry(fact.origin.tu.clone())
            .and_modify(|local| *local = (*local).max(fact.origin.local + 1))
            .or_insert(fact.origin.local + 1);
    }

    let mut callbacks = vec![];
    for fact in facts.iter_mut() {
        let owner = SyntheticOwner {
            origin: fact.origin.clone(),
            name: fact.name.clone(),
            spelling: fact.spelling.clone(),
            expansion: fact.expansion.clone(),
            main_file: fact.main_file,
            root: fact.root,
            system: fact.system,
        };
        let fields = match &mut fact.data {
            FactData::Record { fields, .. } => fields,
            FactData::Typedef {
                target: TypeRef::InlineRecord(record),
            } => &mut record.fields,
            _ => continue,
        };
        materialize_field_callbacks(fields, &owner, &mut used, &mut next_local, &mut callbacks);
    }
    facts.extend(callbacks);
}

struct SyntheticOwner {
    origin: Origin,
    name: String,
    spelling: Location,
    expansion: Location,
    main_file: bool,
    root: bool,
    system: bool,
}

fn materialize_field_callbacks(
    fields: &mut [Field],
    owner: &SyntheticOwner,
    used: &mut BTreeSet<String>,
    next_local: &mut HashMap<String, u32>,
    callbacks: &mut Vec<Fact>,
) {
    for field in fields {
        let stem = format!(
            "{}_{}",
            owner.name.trim_start_matches('_'),
            field.name.trim_start_matches('_')
        );
        materialize_type_callbacks(&mut field.ty, &stem, owner, used, next_local, callbacks);
    }
}

fn materialize_type_callbacks(
    ty: &mut TypeRef,
    stem: &str,
    owner: &SyntheticOwner,
    used: &mut BTreeSet<String>,
    next_local: &mut HashMap<String, u32>,
    callbacks: &mut Vec<Fact>,
) {
    match ty {
        TypeRef::FunctionPointer {
            convention,
            params,
            result,
        } => {
            let name = unique_synthetic_name(stem, used);
            let local = next_local.entry(owner.origin.tu.clone()).or_default();
            let origin = Origin {
                tu: owner.origin.tu.clone(),
                local: *local,
            };
            *local += 1;
            let callback = Fact {
                origin,
                parent: None,
                kind: FactKind::Typedef,
                name: name.clone(),
                spelling: owner.spelling.clone(),
                expansion: owner.expansion.clone(),
                definition: true,
                main_file: owner.main_file,
                root: owner.root,
                system: owner.system,
                data: FactData::Callback {
                    convention: *convention,
                    params: params
                        .iter()
                        .enumerate()
                        .map(|(index, ty)| Parameter {
                            name: format!("arg{index}"),
                            ty: ty.clone(),
                            annotation: ParamAnnotation::default(),
                        })
                        .collect(),
                    result: result.as_ref().clone(),
                },
            };
            callbacks.push(callback);
            *ty = TypeRef::Named {
                name,
                declaration: owner.spelling.clone(),
            };
        }
        TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. }
        | TypeRef::Array { target, .. } => {
            materialize_type_callbacks(target, stem, owner, used, next_local, callbacks);
        }
        TypeRef::InlineRecord(record) => {
            for field in &mut record.fields {
                let nested = format!("{stem}_{}", field.name.trim_start_matches('_'));
                materialize_type_callbacks(
                    &mut field.ty,
                    &nested,
                    owner,
                    used,
                    next_local,
                    callbacks,
                );
            }
        }
        _ => {}
    }
}

fn unique_synthetic_name(stem: &str, used: &mut BTreeSet<String>) -> String {
    let mut name = stem.to_string();
    let mut suffix = 2;
    while !used.insert(name.clone()) {
        name = format!("{stem}_{suffix}");
        suffix += 1;
    }
    name
}

fn decode_reachable_structs(
    facts: &mut [Fact],
    constants: &[Constant],
    extracted: &[Extracted<'_>],
) {
    let mut reachable: HashSet<String> = facts
        .iter()
        .filter(|fact| fact.root)
        .map(|fact| fact.name.clone())
        .collect();
    for fact in facts.iter() {
        fact_type_names(&fact.data, &mut reachable);
    }
    for constant in constants {
        type_names(&constant.ty, &mut reachable);
    }
    let mut pending: HashMap<String, Vec<(usize, usize, CXCursor)>> = HashMap::new();
    for (extraction_index, extraction) in extracted.iter().enumerate() {
        for &(fact_index, cursor) in &extraction.pending_structs {
            pending
                .entry(facts[fact_index].name.clone())
                .or_default()
                .push((extraction_index, fact_index, cursor));
        }
    }
    let mut queue: Vec<_> = reachable.iter().cloned().collect();
    while let Some(name) = queue.pop() {
        let Some(candidates) = pending.remove(&name) else {
            continue;
        };
        for (extraction_index, fact_index, cursor) in candidates {
            let data = fact_data(
                cursor,
                FactKind::Struct,
                &extracted[extraction_index].macros,
            );
            let mut dependencies = HashSet::new();
            fact_type_names(&data, &mut dependencies);
            for dependency in dependencies {
                if reachable.insert(dependency.clone()) {
                    queue.push(dependency);
                }
            }
            facts[fact_index].data = data;
        }
    }
}

fn decode_selected_macro_definitions(
    facts: &mut [Fact],
    extracted: &[Extracted<'_>],
    selected: &BTreeSet<String>,
) {
    let mut roots: HashSet<_> = facts
        .iter()
        .filter(|fact| fact.root && fact.kind == FactKind::Macro)
        .map(|fact| fact.name.clone())
        .collect();
    roots.extend(selected.iter().cloned());
    for extraction in extracted {
        for &(index, cursor) in &extraction.pending_macros {
            if roots.contains(facts[index].name.as_str()) {
                facts[index].data = fact_data(cursor, FactKind::Macro, &extraction.macros);
            }
        }
    }
}

fn fact_type_names(data: &FactData, names: &mut HashSet<String>) {
    let mut visit = |ty| type_names(ty, names);
    match data {
        FactData::Typedef { target } => visit(target),
        FactData::Callback { params, result, .. } | FactData::Function { params, result, .. } => {
            visit(result);
            for param in params {
                visit(&param.ty);
            }
        }
        FactData::Record { base, fields, .. } => {
            if let Some(base) = base {
                visit(base);
            }
            for field in fields {
                visit(&field.ty);
            }
        }
        FactData::Interface { base, methods, .. } => {
            if let Some(base) = base {
                visit(base);
            }
            for method in methods {
                visit(&method.result);
                for param in &method.params {
                    visit(&param.ty);
                }
            }
        }
        FactData::PropertyKey { ty, .. } => {
            names.insert((*ty).to_string());
        }
        FactData::EnumFlag { target } => {
            names.insert(target.clone());
        }
        _ => {}
    }
}

fn type_names(ty: &TypeRef, names: &mut HashSet<String>) {
    match ty {
        TypeRef::Named { name, .. } => {
            names.insert(name.clone());
        }
        TypeRef::Generic { name, args, .. } => {
            names.insert(name.clone());
            for arg in args {
                type_names(arg, names);
            }
        }
        TypeRef::Array { target, .. }
        | TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. } => type_names(target, names),
        TypeRef::FunctionPointer { params, result, .. } => {
            type_names(result, names);
            for param in params {
                type_names(param, names);
            }
        }
        TypeRef::InlineRecord(record) => {
            if let Some(base) = &record.base {
                type_names(base, names);
            }
            for field in &record.fields {
                type_names(&field.ty, names);
            }
        }
        _ => {}
    }
}

fn cursor_children(cursor: CXCursor) -> Vec<CXCursor> {
    struct Visit {
        children: Vec<CXCursor>,
        panic: Option<Box<dyn std::any::Any + Send>>,
    }

    extern "C" fn visit(
        cursor: CXCursor,
        _parent: CXCursor,
        data: CXClientData,
    ) -> CXChildVisitResult {
        let visit = unsafe { &mut *(data as *mut Visit) };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            visit.children.push(cursor);
        })) {
            Ok(()) => CXChildVisit_Continue,
            Err(panic) => {
                visit.panic = Some(panic);
                CXChildVisit_Break
            }
        }
    }

    let mut state = Visit {
        children: vec![],
        panic: None,
    };
    unsafe {
        clang_visitChildren(cursor, visit, &mut state as *mut _ as CXClientData);
    }
    if let Some(panic) = state.panic {
        std::panic::resume_unwind(panic);
    }
    state.children
}

#[derive(Default)]
struct MacroDefinitions<'tu> {
    definitions: HashMap<String, Vec<MacroDefinition>>,
    expansion_orders: HashMap<String, Vec<(u32, usize)>>,
    cursor_orders: HashMap<String, Vec<(u32, u32, usize)>>,
    translation_unit: PhantomData<&'tu TranslationUnit>,
}

struct MacroDefinition {
    order: usize,
    cursor: CXCursor,
    spelling: Option<Location>,
    function_like: bool,
    tokens: OnceCell<Vec<String>>,
}

impl MacroDefinition {
    fn tokens(&self) -> &[String] {
        self.tokens.get_or_init(|| {
            cursor_tokens(self.cursor)
                .into_iter()
                .map(|(_, token)| token)
                .skip(1)
                .collect()
        })
    }
}

impl MacroDefinitions<'_> {
    fn contains_key(&self, name: &str) -> bool {
        self.definitions.contains_key(name)
    }

    fn final_value(&self, name: &str) -> Option<&[String]> {
        self.definitions
            .get(name)?
            .last()
            .map(MacroDefinition::tokens)
    }

    fn definition(&self, cursor: CXCursor, name: &str) -> Option<(&[String], bool)> {
        let definition = self
            .definitions
            .get(name)?
            .iter()
            .find(|definition| unsafe { clang_equalCursors(definition.cursor, cursor) } != 0)?;
        Some((definition.tokens(), definition.function_like))
    }

    fn get_before(&self, name: &str, order: usize) -> Option<&[String]> {
        self.definitions
            .get(name)?
            .iter()
            .filter(|definition| definition.order < order)
            .max_by_key(|definition| definition.order)
            .map(MacroDefinition::tokens)
    }

    fn definition_before(
        &self,
        name: &str,
        order: usize,
        location: Option<&Location>,
    ) -> Option<(&[String], bool)> {
        let definitions = self.definitions.get(name)?;
        let definition = location
            .and_then(|location| {
                definitions
                    .iter()
                    .filter(|definition| {
                        definition.spelling.as_ref().is_some_and(|spelling| {
                            spelling.file == location.file && spelling.offset <= location.offset
                        })
                    })
                    .max_by_key(|definition| {
                        definition
                            .spelling
                            .as_ref()
                            .map_or(0, |spelling| spelling.offset)
                    })
            })
            .or_else(|| {
                definitions
                    .iter()
                    .filter(|definition| definition.order < order)
                    .max_by_key(|definition| definition.order)
            })?;
        Some((definition.tokens(), definition.function_like))
    }

    fn expansion_order(&self, cursor: CXCursor) -> Option<usize> {
        let (file, start, end) = cursor_expansion_extent(cursor)?;
        let expansions = self.expansion_orders.get(&file)?;
        let index = expansions.partition_point(|(offset, _)| *offset < start);
        expansions
            .get(index)
            .filter(|(offset, _)| *offset <= end)
            .map(|(_, order)| *order)
    }

    fn cursor_order(&self, cursor: CXCursor) -> Option<usize> {
        self.expansion_order(cursor).or_else(|| {
            let (file, start, end) = cursor_expansion_extent(cursor)?;
            self.cursor_orders
                .get(&file)?
                .iter()
                .filter(|(candidate_start, candidate_end, _)| {
                    *candidate_start <= start && *candidate_end >= end
                })
                .min_by_key(|(candidate_start, candidate_end, _)| candidate_end - candidate_start)
                .map(|(_, _, order)| *order)
        })
    }

    fn final_definition(&self, name: &str) -> Option<(&[String], bool)> {
        let definition = self.definitions.get(name)?.last()?;
        Some((definition.tokens(), definition.function_like))
    }
}

fn macro_definitions<'tu>(
    _translation_unit: &'tu TranslationUnit,
    cursor: CXCursor,
) -> MacroDefinitions<'tu> {
    let mut result = MacroDefinitions::default();
    for (order, child) in cursor_children(cursor).into_iter().enumerate() {
        let kind = unsafe { clang_getCursorKind(child) };
        if let Some((file, start, end)) = cursor_expansion_extent(child) {
            result
                .cursor_orders
                .entry(file)
                .or_default()
                .push((start, end, order));
        }
        if kind == CXCursor_MacroDefinition {
            let name = cx_string(unsafe { clang_getCursorSpelling(child) });
            let function_like = unsafe { clang_Cursor_isMacroFunctionLike(child) } != 0;
            result
                .definitions
                .entry(name)
                .or_default()
                .push(MacroDefinition {
                    order,
                    cursor: child,
                    spelling: cursor_locations(child).map(|(spelling, _, _, _)| spelling),
                    function_like,
                    tokens: OnceCell::new(),
                });
        } else if kind == CXCursor_MacroExpansion
            && let Some((_, expansion, _, _)) = cursor_locations(child)
        {
            result
                .expansion_orders
                .entry(expansion.file)
                .or_default()
                .push((expansion.offset, order));
        }
    }
    for expansions in result.expansion_orders.values_mut() {
        expansions.sort_unstable_by_key(|(offset, order)| (*offset, std::cmp::Reverse(*order)));
        expansions.dedup_by_key(|(offset, _)| *offset);
    }
    result
}

fn cursor_expansion_extent(cursor: CXCursor) -> Option<(String, u32, u32)> {
    let range = unsafe { clang_getCursorExtent(cursor) };
    let start = source_location(
        unsafe { clang_getRangeStart(range) },
        clang_getExpansionLocation,
    )?;
    let end = source_location(
        unsafe { clang_getRangeEnd(range) },
        clang_getExpansionLocation,
    )?;
    (start.file == end.file).then_some((start.file, start.offset, end.offset))
}

fn source_function_name(
    cursor: CXCursor,
    link_name: &str,
    macros: &MacroDefinitions,
) -> Option<String> {
    let tokens = cursor_tokens(cursor);
    let token_name = tokens
        .iter()
        .enumerate()
        .filter(|(index, _)| tokens.get(index + 1).is_some_and(|(_, token)| token == "("))
        .map(|(_, (_, token))| token)
        .filter(|token| token.as_str() != link_name)
        .filter(|token| {
            macros
                .final_value(token)
                .is_some_and(|replacement| replacement == [link_name])
        })
        .min()
        .cloned();
    token_name.or_else(|| {
        let source = cursor_source(cursor)?;
        source
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .filter(|token| *token != link_name)
            .filter(|token| {
                macros
                    .final_value(token)
                    .is_some_and(|replacement| replacement == [link_name])
            })
            .min()
            .map(str::to_string)
    })
}

fn cursor_source(cursor: CXCursor) -> Option<String> {
    let (_, expansion, _, _) = cursor_locations(cursor)?;
    let source = std::fs::read(expansion.file).ok()?;
    let source = source.get(expansion.offset as usize..)?;
    let len = source
        .iter()
        .take(4096)
        .position(|byte| *byte == b';')
        .map_or(source.len().min(4096), |index| index + 1);
    let source = &source[..len];
    std::str::from_utf8(source).ok().map(str::to_string)
}

fn evaluate_constants(
    index: &Index,
    input: &Input,
    args: &[&str],
    facts: &[Fact],
    sources: &ConstantSources<'_, '_>,
    selected: &BTreeSet<String>,
    timing: bool,
) -> Result<(Vec<Constant>, Option<ProbeMetrics>), Error> {
    const INITIAL_CHUNK: usize = 16384;
    const RECOVERY_CHUNK: usize = 512;
    const ISOLATION_CHUNK: usize = 16;

    let macros = sources.macros;
    let probe_time = timing.then(std::time::Instant::now);
    let mut strings = BTreeMap::new();
    let mut roots = BTreeMap::new();
    for fact in facts
        .iter()
        .filter(|fact| fact.origin.tu == input.name && (fact.root || selected.contains(&fact.name)))
    {
        if let FactData::Macro {
            function_like: false,
            tokens,
        } = &fact.data
        {
            if let Some(value) = string_macro_value(&fact.name, macros, &mut HashSet::new()) {
                let ty = match value {
                    Value::Utf8(_) => TypeRef::Pointer {
                        mutable: false,
                        target: Box::new(TypeRef::Scalar(Scalar::I8)),
                    },
                    Value::Utf16(_) => TypeRef::Pointer {
                        mutable: false,
                        target: Box::new(TypeRef::Scalar(Scalar::U16)),
                    },
                    _ => unreachable!(),
                };
                strings
                    .entry(fact.name.clone())
                    .or_insert_with(|| Constant {
                        root: fact.origin.clone(),
                        definition: fact.origin.clone(),
                        spelling: fact.spelling.clone(),
                        name: fact.name.clone(),
                        ty,
                        value,
                    });
            }
            if macro_may_be_integer(tokens) {
                roots
                    .entry(fact.name.clone())
                    .or_insert_with(|| (fact.origin.clone(), fact.spelling.clone()));
            }
        }
    }
    let mut candidates = BTreeMap::new();
    for fact in facts.iter().filter(|fact| fact.origin.tu == input.name) {
        if let Some((root, spelling)) = roots.get(&fact.name)
            && matches!(
                fact.data,
                FactData::Macro {
                    function_like: false,
                    ..
                }
            )
        {
            candidates.insert(
                fact.name.clone(),
                (root.clone(), fact.origin.clone(), spelling.clone()),
            );
        }
    }
    candidates.retain(|name, _| {
        macros
            .final_definition(name)
            .is_some_and(|(_, function_like)| !function_like)
    });
    let names: Vec<_> = candidates.keys().cloned().collect();
    let source_diagnostics = sources.translation_unit.error_diagnostics();
    let available_parallelism = std::thread::available_parallelism().map_or(1, usize::from);
    let workers = available_parallelism.min(4);
    if names.is_empty() {
        let metrics = timing.then(|| ProbeMetrics {
            candidates: 0,
            string_constants: strings.len(),
            evaluated: 0,
            constants: strings.len(),
            initial_chunk: INITIAL_CHUNK,
            initial_batches: 0,
            recovery_chunk: RECOVERY_CHUNK,
            recovery_batches: 0,
            isolation_chunk: ISOLATION_CHUNK,
            isolation_batches: 0,
            defined_check_tus: 0,
            singleton_probes: 0,
            synthetic_tus: 0,
            retry_tus: 0,
            available_parallelism,
            configured_workers: workers,
            initial_workers: 0,
            recovery_workers: 0,
            isolation_workers: 0,
            singleton_workers: 0,
            elapsed_ms: elapsed_ms(probe_time),
        });
        return Ok((strings.into_values().collect(), metrics));
    }
    let mut evaluated = vec![];
    let mut reached = HashSet::new();
    let batches: Vec<_> = names.chunks(INITIAL_CHUNK).collect();
    let initial_workers = workers.min(batches.len());
    let batch_results =
        run_probe_workers(workers, batches.len(), |index, worker, worker_count| {
            let mut evaluated = vec![];
            let mut reached = HashSet::new();
            for batch in batches.iter().skip(worker).step_by(worker_count) {
                let (batch_evaluated, batch_reached) =
                    evaluate_probe(index, input, args, batch, &source_diagnostics)?;
                evaluated.extend(batch_evaluated);
                reached.extend(batch_reached);
            }
            Ok((evaluated, reached))
        })?;
    for (batch_evaluated, batch_reached) in batch_results {
        evaluated.extend(batch_evaluated);
        reached.extend(batch_reached);
    }
    let missing: Vec<_> = names
        .iter()
        .filter(|name| !reached.contains(name.as_str()))
        .cloned()
        .collect();
    let recovery_batches: Vec<_> = missing.chunks(RECOVERY_CHUNK).collect();
    let recovery_batch_count = recovery_batches.len();
    let recovery_workers = workers.min(recovery_batch_count);
    let recovery_results = run_probe_workers(
        workers,
        recovery_batches.len(),
        |index, worker, worker_count| {
            let mut evaluated = vec![];
            let mut reached = HashSet::new();
            for batch in recovery_batches.iter().skip(worker).step_by(worker_count) {
                let (batch_evaluated, batch_reached) =
                    evaluate_probe(index, input, args, batch, &source_diagnostics)?;
                evaluated.extend(batch_evaluated);
                reached.extend(batch_reached);
            }
            Ok((evaluated, reached))
        },
    )?;
    for (recovery_evaluated, recovery_reached) in recovery_results {
        evaluated.extend(recovery_evaluated);
        reached.extend(recovery_reached);
    }
    let unresolved: Vec<_> = missing
        .into_iter()
        .filter(|name| !reached.contains(name.as_str()))
        .collect();
    let isolation_batches: Vec<_> = unresolved.chunks(ISOLATION_CHUNK).collect();
    let isolation_batch_count = isolation_batches.len();
    let isolation_workers = workers.min(isolation_batch_count);
    let isolation_results = run_probe_workers(
        workers,
        isolation_batches.len(),
        |index, worker, worker_count| {
            let mut evaluated = vec![];
            let mut reached = HashSet::new();
            for batch in isolation_batches.iter().skip(worker).step_by(worker_count) {
                let (batch_evaluated, batch_reached) =
                    evaluate_probe(index, input, args, batch, &source_diagnostics)?;
                evaluated.extend(batch_evaluated);
                reached.extend(batch_reached);
            }
            Ok((evaluated, reached))
        },
    )?;
    for (isolation_evaluated, isolation_reached) in isolation_results {
        evaluated.extend(isolation_evaluated);
        reached.extend(isolation_reached);
    }
    let defined_check_tus = usize::from(!unresolved.is_empty());
    let defined = defined_macros(index, input, args, &unresolved)?;
    let fallback: Vec<_> = unresolved
        .iter()
        .filter(|name| !reached.contains(name.as_str()) && defined.contains(name.as_str()))
        .cloned()
        .collect();
    let fallback_count = fallback.len();
    let fallback_size = fallback.len().div_ceil(workers).max(1);
    let fallback_batches: Vec<_> = fallback.chunks(fallback_size).collect();
    let singleton_workers = workers.min(fallback_batches.len());
    let fallback_results =
        run_probe_workers(workers, fallback_batches.len(), |index, worker, _| {
            evaluate_singleton_probes(
                index,
                input,
                args,
                fallback_batches[worker],
                &reached,
                &source_diagnostics,
            )
        })?;
    for fallback in fallback_results {
        evaluated.extend(fallback);
    }

    let string_constants = strings.len();
    let evaluated_count = evaluated.len();
    let mut constants: Vec<_> = strings.into_values().collect();
    for evaluated in evaluated {
        let Some((root, definition, spelling)) = candidates.get(&evaluated.name) else {
            continue;
        };
        constants.push(Constant {
            root: root.clone(),
            definition: definition.clone(),
            spelling: spelling.clone(),
            name: evaluated.name,
            ty: evaluated.ty,
            value: evaluated.value,
        });
    }
    let synthetic_tus = batches.len()
        + recovery_batch_count
        + isolation_batch_count
        + defined_check_tus
        + fallback_count;
    let retry_tus =
        recovery_batch_count + isolation_batch_count + defined_check_tus + fallback_count;
    let metrics = timing.then(|| ProbeMetrics {
        candidates: names.len(),
        string_constants,
        evaluated: evaluated_count,
        constants: constants.len(),
        initial_chunk: INITIAL_CHUNK,
        initial_batches: batches.len(),
        recovery_chunk: RECOVERY_CHUNK,
        recovery_batches: recovery_batch_count,
        isolation_chunk: ISOLATION_CHUNK,
        isolation_batches: isolation_batch_count,
        defined_check_tus,
        singleton_probes: fallback_count,
        synthetic_tus,
        retry_tus,
        available_parallelism,
        configured_workers: workers,
        initial_workers,
        recovery_workers,
        isolation_workers,
        singleton_workers,
        elapsed_ms: elapsed_ms(probe_time),
    });
    Ok((constants, metrics))
}

fn run_probe_workers<T, F>(workers: usize, batch_count: usize, work: F) -> Result<Vec<T>, Error>
where
    T: Send,
    F: Fn(&Index, usize, usize) -> Result<T, Error> + Sync,
{
    let worker_count = workers.min(batch_count);
    std::thread::scope(|scope| {
        (0..worker_count)
            .map(|worker| {
                let work = &work;
                scope.spawn(move || {
                    let _library = Library::new()?;
                    let index = Index::new()?;
                    work(&index, worker, worker_count)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| {
                thread
                    .join()
                    .map_err(|_| Error("macro probe worker panicked".to_string()))?
            })
            .collect()
    })
}

fn macro_may_be_integer(tokens: &[String]) -> bool {
    !tokens.is_empty()
        && !tokens
            .iter()
            .any(|token| string_literal(token).is_some() || matches!(token.as_str(), "{" | "}"))
}

fn string_macro_value(
    name: &str,
    macros: &MacroDefinitions,
    visited: &mut HashSet<String>,
) -> Option<Value> {
    if !visited.insert(name.to_string()) {
        return None;
    }
    let (mut tokens, function_like) = macros.final_definition(name)?;
    if function_like {
        return None;
    }
    while tokens.len() >= 2 && tokens.first()? == "(" && tokens.last()? == ")" {
        tokens = &tokens[1..tokens.len() - 1];
    }
    if let [alias] = tokens
        && macros.contains_key(alias)
    {
        return string_macro_value(alias, macros, visited);
    }

    let mut value = String::new();
    let mut wide = None;
    for token in tokens {
        let (token_wide, inner) = string_literal(token)?;
        if wide.is_some_and(|wide| wide != token_wide) {
            return None;
        }
        wide = Some(token_wide);
        value.push_str(&decode_c_string(inner, token_wide)?);
    }
    match wide? {
        true => Some(Value::Utf16(value)),
        false => Some(Value::Utf8(value)),
    }
}

fn string_literal(token: &str) -> Option<(bool, &str)> {
    let (wide, quoted) = if let Some(value) = token.strip_prefix("u8") {
        (false, value)
    } else if let Some(value) = token
        .strip_prefix('L')
        .or_else(|| token.strip_prefix('u'))
        .or_else(|| token.strip_prefix('U'))
    {
        (true, value)
    } else {
        (false, token)
    };
    Some((wide, quoted.strip_prefix('"')?.strip_suffix('"')?))
}

fn decode_c_string(inner: &str, wide: bool) -> Option<String> {
    if !wide {
        return decode_c_bytes(inner);
    }
    let mut output = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        let character = match chars.next()? {
            '\\' => '\\',
            '"' => '"',
            '\'' => '\'',
            '?' => '?',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'a' => '\u{07}',
            'b' => '\u{08}',
            'f' => '\u{0c}',
            'v' => '\u{0b}',
            prefix @ ('x' | 'u' | 'U') => {
                let max = match prefix {
                    'u' => 4,
                    'U' => 8,
                    _ => usize::MAX,
                };
                let (value, count) = take_radix(&mut chars, 16, max);
                if count == 0 {
                    return None;
                }
                char::from_u32(value)?
            }
            digit @ '0'..='7' => {
                let mut value = digit.to_digit(8)?;
                let (rest, count) = take_radix(&mut chars, 8, 2);
                value = value * 8u32.pow(count as u32) + rest;
                char::from_u32(value)?
            }
            other => {
                output.push('\\');
                other
            }
        };
        output.push(character);
    }
    Some(output)
}

fn decode_c_bytes(inner: &str) -> Option<String> {
    let mut output = vec![];
    let mut chars = inner.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            let mut bytes = [0; 4];
            output.extend_from_slice(character.encode_utf8(&mut bytes).as_bytes());
            continue;
        }
        match chars.next()? {
            '\\' => output.push(b'\\'),
            '"' => output.push(b'"'),
            '\'' => output.push(b'\''),
            '?' => output.push(b'?'),
            'n' => output.push(b'\n'),
            'r' => output.push(b'\r'),
            't' => output.push(b'\t'),
            'a' => output.push(0x07),
            'b' => output.push(0x08),
            'f' => output.push(0x0c),
            'v' => output.push(0x0b),
            'x' => {
                let (value, count) = take_radix(&mut chars, 16, usize::MAX);
                if count == 0 {
                    return None;
                }
                output.push(u8::try_from(value).ok()?);
            }
            prefix @ ('u' | 'U') => {
                let max = if prefix == 'u' { 4 } else { 8 };
                let (value, count) = take_radix(&mut chars, 16, max);
                if count != max {
                    return None;
                }
                let character = char::from_u32(value)?;
                let mut bytes = [0; 4];
                output.extend_from_slice(character.encode_utf8(&mut bytes).as_bytes());
            }
            digit @ '0'..='7' => {
                let mut value = digit.to_digit(8)?;
                let (rest, count) = take_radix(&mut chars, 8, 2);
                value = value * 8u32.pow(count as u32) + rest;
                output.push(u8::try_from(value).ok()?);
            }
            other => {
                output.push(b'\\');
                let mut bytes = [0; 4];
                output.extend_from_slice(other.encode_utf8(&mut bytes).as_bytes());
            }
        }
    }
    String::from_utf8(output).ok()
}

fn take_radix(
    chars: &mut std::iter::Peekable<impl Iterator<Item = char>>,
    radix: u32,
    max: usize,
) -> (u32, usize) {
    let mut value = 0u32;
    let mut count = 0;
    while count < max {
        let Some(digit) = chars.peek().and_then(|character| character.to_digit(radix)) else {
            break;
        };
        chars.next();
        let Some(next) = value
            .checked_mul(radix)
            .and_then(|value| value.checked_add(digit))
        else {
            return (0, 0);
        };
        value = next;
        count += 1;
    }
    (value, count)
}

fn evaluate_probe(
    index: &Index,
    input: &Input,
    args: &[&str],
    names: &[String],
    source_diagnostics: &[ErrorDiagnostic],
) -> Result<(Vec<Evaluated>, HashSet<String>), Error> {
    let probe = probe_source(names);
    let tu = TranslationUnit::parse_probe(index, input, &probe.source, args)?;
    let synthetic_name = normalize_name(&format!("{}.__clang_eval.cpp", input.name));
    let input_name = normalize_name(&input.name);
    let probe_offset = input.source.len() + 1;
    let mut rejected = HashSet::new();
    let mut unlocalized = false;
    for diagnostic in tu.error_diagnostics() {
        // The original translation unit already accepted these diagnostics.
        if source_diagnostics
            .iter()
            .any(|source| same_source_diagnostic(&diagnostic, source, &input_name, &synthetic_name))
        {
            continue;
        }
        let Some(relative) = (diagnostic.file == synthetic_name)
            .then_some(diagnostic.offset as usize)
            .and_then(|offset| offset.checked_sub(probe_offset))
        else {
            unlocalized = true;
            continue;
        };
        let index = probe.ranges.partition_point(|range| range.end <= relative);
        if !probe
            .ranges
            .get(index)
            .is_some_and(|range| range.contains(&relative))
        {
            unlocalized = true;
            continue;
        }
        rejected.insert(names[index].clone());
    }
    // KeepGoing may produce evaluable cursors for a recovered prefix of an invalid expression.
    if unlocalized {
        return Ok((vec![], HashSet::new()));
    }
    let (mut evaluated, mut reached) = evaluate_parsed_probe(&tu, input);
    evaluated.retain(|value| !rejected.contains(&value.name));
    reached.extend(rejected);
    Ok((evaluated, reached))
}

fn evaluate_singleton_probes(
    index: &Index,
    input: &Input,
    args: &[&str],
    names: &[String],
    reached: &HashSet<String>,
    source_diagnostics: &[ErrorDiagnostic],
) -> Result<Vec<Evaluated>, Error> {
    let mut evaluated = vec![];
    for name in names {
        if !reached.contains(name.as_str()) {
            evaluated.extend(
                evaluate_probe(
                    index,
                    input,
                    args,
                    std::slice::from_ref(name),
                    source_diagnostics,
                )?
                .0,
            );
        }
    }
    Ok(evaluated)
}

fn same_source_diagnostic(
    probe: &ErrorDiagnostic,
    source: &ErrorDiagnostic,
    input_name: &str,
    synthetic_name: &str,
) -> bool {
    if probe.spelling != source.spelling {
        return false;
    }
    if probe.file == source.file {
        return probe.line == source.line && probe.column == source.column;
    }
    probe.file == synthetic_name
        && source.file == input_name
        && probe.offset == source.offset
        && probe.line == source.line
        && probe.column == source.column
}

fn defined_macros(
    index: &Index,
    input: &Input,
    args: &[&str],
    names: &[String],
) -> Result<HashSet<String>, Error> {
    if names.is_empty() {
        return Ok(HashSet::new());
    }
    let mut probe = String::new();
    for name in names {
        probe.push_str(&format!(
            "#ifdef {name}\n\
             enum {{ __clang_defined_{name} = 1 }};\n\
             #endif\n"
        ));
    }
    let tu = TranslationUnit::parse_probe(index, input, &probe, args)?;
    let mut result = HashSet::new();
    for cursor in cursor_children(unsafe { clang_getTranslationUnitCursor(tu.0) }) {
        if unsafe { clang_getCursorKind(cursor) } != CXCursor_EnumDecl {
            continue;
        }
        for constant in cursor_children(cursor) {
            let name = cx_string(unsafe { clang_getCursorSpelling(constant) });
            if let Some(name) = name.strip_prefix("__clang_defined_") {
                result.insert(name.to_string());
            }
        }
    }
    Ok(result)
}

struct ProbeSource {
    source: String,
    ranges: Vec<std::ops::Range<usize>>,
}

fn probe_source(names: &[String]) -> ProbeSource {
    let mut source = String::from(
        "#define __WINDOWS_CLANG_NARG(...) __WINDOWS_CLANG_NARG_(__VA_ARGS__,2,1,0)\n\
         #define __WINDOWS_CLANG_NARG_(_1,_2,N,...) N\n",
    );
    let mut ranges = Vec::with_capacity(names.len());
    for name in names {
        source.push_str(&format!("#ifdef {name}\n"));
        // Directive and EOF diagnostics may be parser fallout from an earlier macro.
        let start = source.len();
        source.push_str(&format!(
            "const auto __clang_eval_{name} = ({name});\n\
             const __int64 __clang_bits_{name} = \
                 (__int64)((__INTPTR_TYPE__)({name}));\n\
             enum {{ __clang_count_{name} = __WINDOWS_CLANG_NARG({name}) }};\n"
        ));
        ranges.push(start..source.len());
        source.push_str("#endif\n");
    }
    ProbeSource { source, ranges }
}

fn evaluate_parsed_probe(tu: &TranslationUnit, input: &Input) -> (Vec<Evaluated>, HashSet<String>) {
    let cursors = cursor_children(unsafe { clang_getTranslationUnitCursor(tu.0) });
    let mut counts = HashMap::new();
    for cursor in &cursors {
        if unsafe { clang_getCursorKind(*cursor) } == CXCursor_EnumDecl {
            for constant in cursor_children(*cursor) {
                let name = cx_string(unsafe { clang_getCursorSpelling(constant) });
                if let Some(name) = name.strip_prefix("__clang_count_") {
                    counts.insert(name.to_string(), unsafe {
                        clang_getEnumConstantDeclValue(constant)
                    });
                }
            }
        }
    }

    let mut result = vec![];
    let integer_values: HashMap<_, _> = cursors
        .iter()
        .filter_map(|cursor| {
            let cursor_name = cx_string(unsafe { clang_getCursorSpelling(*cursor) });
            let name = cursor_name.strip_prefix("__clang_bits_")?;
            Some((name.to_string(), evaluate_integer(*cursor)?))
        })
        .collect();
    for cursor in cursors {
        let cursor_name = cx_string(unsafe { clang_getCursorSpelling(cursor) });
        let Some(name) = cursor_name.strip_prefix("__clang_eval_") else {
            continue;
        };
        if counts.get(name) != Some(&1) {
            continue;
        }
        let ty = unsafe { clang_getCursorType(cursor) };
        let Some(mut ty_ref) = type_ref(ty) else {
            continue;
        };
        let canonical = unsafe { clang_getCanonicalType(ty) };
        if canonical.kind == CXType_Pointer
            && matches!(
                unsafe { clang_getPointeeType(canonical) }.kind,
                CXType_FunctionProto | CXType_FunctionNoProto
            )
        {
            continue;
        }
        if let TypeRef::Named { declaration, .. } = &mut ty_ref
            && declaration.file == format!("{}.__clang_eval.cpp", input.name)
        {
            declaration.file.clone_from(&input.name);
        }
        let value_scalar = scalar(ty);
        let value = if let Some(value_scalar @ (Scalar::F32 | Scalar::F64)) = value_scalar {
            let Some(value) = evaluate_float(cursor, value_scalar) else {
                continue;
            };
            value
        } else {
            let Some(value) =
                evaluate_integer(cursor).or_else(|| integer_values.get(name).copied())
            else {
                continue;
            };
            if value_scalar.is_some_and(|value_scalar| {
                matches!(
                    value_scalar,
                    Scalar::Bool | Scalar::U8 | Scalar::U16 | Scalar::U32 | Scalar::U64
                )
            }) {
                Value::Unsigned(value.0)
            } else {
                Value::Signed(value.1)
            }
        };
        result.push(Evaluated {
            name: name.to_string(),
            ty: ty_ref,
            value,
        });
    }
    let reached = counts.into_keys().collect();
    (result, reached)
}

fn evaluate_integer(cursor: CXCursor) -> Option<(u64, i64)> {
    unsafe {
        let result = clang_Cursor_Evaluate(cursor);
        if result.is_null() {
            return None;
        }

        let value = (clang_EvalResult_getKind(result) == CXEval_Int).then(|| {
            (
                clang_EvalResult_getAsUnsigned(result),
                clang_EvalResult_getAsLongLong(result),
            )
        });
        clang_EvalResult_dispose(result);
        value
    }
}

fn evaluate_variable_constant(cursor: CXCursor, ty: CXType) -> Option<(TypeRef, Value)> {
    if unsafe { clang_isConstQualifiedType(ty) } != 0
        && let Some(value_scalar) = scalar(ty)
    {
        let value = if matches!(value_scalar, Scalar::F32 | Scalar::F64) {
            evaluate_float(cursor, value_scalar)?
        } else {
            let (unsigned, signed) = evaluate_integer(cursor)?;
            if matches!(
                value_scalar,
                Scalar::Bool | Scalar::U8 | Scalar::U16 | Scalar::U32 | Scalar::U64
            ) {
                Value::Unsigned(unsigned)
            } else {
                Value::Signed(signed)
            }
        };
        return Some((type_ref(ty)?, value));
    }

    let canonical = unsafe { clang_getCanonicalType(ty) };
    if !matches!(
        canonical.kind,
        CXType_ConstantArray | CXType_IncompleteArray
    ) {
        return None;
    }
    let element = unsafe { clang_getArrayElementType(canonical) };
    if unsafe { clang_isConstQualifiedType(element) } == 0 && !variable_has_const_qualifier(cursor)
    {
        return None;
    }
    let tokens = cursor_tokens(cursor);
    let equals = tokens
        .iter()
        .position(|(kind, token)| *kind == CXToken_Punctuation && token == "=")?;
    let initializer: Vec<_> = tokens[equals + 1..]
        .iter()
        .filter(|(kind, token)| !(*kind == CXToken_Punctuation && token == ";"))
        .collect();
    let [(CXToken_Literal, literal)] = initializer.as_slice() else {
        return None;
    };
    let (wide, inner) = string_literal(literal)?;
    let (target, value) = match (scalar(element)?, wide) {
        (Scalar::I8 | Scalar::U8, false) => (
            TypeRef::Scalar(Scalar::I8),
            Value::Utf8(decode_c_string(inner, false)?),
        ),
        (Scalar::U16, true) => (
            TypeRef::Scalar(Scalar::U16),
            Value::Utf16(decode_c_string(inner, true)?),
        ),
        _ => return None,
    };
    Some((
        TypeRef::Pointer {
            mutable: false,
            target: Box::new(target),
        },
        value,
    ))
}

fn variable_is_global(cursor: CXCursor) -> bool {
    let parent = unsafe { clang_getCursorSemanticParent(cursor) };
    matches!(
        unsafe { clang_getCursorKind(parent) },
        CXCursor_TranslationUnit | CXCursor_Namespace | CXCursor_LinkageSpec
    )
}

fn variable_has_const_qualifier(cursor: CXCursor) -> bool {
    let name = cx_string(unsafe { clang_getCursorSpelling(cursor) });
    cursor_tokens(cursor)
        .into_iter()
        .take_while(|(_, token)| token != &name)
        .any(|(_, token)| matches!(token.as_str(), "const" | "constexpr"))
}

fn evaluate_float(cursor: CXCursor, scalar: Scalar) -> Option<Value> {
    unsafe {
        let result = clang_Cursor_Evaluate(cursor);
        if result.is_null() {
            return None;
        }
        let value = (clang_EvalResult_getKind(result) == CXEval_Float).then(|| {
            let value = clang_EvalResult_getAsDouble(result);
            match scalar {
                Scalar::F32 => {
                    let value = value as f32;
                    value.is_finite().then(|| Value::F32(value.to_bits()))
                }
                Scalar::F64 => value.is_finite().then(|| Value::F64(value.to_bits())),
                _ => unreachable!(),
            }
        });
        clang_EvalResult_dispose(result);
        value.flatten()
    }
}

fn cursor_locations(cursor: CXCursor) -> Option<(Location, Location, bool, bool)> {
    unsafe {
        let location = clang_getCursorLocation(cursor);
        let expansion = source_location(location, clang_getExpansionLocation)?;
        let spelling = source_location(location, clang_getSpellingLocation)
            .unwrap_or_else(|| expansion.clone());
        Some((
            spelling,
            expansion,
            clang_Location_isFromMainFile(location) != 0,
            clang_Location_isInSystemHeader(location) != 0,
        ))
    }
}

type LocationFn = unsafe fn(CXSourceLocation, *mut CXFile, *mut u32, *mut u32, *mut u32);

fn source_location(location: CXSourceLocation, get: LocationFn) -> Option<Location> {
    unsafe {
        let mut file: CXFile = std::ptr::null_mut();
        let mut offset = 0;
        get(
            location,
            &mut file,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut offset,
        );
        (!file.is_null()).then(|| Location {
            file: normalize_name(&cx_string(clang_getFileName(file))),
            offset,
        })
    }
}

fn fact_kind(kind: CXCursorKind) -> Option<FactKind> {
    Some(match kind {
        CXCursor_ClassDecl => FactKind::Class,
        CXCursor_EnumDecl => FactKind::Enum,
        CXCursor_FunctionDecl => FactKind::Function,
        CXCursor_MacroDefinition => FactKind::Macro,
        CXCursor_Namespace => FactKind::Namespace,
        CXCursor_StructDecl => FactKind::Struct,
        CXCursor_TypedefDecl => FactKind::Typedef,
        CXCursor_UnionDecl => FactKind::Union,
        _ => return None,
    })
}

fn fact_data(cursor: CXCursor, kind: FactKind, macros: &MacroDefinitions) -> FactData {
    match kind {
        FactKind::Enum => {
            let ty = unsafe { clang_getEnumDeclIntegerType(cursor) };
            let Some(repr) = scalar(ty) else {
                return FactData::None;
            };
            let fixed = cursor_tokens(cursor).iter().any(|(_, token)| token == ":");
            let variants = cursor_children(cursor)
                .into_iter()
                .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_EnumConstantDecl)
                .map(|child| Variant {
                    name: cx_string(unsafe { clang_getCursorSpelling(child) }),
                    value: unsafe { clang_getEnumConstantDeclValue(child) },
                })
                .collect();
            FactData::Enum {
                repr,
                variants,
                fixed,
                scoped: unsafe { clang_EnumDecl_isScoped(cursor) } != 0,
            }
        }
        FactKind::Function => {
            if unsafe { clang_getCursorLinkage(cursor) } != CXLinkage_External
                || unsafe { clang_isCursorDefinition(cursor) } != 0
            {
                return FactData::Unsupported {
                    reason: "function is not an external declaration".to_string(),
                };
            }
            let variadic = unsafe { clang_Cursor_isVariadic(cursor) } != 0;
            let result_ty = unsafe { clang_getCursorResultType(cursor) };
            let Some(result) = type_ref(result_ty) else {
                return FactData::Unsupported {
                    reason: format!(
                        "function has unsupported result type `{}`",
                        cx_string(unsafe { clang_getTypeSpelling(result_ty) })
                    ),
                };
            };
            let params = match callable_params(cursor, macros, false) {
                Ok(params) => params,
                Err(reason) => return FactData::Unsupported { reason },
            };
            let function_ty = unsafe { clang_getCursorType(cursor) };
            let Some(convention) = (if variadic {
                calling_convention_fact(function_ty)
            } else {
                source_calling_convention(cursor, macros)
                    .or_else(|| calling_convention_fact(function_ty))
            }) else {
                return FactData::Unsupported {
                    reason: "function has an unsupported calling convention".to_string(),
                };
            };
            FactData::Function {
                link_name: external_link_name(cursor),
                convention,
                params,
                result,
                variadic,
                noreturn: function_is_noreturn(cursor),
            }
        }
        FactKind::Macro => {
            let name = cx_string(unsafe { clang_getCursorSpelling(cursor) });
            let (tokens, function_like) = macros.definition(cursor, &name).map_or_else(
                || {
                    (
                        cursor_tokens(cursor)
                            .into_iter()
                            .map(|(_, token)| token)
                            .skip(1)
                            .collect(),
                        unsafe { clang_Cursor_isMacroFunctionLike(cursor) } != 0,
                    )
                },
                |(tokens, function_like)| (tokens.to_vec(), function_like),
            );
            FactData::Macro {
                function_like,
                tokens,
            }
        }
        FactKind::EnumFlag => {
            let macro_name = cx_string(unsafe { clang_getCursorSpelling(cursor) });
            let target = cursor_tokens(cursor).into_iter().find_map(|(kind, token)| {
                (kind == CXToken_Identifier && token != macro_name).then_some(token)
            });
            if let Some(target) = target {
                FactData::EnumFlag { target }
            } else {
                FactData::Unsupported {
                    reason: "enum flag macro has no type argument".to_string(),
                }
            }
        }
        FactKind::Guid => {
            if unsafe { clang_getCursorKind(cursor) } == CXCursor_VarDecl {
                return variable_guid(cursor).map_or_else(
                    || FactData::Unsupported {
                        reason: "GUID variable has an unsupported initializer".to_string(),
                    },
                    |value| FactData::Guid { value },
                );
            }
            let tokens = cursor_tokens(cursor);
            let macro_name = tokens
                .first()
                .map(|(_, token)| token.as_str())
                .unwrap_or_default();
            if matches!(macro_name, "DEFINE_PROPERTYKEY" | "DEFINE_DEVPROPKEY") {
                return parse_property_key_tokens(&tokens).map_or_else(
                    || FactData::Unsupported {
                        reason: "property-key macro has invalid arguments".to_string(),
                    },
                    |(_, guid, pid)| FactData::PropertyKey {
                        ty: if macro_name == "DEFINE_DEVPROPKEY" {
                            "DEVPROPKEY"
                        } else {
                            "PROPERTYKEY"
                        },
                        guid,
                        pid,
                    },
                );
            }
            let ole = macro_name == "DEFINE_OLEGUID";
            parse_define_guid_tokens(&tokens, ole).map_or_else(
                || FactData::Unsupported {
                    reason: "GUID macro has invalid arguments".to_string(),
                },
                |(_, value)| FactData::Guid { value },
            )
        }
        FactKind::Class | FactKind::Struct | FactKind::Union => {
            if kind == FactKind::Class
                && let Some(layout) = native_opaque_class_layout(cursor)
            {
                return layout;
            }
            if matches!(kind, FactKind::Class | FactKind::Struct) && is_interface(cursor) {
                return interface_fact(cursor, macros);
            }
            let guid = cursor_uuid(cursor);
            let definition_cursor = cursor_definition(cursor);
            let has_definition = unsafe { clang_isCursorDefinition(definition_cursor) } != 0;
            let data_record = match kind {
                FactKind::Class => is_data_class(cursor),
                FactKind::Struct => has_definition,
                FactKind::Union => true,
                _ => unreachable!(),
            };
            if matches!(kind, FactKind::Class | FactKind::Struct)
                && !data_record
                && let Some(guid) = &guid
            {
                return FactData::Class { guid: guid.clone() };
            }
            if kind == FactKind::Class && !data_record && has_definition {
                return FactData::Unsupported {
                    reason: "class is not a public data-only record".to_string(),
                };
            }
            let union = kind == FactKind::Union;
            let definition = unsafe { clang_isCursorDefinition(cursor) } != 0;
            let mut record = if definition {
                match inline_record(cursor, union, Some(macros)) {
                    Ok(record) => record,
                    Err(reason) => return FactData::Unsupported { reason },
                }
            } else {
                InlineRecord {
                    name: None,
                    base: None,
                    fields: vec![],
                    size: unsafe { clang_Type_getSizeOf(clang_getCursorType(cursor)) },
                    align: unsafe { clang_Type_getAlignOf(clang_getCursorType(cursor)) },
                    packing: None,
                    alignment: None,
                    union,
                }
            };
            name_indirect_inline_records(
                &mut record,
                cx_string(unsafe { clang_getCursorSpelling(cursor) }).trim_start_matches('_'),
            );
            FactData::Record {
                base: record.base,
                fields: record.fields,
                size: record.size,
                align: record.align,
                packing: record.packing,
                alignment: record.alignment,
                union: record.union,
            }
        }
        FactKind::Typedef => {
            let ty = unsafe { clang_getTypedefDeclUnderlyingType(cursor) };
            let function = if ty.kind == CXType_Pointer {
                unsafe { clang_getPointeeType(ty) }
            } else {
                ty
            };
            if matches!(function.kind, CXType_FunctionProto | CXType_FunctionNoProto) {
                let inherited_convention = if ty.kind == CXType_Pointer {
                    inherited_function_typedef_calling_convention(cursor, function, macros)
                } else {
                    None
                };
                return function_signature(
                    function,
                    source_calling_convention(cursor, macros).or(inherited_convention),
                )
                .map_or_else(
                    || FactData::Unsupported {
                        reason: "callback has an unsupported signature".to_string(),
                    },
                    |(convention, params, result)| FactData::Callback {
                        convention,
                        params: callback_params(cursor, macros, params),
                        result,
                    },
                );
            }
            type_ref(ty).map_or_else(
                || FactData::Unsupported {
                    reason: format!(
                        "typedef has unsupported type `{}`",
                        cx_string(unsafe { clang_getTypeSpelling(ty) })
                    ),
                },
                |target| FactData::Typedef { target },
            )
        }
        _ => FactData::None,
    }
}

fn native_opaque_class_layout(cursor: CXCursor) -> Option<FactData> {
    let definition = native_opaque_class_definition(cursor)?;
    if unsafe {
        clang_isCursorDefinition(cursor) == 0 || clang_equalCursors(definition, cursor) == 0
    } {
        return None;
    }
    Some(FactData::Record {
        base: None,
        fields: vec![],
        size: i64::from(CXTypeLayoutError_Incomplete),
        align: i64::from(CXTypeLayoutError_Incomplete),
        packing: None,
        alignment: None,
        union: false,
    })
}

fn native_opaque_class_definition(cursor: CXCursor) -> Option<CXCursor> {
    let definition = cursor_definition(cursor);
    if unsafe { clang_getCursorKind(definition) } != CXCursor_ClassDecl
        || unsafe { clang_isCursorDefinition(definition) } == 0
        || is_interface(definition)
        || cursor_uuid(definition).is_some()
        || !expanded_raw_annotations(definition)
            .iter()
            .any(|annotation| annotation.key == "native_opaque" && annotation.value.is_none())
    {
        return None;
    }
    Some(definition)
}

fn is_data_class(cursor: CXCursor) -> bool {
    let cursor = cursor_definition(cursor);
    if unsafe { clang_isCursorDefinition(cursor) } == 0 {
        return false;
    }
    if unsafe { clang_isPODType(clang_getCursorType(cursor)) } == 0 {
        return false;
    }
    let children = cursor_children(cursor);
    let fields: Vec<_> = children
        .iter()
        .copied()
        .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_FieldDecl)
        .collect();
    !fields.is_empty()
        && fields
            .iter()
            .all(|field| unsafe { clang_getCXXAccessSpecifier(*field) } == CX_CXXPublic)
        && !children.iter().any(|child| {
            matches!(
                unsafe { clang_getCursorKind(*child) },
                CXCursor_CXXBaseSpecifier
                    | CXCursor_CXXMethod
                    | CXCursor_Constructor
                    | CXCursor_Destructor
                    | CXCursor_ConversionFunction
                    | CXCursor_FunctionTemplate
            )
        })
}

fn pointer_only_class_layout(
    cursor: CXCursor,
    macros: &MacroDefinitions,
) -> Option<(FactData, bool)> {
    let (cursor, embeddable) = pointer_only_class_definition(cursor)?;
    let mut record =
        inline_record_with_pointer_class_layouts(cursor, false, Some(macros), true).ok()?;
    name_indirect_inline_records(
        &mut record,
        cx_string(unsafe { clang_getCursorSpelling(cursor) }).trim_start_matches('_'),
    );
    Some((
        FactData::Record {
            base: record.base,
            fields: record.fields,
            size: record.size,
            align: record.align,
            packing: record.packing,
            alignment: record.alignment,
            union: record.union,
        },
        embeddable,
    ))
}

fn pointer_only_class_definition(cursor: CXCursor) -> Option<(CXCursor, bool)> {
    let cursor = cursor_definition(cursor);
    if unsafe { clang_isCursorDefinition(cursor) } == 0
        || unsafe { clang_isPODType(clang_getCursorType(cursor)) } != 0
    {
        return None;
    }
    let children = cursor_children(cursor);
    let fields: Vec<_> = children
        .iter()
        .copied()
        .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_FieldDecl)
        .collect();
    if fields.is_empty()
        || children.iter().any(|child| unsafe {
            clang_getCursorKind(*child) == CXCursor_CXXBaseSpecifier
                || (matches!(
                    clang_getCursorKind(*child),
                    CXCursor_CXXMethod | CXCursor_Destructor | CXCursor_ConversionFunction
                ) && clang_CXXMethod_isVirtual(*child) != 0)
        })
    {
        return None;
    }
    let all_public = fields
        .iter()
        .all(|field| unsafe { clang_getCXXAccessSpecifier(*field) } == CX_CXXPublic);
    let all_protected = fields
        .iter()
        .all(|field| unsafe { clang_getCXXAccessSpecifier(*field) } == CX_CXXProtected);
    (all_public || all_protected).then_some((cursor, all_protected))
}

fn external_link_name(cursor: CXCursor) -> String {
    let name = cx_string(unsafe { clang_getCursorSpelling(cursor) });
    let mangled = raw_external_link_name(cursor);
    if mangled == name
        || mangled == format!("_{name}")
        || mangled
            .strip_prefix(&format!("_{name}@"))
            .is_some_and(|bytes| bytes.chars().all(|c| c.is_ascii_digit()))
        || mangled
            .strip_prefix(&format!("@{name}@"))
            .is_some_and(|bytes| bytes.chars().all(|c| c.is_ascii_digit()))
    {
        name
    } else {
        mangled
    }
}

fn raw_external_link_name(cursor: CXCursor) -> String {
    cx_string(unsafe { clang_Cursor_getMangling(cursor) })
}

fn is_interface(cursor: CXCursor) -> bool {
    let definition = unsafe { clang_getCursorDefinition(cursor) };
    let cursor = if unsafe { clang_Cursor_isNull(definition) } == 0 {
        definition
    } else {
        cursor
    };
    if unsafe { clang_isCursorDefinition(cursor) } == 0 {
        return false;
    }
    let children = cursor_children(cursor);
    if children
        .iter()
        .any(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_FieldDecl)
    {
        return false;
    }
    let methods: Vec<_> = children
        .iter()
        .filter(|child| unsafe {
            clang_getCursorKind(**child) == CXCursor_CXXMethod
                && clang_CXXMethod_isVirtual(**child) != 0
        })
        .collect();
    (!methods.is_empty()
        && (cursor_uuid(cursor).is_some()
            || methods
                .iter()
                .all(|method| unsafe { clang_CXXMethod_isPureVirtual(**method) } != 0)))
        || children.iter().any(|child| {
            if unsafe { clang_getCursorKind(*child) } != CXCursor_CXXBaseSpecifier {
                return false;
            }
            let declaration = unsafe { clang_getTypeDeclaration(clang_getCursorType(*child)) };
            (unsafe { clang_Cursor_isNull(declaration) }) == 0 && is_interface(declaration)
        })
}

fn cursor_definition(cursor: CXCursor) -> CXCursor {
    let definition = unsafe { clang_getCursorDefinition(cursor) };
    if unsafe { clang_Cursor_isNull(definition) } == 0 {
        definition
    } else {
        cursor
    }
}

fn interface_inherits_from(cursor: CXCursor, ancestor: CXCursor) -> bool {
    let cursor = cursor_definition(cursor);
    let ancestor = cursor_definition(ancestor);
    for child in cursor_children(cursor) {
        if unsafe { clang_getCursorKind(child) } != CXCursor_CXXBaseSpecifier {
            continue;
        }
        let base = unsafe { clang_getTypeDeclaration(clang_getCursorType(child)) };
        if unsafe { clang_Cursor_isNull(base) } != 0 {
            continue;
        }
        let base = cursor_definition(base);
        if unsafe { clang_equalCursors(base, ancestor) } != 0
            || interface_inherits_from(base, ancestor)
        {
            return true;
        }
    }
    false
}

fn interface_fact(cursor: CXCursor, macros: &MacroDefinitions) -> FactData {
    let children = cursor_children(cursor);
    let async_interface =
        cx_string(unsafe { clang_getCursorSpelling(cursor) }).starts_with("Async");
    let mut bases = vec![];
    for child in &children {
        if unsafe { clang_getCursorKind(*child) } != CXCursor_CXXBaseSpecifier {
            continue;
        }
        let declaration = unsafe { clang_getTypeDeclaration(clang_getCursorType(*child)) };
        if unsafe { clang_Cursor_isNull(declaration) } != 0 || !is_interface(declaration) {
            return FactData::Unsupported {
                reason: "interface base is not an interface".to_string(),
            };
        }
        bases.push((*child, declaration));
    }
    let most_derived: Vec<_> = bases
        .iter()
        .filter(|(_, candidate)| {
            !bases.iter().any(|(_, other)| {
                (unsafe { clang_equalCursors(*candidate, *other) }) == 0
                    && interface_inherits_from(*other, *candidate)
            })
        })
        .collect();
    let base = match most_derived.as_slice() {
        [] => None,
        [(child, _)] => {
            let Some(ty) = type_ref(unsafe { clang_getCursorType(*child) }) else {
                return FactData::Unsupported {
                    reason: "interface has an unsupported base".to_string(),
                };
            };
            Some(ty)
        }
        _ if bases.len() == 2
            && cx_string(unsafe { clang_getCursorSpelling(bases[1].1) }) == "IUnknown" =>
        {
            let Some(ty) = type_ref(unsafe { clang_getCursorType(bases[0].0) }) else {
                return FactData::Unsupported {
                    reason: "interface has an unsupported base".to_string(),
                };
            };
            Some(ty)
        }
        _ => {
            return FactData::Unsupported {
                reason: "interface has unrelated multiple bases".to_string(),
            };
        }
    };
    let guid = cursor_uuid(cursor);
    let mut methods = vec![];
    for child in children {
        match unsafe { clang_getCursorKind(child) } {
            CXCursor_CXXBaseSpecifier => {}
            CXCursor_CXXMethod if unsafe { clang_CXXMethod_isVirtual(child) } != 0 => {
                if method_overrides_base(child) {
                    continue;
                }
                let result_ty = unsafe { clang_getCursorResultType(child) };
                let Some(result) = type_ref(result_ty) else {
                    return FactData::Unsupported {
                        reason: "interface method has an unsupported result".to_string(),
                    };
                };
                let method_name = cx_string(unsafe { clang_getCursorSpelling(child) });
                let method_name =
                    source_function_name(child, &method_name, macros).unwrap_or(method_name);
                let params = match callable_params(
                    child,
                    macros,
                    async_interface && method_name.starts_with("Finish_"),
                ) {
                    Ok(params) => params,
                    Err(reason) => return FactData::Unsupported { reason },
                };
                let tokens = cursor_tokens(child);
                let special =
                    tokens_before_method_name(&tokens, child)
                        .iter()
                        .any(|(kind, token)| {
                            *kind == CXToken_Comment
                                && (token.contains("[propget]") || token.contains("[propput]"))
                        });
                methods.push(Method {
                    name: method_name,
                    params,
                    result,
                    special,
                });
            }
            CXCursor_CXXMethod => {}
            CXCursor_Constructor | CXCursor_Destructor => {
                return FactData::Unsupported {
                    reason: "interface has a constructor or destructor".to_string(),
                };
            }
            _ => {}
        }
    }
    FactData::Interface {
        base,
        guid,
        methods,
    }
}

fn parse_define_guid_tokens(
    tokens: &[(CXTokenKind, String)],
    ole: bool,
) -> Option<(String, String)> {
    let lparen = tokens
        .iter()
        .position(|(kind, token)| *kind == CXToken_Punctuation && token == "(")?;
    let name = tokens[lparen + 1..]
        .iter()
        .find(|(kind, _)| *kind == CXToken_Identifier)?
        .1
        .clone();
    let mut values: Vec<u64> = tokens[lparen + 1..]
        .iter()
        .filter(|(kind, _)| *kind == CXToken_Literal)
        .map(|(_, token)| parse_c_integer(token))
        .collect::<Option<_>>()?;
    if ole {
        if values.len() != 3 {
            return None;
        }

        values.extend_from_slice(&[0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
    }
    Some((name, format_guid(&values)?))
}

fn parse_property_key_tokens(tokens: &[(CXTokenKind, String)]) -> Option<(String, String, u32)> {
    let lparen = tokens
        .iter()
        .position(|(kind, token)| *kind == CXToken_Punctuation && token == "(")?;
    let name = tokens[lparen + 1..]
        .iter()
        .find(|(kind, _)| *kind == CXToken_Identifier)?
        .1
        .clone();
    let values: Vec<u64> = tokens[lparen + 1..]
        .iter()
        .filter(|(kind, _)| *kind == CXToken_Literal)
        .map(|(_, token)| parse_c_integer(token))
        .collect::<Option<_>>()?;
    if values.len() != 12 || values[11] > u32::MAX.into() {
        return None;
    }
    Some((name, format_guid(&values[..11])?, values[11] as u32))
}

fn variable_guid(cursor: CXCursor) -> Option<String> {
    let ty = unsafe { clang_getCursorType(cursor) };
    if unsafe { clang_isConstQualifiedType(ty) } == 0 || !is_guid_type(ty) {
        return None;
    }
    let tokens = cursor_tokens(cursor);
    let equals = tokens
        .iter()
        .position(|(kind, token)| *kind == CXToken_Punctuation && token == "=")?;
    let initializer = &tokens[equals + 1..];
    if initializer.iter().any(|(kind, token)| {
        *kind != CXToken_Literal
            && !(*kind == CXToken_Punctuation && matches!(token.as_str(), "{" | "}" | "," | ";"))
    }) {
        return None;
    }
    let values = initializer
        .iter()
        .filter(|(kind, _)| *kind == CXToken_Literal)
        .map(|(_, token)| parse_c_integer(token))
        .collect::<Option<Vec<_>>>()?;
    format_guid(&values)
}

fn is_guid_type(mut ty: CXType) -> bool {
    loop {
        let declaration = unsafe { clang_getTypeDeclaration(ty) };
        if unsafe { clang_Cursor_isNull(declaration) } != 0 {
            return false;
        }
        let name = cx_string(unsafe { clang_getCursorSpelling(declaration) });
        if matches!(name.as_str(), "GUID" | "IID" | "CLSID" | "FMTID" | "_GUID") {
            return true;
        }
        if unsafe { clang_getCursorKind(declaration) } != CXCursor_TypedefDecl {
            return false;
        }
        ty = unsafe { clang_getTypedefDeclUnderlyingType(declaration) };
    }
}

fn parse_c_integer(value: &str) -> Option<u64> {
    let digits = value.trim_end_matches(['u', 'U', 'l', 'L']);
    if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else {
        digits.parse().ok()
    }
}

fn format_guid(values: &[u64]) -> Option<String> {
    if values.len() != 11
        || values[0] > u32::MAX.into()
        || values[1] > u16::MAX.into()
        || values[2] > u16::MAX.into()
        || values[3..].iter().any(|value| *value > u8::MAX.into())
    {
        return None;
    }
    Some(format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        values[0],
        values[1],
        values[2],
        values[3],
        values[4],
        values[5],
        values[6],
        values[7],
        values[8],
        values[9],
        values[10],
    ))
}

fn cursor_uuid(cursor: CXCursor) -> Option<String> {
    let tu = unsafe { clang_Cursor_getTranslationUnit(cursor) };
    for child in cursor_children(cursor) {
        if unsafe { clang_getCursorKind(child) } != CXCursor_UnexposedAttr {
            continue;
        }
        let range = expansion_range(tu, unsafe { clang_getCursorExtent(child) });
        let mut tokens = std::ptr::null_mut();
        let mut count = 0;
        unsafe { clang_tokenize(tu, range, &mut tokens, &mut count) };
        for index in 0..count {
            let token = unsafe { *tokens.add(index as usize) };
            if unsafe { clang_getTokenKind(token) } == CXToken_Literal {
                let spelling = cx_string(unsafe { clang_getTokenSpelling(tu, token) });
                let value = spelling.trim_matches('"');
                if is_uuid(value) {
                    unsafe { clang_disposeTokens(tu, tokens, count) };
                    return Some(value.to_ascii_lowercase());
                }
            }
        }
        unsafe { clang_disposeTokens(tu, tokens, count) };
    }
    None
}

fn expansion_range(tu: CXTranslationUnit, range: CXSourceRange) -> CXSourceRange {
    unsafe {
        let mut start_file = std::ptr::null_mut();
        let mut start_line = 0;
        let mut start_column = 0;
        let mut start_offset = 0;
        clang_getExpansionLocation(
            clang_getRangeStart(range),
            &mut start_file,
            &mut start_line,
            &mut start_column,
            &mut start_offset,
        );
        let mut end_file = std::ptr::null_mut();
        let mut end_line = 0;
        let mut end_column = 0;
        let mut end_offset = 0;
        clang_getExpansionLocation(
            clang_getRangeEnd(range),
            &mut end_file,
            &mut end_line,
            &mut end_column,
            &mut end_offset,
        );
        clang_getRange(
            clang_getLocation(tu, start_file, start_line, start_column),
            clang_getLocation(tu, end_file, end_line, end_column),
        )
    }
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value
            .chars()
            .enumerate()
            .all(|(index, character)| match index {
                8 | 13 | 18 | 23 => character == '-',
                _ => character.is_ascii_hexdigit(),
            })
}

fn method_overrides_base(cursor: CXCursor) -> bool {
    let mut cursors = std::ptr::null_mut();
    let mut count = 0;
    unsafe {
        clang_getOverriddenCursors(cursor, &mut cursors, &mut count);
        clang_disposeOverriddenCursors(cursors);
    }
    count != 0
}

fn callable_params(
    cursor: CXCursor,
    macros: &MacroDefinitions,
    allow_unresolved_size: bool,
) -> Result<Vec<Parameter>, String> {
    let mut params = vec![];
    for child in cursor_children(cursor)
        .into_iter()
        .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_ParmDecl)
    {
        let param_ty = unsafe { clang_getCursorType(child) };
        let Some(ty) = function_param_type_at_cursor(child, param_ty) else {
            return Err(format!(
                "parameter has unsupported type `{}`",
                cx_string(unsafe { clang_getTypeSpelling(param_ty) })
            ));
        };
        let mut name = cx_string(unsafe { clang_getCursorSpelling(child) });
        if name.is_empty() {
            name = format!("param{}", params.len());
        }
        let mut annotation = parameter_annotation(child);
        normalize_constant_byte_size(param_ty, &mut annotation);
        if let Some(reason) = &annotation.unsupported {
            return Err(reason.clone());
        }
        params.push(Parameter {
            name,
            ty,
            annotation,
        });
    }
    let names: BTreeSet<_> = params.iter().map(|param| param.name.clone()).collect();
    for param in &mut params {
        let mut discard_size = false;
        if let Some(size) = &mut param.annotation.size {
            match &mut size.value {
                SalSizeValue::Parameter(name) if !names.contains(name) => {
                    if macros.contains_key(name) {
                        size.value = SalSizeValue::Expression(name.clone());
                    } else if allow_unresolved_size {
                        discard_size = true;
                    } else {
                        size.value = SalSizeValue::Expression(name.clone());
                    }
                }
                SalSizeValue::IndirectParameter(name) if !names.contains(name) => {
                    if allow_unresolved_size {
                        discard_size = true;
                    } else {
                        size.value = SalSizeValue::Expression(format!("*{name}"));
                    }
                }
                SalSizeValue::Constant(_) if size.bytes => {
                    return Err("constant byte-size SAL annotations are unsupported".to_string());
                }
                SalSizeValue::Expression(_) => {}
                _ => {}
            }
        }
        if discard_size {
            param.annotation.size = None;
        }
    }
    apply_source_annotations(cursor, macros, &mut params);
    Ok(params)
}

fn callback_params(
    cursor: CXCursor,
    macros: &MacroDefinitions,
    fallback: Vec<TypeRef>,
) -> Vec<Parameter> {
    let mut candidates = vec![cursor];
    candidates.extend(
        cursor_children(cursor)
            .into_iter()
            .filter(|child| unsafe { clang_getCursorKind(*child) == CXCursor_TypeRef })
            .map(|child| unsafe { clang_getCursorReferenced(child) })
            .filter(|child| unsafe { clang_Cursor_isNull(*child) } == 0),
    );
    for candidate in candidates {
        if let Ok(params) = callable_params(candidate, macros, false)
            && params.len() == fallback.len()
        {
            return params;
        }
    }
    fallback
        .into_iter()
        .enumerate()
        .map(|(index, ty)| Parameter {
            name: format!("param{index}"),
            ty,
            annotation: ParamAnnotation::default(),
        })
        .collect()
}

fn normalize_constant_byte_size(ty: CXType, annotation: &mut ParamAnnotation) {
    let Some(SalSize {
        bytes: true,
        value: SalSizeValue::Constant(value),
    }) = &mut annotation.size
    else {
        return;
    };
    let canonical = unsafe { clang_getCanonicalType(ty) };
    if canonical.kind != CXType_Pointer {
        return;
    }
    let element_size = unsafe { clang_Type_getSizeOf(clang_getPointeeType(canonical)) };
    if element_size > 0
        && i64::from(*value) % element_size == 0
        && let Ok(element_count) = i32::try_from(i64::from(*value) / element_size)
    {
        *value = element_count;
        if let Some(size) = &mut annotation.size {
            size.bytes = false;
        }
    }
}

fn cursor_tokens(cursor: CXCursor) -> Vec<(CXTokenKind, String)> {
    let tu = unsafe { clang_Cursor_getTranslationUnit(cursor) };
    let range = expansion_range(tu, unsafe { clang_getCursorExtent(cursor) });
    let mut tokens = std::ptr::null_mut();
    let mut count = 0;
    unsafe { clang_tokenize(tu, range, &mut tokens, &mut count) };
    let result = (0..count)
        .map(|index| {
            let token = unsafe { *tokens.add(index as usize) };
            (
                unsafe { clang_getTokenKind(token) },
                cx_string(unsafe { clang_getTokenSpelling(tu, token) }),
            )
        })
        .collect();
    unsafe { clang_disposeTokens(tu, tokens, count) };
    result
}

fn declare_handle_name(tokens: &[(CXTokenKind, String)]) -> Option<&str> {
    match tokens {
        [
            (CXToken_Identifier, macro_name),
            (_, open),
            (CXToken_Identifier, name),
            (_, close),
        ] if macro_name == "DECLARE_HANDLE" && open == "(" && close == ")" => Some(name),
        _ => None,
    }
}

fn tokens_before_method_name(
    tokens: &[(CXTokenKind, String)],
    cursor: CXCursor,
) -> &[(CXTokenKind, String)] {
    let name = cx_string(unsafe { clang_getCursorSpelling(cursor) });
    let end = tokens
        .iter()
        .position(|(kind, token)| *kind == CXToken_Identifier && token == &name)
        .unwrap_or(0);
    &tokens[..end]
}

fn apply_source_annotations(cursor: CXCursor, macros: &MacroDefinitions, params: &mut [Parameter]) {
    let tokens = cursor_tokens(cursor);
    let parameter_cursors: Vec<_> = cursor_children(cursor)
        .into_iter()
        .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_ParmDecl)
        .collect();
    let cursor_name = cx_string(unsafe { clang_getCursorSpelling(cursor) });
    let name_index = tokens
        .iter()
        .position(|(kind, token)| *kind == CXToken_Identifier && token == &cursor_name);
    let Some(open) = tokens
        .iter()
        .enumerate()
        .skip(name_index.map_or(0, |index| index + 1))
        .find(|(_, (kind, token))| *kind == CXToken_Punctuation && token == "(")
        .map(|(index, _)| index)
    else {
        return;
    };
    let mut index = 0;
    let mut depth = 1;
    for (kind, token) in &tokens[open + 1..] {
        match (*kind, token.as_str()) {
            (CXToken_Punctuation, "(") => depth += 1,
            (CXToken_Punctuation, ")") => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            (CXToken_Punctuation, ",") if depth == 1 => index += 1,
            (CXToken_Identifier, annotation)
                if depth == 1 && index < params.len() && annotation.starts_with("_COM_Outptr_") =>
            {
                let param = &mut params[index].annotation;
                param.output = true;
                param.com_out_ptr |= parameter_cursors.get(index).is_some_and(|cursor| {
                    is_void_double_pointer(unsafe { clang_getCursorType(*cursor) })
                });
                param.optional |= annotation.contains("_opt_");
            }
            (CXToken_Identifier, annotation)
                if depth == 1 && index < params.len() && macros.contains_key(annotation) =>
            {
                let param = &mut params[index].annotation;
                match annotation {
                    "IN" => param.input = true,
                    "OUT" => param.output = true,
                    "OPTIONAL" => param.optional = true,
                    _ => {}
                }
            }
            (CXToken_Comment, comment) if depth == 1 && index < params.len() => {
                let annotation = &mut params[index].annotation;
                annotation.input |= comment.contains("[in]");
                annotation.output |= comment.contains("[out]");
                annotation.optional |= comment.contains("[optional]");
                annotation.retval |= comment.contains("[retval]");
                annotation.com_out_ptr |= comment.contains("[iid_is]") && annotation.output;
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
struct RawAnnotation {
    key: String,
    value: Option<String>,
}

fn validate_win32metadata_annotation(
    target: CXCursor,
    attribute: CXCursor,
    spelling: &str,
) -> Result<(), Error> {
    let raw = parse_raw_annotation(spelling).unwrap();
    let requires_value = matches!(
        raw.key.as_str(),
        "import_library"
            | "static_library"
            | "raii_free"
            | "invalid_handle"
            | "free_with"
            | "array_count_param"
            | "array_count_const"
            | "array_count_field"
            | "memory_size_param"
            | "ignore_if_return"
            | "also_usable_for"
            | "associated_enum"
            | "associated_constant"
            | "native_inheritance"
            | "struct_size_field"
            | "native_encoding"
            | "supported_os"
    );
    let valueless = matches!(
        raw.key.as_str(),
        "set_last_error"
            | "preserve_result"
            | "can_return_errors_as_success"
            | "can_return_multiple_success_values"
            | "agile"
            | "do_not_release"
            | "not_null_terminated"
            | "null_null_terminated"
            | "retained"
            | "in"
            | "out"
            | "optional"
            | "reserved"
            | "retval"
            | "com_out_ptr"
            | "native_opaque"
            | "const"
            | "ansi"
            | "unicode"
    );
    let target_kind = unsafe { clang_getCursorKind(target) };
    let message = if !requires_value && !valueless {
        Some(format!("unknown win32metadata annotation `{}`", raw.key))
    } else if requires_value
        && raw
            .value
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        Some(format!(
            "win32metadata annotation `{}` requires a value",
            raw.key
        ))
    } else if valueless && raw.value.is_some() {
        Some(format!(
            "win32metadata annotation `{}` does not accept a value",
            raw.key
        ))
    } else if !annotation_target_allowed(&raw.key, target_kind)
        || (raw.key == "native_opaque"
            && !native_opaque_annotation_target_allowed(target, attribute))
        || (raw.key == "associated_enum"
            && target_kind == CXCursor_TypedefDecl
            && !typedef_is_callback(target))
        || (matches!(target_kind, CXCursor_StructDecl | CXCursor_UnionDecl)
            && unsafe { clang_Cursor_isAnonymousRecordDecl(target) } != 0)
    {
        Some(format!(
            "win32metadata annotation `{}` is not valid on this declaration",
            raw.key
        ))
    } else if raw.key == "associated_enum"
        && matches!(
            target_kind,
            CXCursor_FunctionDecl | CXCursor_CXXMethod | CXCursor_TypedefDecl
        )
        && annotation_result_type(target).is_some_and(|ty| ty.kind == CXType_Void)
    {
        Some("win32metadata annotation `associated_enum` requires a non-void return".to_string())
    } else if raw.key == "invalid_handle"
        && raw.value.as_deref().is_some_and(|value| {
            parse_annotation_integer(value).is_none() && !is_c_identifier(value.trim())
        })
    {
        Some(format!(
            "invalid-handle sentinel `{}` must be an integer literal or object-like macro",
            raw.value.unwrap()
        ))
    } else {
        None
    };
    if let Some(message) = message {
        return Err(annotation_error(attribute, &message));
    }
    Ok(())
}

fn annotation_target_allowed(key: &str, target: CXCursorKind) -> bool {
    match key {
        "set_last_error" | "import_library" | "static_library" => target == CXCursor_FunctionDecl,
        "preserve_result"
        | "can_return_errors_as_success"
        | "can_return_multiple_success_values" => {
            matches!(target, CXCursor_FunctionDecl | CXCursor_CXXMethod)
        }
        "agile" => matches!(
            target,
            CXCursor_ClassDecl | CXCursor_StructDecl | CXCursor_ClassTemplate
        ),
        "raii_free" | "invalid_handle" => matches!(
            target,
            CXCursor_FunctionDecl | CXCursor_CXXMethod | CXCursor_ParmDecl | CXCursor_TypedefDecl
        ),
        "free_with" | "do_not_release" | "not_null_terminated" | "null_null_terminated" => {
            matches!(
                target,
                CXCursor_FunctionDecl | CXCursor_CXXMethod | CXCursor_ParmDecl | CXCursor_FieldDecl
            )
        }
        "retained" | "ignore_if_return" | "array_count_param" | "array_count_const"
        | "memory_size_param" | "in" | "out" | "optional" | "reserved" | "retval"
        | "com_out_ptr" => target == CXCursor_ParmDecl,
        "array_count_field" => target == CXCursor_FieldDecl,
        "also_usable_for" => target == CXCursor_TypedefDecl,
        "associated_enum" => matches!(
            target,
            CXCursor_FunctionDecl
                | CXCursor_CXXMethod
                | CXCursor_ParmDecl
                | CXCursor_FieldDecl
                | CXCursor_VarDecl
                | CXCursor_EnumConstantDecl
                | CXCursor_TypedefDecl
        ),
        "associated_constant" => target == CXCursor_EnumDecl,
        "native_inheritance" | "struct_size_field" => {
            matches!(target, CXCursor_ClassDecl | CXCursor_StructDecl)
        }
        "native_opaque" => target == CXCursor_ClassDecl,
        "native_encoding" => matches!(target, CXCursor_FieldDecl | CXCursor_VarDecl),
        "const" => matches!(target, CXCursor_ParmDecl | CXCursor_FieldDecl),
        "ansi" | "unicode" => matches!(
            target,
            CXCursor_FunctionDecl | CXCursor_CXXMethod | CXCursor_FieldDecl | CXCursor_VarDecl
        ),
        "supported_os" => matches!(
            target,
            CXCursor_FunctionDecl
                | CXCursor_CXXMethod
                | CXCursor_ClassDecl
                | CXCursor_StructDecl
                | CXCursor_UnionDecl
                | CXCursor_EnumDecl
                | CXCursor_TypedefDecl
        ),
        _ => false,
    }
}

fn annotation_result_type(cursor: CXCursor) -> Option<CXType> {
    let kind = unsafe { clang_getCursorKind(cursor) };
    if matches!(kind, CXCursor_FunctionDecl | CXCursor_CXXMethod) {
        return Some(unsafe { clang_getCursorResultType(cursor) });
    }
    if kind != CXCursor_TypedefDecl {
        return None;
    }
    let mut ty = unsafe { clang_getTypedefDeclUnderlyingType(cursor) };
    if ty.kind == CXType_Pointer {
        ty = unsafe { clang_getPointeeType(ty) };
    }
    matches!(ty.kind, CXType_FunctionProto | CXType_FunctionNoProto)
        .then(|| unsafe { clang_getResultType(ty) })
}

fn typedef_is_callback(cursor: CXCursor) -> bool {
    annotation_result_type(cursor).is_some()
}

fn typedef_is_function_pointer_alias(cursor: CXCursor) -> bool {
    let ty = unsafe { clang_getTypedefDeclUnderlyingType(cursor) };
    ty.kind == CXType_Pointer
        && matches!(
            unsafe { clang_getPointeeType(ty) }.kind,
            CXType_FunctionProto | CXType_FunctionNoProto
        )
}

fn native_opaque_annotation_target_allowed(target: CXCursor, attribute: CXCursor) -> bool {
    let definition = cursor_definition(target);
    if unsafe { clang_getCursorKind(definition) } != CXCursor_ClassDecl
        || unsafe { clang_isCursorDefinition(definition) } == 0
        || cx_string(unsafe { clang_getCursorSpelling(definition) }).is_empty()
        || is_interface(definition)
        || cursor_uuid(definition).is_some()
    {
        return false;
    }
    let Some((_, expansion, _, _)) = cursor_locations(attribute) else {
        return false;
    };
    let Some((file, start, end)) = cursor_expansion_extent(definition) else {
        return false;
    };
    expansion.file.eq_ignore_ascii_case(&file) && (start..end).contains(&expansion.offset)
}

fn annotation_error(cursor: CXCursor, message: &str) -> Error {
    cursor_locations(cursor).map_or_else(
        || Error(message.to_string()),
        |(spelling, _, _, _)| Error(format!("{}:{}: {message}", spelling.file, spelling.offset)),
    )
}

fn parse_raw_annotation(spelling: &str) -> Option<RawAnnotation> {
    let payload = spelling.strip_prefix("win32metadata:")?;
    let (key, value) = payload
        .split_once('=')
        .map_or((payload, None), |(key, value)| {
            (key, Some(value.to_string()))
        });
    Some(RawAnnotation {
        key: key.to_string(),
        value,
    })
}

fn expanded_raw_annotations(cursor: CXCursor) -> Vec<RawAnnotation> {
    let mut result = vec![];
    for child in cursor_children(cursor) {
        if unsafe { clang_getCursorKind(child) } != CXCursor_AnnotateAttr {
            continue;
        }
        let spelling = cx_string(unsafe { clang_getCursorSpelling(child) });
        let Some(annotation) = parse_raw_annotation(&spelling) else {
            continue;
        };
        if annotation.key != "raii_free" {
            result.push(annotation);
            continue;
        }
        let mut values = annotation
            .value
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim);
        if let Some(cleanup) = values.next() {
            result.push(RawAnnotation {
                key: "raii_free".to_string(),
                value: Some(cleanup.to_string()),
            });
        }
        result.extend(
            values
                .filter(|value| !value.is_empty())
                .map(|value| RawAnnotation {
                    key: "invalid_handle".to_string(),
                    value: Some(value.to_string()),
                }),
        );
    }
    result
}

fn annotation_values(
    cursor: CXCursor,
    macros: &MacroDefinitions,
) -> Result<Vec<Annotation>, Error> {
    let before = macros.cursor_order(cursor).unwrap_or(usize::MAX);
    let location = cursor_locations(cursor).map(|(spelling, _, _, _)| spelling);
    expanded_raw_annotations(cursor)
        .into_iter()
        .filter_map(|mut raw| {
            if raw.key == "invalid_handle"
                && let Some(value) = raw.value.as_deref()
                && parse_annotation_integer(value).is_none()
            {
                let name = value.trim();
                let Some(value) = resolve_annotation_macro_integer(
                    name,
                    macros,
                    before,
                    location.as_ref(),
                    &mut BTreeSet::new(),
                ) else {
                    return Some(Err(Error(format!(
                        "could not evaluate invalid-handle sentinel `{name}`"
                    ))));
                };
                raw.value = Some(value.to_string());
            }
            annotation_value(raw).map(Ok)
        })
        .collect()
}

fn annotation_value(raw: RawAnnotation) -> Option<Annotation> {
    let value = raw.value;
    Some(match raw.key.as_str() {
        "set_last_error" => Annotation::SetLastError,
        "import_library" => Annotation::ImportLibrary(value?),
        "preserve_result" => Annotation::PreserveResult,
        "raii_free" => Annotation::RaiiFree(value?),
        "invalid_handle" => Annotation::InvalidHandle(value?),
        "free_with" => Annotation::FreeWith(value?),
        "do_not_release" => Annotation::DoNotRelease,
        "not_null_terminated" => Annotation::NotNullTerminated,
        "null_null_terminated" => Annotation::NullNullTerminated,
        "array_count_param" => Annotation::ArrayCountParam(value?),
        "array_count_const" => Annotation::ArrayCountConst(value?),
        "array_count_field" => Annotation::ArrayCountField(value?),
        "memory_size_param" => Annotation::MemorySizeParam(value?),
        "can_return_errors_as_success" => Annotation::CanReturnErrorsAsSuccess,
        "can_return_multiple_success_values" => Annotation::CanReturnMultipleSuccessValues,
        "retained" => Annotation::Retained,
        "ignore_if_return" => Annotation::IgnoreIfReturn(value?),
        "also_usable_for" => Annotation::AlsoUsableFor(value?),
        "associated_enum" => Annotation::AssociatedEnum(value?),
        "associated_constant" => Annotation::AssociatedConstant(value?),
        "native_inheritance" => Annotation::NativeInheritance(value?),
        "struct_size_field" => Annotation::StructSizeField(value?),
        "native_encoding" => Annotation::NativeEncoding(value?),
        "ansi" => Annotation::Ansi,
        "unicode" => Annotation::Unicode,
        "agile" => Annotation::Agile,
        "const" => Annotation::Const,
        "static_library" => Annotation::StaticLibrary(value?),
        "supported_os" => Annotation::SupportedOs(value?),
        "in" => Annotation::In,
        "out" => Annotation::Out,
        "optional" => Annotation::Optional,
        "reserved" => Annotation::Reserved,
        "com_out_ptr" => Annotation::ComOutPtr,
        "retval" => Annotation::Retval,
        "native_opaque" => return None,
        _ => return None,
    })
}

fn collect_fact_annotations(
    cursor: CXCursor,
    kind: FactKind,
    origin: &Origin,
    macros: &MacroDefinitions,
    annotations: &mut BTreeMap<AnnotationTarget, Vec<Annotation>>,
) -> Result<(), Error> {
    let direct = annotation_values(cursor, macros)?;
    let callback = kind == FactKind::Typedef && typedef_is_callback(cursor);
    let (declaration, result): (Vec<_>, Vec<_>) = direct.into_iter().partition(|annotation| {
        !matches!(
            annotation,
            Annotation::RaiiFree(_)
                | Annotation::InvalidHandle(_)
                | Annotation::FreeWith(_)
                | Annotation::DoNotRelease
                | Annotation::NotNullTerminated
                | Annotation::NullNullTerminated
                | Annotation::AssociatedEnum(_)
        ) || !matches!(kind, FactKind::Function) && !callback
    });
    insert_annotations(
        annotations,
        AnnotationTarget::Declaration(origin.clone()),
        declaration,
    );
    insert_annotations(
        annotations,
        AnnotationTarget::Return(origin.clone()),
        result,
    );

    if matches!(kind, FactKind::Function | FactKind::Typedef) {
        for (index, parameter) in cursor_children(cursor)
            .into_iter()
            .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_ParmDecl)
            .enumerate()
        {
            insert_annotations(
                annotations,
                AnnotationTarget::Parameter {
                    declaration: origin.clone(),
                    index,
                },
                annotation_values(parameter, macros)?,
            );
        }
    }

    if matches!(kind, FactKind::Class | FactKind::Struct | FactKind::Union) {
        collect_record_field_annotations(cursor, origin, macros, annotations, &[])?;
    }

    if kind == FactKind::Enum {
        for (index, variant) in cursor_children(cursor)
            .into_iter()
            .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_EnumConstantDecl)
            .enumerate()
        {
            insert_annotations(
                annotations,
                AnnotationTarget::Variant {
                    declaration: origin.clone(),
                    index,
                },
                annotation_values(variant, macros)?,
            );
        }
    }

    if matches!(kind, FactKind::Class | FactKind::Struct) && is_interface(cursor) {
        for (method_index, method) in cursor_children(cursor)
            .into_iter()
            .filter(|child| {
                (unsafe {
                    clang_getCursorKind(*child) == CXCursor_CXXMethod
                        && clang_CXXMethod_isVirtual(*child) != 0
                }) && !method_overrides_base(*child)
            })
            .enumerate()
        {
            let (declaration, result): (Vec<_>, Vec<_>) = annotation_values(method, macros)?
                .into_iter()
                .partition(|annotation| {
                    !matches!(
                        annotation,
                        Annotation::RaiiFree(_)
                            | Annotation::InvalidHandle(_)
                            | Annotation::FreeWith(_)
                            | Annotation::DoNotRelease
                            | Annotation::NotNullTerminated
                            | Annotation::NullNullTerminated
                            | Annotation::AssociatedEnum(_)
                    )
                });
            insert_annotations(
                annotations,
                AnnotationTarget::Method {
                    declaration: origin.clone(),
                    index: method_index,
                },
                declaration,
            );
            insert_annotations(
                annotations,
                AnnotationTarget::MethodReturn {
                    declaration: origin.clone(),
                    index: method_index,
                },
                result,
            );
            for (parameter_index, parameter) in cursor_children(method)
                .into_iter()
                .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_ParmDecl)
                .enumerate()
            {
                insert_annotations(
                    annotations,
                    AnnotationTarget::MethodParameter {
                        declaration: origin.clone(),
                        method: method_index,
                        parameter: parameter_index,
                    },
                    annotation_values(parameter, macros)?,
                );
            }
        }
    }
    Ok(())
}

fn collect_record_field_annotations(
    cursor: CXCursor,
    origin: &Origin,
    macros: &MacroDefinitions,
    annotations: &mut BTreeMap<AnnotationTarget, Vec<Annotation>>,
    prefix: &[usize],
) -> Result<(), Error> {
    let mut index = 0;
    for child in cursor_children(cursor) {
        match unsafe { clang_getCursorKind(child) } {
            CXCursor_CXXBaseSpecifier => index += 1,
            CXCursor_FieldDecl => {
                let target = if prefix.is_empty() {
                    AnnotationTarget::Field {
                        declaration: origin.clone(),
                        index,
                    }
                } else {
                    let mut path = prefix.to_vec();
                    path.push(index);
                    AnnotationTarget::NestedField {
                        declaration: origin.clone(),
                        path,
                    }
                };
                insert_annotations(annotations, target, annotation_values(child, macros)?);
                index += 1;
            }
            CXCursor_StructDecl | CXCursor_UnionDecl
                if unsafe { clang_Cursor_isAnonymousRecordDecl(child) } != 0 =>
            {
                let mut path = prefix.to_vec();
                path.push(index);
                collect_record_field_annotations(child, origin, macros, annotations, &path)?;
                index += 1;
            }
            _ => {}
        }
    }
    Ok(())
}

fn insert_annotations(
    annotations: &mut BTreeMap<AnnotationTarget, Vec<Annotation>>,
    target: AnnotationTarget,
    values: Vec<Annotation>,
) {
    if values.is_empty() {
        return;
    }
    annotations.entry(target).or_default().extend(values);
}

fn merge_redeclaration_annotations(
    facts: &[Fact],
    annotations: &mut BTreeMap<AnnotationTarget, Vec<Annotation>>,
) -> Result<(), Error> {
    let facts_by_origin: HashMap<_, _> = facts.iter().map(|fact| (&fact.origin, fact)).collect();
    let annotated_keys: BTreeSet<_> = annotations
        .keys()
        .filter_map(|target| facts_by_origin.get(annotation_target_origin(target)))
        .map(|fact| (fact.kind, fact.name.as_str()))
        .collect();
    let mut candidates: BTreeMap<(FactKind, &str), Vec<&Fact>> = BTreeMap::new();
    for fact in facts {
        let key = (fact.kind, fact.name.as_str());
        if annotated_keys.contains(&key) {
            candidates.entry(key).or_default().push(fact);
        }
    }

    for ((_, name), candidates) in candidates {
        let mut groups: Vec<Vec<&Fact>> = vec![];
        for candidate in candidates {
            if let Some(group) = groups.iter_mut().find(|group| {
                group.iter().all(|fact| {
                    annotation_declarations_compatible(fact, candidate, &facts_by_origin)
                })
            }) {
                group.push(candidate);
            } else {
                groups.push(vec![candidate]);
            }
        }

        for group in groups {
            if !group.iter().any(|fact| fact.root) {
                continue;
            }
            let origins: BTreeSet<_> = group.iter().map(|fact| fact.origin.clone()).collect();
            let mut slots: BTreeMap<AnnotationSlot, Vec<Annotation>> = BTreeMap::new();
            for (target, values) in annotations.iter() {
                if origins.contains(annotation_target_origin(target)) {
                    slots
                        .entry(annotation_target_slot(target))
                        .or_default()
                        .extend(values.iter().cloned());
                }
            }
            for (slot, values) in slots {
                let values = merge_annotation_values(values, name)?;
                for origin in &origins {
                    annotations.insert(
                        annotation_slot_target(origin.clone(), slot.clone()),
                        values.clone(),
                    );
                }
            }
        }
    }
    Ok(())
}

pub(super) fn annotation_declarations_compatible(
    left: &Fact,
    right: &Fact,
    facts: &HashMap<&Origin, &Fact>,
) -> bool {
    if left.kind != right.kind
        || left.name != right.name
        || annotation_parent_path(left, facts) != annotation_parent_path(right, facts)
    {
        return false;
    }
    match (&left.data, &right.data) {
        (
            FactData::Function {
                link_name: left_link,
                convention: left_convention,
                params: left_params,
                result: left_result,
                variadic: left_variadic,
                noreturn: left_noreturn,
            },
            FactData::Function {
                link_name: right_link,
                convention: right_convention,
                params: right_params,
                result: right_result,
                variadic: right_variadic,
                noreturn: right_noreturn,
            },
        ) => {
            left_link == right_link
                && left_convention == right_convention
                && left_variadic == right_variadic
                && left_noreturn == right_noreturn
                && annotation_params_compatible(left_params, right_params)
                && annotation_types_compatible(left_result, right_result)
        }
        (
            FactData::Callback {
                convention: left_convention,
                params: left_params,
                result: left_result,
            },
            FactData::Callback {
                convention: right_convention,
                params: right_params,
                result: right_result,
            },
        ) => {
            left_convention == right_convention
                && annotation_params_compatible(left_params, right_params)
                && annotation_types_compatible(left_result, right_result)
        }
        (
            FactData::Typedef {
                target: left_target,
            },
            FactData::Typedef {
                target: right_target,
            },
        ) => annotation_types_compatible(left_target, right_target),
        (
            FactData::Enum {
                repr: left_repr,
                variants: left_variants,
                fixed: left_fixed,
                scoped: left_scoped,
            },
            FactData::Enum {
                repr: right_repr,
                variants: right_variants,
                fixed: right_fixed,
                scoped: right_scoped,
            },
        ) => {
            left_repr == right_repr
                && left_fixed == right_fixed
                && left_scoped == right_scoped
                && (!left.definition || !right.definition || left_variants == right_variants)
        }
        (
            FactData::Record {
                base: left_base,
                fields: left_fields,
                size: left_size,
                align: left_align,
                packing: left_packing,
                alignment: left_alignment,
                union: left_union,
            },
            FactData::Record {
                base: right_base,
                fields: right_fields,
                size: right_size,
                align: right_align,
                packing: right_packing,
                alignment: right_alignment,
                union: right_union,
            },
        ) => {
            left_union == right_union
                && (!left.definition
                    || !right.definition
                    || (left_size == right_size
                        && left_align == right_align
                        && left_packing == right_packing
                        && left_alignment == right_alignment
                        && annotation_optional_types_compatible(left_base, right_base)
                        && annotation_fields_compatible(left_fields, right_fields)))
        }
        (
            FactData::Interface {
                base: left_base,
                guid: left_guid,
                methods: left_methods,
            },
            FactData::Interface {
                base: right_base,
                guid: right_guid,
                methods: right_methods,
            },
        ) => {
            (!left.definition || !right.definition)
                || (left_guid == right_guid
                    && annotation_optional_types_compatible(left_base, right_base)
                    && annotation_methods_compatible(left_methods, right_methods))
        }
        (FactData::None, FactData::Record { .. } | FactData::Interface { .. })
        | (FactData::Record { .. } | FactData::Interface { .. }, FactData::None)
            if !left.root || !right.root =>
        {
            true
        }
        _ => left.data == right.data,
    }
}

fn annotation_parent_path(fact: &Fact, facts: &HashMap<&Origin, &Fact>) -> Vec<(FactKind, String)> {
    let mut result = vec![];
    let mut parent = fact.parent.as_ref();
    while let Some(origin) = parent {
        let Some(fact) = facts.get(origin) else {
            break;
        };
        result.push((fact.kind, fact.name.clone()));
        parent = fact.parent.as_ref();
    }
    result.reverse();
    result
}

fn annotation_params_compatible(left: &[Parameter], right: &[Parameter]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| annotation_types_compatible(&left.ty, &right.ty))
}

fn annotation_fields_compatible(left: &[Field], right: &[Field]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.name == right.name
                && left.offset == right.offset
                && left.align == right.align
                && left.size == right.size
                && left.bit_width == right.bit_width
                && annotation_types_compatible(&left.ty, &right.ty)
        })
}

fn annotation_methods_compatible(left: &[Method], right: &[Method]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.name == right.name
                && left.special == right.special
                && annotation_params_compatible(&left.params, &right.params)
                && annotation_types_compatible(&left.result, &right.result)
        })
}

fn annotation_optional_types_compatible(left: &Option<TypeRef>, right: &Option<TypeRef>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => annotation_types_compatible(left, right),
        _ => false,
    }
}

fn annotation_types_compatible(left: &TypeRef, right: &TypeRef) -> bool {
    match (left, right) {
        (TypeRef::Void, TypeRef::Void)
        | (TypeRef::String, TypeRef::String)
        | (TypeRef::Object, TypeRef::Object) => true,
        (TypeRef::Scalar(left), TypeRef::Scalar(right)) => left == right,
        (
            TypeRef::Named {
                name: left_name, ..
            },
            TypeRef::Named {
                name: right_name, ..
            },
        ) => left_name == right_name,
        (
            TypeRef::Pointer {
                mutable: left_mutable,
                target: left_target,
            },
            TypeRef::Pointer {
                mutable: right_mutable,
                target: right_target,
            },
        )
        | (
            TypeRef::Reference {
                mutable: left_mutable,
                target: left_target,
            },
            TypeRef::Reference {
                mutable: right_mutable,
                target: right_target,
            },
        ) => {
            left_mutable == right_mutable && annotation_types_compatible(left_target, right_target)
        }
        (
            TypeRef::FunctionPointer {
                convention: left_convention,
                params: left_params,
                result: left_result,
            },
            TypeRef::FunctionPointer {
                convention: right_convention,
                params: right_params,
                result: right_result,
            },
        ) => {
            left_convention == right_convention
                && left_params.len() == right_params.len()
                && left_params
                    .iter()
                    .zip(right_params)
                    .all(|(left, right)| annotation_types_compatible(left, right))
                && annotation_types_compatible(left_result, right_result)
        }
        (
            TypeRef::OpaquePointer {
                mutable: left_mutable,
                tag: left_tag,
            },
            TypeRef::OpaquePointer {
                mutable: right_mutable,
                tag: right_tag,
            },
        ) => left_mutable == right_mutable && left_tag == right_tag,
        (
            TypeRef::Array {
                target: left_target,
                len: left_len,
            },
            TypeRef::Array {
                target: right_target,
                len: right_len,
            },
        ) => left_len == right_len && annotation_types_compatible(left_target, right_target),
        (
            TypeRef::Generic {
                name: left_name,
                args: left_args,
                ..
            },
            TypeRef::Generic {
                name: right_name,
                args: right_args,
                ..
            },
        ) => {
            left_name == right_name
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args)
                    .all(|(left, right)| annotation_types_compatible(left, right))
        }
        (TypeRef::InlineRecord(left), TypeRef::InlineRecord(right)) => {
            left.name == right.name
                && left.size == right.size
                && left.align == right.align
                && left.packing == right.packing
                && left.alignment == right.alignment
                && left.union == right.union
                && annotation_optional_types_compatible(&left.base, &right.base)
                && annotation_fields_compatible(&left.fields, &right.fields)
        }
        _ => false,
    }
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
enum AnnotationSlot {
    Declaration,
    Return,
    Parameter(usize),
    Field(usize),
    NestedField(Vec<usize>),
    Variant(usize),
    Method(usize),
    MethodReturn(usize),
    MethodParameter(usize, usize),
}

fn annotation_target_origin(target: &AnnotationTarget) -> &Origin {
    match target {
        AnnotationTarget::Declaration(origin) | AnnotationTarget::Return(origin) => origin,
        AnnotationTarget::Parameter { declaration, .. }
        | AnnotationTarget::Field { declaration, .. }
        | AnnotationTarget::NestedField { declaration, .. }
        | AnnotationTarget::Variant { declaration, .. }
        | AnnotationTarget::Method { declaration, .. }
        | AnnotationTarget::MethodReturn { declaration, .. }
        | AnnotationTarget::MethodParameter { declaration, .. } => declaration,
    }
}

fn annotation_target_slot(target: &AnnotationTarget) -> AnnotationSlot {
    match target {
        AnnotationTarget::Declaration(_) => AnnotationSlot::Declaration,
        AnnotationTarget::Return(_) => AnnotationSlot::Return,
        AnnotationTarget::Parameter { index, .. } => AnnotationSlot::Parameter(*index),
        AnnotationTarget::Field { index, .. } => AnnotationSlot::Field(*index),
        AnnotationTarget::NestedField { path, .. } => AnnotationSlot::NestedField(path.clone()),
        AnnotationTarget::Variant { index, .. } => AnnotationSlot::Variant(*index),
        AnnotationTarget::Method { index, .. } => AnnotationSlot::Method(*index),
        AnnotationTarget::MethodReturn { index, .. } => AnnotationSlot::MethodReturn(*index),
        AnnotationTarget::MethodParameter {
            method, parameter, ..
        } => AnnotationSlot::MethodParameter(*method, *parameter),
    }
}

fn annotation_slot_target(origin: Origin, slot: AnnotationSlot) -> AnnotationTarget {
    match slot {
        AnnotationSlot::Declaration => AnnotationTarget::Declaration(origin),
        AnnotationSlot::Return => AnnotationTarget::Return(origin),
        AnnotationSlot::Parameter(index) => AnnotationTarget::Parameter {
            declaration: origin,
            index,
        },
        AnnotationSlot::Field(index) => AnnotationTarget::Field {
            declaration: origin,
            index,
        },
        AnnotationSlot::NestedField(path) => AnnotationTarget::NestedField {
            declaration: origin,
            path,
        },
        AnnotationSlot::Variant(index) => AnnotationTarget::Variant {
            declaration: origin,
            index,
        },
        AnnotationSlot::Method(index) => AnnotationTarget::Method {
            declaration: origin,
            index,
        },
        AnnotationSlot::MethodReturn(index) => AnnotationTarget::MethodReturn {
            declaration: origin,
            index,
        },
        AnnotationSlot::MethodParameter(method, parameter) => AnnotationTarget::MethodParameter {
            declaration: origin,
            method,
            parameter,
        },
    }
}

fn merge_annotation_values(
    mut values: Vec<Annotation>,
    declaration: &str,
) -> Result<Vec<Annotation>, Error> {
    let import_library = values
        .iter()
        .find(|value| matches!(value, Annotation::ImportLibrary(_)))
        .cloned();
    values.retain(|value| !matches!(value, Annotation::ImportLibrary(_)));
    if let Some(import_library) = import_library {
        values.push(import_library);
    }
    values.sort();
    values.dedup();
    let mut keys = BTreeMap::<&'static str, &Annotation>::new();
    for value in &values {
        let key = annotation_key(value);
        if annotation_is_repeatable(value) {
            continue;
        }
        if let Some(previous) = keys.insert(key, value)
            && previous != value
        {
            return Err(Error(format!(
                "conflicting redeclaration annotation `{key}` on `{declaration}`: \
                 {previous:?} vs {value:?}"
            )));
        }
    }
    Ok(values)
}

fn annotation_key(annotation: &Annotation) -> &'static str {
    match annotation {
        Annotation::SetLastError => "set_last_error",
        Annotation::ImportLibrary(_) => "import_library",
        Annotation::PreserveResult => "preserve_result",
        Annotation::RaiiFree(_) => "raii_free",
        Annotation::InvalidHandle(_) => "invalid_handle",
        Annotation::FreeWith(_) => "free_with",
        Annotation::DoNotRelease => "do_not_release",
        Annotation::NotNullTerminated => "not_null_terminated",
        Annotation::NullNullTerminated => "null_null_terminated",
        Annotation::ArrayCountParam(_) => "array_count_param",
        Annotation::ArrayCountConst(_) => "array_count_const",
        Annotation::ArrayCountField(_) => "array_count_field",
        Annotation::MemorySizeParam(_) => "memory_size_param",
        Annotation::CanReturnErrorsAsSuccess => "can_return_errors_as_success",
        Annotation::CanReturnMultipleSuccessValues => "can_return_multiple_success_values",
        Annotation::Retained => "retained",
        Annotation::IgnoreIfReturn(_) => "ignore_if_return",
        Annotation::AlsoUsableFor(_) => "also_usable_for",
        Annotation::AssociatedEnum(_) => "associated_enum",
        Annotation::AssociatedConstant(_) => "associated_constant",
        Annotation::NativeInheritance(_) => "native_inheritance",
        Annotation::StructSizeField(_) => "struct_size_field",
        Annotation::NativeEncoding(_) => "native_encoding",
        Annotation::Ansi => "ansi",
        Annotation::Unicode => "unicode",
        Annotation::Agile => "agile",
        Annotation::Const => "const",
        Annotation::StaticLibrary(_) => "static_library",
        Annotation::SupportedOs(_) => "supported_os",
        Annotation::In => "in",
        Annotation::Out => "out",
        Annotation::Optional => "optional",
        Annotation::Reserved => "reserved",
        Annotation::ComOutPtr => "com_out_ptr",
        Annotation::Retval => "retval",
    }
}

fn annotation_is_repeatable(annotation: &Annotation) -> bool {
    matches!(
        annotation,
        Annotation::InvalidHandle(_)
            | Annotation::AssociatedConstant(_)
            | Annotation::SupportedOs(_)
    )
}

fn annotation_macro_names(
    annotations: &BTreeMap<AnnotationTarget, Vec<Annotation>>,
) -> BTreeSet<String> {
    annotations
        .values()
        .flatten()
        .filter_map(|annotation| match annotation {
            Annotation::AssociatedConstant(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn associated_constant_names(
    facts: &[Fact],
    annotations: &BTreeMap<AnnotationTarget, Vec<Annotation>>,
) -> BTreeSet<String> {
    let roots: HashSet<_> = facts
        .iter()
        .filter(|fact| fact.root && fact.kind == FactKind::Enum)
        .map(|fact| &fact.origin)
        .collect();
    annotations
        .iter()
        .filter(|(target, _)| {
            matches!(target, AnnotationTarget::Declaration(origin) if roots.contains(origin))
        })
        .flat_map(|(_, values)| values)
        .filter_map(|annotation| match annotation {
            Annotation::AssociatedConstant(name) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn validate_associated_constants(
    names: &BTreeSet<String>,
    constants: &[Constant],
) -> Result<(), Error> {
    for name in names {
        let providers: BTreeSet<_> = constants
            .iter()
            .filter(|constant| constant.name == *name)
            .map(|constant| constant.spelling.file.as_str())
            .collect();
        if providers.is_empty() {
            return Err(Error(format!(
                "associated constant `{name}` has no source provider"
            )));
        }
        if providers.len() != 1 {
            return Err(Error(format!(
                "associated constant `{name}` has multiple source providers"
            )));
        }
        if constants
            .iter()
            .filter(|constant| constant.name == *name)
            .any(|constant| matches!(constant.value, Value::Utf8(_) | Value::Utf16(_)))
        {
            return Err(Error(format!(
                "associated constant `{name}` is not a native integer constant"
            )));
        }
    }
    Ok(())
}

fn resolve_annotation_macro_integer(
    name: &str,
    macros: &MacroDefinitions,
    before: usize,
    location: Option<&Location>,
    seen: &mut BTreeSet<String>,
) -> Option<i64> {
    if !seen.insert(name.to_string()) {
        return None;
    }
    let result = (|| {
        let (tokens, function_like) = macros.definition_before(name, before, location)?;
        if function_like {
            return None;
        }
        AnnotationExpression::new(tokens, macros, before, location, seen).parse()
    })();
    seen.remove(name);
    result
}

struct AnnotationExpression<'a, 'tu> {
    tokens: &'a [String],
    index: usize,
    macros: &'a MacroDefinitions<'tu>,
    before: usize,
    location: Option<&'a Location>,
    seen: &'a mut BTreeSet<String>,
}

impl<'a, 'tu> AnnotationExpression<'a, 'tu> {
    fn new(
        tokens: &'a [String],
        macros: &'a MacroDefinitions<'tu>,
        before: usize,
        location: Option<&'a Location>,
        seen: &'a mut BTreeSet<String>,
    ) -> Self {
        Self {
            tokens,
            index: 0,
            macros,
            before,
            location,
            seen,
        }
    }

    fn parse(mut self) -> Option<i64> {
        let value = self.conditional()?;
        (self.index == self.tokens.len()).then_some(value)
    }

    fn conditional(&mut self) -> Option<i64> {
        let condition = self.binary(1)?;
        if !self.consume("?") {
            return Some(condition);
        }
        let when_true = self.conditional()?;
        self.expect(":")?;
        let when_false = self.conditional()?;
        Some(if condition != 0 {
            when_true
        } else {
            when_false
        })
    }

    fn binary(&mut self, minimum_precedence: u8) -> Option<i64> {
        let mut left = self.unary()?;
        while let Some((precedence, operator)) = self.peek().and_then(annotation_binary_precedence)
        {
            if precedence < minimum_precedence {
                break;
            }
            self.index += 1;
            let right = self.binary(precedence + 1)?;
            left = annotation_binary(operator, left, right)?;
        }
        Some(left)
    }

    fn unary(&mut self) -> Option<i64> {
        if self.consume("+") {
            return self.unary();
        }
        if self.consume("-") {
            return Some(self.unary()?.wrapping_neg());
        }
        if self.consume("~") {
            return Some(!self.unary()?);
        }
        if self.consume("!") {
            return Some(i64::from(self.unary()? == 0));
        }
        if self.peek() == Some("(")
            && let Some(close) = self.matching_close(self.index)
            && self.is_cast(self.index + 1, close)
        {
            self.index = close + 1;
            return self.unary();
        }
        if self.consume("(") {
            let value = self.conditional()?;
            self.expect(")")?;
            return Some(value);
        }
        let token = self.peek()?.to_string();
        self.index += 1;
        parse_annotation_integer(&token).or_else(|| {
            is_c_identifier(&token).then(|| {
                resolve_annotation_macro_integer(
                    &token,
                    self.macros,
                    self.before,
                    self.location,
                    self.seen,
                )
            })?
        })
    }

    fn is_cast(&mut self, start: usize, end: usize) -> bool {
        if start == end {
            return false;
        }
        let tokens = &self.tokens[start..end];
        if tokens.len() == 1
            && is_c_identifier(&tokens[0])
            && resolve_annotation_macro_integer(
                &tokens[0],
                self.macros,
                self.before,
                self.location,
                self.seen,
            )
            .is_some()
        {
            return false;
        }
        tokens.iter().all(|token| {
            is_c_identifier(token)
                || matches!(
                    token.as_str(),
                    "*" | "const" | "volatile" | "signed" | "unsigned" | "long" | "short"
                )
        })
    }

    fn matching_close(&self, open: usize) -> Option<usize> {
        let mut depth = 0;
        for (index, token) in self.tokens.iter().enumerate().skip(open) {
            match token.as_str() {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.index).map(String::as_str)
    }

    fn consume(&mut self, token: &str) -> bool {
        if self.peek() == Some(token) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Option<()> {
        self.consume(token).then_some(())
    }
}

fn annotation_binary_precedence(operator: &str) -> Option<(u8, &'static str)> {
    Some(match operator {
        "||" => (1, "||"),
        "&&" => (2, "&&"),
        "|" => (3, "|"),
        "^" => (4, "^"),
        "&" => (5, "&"),
        "==" => (6, "=="),
        "!=" => (6, "!="),
        "<" => (7, "<"),
        "<=" => (7, "<="),
        ">" => (7, ">"),
        ">=" => (7, ">="),
        "<<" => (8, "<<"),
        ">>" => (8, ">>"),
        "+" => (9, "+"),
        "-" => (9, "-"),
        "*" => (10, "*"),
        "/" => (10, "/"),
        "%" => (10, "%"),
        _ => return None,
    })
}

fn annotation_binary(operator: &str, left: i64, right: i64) -> Option<i64> {
    Some(match operator {
        "||" => i64::from(left != 0 || right != 0),
        "&&" => i64::from(left != 0 && right != 0),
        "|" => left | right,
        "^" => left ^ right,
        "&" => left & right,
        "==" => i64::from(left == right),
        "!=" => i64::from(left != right),
        "<" => i64::from(left < right),
        "<=" => i64::from(left <= right),
        ">" => i64::from(left > right),
        ">=" => i64::from(left >= right),
        "<<" => left.wrapping_shl(right.try_into().ok()?),
        ">>" => left.wrapping_shr(right.try_into().ok()?),
        "+" => left.wrapping_add(right),
        "-" => left.wrapping_sub(right),
        "*" => left.wrapping_mul(right),
        "/" => left.checked_div(right)?,
        "%" => left.checked_rem(right)?,
        _ => return None,
    })
}

fn parse_annotation_integer(value: &str) -> Option<i64> {
    let value = value.trim();
    let (negative, value) = if let Some(value) = value.strip_prefix('-') {
        (true, value)
    } else if let Some(value) = value.strip_prefix('+') {
        (false, value)
    } else {
        (false, value)
    };
    let value = value.trim_end_matches(['u', 'U', 'l', 'L']);
    let magnitude = if let Some(value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        i64::from_str_radix(value, 16).ok()?
    } else {
        value.parse().ok()?
    };
    Some(if negative { -magnitude } else { magnitude })
}

fn parameter_annotation(cursor: CXCursor) -> ParamAnnotation {
    let mut result = ParamAnnotation::default();
    for child in cursor_children(cursor) {
        if unsafe { clang_getCursorKind(child) } != CXCursor_AnnotateAttr {
            continue;
        }

        let annotation = cx_string(unsafe { clang_getCursorSpelling(child) });
        if annotation == "win32metadata:in" {
            result.input = true;
        }
        if annotation == "win32metadata:out" {
            result.output = true;
        }
        if annotation == "win32metadata:optional" {
            result.optional = true;
        }
        if annotation == "win32metadata:reserved" {
            result.reserved = true;
        }
        if annotation == "win32metadata:com_out_ptr" {
            result.com_out_ptr = is_void_double_pointer(unsafe { clang_getCursorType(cursor) });
        }
        if annotation.starts_with("_In_") || annotation.starts_with("_Inout_") {
            result.input = true;
        }
        if annotation.starts_with("_Out_")
            || annotation.starts_with("_Outptr_")
            || annotation.starts_with("_COM_Outptr_")
            || annotation.starts_with("_Inout_")
        {
            result.output = true;
        }
        if annotation.contains("_opt_")
            || (annotation.starts_with("_Outptr_") && annotation.contains("_result_maybenull_"))
        {
            result.optional = true;
        }
        if annotation == "_Reserved_" {
            result.reserved = true;
        }
        if annotation.starts_with("_COM_Outptr_") {
            result.com_out_ptr = is_void_double_pointer(unsafe { clang_getCursorType(cursor) });
        }
        let sal_name = annotation
            .split_once('(')
            .map_or(annotation.as_str(), |value| value.0);
        if sal_name.contains("_z_") || sal_name.ends_with("_z") {
            result.null_terminated = true;
        }
        if annotation == "_NullNull_terminated_" {
            result.null_null_terminated = true;
        }
        if result.size.is_none()
            && (annotation.contains("_reads_")
                || annotation.contains("_writes_")
                || annotation.contains("_updates_"))
            && let Some(argument) = annotation
                .split_once('(')
                .and_then(|(_, rest)| rest.strip_suffix(')'))
                .and_then(|arguments| arguments.split(',').next())
        {
            let argument = argument.trim();
            let value = if let Some(value) = parse_sal_integer(argument) {
                SalSizeValue::Constant(value)
            } else if is_c_identifier(argument) {
                SalSizeValue::Parameter(argument.to_string())
            } else if let Some(argument) = argument.strip_prefix('*').map(str::trim)
                && is_c_identifier(argument)
            {
                SalSizeValue::IndirectParameter(argument.to_string())
            } else {
                SalSizeValue::Expression(argument.to_string())
            };
            result.size = Some(SalSize {
                bytes: annotation.contains("_bytes"),
                value,
            });
        }
    }
    result
}

fn function_is_noreturn(cursor: CXCursor) -> bool {
    let ty = unsafe { clang_getCursorType(cursor) };
    if cx_string(unsafe { clang_getTypeSpelling(ty) }).contains("noreturn") {
        return true;
    }
    cursor_children(cursor).into_iter().any(|child| {
        (unsafe { clang_getCursorKind(child) }) == CXCursor_AnnotateAttr
            && cx_string(unsafe { clang_getCursorSpelling(child) }) == "_Analysis_noreturn_"
    })
}

fn is_void_double_pointer(ty: CXType) -> bool {
    let ty = unsafe { clang_getCanonicalType(ty) };
    if ty.kind != CXType_Pointer {
        return false;
    }
    let ty = unsafe { clang_getCanonicalType(clang_getPointeeType(ty)) };
    if ty.kind != CXType_Pointer {
        return false;
    }
    unsafe { clang_getCanonicalType(clang_getPointeeType(ty)) }.kind == CXType_Void
}

fn parse_sal_integer(value: &str) -> Option<i32> {
    let value = value.trim_end_matches(['u', 'U', 'l', 'L']);
    if let Some(value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        i32::from_str_radix(value, 16).ok()
    } else {
        value.parse().ok()
    }
}

fn is_c_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn function_signature(
    ty: CXType,
    convention: Option<CallingConvention>,
) -> Option<(CallingConvention, Vec<TypeRef>, TypeRef)> {
    let convention = convention.or_else(|| calling_convention_fact(ty))?;
    let result = type_ref(unsafe { clang_getResultType(ty) })?;
    let count = unsafe { clang_getNumArgTypes(ty) };
    if count < 0 {
        return None;
    }
    let params = (0..count)
        .map(|index| {
            function_param_type(unsafe { clang_getArgType(ty, index.try_into().unwrap()) })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((convention, params, result))
}

fn function_param_type(ty: CXType) -> Option<TypeRef> {
    let array = if matches!(ty.kind, CXType_IncompleteArray | CXType_ConstantArray) {
        Some(ty)
    } else {
        let declaration = unsafe { clang_getTypeDeclaration(ty) };
        if unsafe { clang_getCursorKind(declaration) } == CXCursor_TypedefDecl {
            let underlying = unsafe { clang_getTypedefDeclUnderlyingType(declaration) };
            matches!(
                underlying.kind,
                CXType_IncompleteArray | CXType_ConstantArray
            )
            .then_some(underlying)
        } else {
            None
        }
    };
    if let Some(array) = array {
        let canonical = unsafe { clang_getCanonicalType(ty) };
        if canonical.kind == CXType_Pointer {
            return type_ref(canonical);
        }
        let element = unsafe { clang_getArrayElementType(array) };
        return Some(TypeRef::Pointer {
            mutable: unsafe {
                clang_isConstQualifiedType(ty) == 0 && clang_isConstQualifiedType(element) == 0
            },
            target: Box::new(type_ref(element)?),
        });
    }
    if matches!(ty.kind, CXType_FunctionProto | CXType_FunctionNoProto) {
        let (convention, params, result) = function_signature(ty, None)?;
        return Some(TypeRef::FunctionPointer {
            convention,
            params,
            result: Box::new(result),
        });
    }
    type_ref(ty)
}

fn function_param_type_at_cursor(cursor: CXCursor, ty: CXType) -> Option<TypeRef> {
    let mut ty = function_param_type(ty)?;
    preserve_named_function_type(cursor, &mut ty);
    Some(ty)
}

fn preserve_named_function_type(cursor: CXCursor, ty: &mut TypeRef) {
    let Some((name, declaration)) = cursor_children(cursor).into_iter().find_map(|child| {
        if unsafe { clang_getCursorKind(child) } != CXCursor_TypeRef {
            return None;
        }
        let referenced = unsafe { clang_getCursorReferenced(child) };
        if unsafe { clang_getCursorKind(referenced) } != CXCursor_TypedefDecl {
            return None;
        }
        let underlying =
            unsafe { clang_getCanonicalType(clang_getTypedefDeclUnderlyingType(referenced)) };
        if !matches!(
            underlying.kind,
            CXType_FunctionProto | CXType_FunctionNoProto
        ) {
            return None;
        }
        let name = cx_string(unsafe { clang_getCursorSpelling(referenced) });
        let (declaration, _, _, _) = cursor_locations(referenced)?;
        Some((name, declaration))
    }) else {
        return;
    };
    replace_function_pointer(ty, &TypeRef::Named { name, declaration });
}

fn replace_function_pointer(ty: &mut TypeRef, replacement: &TypeRef) -> bool {
    match ty {
        TypeRef::FunctionPointer { .. } => {
            *ty = replacement.clone();
            true
        }
        TypeRef::Pointer { target, .. } | TypeRef::Reference { target, .. } => {
            replace_function_pointer(target, replacement)
        }
        _ => false,
    }
}

fn calling_convention_fact(ty: CXType) -> Option<CallingConvention> {
    Some(match unsafe { clang_getFunctionTypeCallingConv(ty) } {
        CXCallingConv_C => CallingConvention::C,
        CXCallingConv_Default
        | CXCallingConv_X86FastCall
        | CXCallingConv_X86StdCall
        | CXCallingConv_Win64 => CallingConvention::Platform,
        _ => return None,
    })
}

fn inherited_function_typedef_calling_convention(
    cursor: CXCursor,
    function: CXType,
    macros: &MacroDefinitions,
) -> Option<CallingConvention> {
    let function = unsafe { clang_getCanonicalType(function) };
    cursor_children(cursor).into_iter().find_map(|child| {
        if unsafe { clang_getCursorKind(child) } != CXCursor_TypeRef {
            return None;
        }
        let declaration = unsafe { clang_getCursorReferenced(child) };
        if unsafe { clang_getCursorKind(declaration) } != CXCursor_TypedefDecl {
            return None;
        }
        let referenced =
            unsafe { clang_getCanonicalType(clang_getTypedefDeclUnderlyingType(declaration)) };
        if unsafe { clang_equalTypes(function, referenced) } == 0 {
            return None;
        }
        source_calling_convention(declaration, macros)
    })
}

fn source_calling_convention(
    cursor: CXCursor,
    macros: &MacroDefinitions,
) -> Option<CallingConvention> {
    fn literal(token: &str) -> Option<CallingConvention> {
        match token {
            "__stdcall" | "_stdcall" => Some(CallingConvention::Platform),
            "__cdecl" | "_cdecl" => Some(CallingConvention::C),
            _ => None,
        }
    }

    fn resolve(
        token: &str,
        macros: &MacroDefinitions,
        order: usize,
        visited: &mut HashSet<String>,
    ) -> Option<CallingConvention> {
        if let Some(convention) = literal(token) {
            return Some(convention);
        }
        if !visited.insert(token.to_string()) {
            return None;
        }
        macros
            .get_before(token, order)?
            .iter()
            .find_map(|token| resolve(token, macros, order, visited))
    }

    let tokens = cursor_tokens(cursor);
    macros
        .expansion_order(cursor)
        .and_then(|order| {
            tokens_before_method_name(&tokens, cursor)
                .iter()
                .find_map(|(_, token)| resolve(token, macros, order, &mut HashSet::new()))
                .or_else(|| {
                    tokens
                        .iter()
                        .find_map(|(_, token)| resolve(token, macros, order, &mut HashSet::new()))
                })
        })
        .or_else(|| tokens.iter().find_map(|(_, token)| literal(token)))
        .or_else(|| {
            cursor_source(cursor)?
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .find_map(literal)
        })
}

fn inline_record(
    cursor: CXCursor,
    union: bool,
    macros: Option<&MacroDefinitions>,
) -> Result<InlineRecord, String> {
    inline_record_with_pointer_class_layouts(cursor, union, macros, false)
}

fn inline_record_with_pointer_class_layouts(
    cursor: CXCursor,
    union: bool,
    macros: Option<&MacroDefinitions>,
    preserve_pointer_class_layouts: bool,
) -> Result<InlineRecord, String> {
    let mut base = None;
    let mut base_count = 0;
    let mut fields: Vec<Field> = vec![];
    let mut anonymous = 0;
    for child in cursor_children(cursor) {
        let kind = unsafe { clang_getCursorKind(child) };
        if kind == CXCursor_CXXBaseSpecifier {
            if union || unsafe { clang_isVirtualBase(child) } != 0 {
                return Err("record has unsupported inheritance".to_string());
            }
            let base_ty = unsafe { clang_getCursorType(child) };
            let base_ref = if preserve_pointer_class_layouts {
                type_ref_preserving_pointer_class_layouts(base_ty)
            } else {
                type_ref(base_ty)
            }
            .ok_or_else(|| "record base has an unsupported type".to_string())?;
            let align = unsafe { clang_Type_getAlignOf(base_ty) };
            let size = unsafe { clang_Type_getSizeOf(base_ty) };
            if align <= 0 || size < 0 {
                return Err("record base has an invalid layout".to_string());
            }
            let offset = if let Some(previous) = fields.last() {
                align_up((previous.offset + previous.size * 8) / 8, align) * 8
            } else {
                0
            };
            base_count += 1;
            fields.push(Field {
                name: if base_count == 1 {
                    "Base".to_string()
                } else {
                    format!("Base{base_count}")
                },
                ty: base_ref.clone(),
                offset,
                align,
                size,
                bit_width: None,
            });
            base.get_or_insert(base_ref);
        } else if kind == CXCursor_FieldDecl {
            let name = cx_string(unsafe { clang_getCursorSpelling(child) });
            let field_ty = unsafe { clang_getCursorType(child) };
            let mut ty = if preserve_pointer_class_layouts {
                type_ref_preserving_pointer_class_layouts(field_ty)
            } else {
                type_ref(field_ty)
            }
            .ok_or_else(|| {
                format!(
                    "field `{name}` has unsupported type `{}`",
                    cx_string(unsafe { clang_getTypeSpelling(field_ty) })
                )
            })?;
            preserve_named_function_type(child, &mut ty);
            if let TypeRef::FunctionPointer { convention, .. } = &mut ty
                && let Some(source) =
                    macros.and_then(|macros| source_calling_convention(child, macros))
            {
                *convention = source;
            }
            let (align, size) = field_layout(child, field_ty, &name)?;
            let bit_width = if unsafe { clang_Cursor_isBitField(child) } != 0 {
                let width = unsafe { clang_getFieldDeclBitWidth(child) };
                if width < 0 {
                    return Err(format!("bitfield `{name}` width is unavailable"));
                }
                Some(
                    width
                        .try_into()
                        .map_err(|_| format!("bitfield `{name}` width is out of range"))?,
                )
            } else {
                None
            };
            fields.push(Field {
                name,
                ty,
                offset: unsafe { clang_Cursor_getOffsetOfField(child) },
                align,
                size,
                bit_width,
            });
        } else if matches!(kind, CXCursor_StructDecl | CXCursor_UnionDecl)
            && unsafe { clang_Cursor_isAnonymousRecordDecl(child) } != 0
        {
            let nested = inline_record_with_pointer_class_layouts(
                child,
                kind == CXCursor_UnionDecl,
                macros,
                preserve_pointer_class_layouts,
            )?;
            let (promoted, relative) = promoted_members(&nested)
                .into_iter()
                .find_map(|(member, relative)| {
                    let member = CString::new(member).unwrap();
                    let promoted = unsafe {
                        clang_Type_getOffsetOf(clang_getCursorType(cursor), member.as_ptr())
                    };
                    (promoted >= 0).then_some((promoted, relative))
                })
                .ok_or_else(|| "anonymous aggregate offset is unavailable".to_string())?;
            anonymous += 1;
            fields.push(Field {
                name: if anonymous == 1 {
                    "Anonymous".to_string()
                } else {
                    format!("Anonymous{anonymous}")
                },
                offset: promoted - relative,
                align: nested.align,
                size: nested.size,
                bit_width: None,
                ty: TypeRef::InlineRecord(Box::new(nested)),
            });
        }
    }
    let mut names = BTreeSet::new();
    if let Some(duplicate) = fields
        .iter()
        .map(|field| field.name.as_str())
        .filter(|name| !name.is_empty())
        .find(|name| !names.insert(*name))
    {
        return Err(format!("record inheritance duplicates field `{duplicate}`"));
    }

    let ty = unsafe { clang_getCursorType(cursor) };
    let size = unsafe { clang_Type_getSizeOf(ty) };
    let align = unsafe { clang_Type_getAlignOf(ty) };
    let (packing, alignment) = record_layout(&fields, size, align, union)
        .map_err(|reason| format!("{reason}: fields {fields:?}, size {size}, alignment {align}"))?;
    Ok(InlineRecord {
        name: None,
        base,
        fields,
        size,
        align,
        packing,
        alignment,
        union,
    })
}

fn field_layout(cursor: CXCursor, ty: CXType, name: &str) -> Result<(i64, i64), String> {
    if ty.kind == CXType_IncompleteArray {
        return Ok((
            unsafe { clang_Type_getAlignOf(clang_getArrayElementType(ty)) },
            0,
        ));
    }
    let canonical = unsafe { clang_getCanonicalType(ty) };
    if !matches!(
        canonical.kind,
        CXType_LValueReference | CXType_RValueReference
    ) {
        return Ok(unsafe { (clang_Type_getAlignOf(ty), clang_Type_getSizeOf(ty)) });
    }
    let referent = unsafe { clang_getCanonicalType(clang_getPointeeType(canonical)) };
    if matches!(referent.kind, CXType_FunctionProto | CXType_FunctionNoProto) {
        return Err(format!(
            "field `{name}` has unsupported function reference type `{}`",
            cx_string(unsafe { clang_getTypeSpelling(ty) })
        ));
    }
    reference_storage_layout(cursor).ok_or_else(|| {
        format!(
            "field `{name}` has unavailable reference storage layout for `{}`",
            cx_string(unsafe { clang_getTypeSpelling(ty) })
        )
    })
}

fn reference_storage_layout(cursor: CXCursor) -> Option<(i64, i64)> {
    let target =
        unsafe { clang_getTranslationUnitTargetInfo(clang_Cursor_getTranslationUnit(cursor)) };
    if target.is_null() {
        return None;
    }
    let width = unsafe { clang_TargetInfo_getPointerWidth(target) };
    unsafe { clang_TargetInfo_dispose(target) };
    if width <= 0 || width % 8 != 0 {
        return None;
    }
    let size = i64::from(width / 8);
    Some((size, size))
}

fn name_indirect_inline_records(record: &mut InlineRecord, owner: &str) {
    let mut next = 0;
    for field in &mut record.fields {
        name_indirect_inline_type(&mut field.ty, owner, &mut next, false);
    }
}

fn name_indirect_inline_type(ty: &mut TypeRef, owner: &str, next: &mut usize, indirect: bool) {
    match ty {
        TypeRef::Array { target, .. }
        | TypeRef::Pointer { target, .. }
        | TypeRef::Reference { target, .. } => {
            name_indirect_inline_type(target, owner, next, true);
        }
        TypeRef::InlineRecord(record) => {
            let nested_owner = record.name.clone().unwrap_or_else(|| {
                let name = format!("{owner}_{}", *next);
                *next += 1;
                if indirect {
                    record.name = Some(name.clone());
                }
                name
            });
            name_indirect_inline_records(record, &nested_owner);
        }
        _ => {}
    }
}

fn promoted_members(record: &InlineRecord) -> Vec<(&str, i64)> {
    let mut result = vec![];
    for field in &record.fields {
        if !field.name.is_empty() {
            result.push((field.name.as_str(), field.offset));
        }
        if let TypeRef::InlineRecord(nested) = &field.ty {
            for (name, offset) in promoted_members(nested) {
                result.push((name, field.offset + offset));
            }
        }
    }
    result
}

fn type_ref_preserving_pointer_class_layouts(ty: CXType) -> Option<TypeRef> {
    if ty.kind != CXType_Pointer {
        return type_ref(ty);
    }
    let pointee = unsafe { clang_getPointeeType(ty) };
    let canonical_pointee = unsafe { clang_getCanonicalType(pointee) };
    if matches!(
        canonical_pointee.kind,
        CXType_FunctionProto | CXType_FunctionNoProto
    ) {
        return type_ref(ty);
    }
    let declaration = unsafe { clang_getTypeDeclaration(pointee) };
    if let Some(definition) = native_opaque_class_definition(declaration) {
        let name = cx_string(unsafe { clang_getCursorSpelling(definition) });
        let (declaration, _, _, _) = cursor_locations(definition)?;
        return Some(TypeRef::Pointer {
            mutable: unsafe { clang_isConstQualifiedType(pointee) } == 0,
            target: Box::new(TypeRef::Named { name, declaration }),
        });
    }
    if unsafe { clang_Cursor_isNull(declaration) } == 0
        && unsafe { clang_getCursorKind(declaration) } == CXCursor_ClassDecl
        && !is_interface(declaration)
        && cursor_uuid(declaration).is_none()
        && unsafe { clang_isCursorDefinition(cursor_definition(declaration)) } != 0
        && !is_data_class(declaration)
    {
        if let Some((definition, _)) = pointer_only_class_definition(declaration) {
            let name = cx_string(unsafe { clang_getCursorSpelling(definition) });
            let (declaration, _, _, _) = cursor_locations(definition)?;
            return Some(TypeRef::Pointer {
                mutable: unsafe { clang_isConstQualifiedType(pointee) } == 0,
                target: Box::new(TypeRef::Named { name, declaration }),
            });
        }
        return Some(TypeRef::OpaquePointer {
            mutable: unsafe { clang_isConstQualifiedType(pointee) } == 0,
            tag: cx_string(unsafe { clang_getCursorSpelling(declaration) }),
        });
    }
    let target = type_ref_preserving_pointer_class_layouts(pointee)?;
    Some(TypeRef::Pointer {
        mutable: unsafe { clang_isConstQualifiedType(pointee) } == 0,
        target: Box::new(target),
    })
}

fn type_ref(ty: CXType) -> Option<TypeRef> {
    let generic_count = unsafe { clang_Type_getNumTemplateArguments(ty) };
    if generic_count > 0 && ty.kind != CXType_Typedef {
        let declaration = unsafe { clang_getTypeDeclaration(ty) };
        let name = cx_string(unsafe { clang_getCursorSpelling(declaration) });
        let (declaration, _, _, _) = cursor_locations(declaration)?;
        let args = (0..generic_count)
            .map(|index| {
                winrt_generic_arg(unsafe {
                    clang_Type_getTemplateArgumentAsType(ty, index.try_into().unwrap())
                })
            })
            .collect::<Option<Vec<_>>>()?;
        return Some(TypeRef::Generic {
            name,
            declaration,
            args,
        });
    }
    if ty.kind == CXType_Elaborated {
        let declaration = unsafe { clang_getTypeDeclaration(ty) };
        if unsafe { clang_Cursor_isNull(declaration) } == 0 {
            let kind = unsafe { clang_getCursorKind(declaration) };
            if matches!(kind, CXCursor_StructDecl | CXCursor_UnionDecl)
                && cx_string(unsafe { clang_getTypeSpelling(ty) }).contains("(unnamed at ")
            {
                return inline_record(declaration, kind == CXCursor_UnionDecl, None)
                    .ok()
                    .map(|record| TypeRef::InlineRecord(Box::new(record)));
            }
        }
        return type_ref(unsafe { clang_Type_getNamedType(ty) });
    }
    if ty.kind == CXType_Void {
        return Some(TypeRef::Void);
    }
    if ty.kind == CXType_Pointer {
        let pointee = unsafe { clang_getPointeeType(ty) };
        let canonical_pointee = unsafe { clang_getCanonicalType(pointee) };
        if matches!(
            canonical_pointee.kind,
            CXType_FunctionProto | CXType_FunctionNoProto
        ) {
            let (convention, params, result) = function_signature(canonical_pointee, None)?;
            return Some(TypeRef::FunctionPointer {
                convention,
                params,
                result: Box::new(result),
            });
        }
        let declaration = unsafe { clang_getTypeDeclaration(pointee) };
        if let Some(definition) = native_opaque_class_definition(declaration) {
            let name = cx_string(unsafe { clang_getCursorSpelling(definition) });
            let (declaration, _, _, _) = cursor_locations(definition)?;
            return Some(TypeRef::Pointer {
                mutable: unsafe { clang_isConstQualifiedType(pointee) } == 0,
                target: Box::new(TypeRef::Named { name, declaration }),
            });
        }
        if unsafe { clang_Cursor_isNull(declaration) } == 0
            && unsafe { clang_getCursorKind(declaration) } == CXCursor_ClassDecl
            && !is_interface(declaration)
            && cursor_uuid(declaration).is_none()
            && unsafe { clang_isCursorDefinition(cursor_definition(declaration)) } != 0
            && !is_data_class(declaration)
        {
            return Some(TypeRef::OpaquePointer {
                mutable: unsafe { clang_isConstQualifiedType(pointee) } == 0,
                tag: cx_string(unsafe { clang_getCursorSpelling(declaration) }),
            });
        }
        let Some(target) = type_ref(pointee) else {
            let spelling = cx_string(unsafe { clang_getTypeSpelling(pointee) });
            let tag = spelling
                .strip_prefix("struct ")
                .filter(|tag| tag.ends_with("__"))?;
            return Some(TypeRef::OpaquePointer {
                mutable: unsafe { clang_isConstQualifiedType(pointee) } == 0,
                tag: tag.to_string(),
            });
        };
        return Some(TypeRef::Pointer {
            mutable: unsafe { clang_isConstQualifiedType(pointee) } == 0,
            target: Box::new(target),
        });
    }
    if ty.kind == CXType_LValueReference || ty.kind == CXType_RValueReference {
        let referent = unsafe { clang_getPointeeType(ty) };
        let target = type_ref(referent)?;
        return Some(TypeRef::Reference {
            mutable: unsafe { clang_isConstQualifiedType(referent) } == 0,
            target: Box::new(target),
        });
    }
    if matches!(ty.kind, CXType_ConstantArray | CXType_IncompleteArray) {
        let target = unsafe { clang_getArrayElementType(ty) };
        let len = if ty.kind == CXType_IncompleteArray {
            0
        } else {
            let len = unsafe { clang_getArraySize(ty) };
            if len < 0 {
                return None;
            }
            len.try_into().unwrap()
        };
        let target = type_ref(target)?;
        return Some(TypeRef::Array {
            target: Box::new(target),
            len,
        });
    }
    let declaration = unsafe { clang_getTypeDeclaration(ty) };
    let definition = unsafe { clang_getCursorDefinition(declaration) };
    let declaration = if unsafe { clang_Cursor_isNull(definition) } == 0 {
        definition
    } else {
        declaration
    };
    if unsafe { clang_Cursor_isNull(declaration) } == 0 {
        let kind = unsafe { clang_getCursorKind(declaration) };
        let spelling = cx_string(unsafe { clang_getTypeSpelling(ty) });
        if kind == CXCursor_EnumDecl && spelling.contains("(unnamed enum at ") {
            return scalar(ty).map(TypeRef::Scalar);
        }
        if matches!(kind, CXCursor_StructDecl | CXCursor_UnionDecl)
            && (unsafe { clang_Cursor_isAnonymous(declaration) } != 0
                || cx_string(unsafe { clang_getCursorSpelling(declaration) }).is_empty()
                || spelling.contains("(unnamed at "))
        {
            return inline_record(declaration, kind == CXCursor_UnionDecl, None)
                .ok()
                .map(|record| TypeRef::InlineRecord(Box::new(record)));
        }
        if matches!(
            kind,
            CXCursor_ClassDecl
                | CXCursor_EnumDecl
                | CXCursor_StructDecl
                | CXCursor_TypedefDecl
                | CXCursor_UnionDecl
        ) {
            let name = cx_string(unsafe { clang_getCursorSpelling(declaration) });
            if kind == CXCursor_EnumDecl && name.contains("(unnamed enum at ") {
                return scalar(ty).map(TypeRef::Scalar);
            }
            if !name.is_empty()
                && let Some((location, _, _, _)) = cursor_locations(declaration)
            {
                return Some(TypeRef::Named {
                    name,
                    declaration: location,
                });
            }
        }
        if matches!(kind, CXCursor_StructDecl | CXCursor_UnionDecl)
            && cx_string(unsafe { clang_getCursorSpelling(declaration) }).ends_with("__")
        {
            return inline_record(declaration, kind == CXCursor_UnionDecl, None)
                .ok()
                .map(|record| TypeRef::InlineRecord(Box::new(record)));
        }
    }
    let canonical = unsafe { clang_getCanonicalType(ty) };
    if canonical.kind != ty.kind {
        return type_ref(canonical);
    }
    scalar(ty).map(TypeRef::Scalar)
}

fn winrt_generic_arg(mut ty: CXType) -> Option<TypeRef> {
    ty = unsafe { clang_getCanonicalType(ty) };
    while ty.kind == CXType_Pointer {
        ty = unsafe { clang_getPointeeType(ty) };
    }
    let canonical = unsafe { clang_getCanonicalType(ty) };
    let declaration = unsafe { clang_getTypeDeclaration(canonical) };
    match cx_string(unsafe { clang_getCursorSpelling(declaration) }).as_str() {
        "HSTRING__" => Some(TypeRef::String),
        "IInspectable" => Some(TypeRef::Object),
        _ => type_ref(ty),
    }
}

fn scalar(ty: CXType) -> Option<Scalar> {
    let ty = unsafe { clang_getCanonicalType(ty) };
    Some(match ty.kind {
        CXType_Bool => Scalar::Bool,
        CXType_Float => Scalar::F32,
        CXType_Double | CXType_LongDouble => Scalar::F64,
        CXType_WChar | CXType_Char16 => Scalar::U16,
        CXType_Char32 => Scalar::U32,
        CXType_Char_S | CXType_SChar => Scalar::I8,
        CXType_Char_U | CXType_UChar => Scalar::U8,
        CXType_Short => Scalar::I16,
        CXType_UShort => Scalar::U16,
        CXType_Int | CXType_Long => Scalar::I32,
        CXType_UInt | CXType_ULong => Scalar::U32,
        CXType_LongLong => Scalar::I64,
        CXType_ULongLong => Scalar::U64,
        CXType_Enum => {
            let declaration = unsafe { clang_getTypeDeclaration(ty) };
            return scalar(unsafe { clang_getEnumDeclIntegerType(declaration) });
        }
        _ => return None,
    })
}

fn cx_string(value: CXString) -> String {
    unsafe {
        let pointer = clang_getCString(value);
        let result = if pointer.is_null() {
            String::new()
        } else {
            CStr::from_ptr(pointer).to_string_lossy().into_owned()
        };
        clang_disposeString(value);
        result
    }
}
