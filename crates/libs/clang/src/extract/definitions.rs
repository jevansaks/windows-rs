use super::*;
#[cfg(test)]
use std::cell::Cell;
use std::sync::Mutex;

#[cfg(test)]
thread_local! {
    static BUILDS: Cell<usize> = const { Cell::new(0) };
}

#[derive(Clone, Copy)]
struct Observation {
    cursor: CXCursor,
    definition: bool,
    skipped: bool,
}

#[derive(Default)]
struct Observations {
    functions: Vec<Observation>,
    error: Option<String>,
}

// Cursors belong to the live, immutable TU; callback access is serialized by the mutex.
unsafe impl Send for Observations {}

struct Callback {
    library: Arc<SharedLibrary>,
    observations: Mutex<Observations>,
}

pub(super) struct Definitions {
    tu: CXTranslationUnit,
    buckets: HashMap<u32, Vec<Observation>>,
    pub(super) observations: usize,
    pub(super) lookups: usize,
}

impl Definitions {
    pub(super) fn new(tu: CXTranslationUnit, index: &Index) -> Result<Self, Error> {
        #[cfg(test)]
        BUILDS.set(BUILDS.get() + 1);
        let callback = Callback {
            library: get_library()
                .ok_or_else(|| Error("native definition index has no libclang context".into()))?,
            observations: Mutex::new(Observations::default()),
        };
        let callback_size = size_of::<IndexerCallbacks>()
            .try_into()
            .map_err(|_| Error("native definition index callback size is invalid".into()))?;
        let action = unsafe { clang_IndexAction_create(index.0) };
        if action.is_null() {
            return Err(Error(
                "failed to create native definition index action".into(),
            ));
        }
        let mut callbacks: IndexerCallbacks = unsafe { std::mem::zeroed() };
        callbacks.indexDeclaration = Some(index_declaration);
        let result = unsafe {
            clang_indexTranslationUnit(
                action,
                std::ptr::from_ref(&callback).cast_mut().cast(),
                &mut callbacks,
                callback_size,
                0,
                tu,
            )
        };
        unsafe { clang_IndexAction_dispose(action) };
        if result != 0 {
            return Err(Error(format!(
                "native definition indexing failed: {result}"
            )));
        }
        let observations = callback
            .observations
            .into_inner()
            .map_err(|_| Error("native definition index callback state was poisoned".into()))?;
        if let Some(error) = observations.error {
            return Err(Error(error));
        }
        let mut index = Self {
            tu,
            buckets: HashMap::new(),
            observations: observations.functions.len(),
            lookups: 0,
        };
        for observation in observations.functions {
            if unsafe { clang_Cursor_getTranslationUnit(observation.cursor) } != tu {
                return Err(Error(
                    "native definition index returned a foreign TU cursor".into(),
                ));
            }
            index
                .buckets
                .entry(unsafe { clang_hashCursor(observation.cursor) })
                .or_default()
                .push(observation);
        }
        Ok(index)
    }

    pub(super) fn definition(&mut self, cursor: CXCursor) -> Result<bool, Error> {
        self.lookups += 1;
        if unsafe { clang_Cursor_getTranslationUnit(cursor) } != self.tu {
            return Err(Error(
                "native definition lookup used a foreign TU cursor".into(),
            ));
        }
        let hash = unsafe { clang_hashCursor(cursor) };
        let mut matched = None;
        for observation in self.buckets.get(&hash).into_iter().flatten() {
            if unsafe { clang_equalCursors(cursor, observation.cursor) } == 0 {
                continue;
            }
            let evidence = (observation.definition, observation.skipped);
            if matched.is_some_and(|previous| previous != evidence) {
                return Err(Error(
                    "conflicting exact native function definition observations".into(),
                ));
            }
            matched = Some(evidence);
        }
        matched.map(|(definition, _)| definition).ok_or_else(|| {
            Error(format!(
                "missing exact native definition observation for function `{}`",
                cx_string(unsafe { clang_getCursorSpelling(cursor) }),
            ))
        })
    }
}

