use std::collections::BTreeMap;

/// Stable identity for one source registered in a [`DiagnosticReport`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(u32);

impl SourceId {
    pub(crate) const UNKNOWN: Self = Self(0);

    pub(crate) fn new(value: u32) -> Self {
        Self(value)
    }

    pub(crate) fn is_valid(self) -> bool {
        self.0 != 0
    }

    /// Returns the numeric identity assigned by the diagnostic report.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Compilation cannot continue.
    Error,
    /// Compilation may continue, but the source deserves attention.
    Warning,
}

/// Whether a diagnostic label identifies the main problem or related source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelStyle {
    /// The source range that caused the diagnostic.
    Primary,
    /// A related source range, such as an earlier declaration.
    Secondary,
}

/// A source position with one-based lines and zero-based columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    /// One-based source line, or `0` when no line is available.
    pub line: usize,
    /// Zero-based source column.
    pub column: usize,
}

/// A labeled source range attached to a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    /// Whether this is the primary or a related source range.
    pub style: LabelStyle,
    /// Name of the source containing the range.
    pub source: String,
    /// Identity of the source containing the range, when registered in a report.
    pub source_id: Option<SourceId>,
    /// Inclusive start of the range.
    pub start: Position,
    /// Exclusive end of the range.
    pub end: Position,
    /// Text displayed next to the range.
    pub message: String,
}

impl Label {
    /// Creates a primary point label.
    pub fn primary(source: &str, line: usize, column: usize) -> Self {
        Self {
            style: LabelStyle::Primary,
            source: source.to_string(),
            source_id: None,
            start: Position { line, column },
            end: Position { line, column },
            message: String::new(),
        }
    }

    /// Creates a secondary point label.
    pub fn secondary(source: &str, line: usize, column: usize, message: &str) -> Self {
        Self {
            style: LabelStyle::Secondary,
            source: source.to_string(),
            source_id: None,
            start: Position { line, column },
            end: Position { line, column },
            message: message.to_string(),
        }
    }

    /// Sets the exclusive end of this label.
    pub fn with_end(mut self, line: usize, column: usize) -> Self {
        self.end = Position { line, column };
        self
    }

    /// Assigns the source identity registered in the diagnostic report.
    pub fn with_source_id(mut self, source_id: SourceId) -> Self {
        self.source_id = Some(source_id);
        self
    }

    /// Sets the text displayed next to this label.
    pub fn with_message(mut self, message: &str) -> Self {
        self.message = message.to_string();
        self
    }
}

/// A structured diagnostic produced while reading, writing, or formatting RDL.
#[derive(Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Stable diagnostic code, when one has been assigned.
    pub code: Option<String>,
    /// Diagnostic severity.
    pub severity: Severity,
    /// Human-readable description of what went wrong.
    pub message: String,
    /// Name of the primary source, if known.
    pub file_name: String,
    /// Identity of the primary source, when registered in a report.
    pub source_id: Option<SourceId>,
    /// One-based line number of the primary source, or `0` if not applicable.
    pub line: usize,
    /// Zero-based column number of the primary source.
    pub column: usize,
    /// Primary and related source ranges.
    pub labels: Vec<Label>,
    /// Additional context for the diagnostic.
    pub notes: Vec<String>,
    /// Actionable suggestions for resolving the diagnostic.
    pub help: Vec<String>,
}

impl Diagnostic {
    /// Creates an error diagnostic with the given primary source position.
    pub fn new(message: &str, file_name: &str, line: usize, column: usize) -> Self {
        let labels = if file_name.is_empty() {
            vec![]
        } else {
            vec![Label::primary(file_name, line, column)]
        };

        Self {
            code: None,
            severity: Severity::Error,
            message: message.to_string(),
            file_name: file_name.to_string(),
            source_id: None,
            line,
            column,
            labels,
            notes: vec![],
            help: vec![],
        }
    }

    /// Assigns a stable code to the diagnostic.
    pub fn with_code(mut self, code: &str) -> Self {
        self.code = Some(code.to_string());
        self
    }

    /// Sets the diagnostic severity.
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Assigns the primary source identity and any matching labels without an identity.
    pub fn with_source_id(mut self, source_id: SourceId) -> Self {
        self.source_id = Some(source_id);
        for label in &mut self.labels {
            if label.source_id.is_none() && label.source == self.file_name {
                label.source_id = Some(source_id);
            }
        }
        self
    }

    /// Adds a source label.
    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    /// Replaces the primary source label and legacy primary location.
    pub fn with_primary_label(mut self, label: Label) -> Self {
        self.file_name.clone_from(&label.source);
        self.source_id = label.source_id;
        self.line = label.start.line;
        self.column = label.start.column;

        if let Some(primary) = self
            .labels
            .iter_mut()
            .find(|existing| existing.style == LabelStyle::Primary)
        {
            *primary = label;
        } else {
            self.labels.insert(0, label);
        }
        self
    }

    /// Adds explanatory context.
    pub fn with_note(mut self, note: &str) -> Self {
        self.notes.push(note.to_string());
        self
    }

    /// Adds an actionable suggestion.
    pub fn with_help(mut self, help: &str) -> Self {
        self.help.push(help.to_string());
        self
    }
}

