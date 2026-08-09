mod diagnostics;
mod expand;

use diagnostics::{Color, Format, Renderer};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use windows_metadata as metadata;
use windows_rdl::{Diagnostic, Error, Label, Reader, formatter};

const HELP: &str = "\
Riddle checks and compiles RDL and validates Windows metadata.

Usage:
  riddle check [options] <input>...
  riddle build [options] <input>... --out <path>
  riddle expand [options] <input>...
  riddle dump <input>...
  riddle validate [options] <input>...
  riddle fmt [--check] <input>...

Options:
  -r, --reference <path>  Add a .winmd reference file or directory
      --no-default        Do not use the default Windows metadata references
      --profile <name>    Validation profile: common, win32, winrt, or windows
      --color <when>      Color diagnostics: auto, always, or never
      --format <name>     Diagnostic format: human, short, or json
  -o, --out <path>        Output .winmd path for build
      --check             Check formatting without changing files
  -h, --help              Print help
  -V, --version           Print version

Use - with check, build, or fmt to read RDL from standard input.
";

#[derive(Clone, Copy)]
enum Command {
    Check,
    Build,
    Expand,
    Dump,
    Validate,
    Fmt,
}

struct Options {
    command: Command,
    inputs: Vec<PathBuf>,
    references: Vec<PathBuf>,
    output: Option<PathBuf>,
    reference_default: bool,
    format_check: bool,
    validation_profile: metadata::validator::ValidationProfile,
    diagnostic_color: Color,
    diagnostic_format: Format,
    stdin: Option<String>,
}

enum ParseResult {
    Run(Options),
    Help,
    Version,
}

