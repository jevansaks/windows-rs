use super::*;
use std::mem::size_of;

#[derive(Default, Debug, PartialEq, Eq)]
struct Bytes {
    logical: usize,
    capacity: usize,
    map_entries: usize,
    map_payload_estimate: usize,
    hash_capacity_estimate: usize,
}

impl Bytes {
    fn string(&mut self, value: &String) {
        self.logical += value.len();
        self.capacity += value.capacity();
    }

    fn vector<T>(&mut self, value: &Vec<T>) {
        self.logical += value.len() * size_of::<T>();
        self.capacity += value.capacity() * size_of::<T>();
    }

    fn boxed<T>(&mut self, _: &T) {
        self.logical += size_of::<T>();
        self.capacity += size_of::<T>();
    }

    fn map<K, V>(&mut self, len: usize) {
        self.map_entries += len;
        self.map_payload_estimate += len * size_of::<(K, V)>();
    }

    fn origin(&mut self, value: &Origin) {
        self.string(&value.tu);
    }

    fn location(&mut self, value: &Location) {
        self.string(&value.file);
    }

    fn target(&mut self, value: &AnnotationTarget) {
        self.origin(value.origin());
        if let AnnotationTarget::NestedField { path, .. } = value {
            self.vector(path);
        }
    }

    fn size(&mut self, value: &SalSize) {
        match &value.value {
            SalSizeValue::Constant(_) => {}
            SalSizeValue::Parameter(value)
            | SalSizeValue::IndirectParameter(value)
            | SalSizeValue::Expression(value) => self.string(value),
        }
    }

    fn parameters(&mut self, values: &Vec<Parameter>) {
        self.vector(values);
        for Parameter {
            name,
            ty,
            annotation,
        } in values
        {
            self.string(name);
            self.ty(ty);
            if let Some(size) = &annotation.size {
                self.size(size);
            }
            if let Some(reason) = &annotation.unsupported {
                self.string(reason);
            }
        }
    }

    fn fields(&mut self, values: &Vec<Field>) {
        self.vector(values);
        for field in values {
            self.string(&field.name);
            self.ty(&field.ty);
        }
    }

    fn ty(&mut self, value: &TypeRef) {
        match value {
            TypeRef::Void | TypeRef::String | TypeRef::Object | TypeRef::Scalar(_) => {}
            TypeRef::Named { name, declaration } => {
                self.string(name);
                self.location(declaration);
            }
            TypeRef::Pointer { target, .. }
            | TypeRef::Reference { target, .. }
            | TypeRef::Array { target, .. } => {
                self.boxed(target.as_ref());
                self.ty(target);
            }
            TypeRef::FunctionPointer { params, result, .. } => {
                self.vector(params);
                for param in params {
                    self.ty(param);
                }
                self.boxed(result.as_ref());
                self.ty(result);
            }
            TypeRef::OpaquePointer { tag, .. } => self.string(tag),
            TypeRef::Generic {
                name,
                declaration,
                args,
            } => {
                self.string(name);
                self.location(declaration);
                self.vector(args);
                for arg in args {
                    self.ty(arg);
                }
            }
            TypeRef::InlineRecord(record) => {
                self.boxed(record.as_ref());
                if let Some(name) = &record.name {
                    self.string(name);
                }
                if let Some(base) = &record.base {
                    self.ty(base);
                }
                self.fields(&record.fields);
            }
        }
    }

    fn tokens(&mut self, values: &Vec<String>) {
        self.vector(values);
        for value in values {
            self.string(value);
        }
    }

    fn data(&mut self, value: &FactData) {
        match value {
            FactData::Callback { params, result, .. } => {
                self.parameters(params);
                self.ty(result);
            }
            FactData::Class { guid } | FactData::PropertyKey { guid, .. } => self.string(guid),
            FactData::Enum { variants, .. } => {
                self.vector(variants);
                for variant in variants {
                    self.string(&variant.name);
                }
            }
            FactData::EnumFlag { target } => self.string(target),
            FactData::Guid { value } => self.string(value),
            FactData::Macro { tokens, .. } => self.tokens(tokens),
            FactData::Function {
                link_name,
                params,
                result,
                ..
            }
            | FactData::NonEmittableFunction {
                signature:
                    FunctionSignature {
                        link_name,
                        params,
                        result,
                        ..
                    },
                ..
            } => {
                self.string(link_name);
                self.parameters(params);
                self.ty(result);
            }
            FactData::Interface {
                base,
                guid,
                methods,
            } => {
                if let Some(base) = base {
                    self.ty(base);
                }
                if let Some(guid) = guid {
                    self.string(guid);
                }
                self.vector(methods);
                for method in methods {
                    self.string(&method.name);
                    self.parameters(&method.params);
                    self.ty(&method.result);
                }
            }
            FactData::Record { base, fields, .. } => {
                if let Some(base) = base {
                    self.ty(base);
                }
                self.fields(fields);
            }
            FactData::Typedef { target } => self.ty(target),
            FactData::Unsupported { reason } => self.string(reason),
            FactData::None => {}
        }
    }