extern "C" fn index_declaration(data: CXClientData, info: *const CXIdxDeclInfo) {
    let callback = unsafe { &*data.cast::<Callback>() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _library = Library::from_shared(callback.library.clone());
        if info.is_null() {
            return Err("native definition index returned null declaration info".to_string());
        }
        let info = unsafe { &*info };
        if unsafe { clang_getCursorKind(info.cursor) } != CXCursor_FunctionDecl {
            return Ok(());
        }
        let skipped = info.flags
            & u32::try_from(CXIdxDeclFlag_Skipped)
                .map_err(|_| "invalid native skipped-body flag")?
            != 0;
        if !matches!(info.isDefinition, 0 | 1) || skipped && info.isDefinition == 0 {
            return Err("invalid native function definition disposition".to_string());
        }
        let mut observations = callback
            .observations
            .lock()
            .map_err(|_| "native definition index callback state was poisoned")?;
        observations.functions.push(Observation {
            cursor: info.cursor,
            definition: info.isDefinition != 0,
            skipped,
        });
        Ok(())
    }));
    let error = match result {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error,
        Err(_) => "native definition indexing callback panicked".to_string(),
    };
    let mut observations = match callback.observations.lock() {
        Ok(observations) => observations,
        Err(poisoned) => poisoned.into_inner(),
    };
    observations.error.get_or_insert(error);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn functions(cursor: CXCursor, output: &mut Vec<CXCursor>) {
        for child in cursor_children(cursor) {
            if unsafe { clang_getCursorKind(child) } == CXCursor_FunctionDecl {
                output.push(child);
            }
            functions(child, output);
        }
    }

    #[test]
    fn exact_cursor_coverage_redeclarations_and_failures() {
        helpers::ensure_libclang();
        let _library = Library::new().unwrap();
        let index = Index::new().unwrap();
        let source = r#"
            extern "C" int Declaration(int value);
            extern "C" inline int InlineDeclaration(int value);
            extern "C" inline int InlineDefinition(int value) { return value; }
            extern "C" int Definition(int value) { return value; }
            extern "C" int Redeclared(int value);
            extern "C" int Redeclared(int value) { return value; }
            extern "C" { static int Internal(int value); }
            extern "C" { static int InternalDefinition(int value) { return value; } }
            extern "C" constexpr int Constexpr(int value) { return value; }
        "#;
        for target in [
            "i686-pc-windows-msvc",
            "x86_64-pc-windows-msvc",
            "aarch64-pc-windows-msvc",
        ] {
            let target_arg = format!("--target={target}");
            let args = ["-x", "c++", "-std=c++20", &target_arg];
            let input = Input::new("definitions.hpp", source);
            let tu = TranslationUnit::parse(&index, &input, &args).unwrap();
            let mut cursors = Vec::new();
            functions(
                unsafe { clang_getTranslationUnitCursor(tu.0) },
                &mut cursors,
            );
            let mut definitions = Definitions::new(tu.0, &index).unwrap();
            assert_eq!(definitions.observations, cursors.len());
            assert_eq!(cursors.len(), 9);
            for (cursor, expected) in cursors
                .iter()
                .zip([false, false, true, true, false, true, false, true, true])
            {
                assert_eq!(definitions.definition(*cursor).unwrap(), expected);
            }
            assert_eq!(definitions.lookups, 9);
            let first = cursors[0];
            let hash = unsafe { clang_hashCursor(first) };
            let second = cursors[2];
            definitions
                .buckets
                .entry(hash)
                .or_default()
                .push(Observation {
                    cursor: second,
                    definition: true,
                    skipped: true,
                });
            assert!(!definitions.definition(first).unwrap());
            definitions
                .buckets
                .entry(hash)
                .or_default()
                .push(Observation {
                    cursor: first,
                    definition: true,
                    skipped: true,
                });
            assert!(
                definitions
                    .definition(first)
                    .unwrap_err()
                    .to_string()
                    .contains("conflicting")
            );
            definitions.buckets.clear();
            assert!(
                definitions
                    .definition(first)
                    .unwrap_err()
                    .to_string()
                    .contains("missing")
            );
            let foreign =
                TranslationUnit::parse(&index, &Input::new("foreign.hpp", source), &args).unwrap();
            let mut other_cursors = Vec::new();
            functions(
                unsafe { clang_getTranslationUnitCursor(foreign.0) },
                &mut other_cursors,
            );
            assert!(
                definitions
                    .definition(other_cursors[0])
                    .unwrap_err()
                    .to_string()
                    .contains("foreign TU")
            );
            let mut other = Definitions::new(foreign.0, &index).unwrap();
            assert!(!other.definition(other_cursors[0]).unwrap());
            assert!(other.definition(first).is_err());
        }
    }

    #[test]
    fn only_original_extraction_builds_indexes() {
        helpers::ensure_libclang();
        let _library = Library::new().unwrap();
        let index = Index::new().unwrap();
        let args = ["-x", "c++", "--target=x86_64-pc-windows-msvc"];
        let before = BUILDS.get();
        let input = Input::new("probe.hpp", "const int Probe = 3;");
        let _probe = TranslationUnit::parse_probe(&index, &input, &input.source, &args).unwrap();
        assert_eq!(BUILDS.get(), before);
        extract(
            [
                Input::new("first.hpp", "#define VALUE 7\nextern \"C\" int First();"),
                Input::new("second.hpp", "#define OTHER 8\nextern \"C\" int Second();"),
            ],
            &args,
        )
        .unwrap();
        assert_eq!(BUILDS.get(), before + 2);
    }
}
