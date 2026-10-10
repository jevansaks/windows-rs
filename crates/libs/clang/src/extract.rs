use super::*;
use clang_sys::*;
use std::cell::{OnceCell, RefCell};
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::sync::Arc;

mod definitions;
use definitions::Definitions;

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
    let traversal_time = timing.then(std::time::Instant::now);
    let context = NativeContext {
        args,
        input_arguments: &input_arguments,
        timing,
        validate_annotations,
    };
    let shared_library = library.shared();
    #[cfg(test)]
    let lifetime = native_lifetime::current();
    // Keep phase failures inside the owned outcome so even serial scheduling visits later parses.
    let outcomes = try_map_ordered_bounded(
        &inputs,
        options.parallelism(),
        || NativeWorker {
            _library: Library::from_shared(shared_library.clone()),
            #[cfg(test)]
            _lifetime: native_lifetime::Scope::new(lifetime.clone()),
        },
        |_, input| Ok::<_, std::convert::Infallible>(extract_original(input, &context)),
    )
    .unwrap();
    let native = ordered_native_results(outcomes)?;
    if timing {
        eprintln!(
            "windows-clang timing phase=native-inputs-total target={} input_tus={} configured_workers={} elapsed_ms={:.3}",
            target.as_deref().unwrap(),
            inputs.len(),
            options.parallelism().max(1).min(inputs.len()),
            elapsed_ms(traversal_time)
        );
    }
    let mut output = ExtractionBuffers::default();
    let mut included_files = vec![];
    let mut extracted = vec![];
    let mut traversal_cursors = 0;
    let mut traversal_facts = 0;
    let mut traversal_constants = 0;
    for (input, mut result) in inputs.iter().zip(native) {
        if timing {
            eprintln!(
                "windows-clang timing phase=parse-tu target={} tu={:?} source_bytes={} elapsed_ms={:.3}",
                target.as_deref().unwrap(),
                input.name,
                input.source.len(),
                result.parse_ms,
            );
        }
        if let Some(metrics) = result.metrics {
            traversal_cursors += metrics.cursors;
            traversal_facts += metrics.facts;
            traversal_constants += metrics.constants;
            eprintln!(
                "windows-clang timing phase=extract-tu target={} tu={:?} macro_definitions={} macro_expansion_files={} cursors={} facts={} constants={} root_cache_misses={} exclusion_cache_misses={} definition_index_builds=1 definition_observations={} definition_lookups={} definition_index_ms={:.3} macro_index_ms={:.3} traversal_ms={:.3} elapsed_ms={:.3}",
                target.as_deref().unwrap(),
                input.name,
                metrics.macro_definitions,
                metrics.macro_expansion_files,
                metrics.cursors,
                metrics.facts,
                metrics.constants,
                metrics.root_cache_misses,
                metrics.exclusion_cache_misses,
                metrics.definition_observations,
                metrics.definition_lookups,
                metrics.definition_index_ms,
                metrics.macro_index_ms,
                metrics.traversal_ms,
                metrics.elapsed_ms
            );
        }
        let offset = output.facts.len();
        for (index, _) in result
            .extracted
            .pending_structs
            .iter_mut()
            .chain(&mut result.extracted.pending_macros)
        {
            *index += offset;
        }
        output.append(result.output);
        included_files.append(&mut result.included_files);
        extracted.push(result.extracted);
    }
    let ExtractionBuffers {
        mut facts,
        mut constants,
        mut value_declarations,
        mut annotations,
        sal_constant_sizes,
        declaration_guids,
        class_canonical_origins,
        pointer_callback_aliases,
        pointer_only_class_layouts,
        embeddable_class_layouts,
        clang_flag_enums,
    } = output;
    let source_annotations = annotations.clone();
    let function_origins: HashSet<_> = facts
        .iter()
        .filter(|fact| fact.kind == FactKind::Function)
        .map(|fact| &fact.origin)
        .collect();
    let function_annotations = source_annotations
        .iter()
        .filter(|(target, _)| function_origins.contains(annotation_target_origin(target)))
        .map(|(target, annotations)| (target.clone(), annotations.clone()))
        .collect();
    merge_redeclaration_annotations(&facts, &mut annotations)?;
    let annotation_macros = annotation_macro_names(&annotations);
    let associated_constants = associated_constant_names(&facts, &annotations);
    decode_selected_macro_definitions(&mut facts, &mut extracted, &annotation_macros);
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
    for (input, extracted) in inputs.iter().zip(&extracted) {
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
                diagnostics: &extracted.diagnostics,
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
    decode_reachable_structs(&mut facts, &constants, &mut extracted);
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
    let next_local = extracted
        .iter()
        .zip(&inputs)
        .map(|(extracted, input)| (input.name.clone(), extracted.next_local))
        .collect();
    materialize_anonymous_callbacks(&mut facts, next_local);
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
    constants.sort();
    value_declarations.sort();
    let mut origins: HashSet<_> = facts.iter().map(|fact| &fact.origin).collect();
    for declaration in &value_declarations {
        if !origins.insert(&declaration.origin) {
            return Err(Error(format!(
                "duplicate native value origin `{}`",
                origin(&declaration.origin)
            )));
        }
    }
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
        value_declarations,
        included_files,
        declare_handles,
        annotations,
        source_annotations,
        function_annotations,
        sal_constant_sizes,
        declaration_guids,
        class_canonical_origins,
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
        if *scoped || fact.parent.is_some() {
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
        let Some([(fact_index, variant_index, repr, enum_offset, enum_value)]) = enum_members
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
            macro_before_enum_matches(constant, *repr, *enum_value, &scalar_aliases).then_some(None)
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
    source_width == enum_width && source_value == enum_value
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
    extracted: &[Extracted],
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

struct TranslationUnit(
    CXTranslationUnit,
    #[cfg(test)] Option<Arc<native_lifetime::Counters>>,
);

struct NativeWorker {
    _library: Library,
    #[cfg(test)]
    _lifetime: native_lifetime::Scope,
}

struct NativeContext<'a> {
    args: &'a [&'a str],
    input_arguments: &'a BTreeMap<String, Vec<String>>,
    timing: bool,
    validate_annotations: bool,
}

struct NativeExtraction {
    output: ExtractionBuffers,
    extracted: Extracted,
    included_files: Vec<IncludedFile>,
    parse_ms: f64,
    metrics: Option<ExtractionMetrics>,
}

enum NativeFailure {
    Parse(Error),
    Traversal(Error),
}

fn extract_original(
    input: &Input,
    context: &NativeContext<'_>,
) -> Result<NativeExtraction, NativeFailure> {
    let index = Index::new().map_err(NativeFailure::Parse)?;
    let local_arguments = context.input_arguments.get(&input.name);
    let arguments: Vec<_> = context
        .args
        .iter()
        .copied()
        .chain(local_arguments.into_iter().flatten().map(String::as_str))
        .collect();
    let start = context.timing.then(std::time::Instant::now);
    let translation_unit =
        TranslationUnit::parse(&index, input, &arguments).map_err(NativeFailure::Parse)?;
    let parse_ms = elapsed_ms(start);
    let included_files = translation_unit.included_files(&input.name);
    let mut output = ExtractionBuffers::default();
    let (extracted, metrics) = translation_unit
        .extract(
            &index,
            input,
            &arguments,
            &mut output.state(),
            context.timing,
            context.validate_annotations,
        )
        .map_err(NativeFailure::Traversal)?;
    Ok(NativeExtraction {
        output,
        extracted,
        included_files,
        parse_ms,
        metrics,
    })
}

