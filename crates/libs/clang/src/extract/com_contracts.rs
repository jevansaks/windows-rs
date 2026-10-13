use super::*;

#[derive(Clone)]
struct Captured {
    contract: Arc<NativeComContract>,
    methods: Vec<CXCursor>,
}

struct Capture<'a> {
    cached: HashMap<u32, Vec<(CXCursor, Option<Captured>)>>,
    active: Vec<CXCursor>,
    pointer_width: i32,
    native_types: &'a mut native_types::Capture,
    origins: &'a HashMap<u32, Vec<(CXCursor, Origin)>>,
    target: Option<String>,
    rejected_parameters: usize,
    reported_parameters: usize,
    register_policy: Result<(), &'static str>,
}

pub(super) fn capture(
    traversal: &mut Traversal<'_>,
    output: &mut BTreeMap<Origin, Arc<NativeComContract>>,
    unproved: &mut BTreeSet<Origin>,
    args: &[&str],
) {
    let Some((_, cursor)) = traversal.pending_classes.first() else {
        return;
    };
    let target =
        unsafe { clang_getTranslationUnitTargetInfo(clang_Cursor_getTranslationUnit(*cursor)) };
    let timing = timings_enabled();
    let triple =
        (!target.is_null()).then(|| cx_string(unsafe { clang_TargetInfo_getTriple(target) }));
    let target_name = timing.then(|| triple.clone()).flatten();
    let mut phase = TimingPhase::new(target_name.as_deref(), "native-com-capture");
    let pointer_width = if target.is_null() {
        0
    } else {
        let width = unsafe { clang_TargetInfo_getPointerWidth(target) };
        unsafe { clang_TargetInfo_dispose(target) };
        width
    };
    let mut capture = Capture {
        pointer_width,
        cached: HashMap::new(),
        active: Vec::new(),
        native_types: &mut traversal.native_types,
        origins: &traversal.seen,
        target: target_name,
        rejected_parameters: 0,
        reported_parameters: 0,
        register_policy: native_types::register_policy(args).and_then(|()| {
            triple
                .as_deref()
                .ok_or("missing-native-target")
                .and_then(|triple| native_types::target_policy(triple, pointer_width))
        }),
    };
    for (origin, cursor) in &traversal.pending_classes {
        phase.visit(Some(traversal.pending_classes.len()));
        if let Some(captured) = capture.get(*cursor, traversal.macros) {
            output.insert(origin.clone(), captured.contract);
        } else {
            let definition = cursor_definition(*cursor);
            let children = cursor_children(definition);
            if unsafe { clang_getCursorKind(definition) } == CXCursor_StructDecl
                && children
                    .iter()
                    .any(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_FieldDecl)
                && (cursor_uuid(definition).is_some()
                    || cursor_uuid(unsafe { clang_getCanonicalCursor(definition) }).is_some())
                && children.iter().any(|child| unsafe {
                    match clang_getCursorKind(*child) {
                        CXCursor_CXXMethod => clang_CXXMethod_isVirtual(*child) != 0,
                        CXCursor_CXXBaseSpecifier => {
                            is_interface(clang_getTypeDeclaration(clang_getCursorType(*child)))
                        }
                        _ => false,
                    }
                })
            {
                unproved.insert(origin.clone());
            }
        }
    }
    phase.event("complete", None);
    if let Some(target) = &capture.target {
        eprintln!(
            "windows-clang timing phase=native-parameter-capture target={target} \
             tu={:?} event=rejection-summary rejected_parameters={} detailed_parameters={} \
             truncated_parameters={} diagnostic_limit=8",
            traversal.tu,
            capture.rejected_parameters,
            capture.reported_parameters,
            capture.rejected_parameters - capture.reported_parameters,
        );
    }
}