/// An owned RDL error diagnostic.
///
/// The diagnostic is boxed so functions returning `Result<T, Error>` keep a small error variant.
/// `Deref` preserves field access such as `error.message` and `error.labels`.
#[derive(Clone)]
pub struct Error(Box<Diagnostic>);

impl Error {
    /// Creates an error with the given primary source position.
    pub fn new(message: &str, file_name: &str, line: usize, column: usize) -> Self {
        Self(Box::new(Diagnostic::new(message, file_name, line, column)))
    }

    /// Returns the structured diagnostic.
    pub fn diagnostic(&self) -> &Diagnostic {
        &self.0
    }

    /// Consumes the error and returns the structured diagnostic.
    pub fn into_diagnostic(self) -> Diagnostic {
        *self.0
    }

    /// Assigns a stable code to the diagnostic.
    pub fn with_code(mut self, code: &str) -> Self {
        self.0.code = Some(code.to_string());
        self
    }

    /// Sets the diagnostic severity.
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.0.severity = severity;
        self
    }

    /// Assigns the primary source identity and any matching labels without an identity.
    pub fn with_source_id(mut self, source_id: SourceId) -> Self {
        self.0.source_id = Some(source_id);
        for label in &mut self.0.labels {
            if label.source_id.is_none() && label.source == self.0.file_name {
                label.source_id = Some(source_id);
            }
        }
        self
    }

    /// Adds a source label.
    pub fn with_label(mut self, label: Label) -> Self {
        self.0.labels.push(label);
        self
    }

    /// Replaces the primary source label and legacy primary location.
    pub fn with_primary_label(mut self, label: Label) -> Self {
        self.0.file_name.clone_from(&label.source);
        self.0.source_id = label.source_id;
        self.0.line = label.start.line;
        self.0.column = label.start.column;

        if let Some(primary) = self
            .0
            .labels
            .iter_mut()
            .find(|existing| existing.style == LabelStyle::Primary)
        {
            *primary = label;
        } else {
            self.0.labels.insert(0, label);
        }
        self
    }

    /// Adds explanatory context.
    pub fn with_note(mut self, note: &str) -> Self {
        self.0.notes.push(note.to_string());
        self
    }

    /// Adds an actionable suggestion.
    pub fn with_help(mut self, help: &str) -> Self {
        self.0.help.push(help.to_string());
        self
    }
}

impl std::ops::Deref for Error {
    type Target = Diagnostic;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::error::Error for Error {}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Diagnostic {}

impl std::fmt::Debug for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let code = self
            .code
            .as_deref()
            .map_or_else(String::new, |code| format!("[{code}]"));

        if self.line != 0 || self.column != 0 {
            write!(
                f,
                "\n{severity}{code}: {}\n --> {}:{}:{}",
                self.message,
                self.file_name,
                self.line,
                self.column + 1
            )
        } else if self.file_name.is_empty() {
            write!(f, "\n{severity}{code}: {}", self.message)
        } else {
            write!(
                f,
                "\n{severity}{code}: {}\n --> {}",
                self.message, self.file_name
            )
        }
    }
}

/// Diagnostics and source text produced by checking RDL inputs.
#[derive(Clone, Default)]
pub struct DiagnosticReport {
    diagnostics: Vec<Diagnostic>,
    sources: BTreeMap<SourceId, String>,
    source_ids: BTreeMap<String, Vec<SourceId>>,
    blocked: bool,
}

impl DiagnosticReport {
    /// Returns every diagnostic produced by the check.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Returns the original source text for a diagnostic source name.
    pub fn source(&self, name: &str) -> Option<&str> {
        let ids = self.source_ids.get(name)?;
        if ids.len() != 1 {
            return None;
        }
        self.source_by_id(ids[0])
    }

    /// Returns source text by its stable identity.
    pub fn source_by_id(&self, source_id: SourceId) -> Option<&str> {
        self.sources.get(&source_id).map(String::as_str)
    }

    /// Returns whether the report contains no errors.
    pub fn is_success(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    pub(crate) fn add_source(&mut self, name: String, source: String) -> SourceId {
        let source_id = SourceId::new(self.sources.len() as u32 + 1);
        let ids = self.source_ids.entry(name).or_default();
        ids.push(source_id);
        self.sources.insert(source_id, source);
        source_id
    }

    pub(crate) fn push(&mut self, error: Error) {
        let diagnostic = error.into_diagnostic();
        self.blocked |= diagnostic.severity == Severity::Error;
        self.diagnostics.push(diagnostic);
    }

    pub(crate) fn push_recoverable(&mut self, error: Error) {
        self.diagnostics.push(error.into_diagnostic());
    }

    pub(crate) fn extend(&mut self, errors: impl IntoIterator<Item = Error>) {
        let errors: Vec<_> = errors.into_iter().map(Error::into_diagnostic).collect();
        self.blocked |= errors
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error);
        self.diagnostics.extend(errors);
    }

    pub(crate) fn is_blocked(&self) -> bool {
        self.blocked
    }

    pub(crate) fn into_error(self) -> Option<Error> {
        self.diagnostics
            .into_iter()
            .find(|diagnostic| diagnostic.severity == Severity::Error)
            .map(Error::from)
    }
}

impl From<Diagnostic> for Error {
    fn from(value: Diagnostic) -> Self {
        Self(Box::new(value))
    }
}
