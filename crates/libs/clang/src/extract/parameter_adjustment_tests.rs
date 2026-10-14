use super::*;

const ARGS: &[&str] = &[
    "--no-default-config",
    "-x",
    "c++",
    "-std=c++17",
    "--target=x86_64-pc-windows-msvc",
    "-nostdinc",
];

pub(super) fn observe(
    method: CXCursor,
    parameter: CXCursor,
    index: usize,
    actual: Option<&NativeType>,
    unsupported: Option<(&'static str, CXCursor)>,
) {
    let tu = unsafe { clang_Cursor_getTranslationUnit(method) };
    let name = cx_string(unsafe { clang_getTranslationUnitSpelling(tu) });
    if !name.starts_with("parameter-probe-") {
        return;
    }
    let owner = native_types::declaration(method).unwrap();
    let declaration = native_types::declaration(parameter).unwrap();
    let function = unsafe { clang_getCursorType(method) };
    let arity = unsafe { clang_getNumArgTypes(function) };
    let parameters: Vec<_> = cursor_children(method)
        .into_iter()
        .filter(|cursor| unsafe { clang_getCursorKind(*cursor) } == CXCursor_ParmDecl)
        .collect();
    assert_eq!(usize::try_from(arity).unwrap(), parameters.len());
    assert!(unsafe { clang_equalCursors(parameters[index], parameter) } != 0);
    assert!(unsafe { clang_equalCursors(clang_getCursorSemanticParent(parameter), method) } != 0);
    assert_eq!(unsafe { clang_Cursor_getTranslationUnit(parameter) }, tu);
    let raw = unsafe { clang_getCursorType(parameter) };
    let adjusted = unsafe { clang_getArgType(function, u32::try_from(index).unwrap()) };
    let raw_canonical = unsafe { clang_getCanonicalType(raw) };
    let adjusted_canonical = unsafe { clang_getCanonicalType(adjusted) };
    let attributes: Vec<_> = cursor_children(parameter)
        .into_iter()
        .filter(|child| unsafe { clang_isAttribute(clang_getCursorKind(*child)) } != 0)
        .map(|child| {
            (
                unsafe { clang_getCursorKind(child) },
                cx_string(unsafe { clang_getCursorSpelling(child) }),
                cursor_locations(child),
            )
        })
        .collect();
    let mut capture = native_types::Capture::for_arguments(ARGS);
    let (raw_native, raw_rejection) = if let Some((reason, witness)) = unsupported {
        capture.rejected_diagnostic(raw, reason, witness)
    } else {
        capture.capture_diagnostic(raw)
    };
    let (adjusted_native, adjusted_rejection) = capture.capture_diagnostic(adjusted);
    canonical_function_observation(method, parameter, index, function, raw, &mut capture);
    if unsupported.is_none()
        && matches!(
            raw_canonical.kind,
            CXType_IncompleteArray | CXType_ConstantArray
        )
    {
        if let Some(actual) = actual {
            assert_eq!(actual.kind, CXType_Pointer);
        }
        if raw_canonical.kind == CXType_IncompleteArray {
            assert!(raw_native.is_none());
            assert!(
                raw_rejection
                    .as_ref()
                    .unwrap()
                    .contains("unsupported-canonical-kind")
            );
        } else {
            assert!(matches!(
                raw_native.as_ref().unwrap().layer,
                NativeTypeLayer::Array { .. }
            ));
        }
    } else {
        assert_eq!(raw_native.is_some(), actual.is_some());
    }
    eprintln!(
        "PARAMETER_NATIVE input={name:?} owner={owner:?} arity={arity} index={index} \
         parameter={declaration:?} exact_order=true same_tu=true raw_kind={} raw_spelling={:?} \
         raw_canonical_kind={} raw_canonical_spelling={:?} raw_const={} raw_volatile={} \
         raw_restrict={} attributes={attributes:?} unsupported={:?} adjusted_kind={} \
         adjusted_spelling={:?} adjusted_canonical_kind={} adjusted_canonical_spelling={:?} \
         actual_present={} raw_present={} raw_rejection={raw_rejection:?} \
         adjusted_present={} adjusted_rejection={adjusted_rejection:?}",
        raw.kind,
        cx_string(unsafe { clang_getTypeSpelling(raw) }),
        raw_canonical.kind,
        cx_string(unsafe { clang_getTypeSpelling(raw_canonical) }),
        unsafe { clang_isConstQualifiedType(raw_canonical) },
        unsafe { clang_isVolatileQualifiedType(raw_canonical) },
        unsafe { clang_isRestrictQualifiedType(raw_canonical) },
        unsupported.map(|(reason, _)| reason),
        adjusted.kind,
        cx_string(unsafe { clang_getTypeSpelling(adjusted) }),
        adjusted_canonical.kind,
        cx_string(unsafe { clang_getTypeSpelling(adjusted_canonical) }),
        actual.is_some(),
        raw_native.is_some(),
        adjusted_native.is_some(),
    );
}

fn canonical_function_observation(
    method: CXCursor,
    parameter: CXCursor,
    index: usize,
    function: CXType,
    raw: CXType,
    capture: &mut native_types::Capture,
) {
    let tu = unsafe { clang_Cursor_getTranslationUnit(method) };
    let canonical_function = unsafe { clang_getCanonicalType(function) };
    assert_eq!(canonical_function.kind, CXType_FunctionProto);
    assert_eq!(unsafe { clang_getNumArgTypes(function) }, unsafe {
        clang_getNumArgTypes(canonical_function)
    });
    native_types::function_transport(function, native_types::register_policy(ARGS)).unwrap();
    let argument = unsafe { clang_getArgType(canonical_function, u32::try_from(index).unwrap()) };
    let canonical_argument = unsafe { clang_getCanonicalType(argument) };
    let source = unsafe { clang_getCanonicalType(raw) };
    let target = unsafe { clang_getTranslationUnitTargetInfo(tu) };
    assert!(!target.is_null());
    let triple = cx_string(unsafe { clang_TargetInfo_getTriple(target) });
    let width = unsafe { clang_TargetInfo_getPointerWidth(target) };
    unsafe { clang_TargetInfo_dispose(target) };
    native_types::target_policy(&triple, width).unwrap();
    let (native, rejection) = capture.capture_diagnostic(argument);
    let name = cx_string(unsafe { clang_getCursorSpelling(method) });
    if name == "Elements" && index == 0 {
        for cursor in cursor_children(unsafe { clang_getTranslationUnitCursor(tu) }) {
            if cx_string(unsafe { clang_getCursorSpelling(cursor) }) == "noncallable_incomplete" {
                assert_eq!(unsafe { clang_Cursor_getTranslationUnit(cursor) }, tu);
                let ty = unsafe { clang_getCursorType(cursor) };
                assert_eq!(
                    unsafe { clang_getCanonicalType(ty) }.kind,
                    CXType_IncompleteArray
                );
                let (native, rejection) = capture.capture_diagnostic(ty);
                assert!(native.is_none());
                assert!(
                    rejection
                        .as_ref()
                        .unwrap()
                        .contains("unsupported-canonical-kind")
                );
                eprintln!(
                    "NONCALLABLE_INCOMPLETE declaration={:?} rejection={rejection:?}",
                    native_types::declaration(cursor).unwrap()
                );
            }
        }
    }
    let mut array_proof = None;
    if matches!(source.kind, CXType_ConstantArray | CXType_IncompleteArray) {
        let parent = unsafe { clang_getCursorSemanticParent(method) };
        let controls: Vec<_> = cursor_children(parent)
            .into_iter()
            .filter(|cursor| {
                (unsafe { clang_getCursorKind(*cursor) }) == CXCursor_CXXMethod
                    && cx_string(unsafe { clang_getCursorSpelling(*cursor) })
                        == format!("Pointer{name}")
            })
            .collect();
        assert_eq!(controls.len(), 1);
        let control = controls[0];
        assert_eq!(unsafe { clang_Cursor_getTranslationUnit(control) }, tu);
        assert!(unsafe { clang_equalCursors(clang_getCursorSemanticParent(control), parent) } != 0);
        let control_parameters: Vec<_> = cursor_children(control)
            .into_iter()
            .filter(|cursor| unsafe { clang_getCursorKind(*cursor) } == CXCursor_ParmDecl)
            .collect();
        assert_eq!(
            control_parameters.len(),
            usize::try_from(unsafe { clang_getNumArgTypes(function) }).unwrap()
        );
        let control_parameter = control_parameters[index];
        assert_eq!(
            unsafe { clang_Cursor_getTranslationUnit(control_parameter) },
            tu
        );
        let control_type = unsafe { clang_getCursorType(control_parameter) };
        let control_canonical = unsafe { clang_getCanonicalType(control_type) };
        assert_eq!(control_canonical.kind, CXType_Pointer);
        let (projected, source_array) = function_param_view(raw).unwrap();
        let source_array = source_array.unwrap();
        let element = unsafe { clang_getArrayElementType(source_array) };
        let canonical_container_element = unsafe { clang_getArrayElementType(source) };
        let control_pointee = unsafe { clang_getPointeeType(control_canonical) };
        let control_equal = unsafe { clang_equalTypes(element, control_pointee) } != 0;
        let element = unsafe { clang_getCanonicalType(element) };
        let control_pointee = unsafe { clang_getCanonicalType(control_pointee) };
        let canonical_control_equal = unsafe { clang_equalTypes(element, control_pointee) } != 0;
        let control_match =
            native_types::array_parameter_element_match(projected, source_array, control_pointee);
        let context_parameters: Vec<_> = cursor_children(method)
            .into_iter()
            .filter(|cursor| unsafe { clang_getCursorKind(*cursor) } == CXCursor_ParmDecl)
            .collect();
        let (context_native, context_rejection) =
            capture.capture_parameter(method, &context_parameters, index, width, true);
        let control_size = unsafe { clang_Type_getSizeOf(control_canonical) };
        let control_align = unsafe { clang_Type_getAlignOf(control_canonical) };
        assert_eq!(control_size * 8, i64::from(width));
        assert!(control_align > 0);
        assert_eq!(unsafe { clang_getAddressSpace(control_canonical) }, 0);
        let pointer = canonical_argument.kind == CXType_Pointer;
        let element_match =
            native_types::array_parameter_element_match(projected, source_array, unsafe {
                clang_getPointeeType(canonical_argument)
            });
        let element_equal = pointer && element_match.is_ok();
        let size = unsafe { clang_Type_getSizeOf(canonical_argument) };
        let align = unsafe { clang_Type_getAlignOf(canonical_argument) };
        let address_space = unsafe { clang_getAddressSpace(canonical_argument) };
        let proved = pointer
            && element_equal
            && control_match.is_ok()
            && size == control_size
            && align == control_align
            && address_space == 0
            && native.is_some();
        assert!(proved);
        if name == "Const" || name == "InnerTwo" {
            for (candidate, reason) in if name == "Const" {
                vec![
                    (
                        "PointerPlain",
                        "unproved-native-array-parameter-qualification",
                    ),
                    ("PointerOther", "unproved-native-array-parameter-element"),
                ]
            } else {
                vec![(
                    "PointerInnerThree",
                    "unproved-native-array-parameter-element",
                )]
            } {
                let candidate = cursor_children(parent)
                    .into_iter()
                    .find(|cursor| {
                        cx_string(unsafe { clang_getCursorSpelling(*cursor) }) == candidate
                    })
                    .unwrap();
                assert_eq!(unsafe { clang_Cursor_getTranslationUnit(candidate) }, tu);
                let argument = unsafe {
                    clang_getArgType(
                        clang_getCanonicalType(clang_getCursorType(candidate)),
                        if name == "Const" { 0 } else { 1 },
                    )
                };
                let pointee = unsafe { clang_getPointeeType(argument) };
                let result =
                    native_types::array_parameter_element_match(projected, source_array, pointee);
                assert_eq!(result, Err(reason));
                eprintln!(
                    "ARRAY_ELEMENT_NEGATIVE method={name:?} candidate={:?} \
                    pointee={:?} result={result:?}",
                    native_types::declaration(candidate).unwrap(),
                    cx_string(unsafe { clang_getTypeSpelling(pointee) })
                );
            }
        }
        if name == "Elements" && index == 1 {
            let parameters: Vec<_> = cursor_children(method)
                .into_iter()
                .filter(|cursor| unsafe { clang_getCursorKind(*cursor) } == CXCursor_ParmDecl)
                .collect();
            let mut wrong_order = parameters.clone();
            wrong_order.swap(1, 3);
            for (case, owner, parameters, ordinal, width, reason) in [
                (
                    "owner",
                    control,
                    parameters.as_slice(),
                    index,
                    width,
                    "unproved-native-array-parameter-owner",
                ),
                (
                    "order",
                    method,
                    wrong_order.as_slice(),
                    index,
                    width,
                    "unproved-native-array-parameter-owner",
                ),
                (
                    "arity",
                    method,
                    &parameters[..2],
                    index,
                    width,
                    "unproved-native-array-parameter-owner",
                ),
                (
                    "index",
                    method,
                    parameters.as_slice(),
                    parameters.len(),
                    width,
                    "unproved-native-array-parameter-index",
                ),
                (
                    "target",
                    method,
                    parameters.as_slice(),
                    index,
                    32,
                    "unproved-native-array-parameter-pointer",
                ),
                (
                    "null-owner",
                    unsafe { clang_getNullCursor() },
                    parameters.as_slice(),
                    index,
                    width,
                    "unproved-native-array-parameter-owner",
                ),
            ] {
                let (native, rejection) =
                    capture.capture_parameter(owner, parameters, ordinal, width, true);
                assert!(native.is_none(), "{case}");
                assert!(
                    rejection.as_ref().unwrap().contains(reason),
                    "{case}: {rejection:?}"
                );
                eprintln!("ARRAY_CONTEXT_NEGATIVE case={case} rejection={rejection:?}");
            }
        }
        array_proof = Some((
            proved,
            pointer,
            element_equal,
            size,
            align,
            address_space,
            control_size,
            control_align,
        ));
        eprintln!(
            "CANONICAL_ARRAY_CONTROL input={:?} method={:?} parameter={:?} index={index} \
             control={:?} control_parameter={:?} element_kind={} element_spelling={:?} \
             element_cv={:?} control_pointee_kind={} control_pointee_spelling={:?} \
             control_pointee_cv={:?} native_element_equal={control_equal} \
             canonical_native_element_equal={canonical_control_equal} \
             qualified_element_match={element_match:?} control_match={control_match:?} \
             original_outer_const={} projected_outer_const={} source_array={:?} \
             context_native_present={} context_rejection={context_rejection:?} \
             canonical_container_element_kind={} canonical_container_element={:?} \
             canonical_container_element_const={} \
             proof_tuple={array_proof:?}",
            cx_string(unsafe { clang_getTranslationUnitSpelling(tu) }),
            native_types::declaration(method).unwrap(),
            native_types::declaration(parameter).unwrap(),
            native_types::declaration(control).unwrap(),
            native_types::declaration(control_parameter).unwrap(),
            element.kind,
            cx_string(unsafe { clang_getTypeSpelling(element) }),
            (
                unsafe { clang_isConstQualifiedType(element) },
                unsafe { clang_isVolatileQualifiedType(element) },
                unsafe { clang_isRestrictQualifiedType(element) }
            ),
            control_pointee.kind,
            cx_string(unsafe { clang_getTypeSpelling(control_pointee) }),
            (
                unsafe { clang_isConstQualifiedType(control_pointee) },
                unsafe { clang_isVolatileQualifiedType(control_pointee) },
                unsafe { clang_isRestrictQualifiedType(control_pointee) }
            ),
            unsafe { clang_isConstQualifiedType(raw) },
            unsafe { clang_isConstQualifiedType(projected) },
            cx_string(unsafe { clang_getTypeSpelling(source_array) }),
            context_native.is_some(),
            canonical_container_element.kind,
            cx_string(unsafe { clang_getTypeSpelling(canonical_container_element) }),
            unsafe { clang_isConstQualifiedType(canonical_container_element) },
        );
    }
    eprintln!(
        "CANONICAL_FUNCTION_PARAMETER input={:?} method={name:?} index={index} \
         raw_function_kind={} raw_function={:?} canonical_function_kind={} \
         canonical_function={:?} source_kind={} source_spelling={:?} \
         canonical_function_argument_kind={} canonical_function_argument={:?} \
         argument_canonical_kind={} argument_canonical={:?} native_present={} \
         rejection={rejection:?} target={triple:?} pointer_width={width} \
         array_proof={array_proof:?}",
        cx_string(unsafe { clang_getTranslationUnitSpelling(tu) }),
        function.kind,
        cx_string(unsafe { clang_getTypeSpelling(function) }),
        canonical_function.kind,
        cx_string(unsafe { clang_getTypeSpelling(canonical_function) }),
        source.kind,
        cx_string(unsafe { clang_getTypeSpelling(source) }),
        argument.kind,
        cx_string(unsafe { clang_getTypeSpelling(argument) }),
        canonical_argument.kind,
        cx_string(unsafe { clang_getTypeSpelling(canonical_argument) }),
        native.is_some(),
    );
}

fn plan(case: &str, reversed: bool) -> HeaderPartitionPlan {
    let header = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("extract")
        .join("parameter_adjustment_tests.hpp");
    let header = header.to_str().unwrap();
    let source = |alternative| {
        format!(
            "{}#define PARAMETER_{case}\n#include {header:?}\n",
            if alternative {
                "#define PARAMETER_ALTERNATIVE\n"
            } else {
                ""
            },
        )
    };
    let mut inputs = vec![
        Input::new("parameter-probe-home.cpp", source(false))
            .with_roots([header])
            .partitioned("Home"),
        Input::new("parameter-probe-alternative.cpp", source(true))
            .with_roots([header])
            .partitioned("Alternative"),
    ];
    if reversed {
        inputs.reverse();
    }
    let policy = HeaderPartitionPolicy::new()
        .with_partition_authority("Home", 0, "parameter-probe-home.cpp")
        .with_partition_authority("Alternative", 1, "parameter-probe-alternative.cpp")
        .with_traversed_header(header, RootPartition::new("Home", "Test.Home"))
        .with_traversed_header(header, RootPartition::new("Alternative", "Test.Other"));
    extract_ranked_header_partitions(inputs, ARGS, &policy, &NamespaceAuthorities::new()).unwrap()
}

#[test]
fn parameter_adjustment_native_ranked_causal_probe() {
    helpers::ensure_libclang();
    let references = BTreeMap::new();
    let options = EmitOptions::new("Test.Common", &references);
    let mut reports = BTreeMap::new();
    for reversed in [false, true] {
        for (case, interface) in [
            ("ARRAYS", "ArrayProbe"),
            ("ALIASES", "AliasProbe"),
            ("POINTERS", "PointerProbe"),
            ("ATTRIBUTES", "AttributeProbe"),
        ] {
            let plan = plan(case, reversed);
            for input in [
                "parameter-probe-home.cpp",
                "parameter-probe-alternative.cpp",
            ] {
                let fact = plan
                    .snapshot()
                    .facts()
                    .iter()
                    .find(|fact| fact.origin.tu == input && fact.name == interface)
                    .unwrap();
                let contract = &plan.snapshot.native_com_contracts[&fact.origin];
                for (slot, method) in contract.slots.iter().enumerate() {
                    assert_eq!(method.params.len(), method.native_params.len());
                    eprintln!(
                        "PARAMETER_CONTRACT case={case} reversed={reversed} \
                         input={input} origin={:?} slot={slot} method={} \
                         native_params={:?} projected_params={:?}",
                        fact.origin,
                        method.name,
                        method
                            .native_params
                            .iter()
                            .map(Option::is_some)
                            .collect::<Vec<_>>(),
                        method.params,
                    );
                }
                if case == "POINTERS" {
                    assert!(contract.slots.iter().all(|method| {
                        method.native_result.is_some()
                            && method.native_params.iter().all(Option::is_some)
                    }));
                    assert!(contract.slots.last().unwrap().params[0].annotation.input);
                }
                if matches!(case, "ARRAYS" | "ALIASES") {
                    assert!(contract.slots.iter().all(|method| {
                        method.native_result.is_some()
                            && method.native_params.iter().all(Option::is_some)
                    }));
                    for method in &contract.slots {
                        if !method.name.starts_with("Pointer") {
                            let control = contract
                                .slots
                                .iter()
                                .find(|candidate| {
                                    candidate.name == format!("Pointer{}", method.name)
                                })
                                .unwrap();
                            assert_eq!(method.native_params, control.native_params);
                        }
                        if case == "ALIASES" {
                            let plain = contract.slots.iter().find(|m| m.name == "Plain").unwrap();
                            let constant =
                                contract.slots.iter().find(|m| m.name == "Const").unwrap();
                            assert!(plain.native_params[0].is_some());
                            assert!(constant.native_params[0].is_some());
                            assert!(matches!(
                                &constant.params[0].ty,
                                TypeRef::Pointer { mutable: false, .. }
                            ));
                        }
                    }
                }
            }
            match plan.emit_with_report(&options) {
                Ok(report) => {
                    eprintln!("PARAMETER_GUARD case={case} reversed={reversed} result=emitted");
                    assert_ne!(
                        case, "ATTRIBUTES",
                        "unexpected attribute admission: {report:?}"
                    );
                    for row in &report.definition_evidence {
                        assert_eq!(row.selected.input, "parameter-probe-home.cpp");
                        assert_eq!(row.selected.rank, Some(0));
                    }
                    let text = report.partitions.values().cloned().collect::<String>();
                    let element = report
                        .definition_evidence
                        .iter()
                        .find(|row| row.identity.name == "Element")
                        .unwrap();
                    assert!(matches!(
                        element.outcome,
                        DefinitionComparison::Different { .. }
                    ));
                    assert_eq!(text.matches(&element.message).count(), 1);
                    if case == "ARRAYS" {
                        let directory = std::env::temp_dir().join(format!(
                            "windows-clang-parameter-{}-{reversed}",
                            std::process::id()
                        ));
                        std::fs::create_dir(&directory).unwrap();
                        let file = directory.join("parameters.winmd");
                        windows_rdl::reader()
                            .input_texts(report.partitions.values())
                            .reference_default()
                            .output(&file)
                            .write()
                            .unwrap();
                        let metadata = windows_metadata::reader::Index::read(&file).unwrap();
                        let record = metadata.expect("Test.Home", "Element");
                        assert_eq!(
                            record.fields().next().unwrap().ty(),
                            windows_metadata::Type::I32
                        );
                        let interface = metadata.expect("Test.Home", "ArrayProbe");
                        let methods: Vec<_> = interface.methods().collect();
                        let elements = methods
                            .iter()
                            .find(|method| method.name() == "Elements")
                            .unwrap();
                        let signature = elements.signature(&[]);
                        assert_eq!(signature.return_type, windows_metadata::Type::I32);
                        assert_eq!(signature.types[0], windows_metadata::Type::U32);
                        assert_eq!(signature.types[2], windows_metadata::Type::U32);
                        let element_pointer = windows_metadata::Type::PtrConst(
                            Box::new(windows_metadata::Type::value_named("Test.Home", "Element")),
                            1,
                        );
                        assert_eq!(signature.types[1], element_pointer);
                        assert_eq!(signature.types[3], element_pointer);
                        for (name, count) in [("values", 0), ("more", 2)] {
                            let parameter = elements.params().find(|p| p.name() == name).unwrap();
                            assert_eq!(
                                parameter.buffer_relationship(),
                                Some(windows_metadata::reader::BufferRelationship::ElementsParam(
                                    count
                                ))
                            );
                        }
                        let fixed = methods
                            .iter()
                            .find(|method| method.name() == "Fixed")
                            .unwrap();
                        assert_eq!(fixed.signature(&[]).types[1], element_pointer);
                        let infos = methods
                            .iter()
                            .find(|method| method.name() == "Infos")
                            .unwrap();
                        assert_eq!(
                            infos.signature(&[]).types[1],
                            windows_metadata::Type::PtrMut(
                                Box::new(windows_metadata::Type::value_named("Test.Home", "Info")),
                                1
                            )
                        );
                        eprintln!("PARAMETER_RDL reversed={reversed} text={text:?}");
                        eprintln!(
                            "PARAMETER_WINMD reversed={reversed} file={file:?} signature={signature:?}"
                        );
                    }
                    if case == "ALIASES" {
                        let directory = std::env::temp_dir().join(format!(
                            "windows-clang-alias-{}-{reversed}",
                            std::process::id()
                        ));
                        std::fs::create_dir(&directory).unwrap();
                        let file = directory.join("aliases.winmd");
                        windows_rdl::reader()
                            .input_texts(report.partitions.values())
                            .reference_default()
                            .output(&file)
                            .write()
                            .unwrap();
                        let metadata = windows_metadata::reader::Index::read(&file).unwrap();
                        assert_eq!(
                            metadata
                                .expect("Test.Home", "Element")
                                .fields()
                                .next()
                                .unwrap()
                                .ty(),
                            windows_metadata::Type::I32
                        );
                        let interface = metadata.expect("Test.Home", "AliasProbe");
                        for (name, expected) in [
                            (
                                "Plain",
                                windows_metadata::Type::PtrMut(
                                    Box::new(windows_metadata::Type::value_named(
                                        "Test.Home",
                                        "Element",
                                    )),
                                    1,
                                ),
                            ),
                            (
                                "PointerPlain",
                                windows_metadata::Type::PtrMut(
                                    Box::new(windows_metadata::Type::value_named(
                                        "Test.Home",
                                        "Element",
                                    )),
                                    1,
                                ),
                            ),
                            (
                                "Const",
                                windows_metadata::Type::PtrConst(
                                    Box::new(windows_metadata::Type::value_named(
                                        "Test.Home",
                                        "Element",
                                    )),
                                    1,
                                ),
                            ),
                            (
                                "PointerConst",
                                windows_metadata::Type::PtrConst(
                                    Box::new(windows_metadata::Type::value_named(
                                        "Test.Home",
                                        "Element",
                                    )),
                                    1,
                                ),
                            ),
                        ] {
                            let method = interface.methods().find(|m| m.name() == name).unwrap();
                            let signature = method.signature(&[]);
                            assert_eq!(signature.return_type, windows_metadata::Type::I32);
                            assert_eq!(signature.types, [expected]);
                            eprintln!(
                                "ALIAS_WINMD_SIGNATURE reversed={reversed} name={name} \
                                signature={signature:?}"
                            );
                        }
                        eprintln!("ALIAS_RDL reversed={reversed} text={text:?}");
                        eprintln!("ALIAS_WINMD reversed={reversed} file={file:?}");
                    }
                    assert!(
                        report
                            .partitions
                            .values()
                            .any(|text| text.contains(&format!("interface {interface}")))
                    );
                    if let Some(previous) = reports.get(case) {
                        assert_eq!(&report, previous);
                    } else {
                        reports.insert(case, report);
                    }
                }
                Err(error) => {
                    let error = error.to_string();
                    assert_eq!(case, "ATTRIBUTES", "{error}");
                    assert!(error.contains("unproved callable evidence"), "{error}");
                    assert!(
                        error.contains(&format!("selected interface `{interface}`")),
                        "{error}"
                    );
                    assert!(
                        !error.contains("no captured native semantic-parent relation"),
                        "{error}"
                    );
                    assert!(!error.contains("suppressed or excluded"), "{error}");
                    eprintln!("PARAMETER_GUARD case={case} reversed={reversed} error={error}");
                }
            }
        }
    }
}