impl Capture<'_> {
    fn get(&mut self, cursor: CXCursor, macros: &MacroDefinitions) -> Option<Captured> {
        let cursor = cursor_definition(cursor);
        let hash = unsafe { clang_hashCursor(cursor) };
        if let Some((_, result)) = self
            .cached
            .get(&hash)
            .into_iter()
            .flatten()
            .find(|(candidate, _)| unsafe { clang_equalCursors(*candidate, cursor) != 0 })
        {
            return result.clone();
        }
        if self
            .active
            .iter()
            .any(|candidate| unsafe { clang_equalCursors(*candidate, cursor) != 0 })
        {
            return None;
        }
        self.active.push(cursor);
        let result = self.read(cursor, macros);
        self.active.pop();
        self.cached
            .entry(hash)
            .or_default()
            .push((cursor, result.clone()));
        result
    }

    fn read(&mut self, cursor: CXCursor, macros: &MacroDefinitions) -> Option<Captured> {
        if unsafe { clang_isCursorDefinition(cursor) } == 0
            || !unsafe { clang_Cursor_getModule(cursor) }.is_null()
        {
            return None;
        }
        let canonical = unsafe { clang_getCanonicalCursor(cursor) };
        let guid = match cursor_uuid(cursor).or_else(|| cursor_uuid(canonical)) {
            Some(guid) => guid,
            // An absent IID is evidence for an ordinary interface, not a COM provider key.
            None if is_interface(cursor) => String::new(),
            None => return None,
        };
        if cursor_uuid(canonical).is_some_and(|other| other != guid) {
            return None;
        }
        let children = cursor_children(cursor);
        let bases: Vec<_> = children
            .iter()
            .copied()
            .filter(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_CXXBaseSpecifier)
            .collect();
        let mut inherited = match bases.as_slice() {
            [] => Captured {
                contract: Arc::new(NativeComContract {
                    guid: guid.clone(),
                    has_fields: false,
                    pointer_width: self.pointer_width,
                    bases: Vec::new(),
                    slots: Vec::new(),
                }),
                methods: Vec::new(),
            },
            [base]
                if unsafe {
                    clang_isVirtualBase(*base) == 0
                        && clang_getCXXAccessSpecifier(*base) == CX_CXXPublic
                } =>
            {
                let declaration = unsafe { clang_getTypeDeclaration(clang_getCursorType(*base)) };
                if !is_interface(declaration) {
                    return None;
                }
                self.get(declaration, macros)?
            }
            _ => return None,
        };
        let mut slots = inherited.contract.slots.clone();
        let mut base_guids = inherited.contract.bases.clone();
        if !bases.is_empty() {
            base_guids.push(inherited.contract.guid.clone());
        }
        for child in children {
            match unsafe { clang_getCursorKind(child) } {
                CXCursor_Constructor | CXCursor_Destructor | CXCursor_ConversionFunction => {
                    return None;
                }
                CXCursor_CXXMethod if unsafe { clang_CXXMethod_isVirtual(child) } != 0 => {
                    let function = unsafe { clang_getCursorType(child) };
                    let canonical_function = unsafe { clang_getCanonicalType(function) };
                    let convention =
                        unsafe { clang_getFunctionTypeCallingConv(canonical_function) };
                    let exception = unsafe { clang_getCursorExceptionSpecificationType(child) };
                    if unsafe { clang_Cursor_isVariadic(child) } != 0
                        || unsafe { clang_CXXMethod_isConst(child) } != 0
                        || unsafe { clang_Type_getCXXRefQualifier(canonical_function) }
                            != CXRefQualifier_None
                        || !match self.pointer_width {
                            32 => convention == CXCallingConv_X86StdCall,
                            64 => matches!(convention, CXCallingConv_C | CXCallingConv_Win64),
                            _ => false,
                        }
                    {
                        return None;
                    }
                    let transport_error =
                        native_types::function_transport(function, self.register_policy)
                            .err()
                            .map(|reason| (reason, child))
                            .or_else(|| {
                                native_types::unsupported_declaration_attribute(child).map(
                                    |attribute| ("unsupported-native-method-attribute", attribute),
                                )
                            });
                    let native_result = transport_error
                        .is_none()
                        .then(|| {
                            self.native_types
                                .capture(unsafe { clang_getCursorResultType(child) })
                        })
                        .flatten();
                    let mut rejections = Vec::new();
                    let native_params = cursor_children(child)
                        .into_iter()
                        .filter(|param| unsafe { clang_getCursorKind(*param) == CXCursor_ParmDecl })
                        .enumerate()
                        .map(|(parameter, param)| {
                            let ty = unsafe { clang_getCursorType(param) };
                            let unsupported = transport_error.or_else(|| {
                                native_types::unsupported_declaration_attribute(param).map(
                                    |attribute| {
                                        ("unsupported-native-parameter-attribute", attribute)
                                    },
                                )
                            });
                            let (native, rejection) = if let Some((reason, witness)) = unsupported {
                                if self.target.is_some() && self.rejected_parameters < 8 {
                                    self.native_types.rejected_diagnostic(ty, reason, witness)
                                } else {
                                    (None, None)
                                }
                            } else if self.target.is_some() && self.rejected_parameters < 8 {
                                self.native_types.capture_diagnostic(ty)
                            } else {
                                (self.native_types.capture(ty), None)
                            };
                            if self.target.is_some() && native.is_none() {
                                self.rejected_parameters += 1;
                                if let Some(reason) = rejection {
                                    rejections.push((parameter, param, reason));
                                }
                            }
                            native
                        })
                        .collect();
                    let method = NativeComMethod {
                        name: cx_string(unsafe { clang_getCursorSpelling(child) }),
                        convention,
                        params: callable_params(child, macros, false).ok()?,
                        result: type_ref(unsafe { clang_getCursorResultType(child) })?,
                        native_params,
                        native_result,
                        native_exception: (exception >= 0).then_some(exception),
                    };
                    let mut overridden = std::ptr::null_mut();
                    let mut count = 0;
                    unsafe { clang_getOverriddenCursors(child, &mut overridden, &mut count) };
                    let targets = if count == 0 {
                        Vec::new()
                    } else {
                        unsafe { std::slice::from_raw_parts(overridden, count as usize) }.to_vec()
                    };
                    unsafe { clang_disposeOverriddenCursors(overridden) };
                    let slot = match targets.as_slice() {
                        [] => {
                            let index = slots.len();
                            inherited.methods.push(child);
                            slots.push(method);
                            index
                        }
                        [target] => {
                            let canonical = unsafe { clang_getCanonicalCursor(*target) };
                            let positions: Vec<_> = inherited
                                .methods
                                .iter()
                                .enumerate()
                                .filter(|(_, candidate)| unsafe {
                                    clang_equalCursors(
                                        clang_getCanonicalCursor(**candidate),
                                        canonical,
                                    ) != 0
                                })
                                .map(|(index, _)| index)
                                .collect();
                            let [index] = positions.as_slice() else {
                                return None;
                            };
                            let previous = &slots[*index];
                            if previous.convention != method.convention
                                || previous.name != method.name
                                || !same_signature(inherited.methods[*index], child)
                            {
                                return None;
                            }

                            inherited.methods[*index] = child;
                            slots[*index] = method;
                            *index
                        }
                        _ => return None,
                    };
                    for (parameter, param, reason) in rejections {
                        let parameter_type = unsafe { clang_getCursorType(param) };
                        let parameter_canonical = unsafe { clang_getCanonicalType(parameter_type) };
                        let origin = self.origins
                            .get(&unsafe { clang_hashCursor(cursor) })
                            .into_iter()
                            .flatten()
                            .find(|(candidate, _)| unsafe { clang_equalCursors(*candidate, cursor) } != 0)
                            .map(|(_, origin)| origin);
                        eprintln!(
                            "windows-clang timing phase=native-parameter-capture target={} \
                             event=rejected origin={origin:?} interface={:?} interface_source={:?} \
                             slot={slot} method={:?} parameter={parameter} parameter_source={:?} \
                             parameter_type_spelling={:?} parameter_canonical_kind={} \
                             parameter_const={} parameter_volatile={} parameter_restrict={} {reason}",
                            self.target.as_deref().unwrap(),
                            cx_string(unsafe { clang_getCursorSpelling(cursor) }),
                            cursor_locations(cursor),
                            slots[slot].name,
                            cursor_locations(param),
                            cx_string(unsafe { clang_getTypeSpelling(parameter_type) }),
                            parameter_canonical.kind,
                            unsafe { clang_isConstQualifiedType(parameter_canonical) },
                            unsafe { clang_isVolatileQualifiedType(parameter_canonical) },
                            unsafe { clang_isRestrictQualifiedType(parameter_canonical) },
                        );
                        self.reported_parameters += 1;
                    }
                }
                _ => {}
            }
        }
        if slots.is_empty() {
            return None;
        }
        Some(Captured {
            contract: Arc::new(NativeComContract {
                guid,
                pointer_width: self.pointer_width,
                has_fields: cursor_children(cursor)
                    .iter()
                    .any(|child| unsafe { clang_getCursorKind(*child) } == CXCursor_FieldDecl),
                bases: base_guids,
                slots,
            }),
            methods: inherited.methods,
        })
    }
}

