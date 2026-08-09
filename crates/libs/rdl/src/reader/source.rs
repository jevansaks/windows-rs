use super::*;

pub(super) struct InputText {
    pub name: String,
    pub text: String,
}

/// Parses one `.rdl` file and returns the items it defines under `namespace`.
pub(crate) fn item_names(path: impl AsRef<Path>, namespace: &str) -> Result<Vec<String>, Error> {
    let path = path.as_ref().to_path_buf();
    let mut report = DiagnosticReport::default();
    let input = expand_rdl_files(std::slice::from_ref(&path), &[], &mut report);
    if let Some(error) = report.into_error() {
        return Err(error);
    }
    let mut index = Index::new();
    for file in &input {
        for item in &file.items {
            index.insert(file, "", item);
        }
    }
    let mut names = vec![];
    if let Some(ns) = index.namespaces.get(namespace) {
        names.extend(ns.types.keys().cloned());
        names.extend(ns.functions.keys().cloned());
        names.extend(ns.constants.keys().cloned());
    }
    Ok(names)
}

/// Rewrites RDL tokens that would otherwise confuse `syn`.
fn preprocess_rdl(contents: &str) -> std::borrow::Cow<'_, str> {
    let needs_in = contents.contains("#[in]");
    let needs_doc = contents.contains("//!");
    if !needs_in && !needs_doc {
        return std::borrow::Cow::Borrowed(contents);
    }
    let mut result = contents.to_string();
    if needs_in {
        result = result.replace("#[in]", "#[r#in]");
    }
    if needs_doc {
        result = result.replace("//!", "//");
    }
    std::borrow::Cow::Owned(result)
}

pub(crate) fn parse_source(name: &str, input: &str) -> Result<(), Error> {
    let contents = preprocess_rdl(input);
    match syn::parse_str::<File>(&contents) {
        Ok(mut file) => {
            let Some(error) = file.take_syntax_errors().into_iter().next() else {
                return Ok(());
            };
            let start = error.span().start();
            Err(Error::new(
                &error.to_string(),
                name,
                start.line,
                start.column,
            ))
        }
        Err(error) => {
            let start = error.span().start();
            Err(Error::new(
                &error.to_string(),
                name,
                start.line,
                start.column,
            ))
        }
    }
}

pub(super) fn expand_rdl_files(
    paths: &[PathBuf],
    input_text: &[InputText],
    report: &mut DiagnosticReport,
) -> Vec<File> {
    let mut input = vec![];

    for path in paths {
        let source = path.to_string_lossy();
        let Ok(contents) = std::fs::read_to_string(path) else {
            report.push(Error::new("failed to read input file", &source, 0, 0));
            continue;
        };
        let source_id = report.add_source(source.to_string(), contents.clone());

        let contents = preprocess_rdl(&contents);
        match syn::parse_str::<File>(&contents) {
            Ok(mut file) => {
                file.source = source.into_owned();
                file.source_id = source_id;
                for error in file.take_syntax_errors() {
                    let start = error.span().start();
                    report.push_recoverable(
                        Error::new(&error.to_string(), &file.source, start.line, start.column)
                            .with_source_id(source_id),
                    );
                }
                input.push(file);
            }
            Err(error) => {
                let start = error.span().start();
                report.push(
                    Error::new(&error.to_string(), &source, start.line, start.column)
                        .with_source_id(source_id),
                );
            }
        }
    }

    for input_text in input_text {
        let source_id = report.add_source(input_text.name.clone(), input_text.text.clone());
        let contents = preprocess_rdl(&input_text.text);
        match syn::parse_str::<File>(&contents) {
            Ok(mut file) => {
                file.source.clone_from(&input_text.name);
                file.source_id = source_id;
                for error in file.take_syntax_errors() {
                    let start = error.span().start();
                    report.push_recoverable(
                        Error::new(&error.to_string(), &file.source, start.line, start.column)
                            .with_source_id(source_id),
                    );
                }
                input.push(file);
            }
            Err(error) => {
                let start = error.span().start();
                report.push(
                    Error::new(
                        &error.to_string(),
                        &input_text.name,
                        start.line,
                        start.column,
                    )
                    .with_source_id(source_id),
                );
            }
        }
    }

    for file in &mut input {
        for item in &mut file.items {
            if let Err(error) = resolve_winrt(item, &file.source, None) {
                report.push(error.with_source_id(file.source_id));
            }
        }
    }

    input
}

fn resolve_winrt(item: &mut Item, source_file: &str, parent: Option<bool>) -> Result<(), Error> {
    match item {
        Item::Enum(item) => {
            item.winrt = read_winrt_expected(source_file, &item.token, &item.attrs, parent)?;
        }
        Item::Interface(item) => {
            item.winrt = read_winrt_expected(source_file, &item.token, &item.attrs, parent)?;
        }
        Item::Struct(item) => {
            item.winrt = read_winrt_expected(source_file, &item.span, &item.attrs, parent)?;
        }
        Item::Attribute(item) => {
            item.winrt = read_winrt_expected(source_file, &item.token, &item.attrs, parent)?;
        }
        Item::Module(item) => {
            let parent = read_winrt(source_file, &item.token, &item.attrs, parent)?;

            for child in &mut item.items {
                resolve_winrt(child, source_file, parent)?;
            }
        }
        _ => {}
    }

    Ok(())
}

fn read_winrt_expected<S: Spanned>(
    source_file: &str,
    span: &S,
    attrs: &[syn::Attribute],
    parent: Option<bool>,
) -> Result<bool, Error> {
    if let Some(winrt) = read_winrt(source_file, span, attrs, parent)? {
        Ok(winrt)
    } else {
        let start = span.span().start();

        Err(Error::new(
            "`winrt` or `win32` attribute required",
            source_file,
            start.line,
            start.column,
        ))
    }
}

fn read_winrt<S: Spanned>(
    source_file: &str,
    span: &S,
    attrs: &[syn::Attribute],
    parent: Option<bool>,
) -> Result<Option<bool>, Error> {
    let mut winrt = false;
    let mut win32 = false;

    for attr in attrs {
        if attr.path().is_ident("winrt") {
            winrt = true;
        } else if attr.path().is_ident("win32") {
            win32 = true;
        }
    }

    if winrt && win32 {
        let start = span.span().start();

        return Err(Error::new(
            "`winrt` and `win32` attributes are mutually exclusive",
            source_file,
            start.line,
            start.column,
        ));
    } else if !winrt
        && !win32
        && let Some(parent) = parent
    {
        if parent {
            winrt = true;
        } else {
            win32 = true;
        }
    }

    if winrt {
        Ok(Some(true))
    } else if win32 {
        Ok(Some(false))
    } else {
        Ok(None)
    }
}