fn main() -> ExitCode {
    match parse(std::env::args().skip(1)) {
        Ok(ParseResult::Help) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(ParseResult::Version) => {
            println!("riddle {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(ParseResult::Run(mut options)) => {
            let renderer = Renderer::new(options.diagnostic_format, options.diagnostic_color);
            if options.inputs.iter().any(|input| input == Path::new("-")) {
                let mut source = String::new();
                if let Err(error) = std::io::stdin().read_to_string(&mut source) {
                    eprintln!("error: failed to read standard input: {error}");
                    return ExitCode::from(1);
                }
                options.stdin = Some(source);
            }

            match options.command {
                Command::Fmt => match format_inputs(&options) {
                    Ok(diagnostics) if diagnostics.is_empty() => ExitCode::SUCCESS,
                    Ok(diagnostics) => {
                        eprint!(
                            "{}",
                            renderer.render(&diagnostics, None, options.stdin.as_deref())
                        );
                        ExitCode::from(1)
                    }
                    Err(error) => {
                        eprint!(
                            "{}",
                            render_error(&renderer, &error, options.stdin.as_deref())
                        );
                        ExitCode::from(1)
                    }
                },
                Command::Validate => match validate_metadata(&options) {
                    Ok((_, errors)) if errors.is_empty() => ExitCode::SUCCESS,
                    Ok((paths, errors)) => {
                        let diagnostics = metadata_diagnostics(&paths, &errors);
                        eprint!("{}", renderer.render(&diagnostics, None, None));
                        ExitCode::from(1)
                    }
                    Err(error) => {
                        eprint!(
                            "{}",
                            render_error(&renderer, &error, options.stdin.as_deref())
                        );
                        ExitCode::from(1)
                    }
                },
                Command::Dump => match dump_metadata(&options) {
                    Ok(output) => {
                        print!("{output}");
                        ExitCode::SUCCESS
                    }
                    Err(error) => {
                        eprint!(
                            "{}",
                            render_error(&renderer, &error, options.stdin.as_deref())
                        );
                        ExitCode::from(1)
                    }
                },
                Command::Check | Command::Build | Command::Expand => {
                    let mut reader = Reader::new();
                    configure(&mut reader, &options);
                    match options.command {
                        Command::Check => {
                            let report = reader.check_all();
                            if !report.diagnostics().is_empty() {
                                eprint!(
                                    "{}",
                                    renderer.render(report.diagnostics(), Some(&report), None)
                                );
                            }
                            if report.is_success() {
                                ExitCode::SUCCESS
                            } else {
                                ExitCode::from(1)
                            }
                        }
                        Command::Build => {
                            match reader.output(options.output.as_ref().unwrap()).write() {
                                Ok(()) => ExitCode::SUCCESS,
                                Err(error) => {
                                    eprint!(
                                        "{}",
                                        render_error(&renderer, &error, options.stdin.as_deref())
                                    );
                                    ExitCode::from(1)
                                }
                            }
                        }
                        Command::Expand => match reader.bytes("expand") {
                            Ok(bytes) => {
                                print!("{}", expand::render(bytes));
                                ExitCode::SUCCESS
                            }
                            Err(error) => {
                                eprint!(
                                    "{}",
                                    render_error(&renderer, &error, options.stdin.as_deref())
                                );
                                ExitCode::from(1)
                            }
                        },
                        Command::Dump | Command::Validate | Command::Fmt => unreachable!(),
                    }
                }
            }
        }
        Err(error) => {
            eprintln!("error: {error}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

fn parse(args: impl Iterator<Item = String>) -> Result<ParseResult, String> {
    let mut args = args.peekable();
    let Some(command) = args.next() else {
        return Ok(ParseResult::Help);
    };
    if command == "-h" || command == "--help" {
        return Ok(ParseResult::Help);
    }
    if command == "-V" || command == "--version" {
        return Ok(ParseResult::Version);
    }

    let command = match command.as_str() {
        "check" => Command::Check,
        "build" => Command::Build,
        "expand" => Command::Expand,
        "dump" => Command::Dump,
        "validate" => Command::Validate,
        "fmt" => Command::Fmt,
        _ => return Err(format!("unknown command `{command}`")),
    };

    let mut inputs = vec![];
    let mut references = vec![];
    let mut output = None;
    let mut reference_default = true;
    let mut format_check = false;
    let mut validation_profile = metadata::validator::ValidationProfile::Common;
    let mut diagnostic_color = Color::Auto;
    let mut diagnostic_format = Format::Human;
    let mut profile_set = false;
    let mut color_set = false;
    let mut format_set = false;
    let mut positional = false;

    while let Some(arg) = args.next() {
        if positional {
            inputs.push(PathBuf::from(arg));
            continue;
        }
        match arg.as_str() {
            "--" => positional = true,
            "-h" | "--help" => return Ok(ParseResult::Help),
            "-V" | "--version" => return Ok(ParseResult::Version),
            "--no-default" => reference_default = false,
            "--check" => format_check = true,
            "--color" => {
                let value = args
                    .next()
                    .ok_or_else(|| "`--color` requires a value".to_string())?;
                if color_set {
                    return Err("color policy may only be specified once".to_string());
                }
                diagnostic_color = Color::parse(&value).ok_or_else(|| {
                    format!("unknown color policy `{value}`; expected auto, always, or never")
                })?;
                color_set = true;
            }
            "--format" => {
                let value = args
                    .next()
                    .ok_or_else(|| "`--format` requires a name".to_string())?;
                if format_set {
                    return Err("diagnostic format may only be specified once".to_string());
                }
                diagnostic_format = Format::parse(&value).ok_or_else(|| {
                    format!("unknown diagnostic format `{value}`; expected human, short, or json")
                })?;
                format_set = true;
            }
            "--profile" => {
                let value = args
                    .next()
                    .ok_or_else(|| "`--profile` requires a name".to_string())?;
                if profile_set {
                    return Err("validation profile may only be specified once".to_string());
                }
                validation_profile = match value.as_str() {
                    "common" => metadata::validator::ValidationProfile::Common,
                    "win32" => metadata::validator::ValidationProfile::Win32,
                    "winrt" => metadata::validator::ValidationProfile::WinRT,
                    "windows" => metadata::validator::ValidationProfile::Windows,
                    _ => {
                        return Err(format!(
                            "unknown validation profile `{value}`; expected common, win32, winrt, or windows"
                        ));
                    }
                };
                profile_set = true;
            }
            "-r" | "--reference" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("`{arg}` requires a path"))?;
                references.push(PathBuf::from(value));
            }
            "-o" | "--out" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("`{arg}` requires a path"))?;
                if output.replace(PathBuf::from(value)).is_some() {
                    return Err("output may only be specified once".to_string());
                }
            }
            _ if arg.starts_with('-') && arg != "-" => {
                return Err(format!("unknown option `{arg}`"));
            }
            _ => inputs.push(PathBuf::from(arg)),
        }
    }

    if inputs.is_empty() {
        return Err("at least one input is required".to_string());
    }
    if inputs
        .iter()
        .filter(|input| *input == Path::new("-"))
        .count()
        > 1
    {
        return Err("standard input may only be specified once".to_string());
    }

    match command {
        Command::Check | Command::Dump | Command::Expand | Command::Validate
            if output.is_some() =>
        {
            return Err("`--out` is only valid with `riddle build`".to_string());
        }
        Command::Build => {
            let Some(path) = output.as_ref() else {
                return Err("`riddle build` requires `--out <path>`".to_string());
            };
            if path
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("winmd"))
            {
                return Err("build output must have a .winmd extension".to_string());
            }
        }
        Command::Fmt => {
            if output.is_some() {
                return Err("`--out` is only valid with `riddle build`".to_string());
            }
            if !references.is_empty() || !reference_default {
                return Err("references are not valid with `riddle fmt`".to_string());
            }
        }
        Command::Check | Command::Expand => {}
        Command::Dump => {
            if inputs.iter().any(|input| input == Path::new("-")) {
                return Err("standard input is not supported by `riddle dump`".to_string());
            }
            if !references.is_empty() || !reference_default {
                return Err("references are not valid with `riddle dump`".to_string());
            }
        }
        Command::Validate => {
            if inputs.iter().any(|input| input == Path::new("-")) {
                return Err("standard input is not supported by `riddle validate`".to_string());
            }
        }
    }
    if format_check && !matches!(command, Command::Fmt) {
        return Err("`--check` is only valid with `riddle fmt`".to_string());
    }
    if profile_set && !matches!(command, Command::Validate) {
        return Err("`--profile` is only valid with `riddle validate`".to_string());
    }

    Ok(ParseResult::Run(Options {
        command,
        inputs,
        references,
        output,
        reference_default,
        format_check,
        validation_profile,
        diagnostic_color,
        diagnostic_format,
        stdin: None,
    }))
}

