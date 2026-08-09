use super::*;

#[derive(Default)]
pub(super) struct OriginMap {
    sources: HashMap<SourceId, String>,
    rows: HashMap<metadata::reader::RowId, SourceOrigin>,
}

#[derive(Clone, Copy)]
struct SourceOrigin {
    source: SourceId,
    start: Position,
    end: Position,
}

impl OriginMap {
    pub(super) fn insert<H: metadata::writer::RowHandle, S: Spanned>(
        &mut self,
        handle: H,
        file: &File,
        source: &S,
    ) {
        let start = source.span().start();
        let end = source.span().end();
        self.sources
            .entry(file.source_id)
            .or_insert_with(|| file.source.clone());
        self.rows.insert(
            handle.row_id(0),
            SourceOrigin {
                source: file.source_id,
                start: Position {
                    line: start.line,
                    column: start.column,
                },
                end: Position {
                    line: end.line,
                    column: end.column,
                },
            },
        );
    }

    pub(super) fn error(&self, error: metadata::validator::ValidationError) -> Error {
        let mut diagnostic = Diagnostic::new(error.message(), "", 0, 0);
        if error.category() == metadata::validator::ValidationCategory::Duplicate {
            diagnostic = diagnostic.with_code("RDL0001");
        }
        let related = error.related();
        let mut promoted_related = false;

        if let Some(primary) = self.rows.get(&error.row()) {
            diagnostic = diagnostic.with_primary_label(self.label(*primary, LabelStyle::Primary));
        } else if let Some(primary) = related.and_then(|row| self.rows.get(&row)) {
            diagnostic = diagnostic.with_primary_label(self.label(*primary, LabelStyle::Primary));
            promoted_related = true;
            diagnostic = diagnostic.with_note(&format!(
                "metadata row {:?}[{}]",
                error.row().table(),
                error.row().row() + 1
            ));
        } else {
            diagnostic = diagnostic.with_note(&format!(
                "metadata row {:?}[{}]",
                error.row().table(),
                error.row().row() + 1
            ));
        }

        if !promoted_related
            && let Some(related) = related
            && let Some(label) = self.rows.get(&related)
        {
            let mut label = self.label(*label, LabelStyle::Secondary);
            label.message = "first declared here".to_string();
            diagnostic = diagnostic.with_label(label);
        }

        Error::from(diagnostic)
    }

    fn label(&self, origin: SourceOrigin, style: LabelStyle) -> Label {
        Label {
            style,
            source: self.sources[&origin.source].clone(),
            source_id: origin.source.is_valid().then_some(origin.source),
            start: origin.start,
            end: origin.end,
            message: String::new(),
        }
    }
}

#[test]
fn metadata_errors_map_to_source_labels() {
    let source = File {
        items: vec![],
        imports: vec![],
        source: "test.rdl".to_string(),
        source_id: SourceId::UNKNOWN,
        syntax_errors: vec![],
    };
    let name: syn::Ident = syn::parse_quote!(Value);
    let mut output = metadata::writer::File::new("test");
    let first = output.TypeDef(
        "Test",
        "Value",
        metadata::writer::TypeDefOrRef::default(),
        metadata::TypeAttributes::Public,
    );
    let second = output.TypeDef(
        "Test",
        "Value",
        metadata::writer::TypeDefOrRef::default(),
        metadata::TypeAttributes::Public,
    );
    let mut origins = OriginMap::default();
    origins.insert(first, &source, &name);
    origins.insert(second, &source, &name);

    let error = metadata::validator::validate(&output.into_index())
        .into_iter()
        .next()
        .unwrap();
    let error = origins.error(error);

    assert_eq!(error.file_name, "test.rdl");
    assert_eq!(error.labels.len(), 2);
    assert_eq!(error.labels[0].style, LabelStyle::Primary);
    assert_eq!(error.labels[1].style, LabelStyle::Secondary);
}