fn ordered_native_results(
    outcomes: Vec<Result<NativeExtraction, NativeFailure>>,
) -> Result<Vec<NativeExtraction>, Error> {
    let mut output = Vec::with_capacity(outcomes.len());
    let mut parse_error = None;
    let mut traversal_error = None;
    for outcome in outcomes {
        match outcome {
            Ok(result) => output.push(result),
            Err(NativeFailure::Parse(error)) => {
                if parse_error.is_none() {
                    parse_error = Some(error);
                }
            }
            Err(NativeFailure::Traversal(error)) => {
                if traversal_error.is_none() {
                    traversal_error = Some(error);
                }
            }
        }
    }
    if let Some(error) = parse_error.or(traversal_error) {
        Err(error)
    } else {
        Ok(output)
    }
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
    definition_observations: usize,
    definition_lookups: usize,
    definition_index_ms: f64,
    macro_definitions: usize,
    macro_expansion_files: usize,
    cursors: u32,
    facts: usize,
    constants: usize,
    root_cache_misses: usize,
    exclusion_cache_misses: usize,
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

struct ConstantSources<'a> {
    macros: &'a FinalMacros,
    diagnostics: &'a [ErrorDiagnostic],
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

        let result = Self(
            value,
            #[cfg(test)]
            native_lifetime::original(),
        );
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
        Self::parse_probe_files(index, &name, args, std::slice::from_mut(&mut unsaved))
    }

    fn parse_probe_files(
        index: &Index,
        name: &CStr,
        args: &[&str],
        unsaved: &mut [CXUnsavedFile],
    ) -> Result<Self, Error> {
        let mut args: Vec<_> = args.iter().map(|arg| CString::new(*arg).unwrap()).collect();
        args.push(CString::new("-ferror-limit=0").unwrap());
        let pointers: Vec<_> = args.iter().map(|arg| arg.as_ptr()).collect();
        let value = unsafe {
            clang_parseTranslationUnit(
                index.0,
                name.as_ptr(),
                pointers.as_ptr(),
                pointers.len().try_into().unwrap(),
                unsaved.as_mut_ptr(),
                unsaved.len().try_into().unwrap(),
                CXTranslationUnit_KeepGoing | CXTranslationUnit_SkipFunctionBodies,
            )
        };
        if value.is_null() {
            Err(Error(format!(
                "failed to evaluate native constants in `{}`",
                name.to_string_lossy()
            )))
        } else {
            #[cfg(test)]
            native_lifetime::probe();
            Ok(Self(
                value,
                #[cfg(test)]
                None,
            ))
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
        index: &'tu Index,
        input: &'tu Input,
        args: &[&str],
        output: &mut ExtractionState<'_>,
        timing: bool,
        validate_annotations: bool,
    ) -> Result<(Extracted, Option<ExtractionMetrics>), Error> {
        let total_time = timing.then(std::time::Instant::now);
        let phase_time = timing.then(std::time::Instant::now);
        let mut definitions = Definitions::new(self.0, index)
            .map_err(|error| Error(format!("{}: {error}", input.name)))?;
        let definition_index_ms = elapsed_ms(phase_time);
        let phase_time = timing.then(std::time::Instant::now);
        let mut macros = macro_definitions(self, unsafe { clang_getTranslationUnitCursor(self.0) });
        macros.count_probes = Some(CountProbes {
            index,
            input,
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            original: self,
        });
        let macro_index_ms = elapsed_ms(phase_time);
        let phase_time = timing.then(std::time::Instant::now);
        let initial_facts = output.facts.len();
        let initial_constants = output.constants.len();
        let mut traversal = Traversal {
            definitions: &mut definitions,
            tu: &input.name,
            paths: SourcePaths::new(input),
            next: 0,
            seen: HashMap::new(),
            macros: &macros,
            pending_structs: vec![],
            pending_macros: vec![],
            pending_callables: vec![],
            pending_classes: vec![],
            declare_handle_expansions: vec![],
            facts: &mut *output.facts,
            constants: &mut *output.constants,
            value_declarations: &mut *output.value_declarations,
            annotations: &mut *output.annotations,
            declaration_guids: &mut *output.declaration_guids,
            class_canonical_origins: &mut *output.class_canonical_origins,
            pointer_callback_aliases: &mut *output.pointer_callback_aliases,
            pointer_only_class_layouts: &mut *output.pointer_only_class_layouts,
            embeddable_class_layouts: &mut *output.embeddable_class_layouts,
            clang_flag_enums: &mut *output.clang_flag_enums,
            error: None,
            validate_annotations,
        };
        #[cfg(test)]
        let native_traversal = native_lifetime::traversal();
        extract_children(
            unsafe { clang_getTranslationUnitCursor(self.0) },
            None,
            &mut traversal,
        );
        #[cfg(test)]
        drop(native_traversal);
        let traversal_ms = elapsed_ms(phase_time);
        if let Some(error) = traversal.error.take() {
            return Err(error);
        }
        bind_class_origins(
            self.0,
            &traversal.pending_classes,
            &traversal.seen,
            traversal.class_canonical_origins,
        )?;
        let metrics = timing.then(|| ExtractionMetrics {
            definition_observations: traversal.definitions.observations,
            definition_lookups: traversal.definitions.lookups,
            definition_index_ms,
            macro_definitions: macros.definitions.len(),
            macro_expansion_files: macros.expansion_orders.len(),
            cursors: traversal.next,
            facts: traversal.facts.len() - initial_facts,
            constants: traversal.constants.len() - initial_constants,
            root_cache_misses: traversal.paths.roots.len(),
            exclusion_cache_misses: traversal.paths.exclusions.len(),
            macro_index_ms,
            traversal_ms,
            elapsed_ms: elapsed_ms(total_time),
        });
        let pending_structs = std::mem::take(&mut traversal.pending_structs);
        let pending_macros = std::mem::take(&mut traversal.pending_macros);
        let declare_handle_expansions = std::mem::take(&mut traversal.declare_handle_expansions);
        let next_local = traversal.next;
        let pending_callables = std::mem::take(&mut traversal.pending_callables);
        drop(traversal);
        if let Err(error) = macros.resolve_constant_counts(timing) {
            macros
                .fail_constant_counts(&format!("native SAL count probe/context failure: {error}"));
        }
        for (index, cursor) in pending_callables {
            macros.collect_constant_sizes(&output.facts[index], cursor, output.sal_constant_sizes);
        }
        let pending_structs = pending_structs
            .into_iter()
            .map(|(index, cursor)| (index, fact_data(cursor, FactKind::Struct, &macros, false)))
            .collect();
        let pending_macros = pending_macros
            .into_iter()
            .map(|(index, cursor)| (index, fact_data(cursor, FactKind::Macro, &macros, false)))
            .collect();
        let final_macros = FinalMacros(
            macros
                .definitions
                .keys()
                .filter_map(|name| {
                    macros
                        .final_definition(name)
                        .map(|(tokens, function_like)| {
                            (name.clone(), (tokens.to_vec(), function_like))
                        })
                })
                .collect(),
        );
        Ok((
            Extracted {
                macros: final_macros,
                diagnostics: self.error_diagnostics(),
                pending_structs,
                pending_macros,
                declare_handle_expansions,
                next_local,
            },
            metrics,
        ))
    }
}

impl Drop for TranslationUnit {
    fn drop(&mut self) {
        unsafe { clang_disposeTranslationUnit(self.0) };
        #[cfg(test)]
        if let Some(counters) = &self.1 {
            counters
                .live
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

struct Traversal<'a> {
    definitions: &'a mut Definitions,
    tu: &'a str,
    paths: SourcePaths<'a>,
    next: u32,
    seen: HashMap<u32, Vec<(CXCursor, Origin)>>,
    macros: &'a MacroDefinitions<'a>,
    pending_structs: Vec<(usize, CXCursor)>,
    pending_macros: Vec<(usize, CXCursor)>,
    pending_callables: Vec<(usize, CXCursor)>,
    pending_classes: Vec<(Origin, CXCursor)>,
    declare_handle_expansions: Vec<DeclareHandleExpansion>,
    facts: &'a mut Vec<Fact>,
    constants: &'a mut Vec<Constant>,
    value_declarations: &'a mut Vec<ValueDeclaration>,
    annotations: &'a mut BTreeMap<AnnotationTarget, Vec<Annotation>>,
    declaration_guids: &'a mut BTreeMap<Origin, String>,
    class_canonical_origins: &'a mut BTreeMap<Origin, CanonicalRecord>,
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
    value_declarations: &'a mut Vec<ValueDeclaration>,
    annotations: &'a mut BTreeMap<AnnotationTarget, Vec<Annotation>>,
    sal_constant_sizes: &'a mut SalConstantSizes,
    declaration_guids: &'a mut BTreeMap<Origin, String>,
    class_canonical_origins: &'a mut BTreeMap<Origin, CanonicalRecord>,
    pointer_callback_aliases: &'a mut BTreeSet<Origin>,
    pointer_only_class_layouts: &'a mut BTreeMap<Origin, FactData>,
    embeddable_class_layouts: &'a mut BTreeSet<Origin>,
    clang_flag_enums: &'a mut BTreeSet<Origin>,
}

#[derive(Default)]
struct ExtractionBuffers {
    facts: Vec<Fact>,
    constants: Vec<Constant>,
    value_declarations: Vec<ValueDeclaration>,
    annotations: BTreeMap<AnnotationTarget, Vec<Annotation>>,
    sal_constant_sizes: SalConstantSizes,
    declaration_guids: BTreeMap<Origin, String>,
    class_canonical_origins: BTreeMap<Origin, CanonicalRecord>,
    pointer_callback_aliases: BTreeSet<Origin>,
    pointer_only_class_layouts: BTreeMap<Origin, FactData>,
    embeddable_class_layouts: BTreeSet<Origin>,
    clang_flag_enums: BTreeSet<Origin>,
}

impl ExtractionBuffers {
    fn state(&mut self) -> ExtractionState<'_> {
        ExtractionState {
            facts: &mut self.facts,
            constants: &mut self.constants,
            value_declarations: &mut self.value_declarations,
            annotations: &mut self.annotations,
            sal_constant_sizes: &mut self.sal_constant_sizes,
            declaration_guids: &mut self.declaration_guids,
            class_canonical_origins: &mut self.class_canonical_origins,
            pointer_callback_aliases: &mut self.pointer_callback_aliases,
            pointer_only_class_layouts: &mut self.pointer_only_class_layouts,
            embeddable_class_layouts: &mut self.embeddable_class_layouts,
            clang_flag_enums: &mut self.clang_flag_enums,
        }
    }

    fn append(&mut self, mut other: Self) {
        self.facts.append(&mut other.facts);
        self.constants.append(&mut other.constants);
        self.value_declarations
            .append(&mut other.value_declarations);
        self.annotations.append(&mut other.annotations);
        self.sal_constant_sizes
            .append(&mut other.sal_constant_sizes);
        self.declaration_guids.append(&mut other.declaration_guids);
        self.class_canonical_origins
            .append(&mut other.class_canonical_origins);
        self.pointer_callback_aliases
            .append(&mut other.pointer_callback_aliases);
        self.pointer_only_class_layouts
            .append(&mut other.pointer_only_class_layouts);
        self.embeddable_class_layouts
            .append(&mut other.embeddable_class_layouts);
        self.clang_flag_enums.append(&mut other.clang_flag_enums);
    }
}

struct Extracted {
    macros: FinalMacros,
    diagnostics: Vec<ErrorDiagnostic>,
    pending_structs: Vec<(usize, FactData)>,
    pending_macros: Vec<(usize, FactData)>,
    declare_handle_expansions: Vec<DeclareHandleExpansion>,
    next_local: u32,
}

struct FinalMacros(HashMap<String, (Vec<String>, bool)>);

impl FinalMacros {
    fn contains_key(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    fn final_definition(&self, name: &str) -> Option<(&[String], bool)> {
        self.0
            .get(name)
            .map(|(tokens, function_like)| (tokens.as_slice(), *function_like))
    }
}

#[cfg(test)]
mod owned_tests {
    use super::*;

