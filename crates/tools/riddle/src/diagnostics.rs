use std::collections::HashMap;
use std::io::IsTerminal;
use windows_rdl::{Diagnostic, DiagnosticReport, Label, LabelStyle, Severity, SourceId};

#[derive(Clone, Copy, Default)]
pub(super) enum Color {
    #[default]
    Auto,
    Always,
    Never,
}

impl Color {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    fn enabled(self) -> bool {
        match self {
            Self::Auto => std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal(),
            Self::Always => true,
            Self::Never => false,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(super) enum Format {
    #[default]
    Human,
    Short,
    Json,
}

impl Format {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "human" => Some(Self::Human),
            "short" => Some(Self::Short),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

pub(super) struct Renderer {
    format: Format,
    color: bool,
}

impl Renderer {
    pub(super) fn new(format: Format, color: Color) -> Self {
        Self {
            format,
            color: color.enabled() && !matches!(format, Format::Json),
        }
    }

    pub(super) fn render(
        &self,
        diagnostics: &[Diagnostic],
        report: Option<&DiagnosticReport>,
        stdin: Option<&str>,
    ) -> String {
        match self.format {
            Format::Human => self.render_human(diagnostics, report, stdin),
            Format::Short => self.render_short(diagnostics),
            Format::Json => render_json(diagnostics),
        }
    }

    fn render_human(
        &self,
        diagnostics: &[Diagnostic],
        report: Option<&DiagnosticReport>,
        stdin: Option<&str>,
    ) -> String {
        let mut output = String::new();
        for diagnostic in diagnostics {
            self.render_diagnostic(&mut output, diagnostic, report, stdin);
        }
        self.render_summary(&mut output, diagnostics);
        output
    }

    fn render_diagnostic(
        &self,
        output: &mut String,
        diagnostic: &Diagnostic,
        report: Option<&DiagnosticReport>,
        stdin: Option<&str>,
    ) {
        let severity = severity_name(diagnostic.severity);
        let severity = self.paint_severity(diagnostic.severity, severity);
        let code = diagnostic
            .code
            .as_deref()
            .map_or_else(String::new, |code| format!("[{code}]"));
        output.push_str(&format!("{severity}{code}: {}\n", diagnostic.message));

        let labels = diagnostic_labels(diagnostic);
        let mut sources = HashMap::new();
        for label in &labels {
            self.render_label(output, label, report, stdin, &mut sources);
        }
        for note in &diagnostic.notes {
            output.push_str(&format!("  = note: {note}\n"));
        }
        for help in &diagnostic.help {
            output.push_str(&format!("  = help: {help}\n"));
        }
    }

    fn render_label(
        &self,
        output: &mut String,
        label: &Label,
        report: Option<&DiagnosticReport>,
        stdin: Option<&str>,
        sources: &mut HashMap<(Option<SourceId>, String), Option<String>>,
    ) {
        let line = label.start.line;
        let column = label.start.column;
        if line == 0 {
            output.push_str(&format!(" --> {}\n", label.source));
            if !label.message.is_empty() {
                output.push_str(&format!("  = note: {}\n", label.message));
            }
            return;
        }

        output.push_str(&format!(" --> {}:{}:{}\n", label.source, line, column + 1));
        let source = sources
            .entry((label.source_id, label.source.clone()))
            .or_insert_with(|| {
                if let Some(source) = label
                    .source_id
                    .and_then(|source_id| report.and_then(|report| report.source_by_id(source_id)))
                    .or_else(|| report.and_then(|report| report.source(&label.source)))
                {
                    Some(source.to_string())
                } else if label.source == "<stdin>" {
                    stdin.map(str::to_string)
                } else {
                    std::fs::read_to_string(&label.source).ok()
                }
            })
            .as_deref();
        let Some(source_line) = source.and_then(|source| source.lines().nth(line - 1)) else {
            return;
        };

        let width = line.to_string().len();
        output.push_str(&format!("{:width$} |\n", "", width = width));
        output.push_str(&format!("{line:>width$} | {source_line}\n"));
        let end_column = if label.end.line == line {
            label.end.column.max(column + 1)
        } else {
            column + 1
        };
        let marker = match label.style {
            LabelStyle::Primary => '^',
            LabelStyle::Secondary => '-',
        };
        let underline: String = std::iter::repeat_n(marker, end_column - column).collect();
        let underline = if self.color {
            format!("\u{1b}[1;36m{underline}\u{1b}[0m")
        } else {
            underline
        };
        output.push_str(&format!(
            "{:width$} | {}{}",
            "",
            " ".repeat(column),
            underline,
            width = width
        ));
        if !label.message.is_empty() {
            output.push(' ');
            output.push_str(&label.message);
        }
        output.push('\n');
    }

    fn render_short(&self, diagnostics: &[Diagnostic]) -> String {
        let mut output = String::new();
        for diagnostic in diagnostics {
            let severity = severity_name(diagnostic.severity);
            let severity = self.paint_severity(diagnostic.severity, severity);
            let code = diagnostic
                .code
                .as_deref()
                .map_or_else(String::new, |code| format!("[{code}]"));
            if diagnostic.file_name.is_empty() {
                output.push_str(&format!("{severity}{code}: {}\n", diagnostic.message));
            } else if diagnostic.line == 0 {
                output.push_str(&format!(
                    "{}: {severity}{code}: {}\n",
                    diagnostic.file_name, diagnostic.message
                ));
            } else {
                output.push_str(&format!(
                    "{}:{}:{}: {severity}{code}: {}\n",
                    diagnostic.file_name,
                    diagnostic.line,
                    diagnostic.column + 1,
                    diagnostic.message
                ));
            }
        }
        self.render_summary(&mut output, diagnostics);
        output
    }

    fn render_summary(&self, output: &mut String, diagnostics: &[Diagnostic]) {
        let (errors, warnings) = counts(diagnostics);
        if errors == 0 && warnings == 0 {
            return;
        }

        let severity = if errors == 0 {
            Severity::Warning
        } else {
            Severity::Error
        };
        let prefix = self.paint_severity(severity, severity_name(severity));
        let message = match (errors, count_phrase(warnings, "warning")) {
            (0, Some(warnings)) => format!("{warnings} emitted"),
            (1, None) => "aborting due to 1 previous error".to_string(),
            (errors, None) => format!("aborting due to {errors} previous errors"),
            (1, Some(warnings)) => {
                format!("aborting due to 1 previous error; {warnings} emitted")
            }
            (errors, Some(warnings)) => {
                format!("aborting due to {errors} previous errors; {warnings} emitted")
            }
        };
        output.push_str(&format!("{prefix}: {message}\n"));
    }

    fn paint_severity(&self, severity: Severity, text: &str) -> String {
        if !self.color {
            return text.to_string();
        }
        let color = match severity {
            Severity::Error => 31,
            Severity::Warning => 33,
        };
        format!("\u{1b}[1;{color}m{text}\u{1b}[0m")
    }
}

fn diagnostic_labels(diagnostic: &Diagnostic) -> Vec<Label> {
    if !diagnostic.labels.is_empty() || diagnostic.file_name.is_empty() {
        return diagnostic.labels.clone();
    }

    let mut label = Label::primary(&diagnostic.file_name, diagnostic.line, diagnostic.column);
    if let Some(source_id) = diagnostic.source_id {
        label = label.with_source_id(source_id);
    }
    vec![label]
}

fn render_json(diagnostics: &[Diagnostic]) -> String {
    let (errors, warnings) = counts(diagnostics);
    let diagnostics: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| {
            serde_json::json!({
                "severity": severity_name(diagnostic.severity),
                "code": diagnostic.code,
                "message": diagnostic.message,
                "source": {
                    "name": diagnostic.file_name,
                    "id": diagnostic.source_id.map(SourceId::get),
                    "line": diagnostic.line,
                    "column": diagnostic.column,
                },
                "labels": diagnostic.labels.iter().map(|label| {
                    serde_json::json!({
                        "style": match label.style {
                            LabelStyle::Primary => "primary",
                            LabelStyle::Secondary => "secondary",
                        },
                        "source": label.source,
                        "source_id": label.source_id.map(SourceId::get),
                        "start": {
                            "line": label.start.line,
                            "column": label.start.column,
                        },
                        "end": {
                            "line": label.end.line,
                            "column": label.end.column,
                        },
                        "message": label.message,
                    })
                }).collect::<Vec<_>>(),
                "notes": diagnostic.notes,
                "help": diagnostic.help,
            })
        })
        .collect();
    format!(
        "{}\n",
        serde_json::json!({
            "diagnostics": diagnostics,
            "summary": {
                "errors": errors,
                "warnings": warnings,
            },
        })
    )
}

fn counts(diagnostics: &[Diagnostic]) -> (usize, usize) {
    diagnostics
        .iter()
        .fold((0, 0), |(errors, warnings), diagnostic| {
            match diagnostic.severity {
                Severity::Error => (errors + 1, warnings),
                Severity::Warning => (errors, warnings + 1),
            }
        })
}

fn count_phrase(count: usize, name: &str) -> Option<String> {
    (count != 0).then(|| {
        if count == 1 {
            format!("1 {name}")
        } else {
            format!("{count} {name}s")
        }
    })
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_rdl::Reader;

    #[test]
    fn warnings_use_warning_severity_and_summary() {
        let mut diagnostic = Diagnostic::new("unused import", "test.rdl", 1, 0);
        diagnostic.severity = Severity::Warning;
        let output = Renderer::new(Format::Human, Color::Never).render(&[diagnostic], None, None);

        assert!(output.starts_with("warning: unused import"));
        assert!(output.ends_with("warning: 1 warning emitted\n"));
    }

    #[test]
    fn renderer_uses_source_ids_for_duplicate_names() {
        let first = "#[win32] mod Test { struct First { value: i32 } }";
        let second = "#[win32] mod Test { struct First { value: u32 } }";
        let report = Reader::new().input_texts([first, second]).check_all();
        let output = Renderer::new(Format::Human, Color::Never).render(
            report.diagnostics(),
            Some(&report),
            None,
        );

        assert!(output.contains(first));
        assert!(output.contains(second));
    }
}