    fn annotation(&mut self, value: &Annotation) {
        match value {
            Annotation::ImportLibrary(value)
            | Annotation::RaiiFree(value)
            | Annotation::InvalidHandle(value)
            | Annotation::FreeWith(value)
            | Annotation::ArrayCountParam(value)
            | Annotation::ArrayCountConst(value)
            | Annotation::ArrayCountField(value)
            | Annotation::MemorySizeParam(value)
            | Annotation::MemorySizeConst(value)
            | Annotation::IgnoreIfReturn(value)
            | Annotation::AlsoUsableFor(value)
            | Annotation::AssociatedEnum(value)
            | Annotation::AssociatedConstant(value)
            | Annotation::NativeInheritance(value)
            | Annotation::StructSizeField(value)
            | Annotation::NativeEncoding(value)
            | Annotation::StaticLibrary(value)
            | Annotation::SupportedOs(value) => self.string(value),
            Annotation::SetLastError
            | Annotation::PreserveResult
            | Annotation::DoNotRelease
            | Annotation::NotNullTerminated
            | Annotation::NullNullTerminated
            | Annotation::CanReturnErrorsAsSuccess
            | Annotation::CanReturnMultipleSuccessValues
            | Annotation::Retained
            | Annotation::Ansi
            | Annotation::Unicode
            | Annotation::Agile
            | Annotation::Const
            | Annotation::In
            | Annotation::Out
            | Annotation::Optional
            | Annotation::Reserved
            | Annotation::ComOutPtr
            | Annotation::Retval => {}
        }
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Receipt {
    categories: Vec<(&'static str, Bytes)>,
    macro_handles: usize,
    unique_macros: usize,
}

impl Receipt {
    fn category(&mut self, name: &'static str) -> &mut Bytes {
        self.categories.push((name, Bytes::default()));
        &mut self.categories.last_mut().unwrap().1
    }
}

fn measure(enabled: bool, outcome: &Result<NativeExtraction, NativeFailure>) -> Option<Receipt> {
    if !enabled {
        return None;
    }
    let mut receipt = Receipt::default();
    let result = match outcome {
        Ok(result) => result,
        Err(NativeFailure::Parse(error) | NativeFailure::Traversal(error)) => {
            receipt.category("error").string(&error.0);
            return Some(receipt);
        }
    };
    let NativeExtraction {
        output,
        extracted,
        included_files,
        parse_ms: _,
        metrics: _,
    } = result;
    let ExtractionBuffers {
        facts,
        constants,
        value_declarations,
        annotations,
        sal_constant_sizes,
        declaration_guids,
        class_canonical_origins,
        class_canonical_anchors,
        shared_enum_identities,
        module_free_headers: _,
        observation_only_class_origins,
        pointer_callback_aliases,
        pointer_only_class_layouts,
        embeddable_class_layouts,
        clang_flag_enums,
    } = output;
    let bytes = receipt.category("facts");
    bytes.vector(facts);
    for fact in facts {
        bytes.origin(&fact.origin);
        if let Some(parent) = &fact.parent {
            bytes.origin(parent);
        }
        bytes.string(&fact.name);
        bytes.location(&fact.spelling);
        bytes.location(&fact.expansion);
        bytes.data(&fact.data);
    }
    let bytes = receipt.category("constants");
    bytes.vector(constants);
    for constant in constants {
        bytes.origin(&constant.root);
        bytes.origin(&constant.definition);
        bytes.location(&constant.spelling);
        bytes.string(&constant.name);
        bytes.ty(&constant.ty);
        match &constant.value {
            Value::Utf8(value) | Value::Utf16(value) => bytes.string(value),
            Value::F32(_) | Value::F64(_) | Value::Signed(_) | Value::Unsigned(_) => {}
        }
    }
    let bytes = receipt.category("values");
    bytes.vector(value_declarations);
    for value in value_declarations {
        bytes.origin(&value.origin);
        if let Some(parent) = &value.parent {
            bytes.origin(parent);
        }
        bytes.string(&value.name);
        bytes.location(&value.spelling);
        bytes.location(&value.expansion);
    }
    let bytes = receipt.category("annotations");
    bytes.map::<AnnotationTarget, Vec<Annotation>>(annotations.len());
    for (target, values) in annotations {
        bytes.target(target);
        bytes.vector(values);
        for value in values {
            bytes.annotation(value);
        }
    }
    let bytes = receipt.category("sal");
    bytes.map::<AnnotationTarget, Result<SalSize, SalCountError>>(sal_constant_sizes.len());
    for (target, value) in sal_constant_sizes {
        bytes.target(target);
        match value {
            Ok(size) => bytes.size(size),
            Err(SalCountError::Unsupported(reason) | SalCountError::Invalid(reason)) => {
                bytes.string(reason);
            }
        }
    }
    let bytes = receipt.category("guids");
    bytes.map::<Origin, String>(declaration_guids.len());
    for (origin, guid) in declaration_guids {
        bytes.origin(origin);
        bytes.string(guid);
    }
    let bytes = receipt.category("canonical");
    bytes.map::<Origin, CanonicalRecord>(class_canonical_origins.len());
    for (origin, canonical) in class_canonical_origins {
        bytes.origin(origin);
        bytes.origin(canonical.anchor());
    }
    bytes.map::<Origin, ClassCanonicalAnchor>(class_canonical_anchors.len());
    for (origin, anchor) in class_canonical_anchors {
        bytes.origin(origin);
        if let Some(parent) = &anchor.parent {
            bytes.origin(parent);
        }
        bytes.string(&anchor.name);
        bytes.location(&anchor.spelling);
        bytes.location(&anchor.expansion);
        bytes.string(&anchor.template_name);
        bytes.location(&anchor.template_spelling);
        bytes.location(&anchor.template_expansion);
    }
    bytes.map::<Origin, SharedEnumIdentity>(shared_enum_identities.len());
    let mut definitions = HashSet::new();
    for (origin, identity) in shared_enum_identities {
        bytes.origin(origin);
        bytes.origin(&identity.enumeration);
        if definitions.insert(Arc::as_ptr(&identity.definition)) {
            let definition = identity.definition.as_ref();
            bytes.boxed(definition);
            bytes.string(&definition.enum_usr);
            bytes.string(&definition.alias_usr);
            bytes.vector(&definition.tokens);
            for token in &definition.tokens {
                bytes.string(token);
            }
            bytes.data(&definition.data);
        }
    }
    let bytes = receipt.category("layouts");
    bytes.map::<Origin, FactData>(pointer_only_class_layouts.len());
    for (origin, data) in pointer_only_class_layouts {
        bytes.origin(origin);
        bytes.data(data);
    }
    let bytes = receipt.category("origin_sets");
    for origins in [
        observation_only_class_origins,
        pointer_callback_aliases,
        embeddable_class_layouts,
        clang_flag_enums,
    ] {
        bytes.map::<Origin, ()>(origins.len());
        for origin in origins {
            bytes.origin(origin);
        }
    }
    let bytes = receipt.category("includes");
    bytes.vector(included_files);
    for file in included_files {
        bytes.string(&file.input);
        bytes.string(&file.path);
    }
    let Extracted {
        macros,
        diagnostics,
        pending_structs,
        pending_macros,
        root_macros,
        declare_handle_expansions,
        next_local: _,
    } = extracted;
    let bytes = receipt.category("diagnostics");
    bytes.vector(diagnostics);
    for diagnostic in diagnostics {
        bytes.string(&diagnostic.spelling);
        bytes.string(&diagnostic.file);
    }
    let bytes = receipt.category("handles");
    bytes.vector(declare_handle_expansions);
    for expansion in declare_handle_expansions {
        bytes.string(&expansion.name);
        bytes.location(&expansion.location);
    }
    let bytes = receipt.category("deferred_records");
    bytes.vector(pending_structs);
    for (_, data) in pending_structs {
        bytes.data(data);
    }
    let bytes = receipt.category("macro_containers");
    bytes.map::<String, Arc<OwnedMacro>>(macros.0.len());
    bytes.hash_capacity_estimate = macros.0.capacity() * size_of::<(String, Arc<OwnedMacro>)>();
    for name in macros.0.keys() {
        bytes.string(name);
    }
    bytes.vector(root_macros);
    bytes.vector(pending_macros);
    let mut seen = HashSet::new();
    let mut macro_handles = 0;
    let bytes = receipt.category("macro_payloads");
    for captured in macros
        .0
        .values()
        .chain(root_macros.iter().map(|(_, value)| value))
        .chain(pending_macros.iter().map(|(_, value)| value))
    {
        macro_handles += 1;
        if seen.insert(Arc::as_ptr(captured)) {
            bytes.boxed(captured.as_ref());
            bytes.tokens(&captured.tokens);
        }
    }
    receipt.macro_handles = macro_handles;
    receipt.unique_macros = seen.len();
    Some(receipt)
}

fn phase_clocks(outcome: &Result<NativeExtraction, NativeFailure>) -> String {
    const UNAVAILABLE_METRICS: &str = "success_metrics=unavailable \
        definition_index_ms=unavailable macro_index_ms=unavailable traversal_ms=unavailable \
        pre_sal_binding_elapsed_ms=unavailable";
    let Ok(result) = outcome else {
        return format!("parse_ms=unavailable {UNAVAILABLE_METRICS}");
    };
    let mut clocks = format!("parse_ms={:.3} ", result.parse_ms);
    if let Some(metrics) = &result.metrics {
        write!(
            clocks,
            "success_metrics=present definition_index_ms={:.3} macro_index_ms={:.3} \
             traversal_ms={:.3} pre_sal_binding_elapsed_ms={:.3}",
            metrics.definition_index_ms,
            metrics.macro_index_ms,
            metrics.traversal_ms,
            metrics.elapsed_ms,
        )
        .unwrap();
    } else {
        clocks.push_str(UNAVAILABLE_METRICS);
    }
    clocks
}

pub(super) fn emit(
    ordinal: usize,
    input: &Input,
    outcome: &Result<NativeExtraction, NativeFailure>,
    worker_call_ms: f64,
) {
    let receipt = measure(true, outcome).unwrap();
    let clocks = phase_clocks(outcome);
    let (status, failure_phase, success_payload) = match outcome {
        Ok(_) => ("success", "none", "present"),
        Err(NativeFailure::Parse(_)) => ("error", "parse", "absent"),
        Err(NativeFailure::Traversal(_)) => ("error", "traversal", "absent"),
    };
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or_else(
            |error| format!("-{}", error.duration().as_millis()),
            |duration| duration.as_millis().to_string(),
        );
    let mut categories = String::new();
    for (index, (name, bytes)) in receipt.categories.iter().enumerate() {
        if index != 0 {
            categories.push(';');
        }
        write!(
            categories,
            "{name}:{},{},{},{},{}",
            bytes.logical,
            bytes.capacity,
            bytes.map_entries,
            bytes.map_payload_estimate,
            bytes.hash_capacity_estimate,
        )
        .unwrap();
    }
    let logical: usize = receipt
        .categories
        .iter()
        .map(|(_, bytes)| bytes.logical)
        .sum();
    let capacity: usize = receipt
        .categories
        .iter()
        .map(|(_, bytes)| bytes.capacity)
        .sum();
    eprintln!(
        "windows-clang timing phase=owned-tu-complete schema=1 timestamp_unix_ms={timestamp} \
         input_ordinal={ordinal} tu={:?} status={status} failure_phase={failure_phase} \
         worker_call_ms={worker_call_ms:.3} {clocks} \
         success_payload={success_payload} logical_buffer_bytes={logical} capacity_buffer_bytes={capacity} \
         outcome_inline_layout_bytes={} input_source_logical_bytes={} input_source_capacity_bytes={} \
         macro_handles={} unique_macro_payloads={} \
         category_columns=logical_buffer_bytes,capacity_buffer_bytes,map_entries,map_payload_estimate_bytes,hash_capacity_payload_estimate_bytes \
         categories={categories} map_node_overhead=unknown hash_control_overhead=unknown \
         arc_control_overhead=unknown allocator_overhead=unknown native_bytes=unmeasured \
         process_bytes=unmeasured outer_input_context_bytes=unmeasured \
         scheduler_container_bytes=unmeasured diagnostic_temporary_bytes=unmeasured",
        input.name,
        size_of::<Result<NativeExtraction, NativeFailure>>(),
        input.source.len(),
        input.source.capacity(),
        receipt.macro_handles,
        receipt.unique_macros,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_bytes_phase_clocks_preserve_available_and_unavailable_measurements() {
        for failure in [
            NativeFailure::Parse(Error("parse".to_string())),
            NativeFailure::Traversal(Error("traversal".to_string())),
        ] {
            let clocks = phase_clocks(&Err(failure));
            assert!(clocks.starts_with("parse_ms=unavailable success_metrics=unavailable "));
            for field in [
                "definition_index_ms",
                "macro_index_ms",
                "traversal_ms",
                "pre_sal_binding_elapsed_ms",
            ] {
                assert!(clocks.contains(&format!("{field}=unavailable")));
            }
            assert!(!clocks.contains("=0"));
        }
        let mut observed = outcome(vec![Arc::new(OwnedMacro {
            tokens: vec!["token".to_string()],
            function_like: false,
        })]);
        let Ok(result) = &mut observed else {
            panic!();
        };
        result.parse_ms = 2.5;
        assert_eq!(
            phase_clocks(&observed),
            "parse_ms=2.500 success_metrics=unavailable definition_index_ms=unavailable \
             macro_index_ms=unavailable traversal_ms=unavailable pre_sal_binding_elapsed_ms=unavailable",
        );
        let Ok(result) = &mut observed else {
            panic!();
        };
        result.metrics = Some(ExtractionMetrics {
            definition_observations: 0,
            definition_lookups: 0,
            definition_index_ms: 1.25,
            macro_definitions: 0,
            macro_expansion_files: 0,
            cursors: 0,
            facts: 0,
            constants: 0,
            root_cache_misses: 0,
            exclusion_cache_misses: 0,
            macro_index_ms: 0.5,
            traversal_ms: 3.75,
            elapsed_ms: 6.0,
        });
        let before = measure(true, &observed);
        assert_eq!(
            phase_clocks(&observed),
            "parse_ms=2.500 success_metrics=present definition_index_ms=1.250 \
             macro_index_ms=0.500 traversal_ms=3.750 pre_sal_binding_elapsed_ms=6.000",
        );
        assert_eq!(measure(true, &observed), before);
    }

    fn outcome(macros: Vec<Arc<OwnedMacro>>) -> Result<NativeExtraction, NativeFailure> {
        let captured = macros[0].clone();
        Ok(NativeExtraction {
            output: ExtractionBuffers::default(),
            extracted: Extracted {
                macros: FinalMacros(
                    macros
                        .into_iter()
                        .enumerate()
                        .map(|(index, value)| (index.to_string(), value))
                        .collect(),
                ),
                diagnostics: vec![],
                pending_structs: vec![],
                pending_macros: vec![(0, captured.clone())],
                root_macros: vec![(1, captured)],
                declare_handle_expansions: vec![],
                next_local: 0,
            },
            included_files: vec![],
            parse_ms: 0.0,
            metrics: None,
        })
    }

    #[test]
    fn owned_bytes_counts_capacity_without_inline_header_duplication() {
        let mut value = String::with_capacity(37);
        value.push_str("known");
        let mut values = Vec::with_capacity(5);
        values.push(value);
        let mut bytes = Bytes::default();
        bytes.tokens(&values);
        assert_eq!(bytes.logical, size_of::<String>() + 5);
        assert_eq!(
            bytes.capacity,
            5 * size_of::<String>() + values[0].capacity()
        );
        let boxed = Box::new(TypeRef::Pointer {
            mutable: false,
            target: Box::new(TypeRef::Scalar(Scalar::I32)),
        });
        let mut bytes = Bytes::default();
        bytes.boxed(boxed.as_ref());
        bytes.ty(&boxed);
        assert_eq!(bytes.logical, 2 * size_of::<TypeRef>());
        assert_eq!(bytes.capacity, bytes.logical);
    }

    #[test]
    fn owned_bytes_shared_and_equal_distinct_macros() {
        let shared = Arc::new(OwnedMacro {
            tokens: vec!["same".to_string()],
            function_like: false,
        });
        let shared_outcome = outcome(vec![shared.clone(), shared.clone()]);
        let before = Arc::strong_count(&shared);
        assert_eq!(measure(false, &shared_outcome), None);
        let shared_receipt = measure(true, &shared_outcome).unwrap();
        assert_eq!(Arc::strong_count(&shared), before);
        assert_eq!(shared_receipt.macro_handles, 4);
        assert_eq!(shared_receipt.unique_macros, 1);
        let distinct = Arc::new((*shared).clone());
        let distinct_outcome = outcome(vec![shared, distinct]);
        let distinct_receipt = measure(true, &distinct_outcome).unwrap();
        assert_eq!(distinct_receipt.unique_macros, 2);
        let payload = |receipt: &Receipt| {
            let bytes = &receipt
                .categories
                .iter()
                .find(|(name, _)| *name == "macro_payloads")
                .unwrap()
                .1;
            (bytes.logical, bytes.capacity)
        };
        assert_eq!(
            payload(&distinct_receipt),
            (
                payload(&shared_receipt).0 * 2,
                payload(&shared_receipt).1 * 2,
            )
        );
    }

    #[test]
    fn owned_bytes_off_gate_and_failure_are_non_mutating() {
        let error = Err(NativeFailure::Parse(Error(String::with_capacity(64))));
        assert_eq!(measure(false, &error), None);
        let before = match &error {
            Err(NativeFailure::Parse(error)) => error.0.capacity(),
            _ => panic!(),
        };
        let receipt = measure(true, &error).unwrap();
        assert_eq!(receipt.categories[0].1.logical, 0);
        assert_eq!(receipt.categories[0].1.capacity, before);
        assert_eq!(measure(true, &error), Some(receipt));
    }

    #[test]
    fn owned_bytes_recursive_fields_annotations_and_all_categories() {
        let mut observed = outcome(vec![Arc::new(OwnedMacro {
            tokens: vec![],
            function_like: true,
        })]);
        let Ok(result) = &mut observed else {
            panic!();
        };
        let origin = Origin {
            tu: "unit.hpp".to_string(),
            local: 3,
        };
        result.output.annotations.insert(
            AnnotationTarget::NestedField {
                declaration: origin.clone(),
                path: vec![1, 2],
            },
            vec![Annotation::ArrayCountConst("length".to_string())],
        );
        result.output.sal_constant_sizes.insert(
            AnnotationTarget::Declaration(origin),
            Ok(SalSize {
                bytes: true,
                value: SalSizeValue::Expression("length * 2".to_string()),
            }),
        );
        result.extracted.pending_structs.push((
            0,
            FactData::Record {
                base: None,
                fields: vec![Field {
                    name: "field".to_string(),
                    ty: TypeRef::Pointer {
                        mutable: false,
                        target: Box::new(TypeRef::Named {
                            name: "Target".to_string(),
                            declaration: Location {
                                file: "target.hpp".to_string(),
                                offset: 4,
                            },
                        }),
                    },
                    offset: 0,
                    align: 8,
                    size: 8,
                    bit_width: None,
                }],
                size: 8,
                align: 8,
                packing: None,
                alignment: None,
                union: false,
            },
        ));
        let before = format!(
            "{:?}{:?}{:?}",
            result.output.annotations,
            result.output.sal_constant_sizes,
            result.extracted.pending_structs,
        );
        let receipt = measure(true, &observed).unwrap();
        assert_eq!(receipt.categories.len(), 15);
        let annotations = &receipt
            .categories
            .iter()
            .find(|(name, _)| *name == "annotations")
            .unwrap()
            .1;
        assert_eq!(
            annotations.logical,
            8 + 2 * size_of::<usize>() + size_of::<Annotation>() + 6
        );
        assert_eq!(
            annotations.map_payload_estimate,
            size_of::<(AnnotationTarget, Vec<Annotation>)>(),
        );
        let sal = &receipt
            .categories
            .iter()
            .find(|(name, _)| *name == "sal")
            .unwrap()
            .1;
        assert_eq!(sal.logical, 8 + 10);
        let records = &receipt
            .categories
            .iter()
            .find(|(name, _)| *name == "deferred_records")
            .unwrap()
            .1;
        assert_eq!(
            records.logical,
            size_of::<(usize, FactData)>() + size_of::<Field>() + size_of::<TypeRef>() + 5 + 6 + 10,
        );
        let Ok(result) = &observed else {
            panic!();
        };
        assert_eq!(
            before,
            format!(
                "{:?}{:?}{:?}",
                result.output.annotations,
                result.output.sal_constant_sizes,
                result.extracted.pending_structs,
            )
        );
    }
}