fn dump_metadata(options: &Options) -> Result<String, Error> {
    let paths = windows_rdl::expand_input_files(&options.inputs, "winmd")?;
    Ok(expand::render_files(read_metadata_files(&paths)?))
}

fn validate_metadata(
    options: &Options,
) -> Result<(Vec<PathBuf>, Vec<metadata::validator::ValidationError>), Error> {
    let paths = windows_rdl::expand_input_files(&options.inputs, "winmd")?;
    let files = read_metadata_files(&paths)?;
    let index = metadata::reader::Index::new(files);

    let reference_paths = windows_rdl::expand_input_files(&options.references, "winmd")?;
    let mut references = read_metadata_files(&reference_paths)?;
    if options.reference_default {
        references.extend(
            [windows_default::WINRT, windows_default::WIN32]
                .into_iter()
                .map(|bytes| metadata::reader::File::new(bytes.to_vec()).unwrap()),
        );
    }

    let errors = if references.is_empty() {
        metadata::validator::Validator::new(&index)
            .profile(options.validation_profile)
            .validate()
    } else {
        let references = metadata::reader::Index::new(references);
        metadata::validator::Validator::new(&index)
            .references(&references)
            .profile(options.validation_profile)
            .validate()
    };
    Ok((paths, errors))
}

fn read_metadata_files(paths: &[PathBuf]) -> Result<Vec<metadata::reader::File>, Error> {
    paths
        .iter()
        .map(|path| {
            metadata::reader::File::try_read(path)
                .map_err(|error| Error::new(&error.to_string(), &path.to_string_lossy(), 0, 0))
        })
        .collect()
}

fn configure(reader: &mut Reader, options: &Options) {
    for input in &options.inputs {
        if input != Path::new("-") {
            reader.input(input);
        }
    }
    if let Some(source) = &options.stdin {
        reader.input_text_named("<stdin>", source);
    }
    reader.references(&options.references);
    if options.reference_default {
        reader.reference_default();
    }
}

fn metadata_diagnostics(
    paths: &[PathBuf],
    errors: &[metadata::validator::ValidationError],
) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(|error| {
            let path = metadata_path(paths, error.row());
            let mut diagnostic = Diagnostic::new(error.message(), &path, 0, 0).with_note(&format!(
                "metadata row {:?}[{}]",
                error.row().table(),
                error.row().row() + 1
            ));
            if let Some(related) = error.related() {
                diagnostic = diagnostic.with_label(Label::secondary(
                    &metadata_path(paths, related),
                    0,
                    0,
                    &format!(
                        "related metadata row {:?}[{}]",
                        related.table(),
                        related.row() + 1
                    ),
                ));
            }
            diagnostic
        })
        .collect()
}

fn metadata_path(paths: &[PathBuf], row: metadata::reader::RowId) -> String {
    paths.get(row.file()).map_or_else(
        || "<metadata>".to_string(),
        |path| path.to_string_lossy().into_owned(),
    )
}

fn render_error(renderer: &Renderer, error: &Error, stdin: Option<&str>) -> String {
    renderer.render(std::slice::from_ref(error.diagnostic()), None, stdin)
}

fn format_inputs(options: &Options) -> Result<Vec<Diagnostic>, Error> {
    let paths: Vec<_> = options
        .inputs
        .iter()
        .filter(|input| *input != Path::new("-"))
        .collect();
    let paths = windows_rdl::expand_input_files(&paths, "rdl")?;
    let mut outputs = vec![];

    for path in paths {
        let name = path.to_string_lossy().into_owned();
        let source = std::fs::read_to_string(&path)
            .map_err(|_| Error::new("failed to read input file", &name, 0, 0))?;
        let formatted = formatter::format_named(&name, &source)?;
        outputs.push((Some(path), name, source, formatted));
    }
    if let Some(source) = &options.stdin {
        let formatted = formatter::format_named("<stdin>", source)?;
        outputs.push((None, "<stdin>".to_string(), source.clone(), formatted));
    }

    if options.format_check {
        return Ok(outputs
            .iter()
            .filter(|(_, _, source, formatted)| source != formatted)
            .map(|(_, name, _, _)| Diagnostic::new("needs formatting", name, 0, 0))
            .collect());
    }

    for (path, _, source, formatted) in outputs {
        if let Some(path) = path {
            if source != formatted {
                windows_rdl::write_to_file(path, formatted)?;
            }
        } else {
            print!("{formatted}");
        }
    }
    Ok(vec![])
}