    #[test]
    fn whole_native_workers_bound_lifetimes_and_preserve_count_errors() {
        use std::sync::atomic::Ordering;
        helpers::ensure_libclang();
        let inputs: Vec<_> = (0..12)
            .map(|index| {
                Input::new(
                    format!("bounded{index}.hpp"),
                    format!(
                        "#define LIMIT 8\n\
                         extern \"C\" void Good{index}(
                             __attribute__((annotate(\"_In_reads_(LIMIT)\"))) char *value);\n\
                         extern \"C\" void Invalid{index}(
                             __attribute__((annotate(\"_In_reads_(LIMIT - 20)\"))) char *value);\n\
                         struct Record{index} {{ int (*invoke)(int); }};\n\
                         struct _GUID; struct _GUID {{ unsigned int Data1; }};\n\
                         typedef class Coclass{index} Coclass{index};\n\
                         class __declspec(uuid(\"11111111-2222-3333-4455-66778899aabb\"))
                             Coclass{index};\n\
                         class __declspec(uuid(\"11111111-2222-3333-4455-66778899aabb\"))
                             Coclass{index};\n"
                    ),
                )
            })
            .collect();
        let args = [
            "-x",
            "c++",
            "-fms-extensions",
            "--target=x86_64-pc-windows-msvc",
        ];
        let serial = extract(inputs.clone(), &args).unwrap();
        assert!(serial.sal_constant_sizes.values().any(Result::is_err));
        assert_eq!(serial.class_canonical_origins.len(), inputs.len() * 6);
        for fact in serial
            .facts
            .iter()
            .filter(|fact| fact.kind == FactKind::Struct)
        {
            if fact.name == "_GUID" {
                let first = serial
                    .facts
                    .iter()
                    .find(|candidate| {
                        candidate.origin.tu == fact.origin.tu && candidate.name == "_GUID"
                    })
                    .unwrap();
                assert_eq!(
                    serial.class_canonical_origins.get(&fact.origin),
                    Some(&CanonicalRecord::CompilerGenerated {
                        anchor: first.origin.clone(),
                        kind: CanonicalRecordKind::Struct,
                    })
                );
            } else {
                assert_eq!(
                    serial.class_canonical_origins.get(&fact.origin),
                    Some(&CanonicalRecord::Source(fact.origin.clone()))
                );
            }
        }
        let aliases = serial.coclass_aliases().unwrap();
        assert_eq!(aliases.len(), inputs.len());
        for (origin, alias) in &aliases {
            let classes: Vec<_> = serial
                .facts
                .iter()
                .filter(|fact| fact.origin.tu == origin.tu && fact.kind == FactKind::Class)
                .collect();
            assert_eq!(classes.len(), 3);
            assert_eq!(
                alias.canonical,
                CanonicalRecord::Source(classes[0].origin.clone())
            );
            assert_eq!(
                alias.guids,
                [
                    (
                        classes[1].origin.clone(),
                        "11111111-2222-3333-4455-66778899aabb".to_string()
                    ),
                    (
                        classes[2].origin.clone(),
                        "11111111-2222-3333-4455-66778899aabb".to_string()
                    ),
                ]
            );
        }
        assert_eq!(
            serial
                .facts
                .iter()
                .filter(|fact| matches!(fact.data, FactData::Callback { .. }))
                .count(),
            inputs.len(),
        );
        assert_eq!(serial.sal_constant_sizes.len(), inputs.len() * 2);
        for (target, count) in &serial.sal_constant_sizes {
            let AnnotationTarget::Parameter { declaration, .. } = target else {
                panic!();
            };
            let fact = serial
                .facts
                .iter()
                .find(|fact| fact.origin == *declaration)
                .unwrap();
            if fact.name.starts_with("Good") {
                assert_eq!(
                    count,
                    &Ok(SalSize {
                        bytes: false,
                        value: SalSizeValue::Constant(8),
                    })
                );
            } else {
                assert!(fact.name.starts_with("Invalid"));
                assert!(matches!(count, Err(SalCountError::Invalid(_))));
            }
        }
        for workers in [1, 2, 4] {
            let counters = Arc::new(native_lifetime::Counters::new(workers));
            let _scope = native_lifetime::Scope::new(Some(counters.clone()));
            let parallel = extract_with_options(
                inputs.clone(),
                &args,
                &ExtractionOptions::new().with_parallelism(workers),
            )
            .unwrap();
            assert_complete_eq(&serial, &parallel);
            assert_eq!(parallel.coclass_aliases().unwrap(), aliases);
            assert_eq!(counters.originals.load(Ordering::Relaxed), inputs.len());
            assert_eq!(counters.definitions.load(Ordering::Relaxed), inputs.len());
            assert_eq!(counters.live.load(Ordering::Relaxed), 0);
            assert_eq!(counters.maximum.load(Ordering::Relaxed), workers);
            assert_eq!(counters.max_traversals.load(Ordering::Relaxed), workers);
            assert!(counters.probes.load(Ordering::Relaxed) > 0);
        }
    }

    #[test]
    fn owned_evidence_survives_native_context_disposal_and_thread_transfer() {
        helpers::ensure_libclang();
        let mut facts = vec![];
        let mut constants = vec![];
        let mut values = vec![];
        let mut annotations = BTreeMap::new();
        let mut sizes = BTreeMap::new();
        let mut guids = BTreeMap::new();
        let mut class_origins = BTreeMap::new();
        let mut aliases = BTreeSet::new();
        let mut layouts = BTreeMap::new();
        let mut embeddable = BTreeSet::new();
        let mut flags = BTreeSet::new();
        let mut input = Input::new(
            "disposed.hpp",
            "#define TEXT \"payload\"\n#define ALIAS TEXT\nstruct Deferred { int (*call)(int); };\n",
        );
        input.roots.clear();
        let extracted = {
            let _library = Library::new().unwrap();
            let index = Index::new().unwrap();
            let tu = TranslationUnit::parse(
                &index,
                &input,
                &["-x", "c++", "--target=x86_64-pc-windows-msvc"],
            )
            .unwrap();
            let mut output = ExtractionState {
                facts: &mut facts,
                constants: &mut constants,
                value_declarations: &mut values,
                annotations: &mut annotations,
                sal_constant_sizes: &mut sizes,
                declaration_guids: &mut guids,
                class_canonical_origins: &mut class_origins,
                pointer_callback_aliases: &mut aliases,
                pointer_only_class_layouts: &mut layouts,
                embeddable_class_layouts: &mut embeddable,
                clang_flag_enums: &mut flags,
            };
            tu.extract(&index, &input, &[], &mut output, false, false)
                .unwrap()
                .0
        };
        let deferred = facts
            .iter()
            .position(|fact| fact.name == "Deferred")
            .unwrap();
        assert_eq!(facts[deferred].data, FactData::None);
        std::thread::spawn(move || {
            assert_eq!(
                string_macro_value("ALIAS", &extracted.macros, &mut HashSet::new()),
                Some(Value::Utf8("payload".to_string()))
            );
            assert!(extracted.pending_structs.iter().any(|(index, data)| {
                *index == deferred
                    && matches!(data, FactData::Record { fields, .. } if fields.len() == 1)
            }));
            assert!(extracted.pending_macros.iter().any(|(index, data)| {
                facts[*index].name == "ALIAS" && matches!(data, FactData::Macro { .. })
            }));
        })
        .join()
        .unwrap();
    }

    fn assert_complete_eq(left: &Snapshot, right: &Snapshot) {
        assert_eq!(left, right);
        assert_eq!(left.included_files, right.included_files);
        assert_eq!(left.input_order, right.input_order);
        assert_eq!(left.class_canonical_origins, right.class_canonical_origins);
    }

    #[test]
    fn owned_global_evidence_preserves_cross_input_finalization() {
        helpers::ensure_libclang();
        fn assert_send<T: Send>() {}
        assert_send::<Extracted>();
        assert_send::<NativeExtraction>();
        let group = Input::new(
            "group.hpp",
            r#"
                    struct Shared;
                    typedef Shared SharedAlias;
                    struct Root { SharedAlias *shared; };
                    struct __attribute__((annotate("win32metadata:supported_os=Windows10")))
                        Annotated { int value; };
                    enum __attribute__((annotate("win32metadata:associated_constant=EXTRA")))
                        Flags : unsigned long { Local = 1 };
                    struct CallbackOwner { int (*invoke)(int); };
                    constexpr int NativeValue = 13;
                "#,
        );
        let mut provider = Input::new(
            "provider.hpp",
            r#"
                    #define TARGET 42UL
                    #define EXTRA TARGET
                    #define NOISE 99
                    struct Leaf { unsigned value; };
                    struct Shared { Leaf leaf; };
                    struct Annotated { int value; };
                    struct Unreachable { double value; };
                "#,
        );
        provider.roots.clear();
        let collision = Input::new("collision.hpp", "typedef int CallbackOwner_invoke;");
        let inputs = vec![group, provider, collision];
        let args = ["-x", "c++", "-std=c++20", "--target=x86_64-pc-windows-msvc"];
        let serial = extract(inputs.clone(), &args).unwrap();
        for workers in [1, 2, 4] {
            let parallel = extract_with_options(
                inputs.clone(),
                &args,
                &ExtractionOptions::new().with_parallelism(workers),
            )
            .unwrap();
            assert_complete_eq(&serial, &parallel);
        }
        for name in ["Shared", "Leaf"] {
            let fact = serial
                .facts
                .iter()
                .find(|fact| fact.origin.tu == "provider.hpp" && fact.name == name)
                .unwrap();
            assert!(matches!(fact.data, FactData::Record { .. }), "{fact:?}");
        }
        let unreachable = serial
            .facts
            .iter()
            .find(|fact| fact.name == "Unreachable")
            .unwrap();
        assert_eq!(unreachable.data, FactData::None);
        let extra = serial
            .constants
            .iter()
            .find(|constant| constant.name == "EXTRA")
            .unwrap();
        assert_eq!(extra.value, Value::Unsigned(42));
        assert!(
            serial
                .constants
                .iter()
                .all(|constant| constant.name != "NOISE")
        );
        assert!(
            serial
                .facts
                .iter()
                .any(|fact| fact.name == "CallbackOwner_invoke_2")
        );
        for fact in serial.facts.iter().filter(|fact| fact.name == "Annotated") {
            assert!(
                serial.annotations[&AnnotationTarget::Declaration(fact.origin.clone())]
                    .contains(&Annotation::SupportedOs("Windows10".to_string()))
            );
        }
    }

    #[test]
    fn owned_partition_arguments_and_errors_preserve_original_context() {
        helpers::ensure_libclang();
        let mut first = Input::new("first.hpp", "#define RESULT LOCAL_VALUE\n")
            .partitioned("first")
            .with_root("first.hpp", "first", "Test.First");
        first.arguments.push("-DLOCAL_VALUE=11".to_string());
        let mut second = Input::new("second.hpp", "#define RESULT LOCAL_VALUE\n")
            .partitioned("second")
            .with_root("second.hpp", "second", "Test.Second");
        second.arguments.push("-DLOCAL_VALUE=22".to_string());
        let inputs = vec![first, second];
        let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
        let serial = extract_partitioned(inputs.clone(), &args).unwrap();
        for workers in [1, 2, 4] {
            let parallel = extract_partitioned_with_options(
                inputs.clone(),
                &args,
                &ExtractionOptions::new().with_parallelism(workers),
            )
            .unwrap();
            assert_complete_eq(&serial, &parallel);
        }
        let values: Vec<_> = serial
            .constants
            .iter()
            .filter(|constant| constant.name == "RESULT")
            .map(|constant| (&constant.root.tu, &constant.value))
            .collect();
        assert_eq!(
            values,
            vec![
                (&"first.hpp".to_string(), &Value::Signed(11)),
                (&"second.hpp".to_string(), &Value::Signed(22))
            ]
        );
        for workers in [1, 2, 4] {
            let options = ExtractionOptions::new().with_parallelism(workers);
            let error = extract_with_options(
                [
                    Input::new(
                        "annotation.hpp",
                        "struct __attribute__((annotate(\"win32metadata:unknown\"))) A {};",
                    ),
                    Input::new("parse.hpp", "#error PARSE_BEFORE_TRAVERSAL\n"),
                ],
                &["-x", "c++", "-DWIN32METADATA=1"],
                &options,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("PARSE_BEFORE_TRAVERSAL"), "{error}");
            for input in [
                Input::new("source.hpp", "int value;\0"),
                Input::new("name\0.hpp", "int value;"),
            ] {
                assert!(extract_with_options([input], &args, &options).is_err());
            }
        }
    }
}

#[cfg(test)]
mod native_lifetime {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    thread_local! {
        static CURRENT: RefCell<Option<Arc<Counters>>> = const { RefCell::new(None) };
    }

    pub(super) struct Counters {
        pub(super) originals: AtomicUsize,
        pub(super) live: AtomicUsize,
        pub(super) maximum: AtomicUsize,
        pub(super) definitions: AtomicUsize,
        pub(super) probes: AtomicUsize,
        traversals: AtomicUsize,
        pub(super) max_traversals: AtomicUsize,
        barrier: Barrier,
    }

    impl Counters {
        pub(super) fn new(workers: usize) -> Self {
            Self {
                originals: AtomicUsize::new(0),
                live: AtomicUsize::new(0),
                maximum: AtomicUsize::new(0),
                definitions: AtomicUsize::new(0),
                probes: AtomicUsize::new(0),
                traversals: AtomicUsize::new(0),
                max_traversals: AtomicUsize::new(0),
                barrier: Barrier::new(workers),
            }
        }
    }

    pub(super) struct Scope(Option<Arc<Counters>>);

    impl Scope {
        pub(super) fn new(counters: Option<Arc<Counters>>) -> Self {
            Self(CURRENT.replace(counters))
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            CURRENT.replace(self.0.take());
        }
    }

    pub(super) fn current() -> Option<Arc<Counters>> {
        CURRENT.with_borrow(Clone::clone)
    }

    pub(super) fn original() -> Option<Arc<Counters>> {
        let counters = current()?;
        counters.originals.fetch_add(1, Ordering::Relaxed);
        let live = counters.live.fetch_add(1, Ordering::Relaxed) + 1;
        counters.maximum.fetch_max(live, Ordering::Relaxed);
        Some(counters)
    }

    pub(super) fn definition() {
        if let Some(counters) = current() {
            counters.definitions.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn probe() {
        if let Some(counters) = current() {
            counters.probes.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) struct TraversalScope(Arc<Counters>);

    impl Drop for TraversalScope {
        fn drop(&mut self) {
            self.0.traversals.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub(super) fn traversal() -> Option<TraversalScope> {
        let counters = current()?;
        let live = counters.traversals.fetch_add(1, Ordering::Relaxed) + 1;
        counters.max_traversals.fetch_max(live, Ordering::Relaxed);
        counters.barrier.wait();
        Some(TraversalScope(counters))
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DeclareHandleExpansion {
    name: String,
    location: Location,
}

impl Traversal<'_> {
    fn is_root(&mut self, file: &str) -> bool {
        self.paths.is_root(file)
    }

    fn is_source_excluded(&mut self, file: &str) -> bool {
        self.paths.is_source_excluded(file)
    }
}

struct SourcePaths<'a> {
    input: &'a Input,
    roots: HashMap<String, bool>,
    exclusions: HashMap<String, bool>,
    #[cfg(test)]
    root_match_calls: usize,
    #[cfg(test)]
    exclusion_match_calls: usize,
}

impl<'a> SourcePaths<'a> {
    fn new(input: &'a Input) -> Self {
        Self {
            input,
            roots: HashMap::new(),
            exclusions: HashMap::new(),
            #[cfg(test)]
            root_match_calls: 0,
            #[cfg(test)]
            exclusion_match_calls: 0,
        }
    }

    fn is_root(&mut self, file: &str) -> bool {
        if let Some(root) = self.roots.get(file) {
            return *root;
        }
        #[cfg(test)]
        {
            self.root_match_calls += 1;
        }
        let root = is_root_path(
            &self.input.roots,
            &self.input.root_dirs,
            &self.input.root_suffixes,
            &self.input.excluded_roots,
            file,
        );
        self.roots.insert(file.to_string(), root);
        root
    }

    fn is_source_excluded(&mut self, file: &str) -> bool {
        if let Some(excluded) = self.exclusions.get(file) {
            return *excluded;
        }
        #[cfg(test)]
        {
            self.exclusion_match_calls += 1;
        }
        let excluded = self.input.excluded_roots.iter().any(|root| {
            root.ends_with('/') && source_path_is_under(file, root.trim_end_matches('/'))
        });
        self.exclusions.insert(file.to_string(), excluded);
        excluded
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn cached_paths_preserve_matching_boundaries_and_exclusions() {
        let input = Input::new("input.cpp", "")
            .with_roots([r"C:\SDK\Include\Direct.h", "relative.h"])
            .with_root_dirs([r"C:\SDK\Directory"])
            .with_root_suffixes(["nested/suffix.h"])
            .with_excluded_roots(["C:/SDK/Directory/file.h"])
            .with_excluded_source_dirs([r"C:\SDK\Directory\Private"]);
        let mut paths = SourcePaths::new(&input);
        for (file, root, excluded) in [
            (r"c:\sdk\include\DIRECT.H", true, false),
            ("Direct.h", true, false),
            ("C:/SDK/Include/NotDirect.h", false, false),
            ("C:/other/relative.h", true, false),
            ("C:/other/notrelative.h", false, false),
            ("C:/SDK/Directory", true, false),
            (r"c:\sdk\directory\child.h", true, false),
            ("C:/SDK/DirectoryExtra/child.h", false, false),
            ("C:/other/NESTED/suffix.h", true, false),
            ("C:/other/notnested/suffix.h", false, false),
            ("C:/SDK/Directory/FILE.H", false, false),
            ("file.h", false, false),
            ("C:/SDK/Directory/Private/child.h", false, true),
            (r"c:\sdk\directory\PRIVATE", false, true),
            ("C:/SDK/Directory/PrivateExtra/child.h", true, false),
        ] {
            for _ in 0..2 {
                assert_eq!(paths.is_root(file), root, "{file}");
                assert_eq!(paths.is_source_excluded(file), excluded, "{file}");
            }
        }
        assert_eq!(paths.root_match_calls, 15);
        assert_eq!(paths.exclusion_match_calls, 15);
    }

    #[test]
    fn cached_paths_match_once_per_distinct_path_with_many_roots() {
        let input = Input::new("input.cpp", "")
            .with_roots((0..548).map(|index| format!("C:/SDK/root-{index:03}.h")))
            .with_excluded_source_dirs(["C:/SDK/private"]);
        let mut paths = SourcePaths::new(&input);
        for _ in 0..10_000 {
            assert!(paths.is_root("C:/SDK/root-547.h"));
            assert!(!paths.is_root("C:/SDK/missing.h"));
            assert!(!paths.is_root("C:/SDK/private/child.h"));
            assert!(!paths.is_source_excluded("C:/SDK/root-547.h"));
            assert!(!paths.is_source_excluded("C:/SDK/missing.h"));
            assert!(paths.is_source_excluded("C:/SDK/private/child.h"));
        }
        assert_eq!(paths.root_match_calls, 3);
        assert_eq!(paths.exclusion_match_calls, 3);
        assert_eq!(paths.roots.len(), 3);
        assert_eq!(paths.exclusions.len(), 3);
    }

    #[test]
    fn cached_paths_keep_input_policies_independent() {
        let selected = Input::new("selected.cpp", "").with_roots(["shared.h"]);
        let unselected = Input::new("unselected.cpp", "");
        let excluded = Input::new("excluded.cpp", "")
            .with_roots(["shared.h"])
            .with_excluded_source_dirs(["C:/SDK"]);
        let mut selected_paths = SourcePaths::new(&selected);
        let mut unselected_paths = SourcePaths::new(&unselected);
        let mut excluded_paths = SourcePaths::new(&excluded);
        for _ in 0..2 {
            assert!(selected_paths.is_root("C:/SDK/shared.h"));
            assert!(!unselected_paths.is_root("C:/SDK/shared.h"));
            assert!(!excluded_paths.is_root("C:/SDK/shared.h"));
            assert!(!selected_paths.is_source_excluded("C:/SDK/shared.h"));
            assert!(!unselected_paths.is_source_excluded("C:/SDK/shared.h"));
            assert!(excluded_paths.is_source_excluded("C:/SDK/shared.h"));
        }
        for paths in [selected_paths, unselected_paths, excluded_paths] {
            assert_eq!(paths.root_match_calls, 1);
            assert_eq!(paths.exclusion_match_calls, 1);
        }
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
    if kind == CXCursor_CompoundStmt {
        return;
    }
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
        && let Some((spelling, expansion, main_file, system)) = cursor_locations(child)
        && traversal.is_root(&spelling.file)
    {
        let ty = unsafe { clang_getCursorType(child) };
        if unsafe { clang_isConstQualifiedType(ty) } != 0
            && let Some(scalar) = scalar(ty)
            && let Some(value) = match scalar {
                Scalar::F32 | Scalar::F64 => evaluate_float(child, scalar),
                Scalar::Bool | Scalar::U8 | Scalar::U16 | Scalar::U32 | Scalar::U64 => {
                    evaluate_integer(child).map(|value| Value::Unsigned(value.0))
                }
                Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::I64 => {
                    evaluate_integer(child).map(|value| Value::Signed(value.1))
                }
            }
        {
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
            traversal.value_declarations.push(ValueDeclaration {
                origin: origin.clone(),
                parent: parent.cloned(),
                kind: ValueDeclarationKind::Variable,
                name: name.clone(),
                spelling: spelling.clone(),
                expansion,
                definition: unsafe { clang_isCursorDefinition(child) } != 0,
                main_file,
                root: true,
                system,
            });
            traversal.constants.push(Constant {
                root: origin.clone(),
                definition: origin.clone(),
                spelling,
                name: name.clone(),
                ty: TypeRef::Scalar(scalar),
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
        && let Some((spelling, expansion, main_file, system)) = cursor_locations(child)
        && traversal.is_root(&spelling.file)
    {
        let ty = unsafe { clang_getEnumDeclIntegerType(child) };
        if let Some(repr) = scalar(ty) {
            let origin = Origin {
                tu: traversal.tu.to_string(),
                local,
            };
            traversal.value_declarations.push(ValueDeclaration {
                origin: origin.clone(),
                parent: parent.cloned(),
                kind: ValueDeclarationKind::AnonymousEnum,
                name: name.clone(),
                spelling: spelling.clone(),
                expansion,
                definition: unsafe { clang_isCursorDefinition(child) } != 0,
                main_file,
                root: true,
                system,
            });
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
                let root = traversal.paths.is_root(&spelling.file);
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
                    let definition = if fact_kind == FactKind::Function {
                        match traversal.definitions.definition(child) {
                            Ok(definition) => definition,
                            Err(error) => {
                                traversal.error = Some(error);
                                return;
                            }
                        }
                    } else {
                        (unsafe { clang_isCursorDefinition(child) }) != 0
                    };
                    let data = if deferred_struct || deferred_macro {
                        FactData::None
                    } else {
                        fact_data(child, fact_kind, traversal.macros, definition)
                    };
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
                    if matches!(
                        data,
                        FactData::Function { .. }
                            | FactData::NonEmittableFunction { .. }
                            | FactData::Callback { .. }
                            | FactData::Interface { .. }
                    ) {
                        traversal.pending_callables.push((index, child));
                    }
                    let declaration_guid = matches!(fact_kind, FactKind::Class | FactKind::Struct)
                        .then(|| cursor_uuid(child))
                        .flatten();
                    if matches!(fact_kind, FactKind::Class | FactKind::Struct) {
                        traversal.pending_classes.push((origin.clone(), child));
                    }
                    traversal.facts.push(Fact {
                        origin: origin.clone(),
                        parent: parent.cloned(),
                        kind: fact_kind,
                        name,
                        spelling,
                        expansion,
                        definition,
                        main_file,
                        root,
                        system,
                        data,
                    });
                    if clang_flag_enum {
                        traversal.clang_flag_enums.insert(origin.clone());
                    }
                    if let Some(guid) = declaration_guid {
                        traversal.declaration_guids.insert(origin.clone(), guid);
                    }
                    let annotation_source_range = function_annotation_source_range(
                        &traversal.facts[index],
                        &traversal.facts[..index],
                        child,
                    );
                    if let Err(error) = collect_fact_annotations(
                        child,
                        fact_kind,
                        &origin,
                        traversal.macros,
                        annotation_source_range.as_ref(),
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

fn bind_class_origins(
    tu: CXTranslationUnit,
    pending: &[(Origin, CXCursor)],
    seen: &HashMap<u32, Vec<(CXCursor, Origin)>>,
    output: &mut BTreeMap<Origin, CanonicalRecord>,
) -> Result<(), Error> {
    let mut generated: HashMap<u32, Vec<(CXCursor, CanonicalRecord)>> = HashMap::new();
    for (origin, cursor) in pending {
        let canonical = unsafe { clang_getCanonicalCursor(*cursor) };
        if unsafe {
            clang_Cursor_isNull(*cursor) != 0
                || clang_Cursor_isNull(canonical) != 0
                || clang_Cursor_getTranslationUnit(*cursor) != tu
                || clang_Cursor_getTranslationUnit(canonical) != tu
                || !matches!(
                    clang_getCursorKind(*cursor),
                    CXCursor_ClassDecl | CXCursor_StructDecl
                )
                || !matches!(
                    clang_getCursorKind(canonical),
                    CXCursor_ClassDecl | CXCursor_StructDecl
                )
        } {
            return Err(Error(format!(
                "invalid native class canonical cursor for {}",
                super::origin(origin)
            )));
        }
        let hash = unsafe { clang_hashCursor(canonical) };
        let mut matched = None;
        for (candidate, candidate_origin) in seen.get(&hash).into_iter().flatten() {
            if unsafe { clang_equalCursors(canonical, *candidate) } == 0 {
                continue;
            }
            if candidate_origin.tu != origin.tu
                || matched.is_some_and(|previous| previous != candidate_origin)
            {
                return Err(Error(
                    "conflicting exact native class canonical origins".into(),
                ));
            }
            matched = Some(candidate_origin);
        }
        let location = unsafe { clang_getCursorLocation(canonical) };
        let has_source = source_location(location, clang_getSpellingLocation).is_some()
            || source_location(location, clang_getExpansionLocation).is_some();
        let identity = if has_source {
            CanonicalRecord::Source(matched.cloned().ok_or_else(|| {
                Error(format!(
                    "missing exact native class canonical origin for {}",
                    super::origin(origin)
                ))
            })?)
        } else {
            if matched.is_some() || unsafe { clang_isCursorDefinition(canonical) } != 0 {
                return Err(Error("invalid source-free native canonical record".into()));
            }
            let kind = match unsafe { clang_getCursorKind(canonical) } {
                CXCursor_ClassDecl => CanonicalRecordKind::Class,
                CXCursor_StructDecl => CanonicalRecordKind::Struct,
                _ => unreachable!(),
            };
            let bucket = generated.entry(hash).or_default();
            if let Some((_, identity)) = bucket
                .iter()
                .find(|(candidate, _)| unsafe { clang_equalCursors(canonical, *candidate) } != 0)
            {
                identity.clone()
            } else {
                let identity = CanonicalRecord::CompilerGenerated {
                    anchor: origin.clone(),
                    kind,
                };
                bucket.push((canonical, identity.clone()));
                identity
            }
        };
        if let Some(previous) = output.insert(origin.clone(), identity.clone())
            && previous != identity
        {
            return Err(Error("conflicting native class canonical binding".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod class_binding_tests {
    use super::*;

    #[test]
    fn exact_native_class_joins_reject_missing_conflicting_and_foreign_evidence() {
        helpers::ensure_libclang();
        let _library = Library::new().unwrap();
        let index = Index::new().unwrap();
        let input = Input::new("classes.hpp", "class X; class X; class Y;");
        let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
        let tu = TranslationUnit::parse(&index, &input, &args).unwrap();
        let classes: Vec<_> = cursor_children(unsafe { clang_getTranslationUnitCursor(tu.0) })
            .into_iter()
            .filter(|cursor| unsafe { clang_getCursorKind(*cursor) } == CXCursor_ClassDecl)
            .collect();
        let first = Origin {
            tu: input.name.clone(),
            local: 1,
        };
        let second = Origin {
            tu: input.name,
            local: 2,
        };
        let hash = unsafe { clang_hashCursor(classes[0]) };
        let mut seen = HashMap::from([(
            hash,
            vec![(classes[0], first.clone()), (classes[2], second.clone())],
        )]);
        let pending = [(second.clone(), classes[1])];
        let mut output = BTreeMap::new();
        bind_class_origins(tu.0, &pending, &seen, &mut output).unwrap();
        assert_eq!(output.get(&second), Some(&CanonicalRecord::Source(first)));
        seen.get_mut(&hash).unwrap().push((classes[0], second));
        assert!(
            bind_class_origins(tu.0, &pending, &seen, &mut BTreeMap::new())
                .unwrap_err()
                .to_string()
                .contains("conflicting")
        );
        assert!(
            bind_class_origins(tu.0, &pending, &HashMap::new(), &mut BTreeMap::new())
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
        let foreign =
            TranslationUnit::parse(&index, &Input::new("foreign.hpp", "class X;"), &args).unwrap();
        assert!(
            bind_class_origins(foreign.0, &pending, &seen, &mut BTreeMap::new())
                .unwrap_err()
                .to_string()
                .contains("invalid")
        );
    }

    #[test]
    fn mixed_record_native_join_preserves_struct_canonical_and_rejects_union() {
        helpers::ensure_libclang();
        let _library = Library::new().unwrap();
        let index = Index::new().unwrap();
        let input = Input::new("mixed.hpp", "struct X; class X; union U;");
        let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
        let tu = TranslationUnit::parse(&index, &input, &args).unwrap();
        let records: Vec<_> = cursor_children(unsafe { clang_getTranslationUnitCursor(tu.0) })
            .into_iter()
            .filter(|cursor| unsafe {
                matches!(
                    clang_getCursorKind(*cursor),
                    CXCursor_ClassDecl | CXCursor_StructDecl | CXCursor_UnionDecl
                )
            })
            .collect();
        assert_eq!(records.len(), 3);
        let canonical = unsafe { clang_getCanonicalCursor(records[1]) };
        assert_eq!(
            unsafe { clang_getCursorKind(canonical) },
            CXCursor_StructDecl
        );
        assert_ne!(unsafe { clang_equalCursors(records[0], records[1]) }, 1);
        assert_eq!(unsafe { clang_equalCursors(records[0], canonical) }, 1);
        let first = Origin {
            tu: input.name.clone(),
            local: 1,
        };
        let second = Origin {
            tu: input.name,
            local: 2,
        };
        let hash = unsafe { clang_hashCursor(canonical) };
        let seen = HashMap::from([(hash, vec![(records[0], first.clone())])]);
        let mut output = BTreeMap::new();
        bind_class_origins(
            tu.0,
            &[(first.clone(), records[0]), (second.clone(), records[1])],
            &seen,
            &mut output,
        )
        .unwrap();
        assert_eq!(
            output.get(&first),
            Some(&CanonicalRecord::Source(first.clone()))
        );
        assert_eq!(output.get(&second), Some(&CanonicalRecord::Source(first)));
        assert!(
            bind_class_origins(tu.0, &[(second, records[2])], &seen, &mut output)
                .unwrap_err()
                .to_string()
                .contains("invalid")
        );
    }
}

fn materialize_anonymous_callbacks(facts: &mut Vec<Fact>, mut next_local: HashMap<String, u32>) {
    let mut used: BTreeSet<_> = facts.iter().map(|fact| fact.name.clone()).collect();
    for fact in facts.iter() {
        next_local
            .entry(fact.origin.tu.clone())
            .and_modify(|local| *local = (*local).max(fact.origin.local + 1))
            .or_insert(fact.origin.local + 1);
    }

    let mut names = BTreeMap::new();
    for fact in facts.iter_mut() {
        if !matches!(
            fact.data,
            FactData::Record { .. }
                | FactData::Typedef {
                    target: TypeRef::InlineRecord(_)
                }
        ) {
            continue;
        }
        let owner = SyntheticOwner::new(fact);
        if let Some(fields) = callback_fields(&mut fact.data) {
            visit_field_callbacks(fields, &owner.name, &mut vec![], &mut |_, stem, route| {
                names
                    .entry(owner.source(route))
                    .or_insert_with(|| stem.to_string());
            });
        }
    }
    for name in names.values_mut() {
        *name = unique_synthetic_name(name, &mut used);
    }

    let mut callbacks = vec![];
    for fact in facts.iter_mut() {
        if !matches!(
            fact.data,
            FactData::Record { .. }
                | FactData::Typedef {
                    target: TypeRef::InlineRecord(_)
                }
        ) {
            continue;
        }
        let owner = SyntheticOwner::new(fact);
        if let Some(fields) = callback_fields(&mut fact.data) {
            visit_field_callbacks(fields, &owner.name, &mut vec![], &mut |ty, _, route| {
                let TypeRef::FunctionPointer {
                    convention,
                    params,
                    result,
                } = ty
                else {
                    unreachable!();
                };
                let name = names[&owner.source(route)].clone();
                let local = next_local.entry(owner.origin.tu.clone()).or_default();
                let origin = Origin {
                    tu: owner.origin.tu.clone(),
                    local: *local,
                };
                *local += 1;
                callbacks.push(Fact {
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
                });
                *ty = TypeRef::Named {
                    name,
                    declaration: owner.spelling.clone(),
                };
            });
        }
    }
    facts.extend(callbacks);
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CallbackRoute {
    Field { index: usize, name: String },
    Pointer,
    Reference,
    Array,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CallbackSource {
    spelling: Location,
    expansion: Location,
    kind: FactKind,
    name: String,
    route: Vec<CallbackRoute>,
}

struct SyntheticOwner {
    origin: Origin,
    kind: FactKind,
    name: String,
    spelling: Location,
    expansion: Location,
    main_file: bool,
    root: bool,
    system: bool,
}

impl SyntheticOwner {
    fn new(fact: &Fact) -> Self {
        Self {
            origin: fact.origin.clone(),
            kind: fact.kind,
            name: fact.name.clone(),
            spelling: fact.spelling.clone(),
            expansion: fact.expansion.clone(),
            main_file: fact.main_file,
            root: fact.root,
            system: fact.system,
        }
    }

    fn source(&self, route: &[CallbackRoute]) -> CallbackSource {
        CallbackSource {
            spelling: self.spelling.clone(),
            expansion: self.expansion.clone(),
            kind: self.kind,
            name: self.name.clone(),
            route: route.to_vec(),
        }
    }
}

fn callback_fields(data: &mut FactData) -> Option<&mut [Field]> {
    match data {
        FactData::Record { fields, .. } => Some(fields),
        FactData::Typedef {
            target: TypeRef::InlineRecord(record),
        } => Some(&mut record.fields),
        _ => None,
    }
}

fn visit_field_callbacks(
    fields: &mut [Field],
    stem: &str,
    route: &mut Vec<CallbackRoute>,
    visit: &mut impl FnMut(&mut TypeRef, &str, &[CallbackRoute]),
) {
    for (index, field) in fields.iter_mut().enumerate() {
        route.push(CallbackRoute::Field {
            index,
            name: field.name.clone(),
        });
        let stem = format!(
            "{}_{}",
            stem.trim_start_matches('_'),
            field.name.trim_start_matches('_')
        );
        visit_type_callbacks(&mut field.ty, &stem, route, visit);
        route.pop();
    }
}

fn visit_type_callbacks(
    ty: &mut TypeRef,
    stem: &str,
    route: &mut Vec<CallbackRoute>,
    visit: &mut impl FnMut(&mut TypeRef, &str, &[CallbackRoute]),
) {
    match ty {
        TypeRef::FunctionPointer { .. } => visit(ty, stem, route),
        TypeRef::Pointer { target, .. } => {
            route.push(CallbackRoute::Pointer);
            visit_type_callbacks(target, stem, route, visit);
            route.pop();
        }
        TypeRef::Reference { target, .. } => {
            route.push(CallbackRoute::Reference);
            visit_type_callbacks(target, stem, route, visit);
            route.pop();
        }
        TypeRef::Array { target, .. } => {
            route.push(CallbackRoute::Array);
            visit_type_callbacks(target, stem, route, visit);
            route.pop();
        }
        TypeRef::InlineRecord(record) => {
            visit_field_callbacks(&mut record.fields, stem, route, visit);
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
    extracted: &mut [Extracted],
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
    let mut pending: HashMap<String, Vec<(usize, FactData)>> = HashMap::new();
    for extraction in extracted {
        for (fact_index, data) in std::mem::take(&mut extraction.pending_structs) {
            pending
                .entry(facts[fact_index].name.clone())
                .or_default()
                .push((fact_index, data));
        }
    }
    let mut queue: Vec<_> = reachable.iter().cloned().collect();
    while let Some(name) = queue.pop() {
        let Some(candidates) = pending.remove(&name) else {
            continue;
        };
        for (fact_index, data) in candidates {
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
    extracted: &mut [Extracted],
    selected: &BTreeSet<String>,
) {
    let mut roots: HashSet<_> = facts
        .iter()
        .filter(|fact| fact.root && fact.kind == FactKind::Macro)
        .map(|fact| fact.name.clone())
        .collect();
    roots.extend(selected.iter().cloned());
    for extraction in extracted {
        for (index, data) in std::mem::take(&mut extraction.pending_macros) {
            if roots.contains(facts[index].name.as_str()) {
                facts[index].data = data;
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
    count_probes: Option<CountProbes<'tu>>,
    count_requests: RefCell<BTreeMap<Location, (bool, BTreeSet<String>)>>,
    count_values: RefCell<BTreeMap<(Location, String), Result<i32, SalCountError>>>,
}

struct CountProbes<'tu> {
    index: &'tu Index,
    input: &'tu Input,
    args: Vec<String>,
    original: &'tu TranslationUnit,
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
    fn request_constant_count(&self, cursor: CXCursor, expression: &str) {
        let Some((location, inline)) = count_annotation_site(cursor) else {
            return;
        };
        self.count_requests
            .borrow_mut()
            .entry(location)
            .or_insert_with(|| (inline, BTreeSet::new()))
            .1
            .insert(expression.to_string());
    }

    fn fail_constant_counts(&self, error: &str) {
        for (location, (_, expressions)) in self.count_requests.borrow().iter() {
            for expression in expressions {
                self.count_values.borrow_mut().insert(
                    (location.clone(), expression.clone()),
                    Err(SalCountError::Invalid(error.to_string())),
                );
            }
        }
    }

    fn collect_constant_sizes(&self, fact: &Fact, cursor: CXCursor, sizes: &mut SalConstantSizes) {
        let collect = |callable: CXCursor,
                       params: &[Parameter],
                       method: Option<usize>,
                       sizes: &mut SalConstantSizes| {
            let cursors: Vec<_> = cursor_children(callable)
                .into_iter()
                .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_ParmDecl)
                .collect();
            for (index, param) in params.iter().enumerate() {
                let Some(SalSize {
                    bytes,
                    value: SalSizeValue::Expression(expression),
                }) = &param.annotation.size
                else {
                    continue;
                };
                let Some(cursor) = cursors.get(index) else {
                    continue;
                };
                let Some((location, _)) = count_annotation_site(*cursor) else {
                    continue;
                };
                let Some(value) = self
                    .count_values
                    .borrow()
                    .get(&(location.clone(), expression.clone()))
                    .cloned()
                else {
                    continue;
                };
                let target = method.map_or_else(
                    || AnnotationTarget::Parameter {
                        declaration: fact.origin.clone(),
                        index,
                    },
                    |method| AnnotationTarget::MethodParameter {
                        declaration: fact.origin.clone(),
                        method,
                        parameter: index,
                    },
                );
                let value = value.map(|count| {
                    let mut annotation = ParamAnnotation {
                        size: Some(SalSize {
                            bytes: *bytes,
                            value: SalSizeValue::Constant(count),
                        }),
                        ..Default::default()
                    };
                    normalize_constant_byte_size(
                        unsafe { clang_getCursorType(*cursor) },
                        &mut annotation,
                    );
                    annotation.size.unwrap()
                });
                sizes.insert(target, value);
            }
        };
        match &fact.data {
            FactData::Function { params, .. }
            | FactData::NonEmittableFunction {
                signature: FunctionSignature { params, .. },
                ..
            }
            | FactData::Callback { params, .. } => {
                collect(cursor, params, None, sizes);
                if matches!(fact.data, FactData::Callback { .. }) {
                    for child in cursor_children(cursor)
                        .into_iter()
                        .filter(|child| unsafe { clang_getCursorKind(*child) == CXCursor_TypeRef })
                    {
                        let candidate = unsafe { clang_getCursorReferenced(child) };
                        collect(candidate, params, None, sizes);
                    }
                }
            }
            FactData::Interface { methods, .. } => {
                for (index, method) in cursor_children(cursor)
                    .into_iter()
                    .filter(|child| unsafe {
                        clang_getCursorKind(*child) == CXCursor_CXXMethod
                            && clang_CXXMethod_isVirtual(*child) != 0
                    })
                    .filter(|child| !method_overrides_base(*child))
                    .enumerate()
                {
                    if let Some(params) = methods.get(index).map(|method| &method.params) {
                        collect(method, params, Some(index), sizes);
                    }
                }
            }
            _ => {}
        }
    }

    fn resolve_constant_counts(&self, timing: bool) -> Result<(), Error> {
        let requests = self.count_requests.borrow();
        if requests.is_empty() {
            if timing {
                let context = self.count_probes.as_ref().unwrap();
                eprintln!(
                    "windows-clang timing phase=sal-count-probes tu={:?} contexts=0 expressions=0 synthetic_tus=0 resolved=0 elapsed_ms=0",
                    context.input.name
                );
            }
            return Ok(());
        }
        let start = std::time::Instant::now();
        let context = self.count_probes.as_ref().unwrap();
        let mut files = BTreeMap::<String, Vec<(u32, Vec<String>)>>::new();
        let mut keys = Vec::new();
        for (location, (inline, expressions)) in requests.iter() {
            let mut probe = Vec::new();
            for expression in expressions {
                let index = keys.len();
                let annotation = format!("annotate(\"__clang_sal_count_{index}\", ({expression}))");
                probe.push(if *inline {
                    format!("{annotation}, ")
                } else {
                    format!("__attribute__(({annotation})) ")
                });
                keys.push((location.clone(), expression.clone()));
            }
            files
                .entry(location.file.clone())
                .or_default()
                .push((location.offset, probe));
        }
        let mut sources = Vec::new();
        let mut ranges = BTreeMap::<String, Vec<std::ops::Range<u32>>>::new();
        let mut insertions = BTreeMap::<String, Vec<std::ops::Range<u32>>>::new();
        for (file, probes) in files {
            let name = CString::new(file.as_str()).unwrap();
            let native_file = unsafe { clang_getFile(context.original.0, name.as_ptr()) };
            let mut length = 0;
            let contents =
                unsafe { clang_getFileContents(context.original.0, native_file, &mut length) };
            if contents.is_null() {
                return Err(Error(format!("cannot read SAL count source `{file}`")));
            }
            let contents = unsafe { std::slice::from_raw_parts(contents.cast::<u8>(), length) };
            let mut injected = Vec::new();
            let mut previous = 0;
            for (offset, probe) in probes {
                let offset = offset as usize;
                if offset < previous || offset > contents.len() {
                    return Err(Error(format!(
                        "invalid SAL count source offset in `{file}`"
                    )));
                }
                injected.extend_from_slice(&contents[previous..offset]);
                let start = injected.len() as u32;
                // Probe the parameter in place without changing its scope or source line.
                for declaration in probe {
                    let range_start = injected.len() as u32;
                    injected.extend_from_slice(declaration.as_bytes());
                    ranges
                        .entry(file.clone())
                        .or_default()
                        .push(range_start..injected.len() as u32);
                }
                insertions
                    .entry(file.clone())
                    .or_default()
                    .push(start..injected.len() as u32);
                previous = offset;
            }
            injected.extend_from_slice(&contents[previous..]);
            sources.push((name, CString::new(injected).unwrap()));
        }
        let input_name = CString::new(context.input.name.as_str()).unwrap();
        if !ranges.contains_key(&normalize_name(&context.input.name)) {
            sources.push((
                input_name.clone(),
                CString::new(context.input.source.as_str()).unwrap(),
            ));
        }
        let mut unsaved: Vec<_> = sources
            .iter()
            .map(|(name, source)| CXUnsavedFile {
                Filename: name.as_ptr(),
                Contents: source.as_ptr(),
                Length: source.as_bytes().len().try_into().unwrap(),
            })
            .collect();
        let args: Vec<_> = context.args.iter().map(String::as_str).collect();
        let tu =
            TranslationUnit::parse_probe_files(context.index, &input_name, &args, &mut unsaved)?;
        let mut rejected = BTreeMap::new();
        let mut failures = Vec::new();
        let original_diagnostics = context.original.error_diagnostics();
        for diagnostic in tu.error_diagnostics() {
            if let Some(file_ranges) = ranges.get(&diagnostic.file) {
                if let Some(range) = file_ranges
                    .iter()
                    .find(|range| range.contains(&diagnostic.offset))
                {
                    rejected.insert((diagnostic.file, range.start), diagnostic.spelling);
                    continue;
                }
                let removed: u32 = insertions[&diagnostic.file]
                    .iter()
                    .filter(|range| range.end <= diagnostic.offset)
                    .map(|range| range.end - range.start)
                    .sum();
                if original_diagnostics.iter().any(|original| {
                    original.file == diagnostic.file
                        && original.offset == diagnostic.offset - removed
                        && original.spelling == diagnostic.spelling
                }) {
                    continue;
                }
            } else if original_diagnostics.contains(&diagnostic) {
                continue;
            }
            failures.push(diagnostic.spelling);
        }
        fn collect(
            cursor: CXCursor,
            values: &mut BTreeMap<usize, (Location, Result<i32, SalCountError>)>,
        ) {
            if unsafe { clang_getCursorKind(cursor) } == CXCursor_AnnotateAttr
                && let Some(index) = cx_string(unsafe { clang_getCursorSpelling(cursor) })
                    .strip_prefix("__clang_sal_count_")
                    .and_then(|index| index.parse().ok())
                && let Some((location, _, _, _)) = cursor_locations(cursor)
                && let Some(value) = cursor_children(cursor)
                    .into_iter()
                    .find_map(evaluate_integer)
            {
                let value = i32::try_from(value.1)
                    .ok()
                    .filter(|value| *value >= 0)
                    .ok_or_else(|| {
                        SalCountError::Invalid(format!(
                            "native SAL count `{}` is outside the nonnegative i32 range",
                            value.1
                        ))
                    });
                match values.entry(index) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert((location, value));
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().1 != value {
                            entry.get_mut().1 = Err(SalCountError::Invalid(
                                "conflicting native SAL count observations".to_string(),
                            ));
                        }
                    }
                }
            }
            for child in cursor_children(cursor) {
                collect(child, values);
            }
        }
        let mut values = BTreeMap::new();
        collect(unsafe { clang_getTranslationUnitCursor(tu.0) }, &mut values);
        for (index, key) in keys.iter().enumerate() {
            let value = if !failures.is_empty() {
                Err(SalCountError::Invalid(format!(
                    "native SAL count probe/context failure: {}",
                    failures.join("; ")
                )))
            } else if let Some((location, value)) = values.get(&index) {
                if let Some(range) = ranges
                    .get(&location.file)
                    .and_then(|ranges| ranges.iter().find(|range| range.contains(&location.offset)))
                {
                    if let Some(error) = rejected.get(&(location.file.clone(), range.start)) {
                        Err(SalCountError::Unsupported(format!(
                            "SAL count expression `{}` is not a supported compiler constant: {error}",
                            key.1
                        )))
                    } else {
                        value.clone()
                    }
                } else {
                    Err(SalCountError::Invalid(
                        "native SAL count probe/context failure: missing source range".to_string(),
                    ))
                }
            } else {
                Err(SalCountError::Unsupported(format!(
                    "SAL count expression `{}` is not a supported compiler constant",
                    key.1
                )))
            };
            let value = value.map_err(|error| match error {
                SalCountError::Invalid(reason) => {
                    SalCountError::Invalid(format!("SAL count expression `{}`: {reason}", key.1))
                }
                unsupported => unsupported,
            });
            self.count_values.borrow_mut().insert(key.clone(), value);
        }
        if timing {
            eprintln!(
                "windows-clang timing phase=sal-count-probes tu={:?} \
                 contexts={} expressions={} synthetic_tus=1 resolved={} rejected={} elapsed_ms={:.3}",
                context.input.name,
                requests.len(),
                keys.len(),
                self.count_values
                    .borrow()
                    .values()
                    .filter(|value| value.is_ok())
                    .count(),
                self.count_values
                    .borrow()
                    .values()
                    .filter(|value| value.is_err())
                    .count(),
                start.elapsed().as_secs_f64() * 1000.0,
            );
        }
        Ok(())
    }

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
    sources: &ConstantSources<'_>,
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
    let source_diagnostics = sources.diagnostics;
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
                    evaluate_probe(index, input, args, batch, source_diagnostics)?;
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
                    evaluate_probe(index, input, args, batch, source_diagnostics)?;
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
                    evaluate_probe(index, input, args, batch, source_diagnostics)?;
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
                source_diagnostics,
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
    macros: &FinalMacros,
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

fn fact_data(
    cursor: CXCursor,
    kind: FactKind,
    macros: &MacroDefinitions,
    definition: bool,
) -> FactData {
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
            let linkage = match unsafe { clang_getCursorLinkage(cursor) } {
                CXLinkage_NoLinkage => FunctionLinkage::None,
                CXLinkage_Internal => FunctionLinkage::Internal,
                CXLinkage_UniqueExternal => FunctionLinkage::UniqueExternal,
                CXLinkage_External => FunctionLinkage::External,
                linkage => {
                    return FactData::Unsupported {
                        reason: format!("function has unsupported linkage `{linkage}`"),
                    };
                }
            };
            let signature = match native_function_signature(cursor, macros) {
                Ok(signature) => signature,
                Err(reason) => return FactData::Unsupported { reason },
            };
            let reason = if definition {
                Some(FunctionExclusion::Definition { linkage })
            } else if linkage != FunctionLinkage::External {
                Some(FunctionExclusion::NonExternalLinkage { linkage })
            } else {
                None
            };
            if let Some(reason) = reason {
                return FactData::NonEmittableFunction { reason, signature };
            }
            let FunctionSignature {
                link_name,
                convention,
                params,
                result,
                variadic,
                noreturn,
            } = signature;
            FactData::Function {
                link_name,
                convention,
                params,
                result,
                variadic,
                noreturn,
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

fn native_function_signature(
    cursor: CXCursor,
    macros: &MacroDefinitions,
) -> Result<FunctionSignature, String> {
    let variadic = unsafe { clang_Cursor_isVariadic(cursor) } != 0;
    let result_ty = unsafe { clang_getCursorResultType(cursor) };
    let result = type_ref(result_ty).ok_or_else(|| {
        format!(
            "function has unsupported result type `{}`",
            cx_string(unsafe { clang_getTypeSpelling(result_ty) }),
        )
    })?;
    let params = callable_params(cursor, macros, false)?;
    let function_ty = unsafe { clang_getCursorType(cursor) };
    let convention = (if variadic {
        calling_convention_fact(function_ty)
    } else {
        source_calling_convention(cursor, macros).or_else(|| calling_convention_fact(function_ty))
    })
    .ok_or_else(|| "function has an unsupported calling convention".to_string())?;
    Ok(FunctionSignature {
        link_name: external_link_name(cursor),
        convention,
        params,
        result,
        variadic,
        noreturn: function_is_noreturn(cursor),
    })
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
        || !expanded_raw_annotations(definition, None)
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
    let mangled = cx_string(unsafe { clang_Cursor_getMangling(cursor) });
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
    let mut param_cursors = vec![];
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
        param_cursors.push(child);
    }
    let names: BTreeSet<_> = params.iter().map(|param| param.name.clone()).collect();
    for (param, cursor) in params.iter_mut().zip(param_cursors) {
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
                SalSizeValue::Expression(_) => {}
                _ => {}
            }
        }
        if discard_size {
            param.annotation.size = None;
        }
        if let Some(SalSize {
            value: SalSizeValue::Expression(expression),
            ..
        }) = &param.annotation.size
            && !expression
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|name| names.contains(name))
        {
            macros.request_constant_count(cursor, expression);
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
            | "memory_size_const"
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
        | "memory_size_param" | "memory_size_const" | "in" | "out" | "optional" | "reserved"
        | "retval" | "com_out_ptr" => target == CXCursor_ParmDecl,
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

struct AnnotationSourceRange<'a> {
    file: &'a str,
    start: u32,
    end: u32,
}

fn expanded_raw_annotations(
    cursor: CXCursor,
    source_range: Option<&AnnotationSourceRange<'_>>,
) -> Vec<RawAnnotation> {
    let mut result = vec![];
    for child in cursor_children(cursor) {
        if unsafe { clang_getCursorKind(child) } != CXCursor_AnnotateAttr {
            continue;
        }
        if let Some(source_range) = source_range
            && !cursor_locations(child).is_some_and(|(_, expansion, _, _)| {
                expansion.file.eq_ignore_ascii_case(source_range.file)
                    && (source_range.start..source_range.end).contains(&expansion.offset)
            })
        {
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
    annotation_values_in_range(cursor, macros, None)
}

fn annotation_values_in_range(
    cursor: CXCursor,
    macros: &MacroDefinitions,
    source_range: Option<&AnnotationSourceRange<'_>>,
) -> Result<Vec<Annotation>, Error> {
    let before = macros.cursor_order(cursor).unwrap_or(usize::MAX);
    let location = cursor_locations(cursor).map(|(spelling, _, _, _)| spelling);
    expanded_raw_annotations(cursor, source_range)
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
        "memory_size_const" => Annotation::MemorySizeConst(value?),
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

fn function_annotation_source_range<'a>(
    fact: &'a Fact,
    previous: &[Fact],
    cursor: CXCursor,
) -> Option<AnnotationSourceRange<'a>> {
    let (FactData::Function { link_name, .. }
    | FactData::NonEmittableFunction {
        signature: FunctionSignature { link_name, .. },
        ..
    }) = &fact.data
    else {
        return None;
    };
    let (file, _, end) = cursor_expansion_extent(cursor)?;
    if !file.eq_ignore_ascii_case(&fact.expansion.file) {
        return None;
    }
    // Libclang includes inherited annotate attributes on redeclaration cursors.
    let start = previous
        .iter()
        .filter(|candidate| {
            candidate.origin.tu == fact.origin.tu
                && candidate.name == fact.name
                && candidate
                    .expansion
                    .file
                    .eq_ignore_ascii_case(&fact.expansion.file)
                && candidate.expansion.offset < fact.expansion.offset
                && matches!(
                    &candidate.data,
                    FactData::Function { link_name: candidate_link_name, .. }
                    | FactData::NonEmittableFunction {
                        signature: FunctionSignature { link_name: candidate_link_name, .. }, ..
                    } if candidate_link_name == link_name
                )
        })
        .map(|candidate| candidate.expansion.offset)
        .max()
        .unwrap_or(0);
    Some(AnnotationSourceRange {
        file: &fact.expansion.file,
        start,
        end,
    })
}

fn collect_fact_annotations(
    cursor: CXCursor,
    kind: FactKind,
    origin: &Origin,
    macros: &MacroDefinitions,
    source_range: Option<&AnnotationSourceRange<'_>>,
    annotations: &mut BTreeMap<AnnotationTarget, Vec<Annotation>>,
) -> Result<(), Error> {
    let direct = annotation_values_in_range(cursor, macros, source_range)?;
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
                annotation_values_in_range(parameter, macros, source_range)?,
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
                let ty = unsafe { clang_getCursorType(child) };
                if matches!(type_ref(ty), Some(TypeRef::InlineRecord(_))) {
                    let mut path = prefix.to_vec();
                    path.push(index);
                    collect_record_field_annotations(
                        unsafe { clang_getTypeDeclaration(ty) },
                        origin,
                        macros,
                        annotations,
                        &path,
                    )?;
                }
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
        .filter(|fact| fact.kind != FactKind::Function)
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
        Annotation::MemorySizeConst(_) => "memory_size_const",
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

fn count_annotation_site(cursor: CXCursor) -> Option<(Location, bool)> {
    let annotation = cursor_children(cursor).into_iter().find(|child| {
        if unsafe { clang_getCursorKind(*child) } != CXCursor_AnnotateAttr {
            return false;
        }
        let annotation = cx_string(unsafe { clang_getCursorSpelling(*child) });
        (annotation.contains("_reads_")
            || annotation.contains("_writes_")
            || annotation.contains("_updates_"))
            && annotation.contains('(')
    })?;
    let location = source_location(
        unsafe { clang_getRangeStart(clang_getCursorExtent(annotation)) },
        clang_getExpansionLocation,
    )?;
    let inline = cursor_tokens(annotation)
        .first()
        .is_some_and(|(_, token)| token == "annotate");
    Some((location, inline))
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