fn same_signature(left: CXCursor, right: CXCursor) -> bool {
    unsafe {
        let left_ty = clang_getCursorType(left);
        let right_ty = clang_getCursorType(right);
        let count = clang_getNumArgTypes(left_ty);
        count >= 0
            && count == clang_getNumArgTypes(right_ty)
            && clang_equalTypes(
                clang_getCanonicalType(clang_getCursorResultType(left)),
                clang_getCanonicalType(clang_getCursorResultType(right)),
            ) != 0
            && (0..count).all(|index| {
                clang_equalTypes(
                    clang_getCanonicalType(clang_getArgType(left_ty, index as u32)),
                    clang_getCanonicalType(clang_getArgType(right_ty, index as u32)),
                ) != 0
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_com_parameter_rejection_diagnostics_preserve_evidence() {
        const CHILD: &str = "WINDOWS_CLANG_COM_CAPTURE_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            helpers::ensure_libclang();
            let input = Input::new(
                "capture-diagnostics.hpp",
                r#"
                struct __declspec(uuid("46aeb9ad-3d5e-497b-8ccd-0479af1ec3cc")) RejectedValue {
                    int payload;
                };
                typedef const volatile int *const volatile __restrict QualifiedPointer;
                typedef const volatile RejectedValue *const volatile __restrict RejectedPointer;
                struct CaptureBase {
                    virtual void __stdcall Keep(int prefix) = 0;
                };
                struct CaptureProbe : CaptureBase {
                    virtual void __stdcall Keep(int prefix) override = 0;
                    virtual void __stdcall Reject(QualifiedPointer keep, RejectedPointer value) = 0;
                };
                "#,
            );
            let snapshot = extract(
                [input],
                &["-x", "c++", "-std=c++20", "--target=x86_64-pc-windows-msvc"],
            )
            .unwrap();
            let fact = snapshot
                .facts
                .iter()
                .find(|fact| fact.name == "CaptureProbe")
                .unwrap();
            let contract = &snapshot.native_com_contracts[&fact.origin];
            assert_eq!(contract.slots.len(), 2);
            assert_eq!(contract.slots[0].name, "Keep");
            assert_eq!(contract.slots[1].name, "Reject");
            let params = &contract.slots[1].native_params;
            assert_eq!(params.len(), 2);
            assert!(params[1].is_none());
            let native = params[0].as_ref().unwrap();
            assert_eq!(native.kind, CXType_Pointer);
            assert!(native.is_const && native.is_volatile && native.is_restrict);
            println!("\nobserved-origin={:?}", fact.origin);
            println!("observed-source={:?}", fact.spelling);
            println!("observed-contract={contract:?}");
            return;
        }
        let test = concat!(
            module_path!(),
            "::native_com_parameter_rejection_diagnostics_preserve_evidence"
        );
        let mut baseline = None;
        for enabled in [false, true] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    test.split_once("::").unwrap().1,
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env_remove("WINDOWS_CLANG_TIMINGS")
                .env_remove("WINDOWS_CLANG_TIMING");
            if enabled {
                child.env("WINDOWS_CLANG_TIMINGS", "1");
            }
            let output = child.output().unwrap();
            let stdout = String::from_utf8(output.stdout).unwrap();
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(output.status.success(), "{stdout}\n{stderr}");
            assert!(
                stdout.contains("1 passed; 0 failed; 0 ignored;"),
                "{stdout}"
            );
            let observations: Vec<_> = stdout
                .lines()
                .filter(|line| line.starts_with("observed-"))
                .map(str::to_string)
                .collect();
            assert_eq!(observations.len(), 3, "{stdout}");
            if let Some(baseline) = &baseline {
                assert_eq!(&observations, baseline);
            } else {
                baseline = Some(observations.clone());
            }
            if !enabled {
                assert!(!stderr.contains("windows-clang timing "), "{stderr}");
                continue;
            }
            let records: Vec<_> = stderr
                .lines()
                .filter(|line| {
                    line.contains("phase=native-parameter-capture")
                        && line.contains("event=rejected ")
                })
                .collect();
            assert_eq!(records.len(), 1, "{stderr}");
            let record = records[0];
            assert!(
                record.contains(observations[0].strip_prefix("observed-origin=").unwrap()),
                "{record}"
            );
            assert!(
                record.contains(observations[1].strip_prefix("observed-source=").unwrap()),
                "{record}"
            );
            for field in [
                "interface=\"CaptureProbe\"",
                "slot=1 method=\"Reject\" parameter=1",
                "parameter_source=Some(",
                "parameter_type_spelling=\"RejectedPointer\"",
                "parameter_const=1 parameter_volatile=1 parameter_restrict=1",
                "reason=record-attribute",
                "path=[\"indirect\"] depth=1",
                "const=1 volatile=1 restrict=0",
                "witness=Some(",
                "capture-diagnostics.hpp",
            ] {
                assert!(record.contains(field), "{field}: {record}");
            }
            assert!(
                record.contains(&format!("parameter_canonical_kind={CXType_Pointer}")),
                "{record}"
            );
            assert!(
                record.contains(&format!(" canonical_kind={CXType_Record} ")),
                "{record}"
            );
            assert!(stderr.contains("rejected_parameters=1 detailed_parameters=1 truncated_parameters=0 diagnostic_limit=8"), "{stderr}");
            eprintln!("{record}");
        }
    }
}
