use super::source::{InputText, expand_rdl_files};
use super::validate::{
    validate_import_warnings, validate_resolved_symbols, validate_symbols,
    validate_use_declarations,
};
use super::*;

#[derive(Default)]
/// Builder that compiles RDL files into `.winmd` metadata.
pub struct Reader {
    input: Vec<PathBuf>,
    input_text: Vec<InputText>,
    reference: Vec<PathBuf>,
    reference_default: bool,
    reference_bytes: Vec<Vec<u8>>,
    output: PathBuf,
}

impl Reader {
    /// Creates a new builder with default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an input `.rdl` file or directory.
    pub fn input(&mut self, input: impl AsRef<Path>) -> &mut Self {
        self.input.push(input.as_ref().to_path_buf());
        self
    }

    /// Adds inline RDL source text to compile instead of a file on disk.
    pub fn input_text(&mut self, input: &str) -> &mut Self {
        self.input_text_named(".rdl", input)
    }

    /// Adds named inline RDL source text to compile instead of a file on disk.
    pub fn input_text_named(&mut self, name: impl AsRef<str>, input: &str) -> &mut Self {
        self.input_text.push(InputText {
            name: name.as_ref().to_string(),
            text: input.to_string(),
        });
        self
    }

    /// Adds inline RDL source texts to compile instead of files on disk.
    pub fn input_texts<I, S>(&mut self, inputs: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for input in inputs {
            self.input_text(input.as_ref());
        }
        self
    }

    /// Adds named inline RDL source texts to compile instead of files on disk.
    pub fn input_texts_named<I, N, S>(&mut self, inputs: I) -> &mut Self
    where
        I: IntoIterator<Item = (N, S)>,
        N: AsRef<str>,
        S: AsRef<str>,
    {
        for (name, input) in inputs {
            self.input_text_named(name, input.as_ref());
        }
        self
    }

    /// Adds a `.winmd` reference file or directory.
    pub fn reference(&mut self, input: impl AsRef<Path>) -> &mut Self {
        self.reference.push(input.as_ref().to_path_buf());
        self
    }

    /// Adds multiple `.winmd` reference files or directories.
    pub fn references<I, S>(&mut self, inputs: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<Path>,
    {
        for input in inputs {
            self.reference(input);
        }
        self
    }

    /// Adds a `.winmd` reference from memory.
    pub fn reference_bytes(&mut self, input: &[u8]) -> &mut Self {
        self.reference_bytes.push(input.to_vec());
        self
    }

    /// Adds `.winmd` references from memory.
    pub fn reference_byte_sets<I, B>(&mut self, inputs: I) -> &mut Self
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        for input in inputs {
            self.reference_bytes(input.as_ref());
        }
        self
    }

    /// Adds the default Windows metadata references.
    pub fn reference_default(&mut self) -> &mut Self {
        self.reference_default = true;
        self
    }

    /// Adds multiple input `.rdl` files or directories.
    pub fn inputs<I, S>(&mut self, inputs: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<Path>,
    {
        for input in inputs {
            self.input(input);
        }
        self
    }

    /// Sets the output `.winmd` file path.
    pub fn output(&mut self, output: impl AsRef<Path>) -> &mut Self {
        self.output = output.as_ref().to_path_buf();
        self
    }

    /// Compiles the inputs and writes the `.winmd` to the configured output.
    pub fn write(&self) -> Result<(), Error> {
        if self.output.as_os_str().is_empty() {
            return Err(Error::new("output is required", "", 0, 0));
        }

        let assembly_name = self
            .output
            .file_stem()
            .and_then(|file_name| file_name.to_str())
            .ok_or_else(|| Error::new("invalid output", &self.output.to_string_lossy(), 0, 0))?;

        let output = self.compile(assembly_name)?;
        std::fs::write(&self.output, output)
            .map_err(|error| Error::new(&error.to_string(), &self.output.to_string_lossy(), 0, 0))
    }

    /// Compiles the inputs and returns finalized `.winmd` bytes with the given assembly name.
    pub fn bytes(&self, assembly_name: &str) -> Result<Vec<u8>, Error> {
        self.compile(assembly_name)
    }

    /// Parses, validates, resolves, and encodes the inputs without writing a `.winmd`.
    pub fn check(&self) -> Result<(), Error> {
        self.compile("check").map(|_| ())
    }

    /// Checks all inputs and returns every independent diagnostic found before encoding.
    pub fn check_all(&self) -> DiagnosticReport {
        self.compile_report("check").1
    }

    fn compile(&self, assembly_name: &str) -> Result<Vec<u8>, Error> {
        let (output, report) = self.compile_report(assembly_name);
        if let Some(error) = report.into_error() {
            Err(error)
        } else {
            Ok(output.unwrap())
        }
    }

    fn compile_report(&self, assembly_name: &str) -> (Option<Vec<u8>>, DiagnosticReport) {
        let mut report = DiagnosticReport::default();
        let rdl_paths = match expand_input_files(&self.input, "rdl") {
            Ok(paths) => Some(paths),
            Err(error) => {
                report.push(error);
                None
            }
        };
        let reference_paths = match expand_input_files(&self.reference, "winmd") {
            Ok(paths) => Some(paths),
            Err(error) => {
                report.push(error);
                None
            }
        };
        let Some(rdl_paths) = rdl_paths else {
            return (None, report);
        };
        let Some(reference_paths) = reference_paths else {
            return (None, report);
        };

        let input = expand_rdl_files(&rdl_paths, &self.input_text, &mut report);

        let mut index = Index::new();

        for file in &input {
            for item in &file.items {
                index.insert(file, "", item);
            }
        }

        report.extend(validate_symbols(&index));

        let mut reference = vec![];
        let mut invalid_reference = false;

        for file_name in &reference_paths {
            let source = file_name.to_string_lossy();
            if let Some(file) = metadata::reader::File::read(file_name) {
                reference.push(file);
            } else {
                invalid_reference = true;
                report.push(Error::new("invalid reference", &source, 0, 0));
            }
        }

        if self.reference_default {
            reference.extend(
                [windows_default::WINRT, windows_default::WIN32]
                    .into_iter()
                    .map(|bytes| metadata::reader::File::new(bytes.to_vec()).unwrap()),
            );
        }

        for bytes in &self.reference_bytes {
            if let Some(file) = metadata::reader::File::new(bytes.clone()) {
                reference.push(file);
            } else {
                invalid_reference = true;
                report.push(Error::new("invalid reference", "<memory>", 0, 0));
            }
        }

        let reference = metadata::reader::Index::new(reference);
        if !invalid_reference {
            report.extend(validate_use_declarations(&input, &index, &reference));
            report.extend(validate_resolved_symbols(&index, &reference));
        }
        if report.is_blocked() {
            return (None, report);
        }

        match encode(assembly_name, &index, reference) {
            Ok((output, errors)) => {
                report.extend(errors);
                report.extend(validate_import_warnings(&input));
                (report.is_success().then_some(output), report)
            }
            Err(error) => {
                report.push(error);
                (None, report)
            }
        }
    }
}

#[test]
fn use_glob_resolves_type() {
    let output = std::env::temp_dir().join("windows_rdl_use_glob_resolves_type.winmd");

    Reader::new()
        .input_text(
            r#"
use Other::*;

#[winrt]
mod Test {
    struct Thing {
        a: Point,
    }
}

#[winrt]
mod Other {
    struct Point {
        x: i32,
        y: i32,
    }
}
        "#,
        )
        .output(&output)
        .write()
        .unwrap();
}
