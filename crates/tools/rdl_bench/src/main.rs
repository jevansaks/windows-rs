use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows_metadata as metadata;

const WINRT_RDL: &str = "metadata/winrt";
const WINRT_WINMD: &str = "crates/libs/default/Windows.winmd";
const OUTPUT_DIR: &str = "target/rdl-bench";

struct Corpus {
    files: Vec<(String, String)>,
    bytes: usize,
}

struct Measurement {
    name: &'static str,
    median: Duration,
    min: Duration,
    max: Duration,
}

fn main() {
    require_release();

    let samples = samples();
    let corpus = load_corpus(WINRT_RDL);
    let winmd = std::fs::read(WINRT_WINMD)
        .unwrap_or_else(|error| panic!("failed to read `{WINRT_WINMD}`: {error}"));
    let output = Path::new(OUTPUT_DIR);
    std::fs::create_dir_all(output)
        .unwrap_or_else(|error| panic!("failed to create `{OUTPUT_DIR}`: {error}"));

    let reader = build_reader(&corpus);
    let check = measure("check WinRT RDL", samples, || {
        reader
            .check()
            .unwrap_or_else(|error| panic!("WinRT RDL check failed: {error}"));
    });

    let validate = measure("validate Windows.winmd", samples, || {
        let file = metadata::reader::File::try_new(winmd.clone())
            .unwrap_or_else(|error| panic!("failed to read Windows metadata: {error}"));
        let index = metadata::reader::Index::new(vec![file]);
        let errors = metadata::validator::Validator::new(&index)
            .profile(metadata::validator::ValidationProfile::WinRT)
            .validate();
        assert!(errors.is_empty(), "Windows metadata validation failed");
        black_box(index);
    });

    let format = measure("format WinRT RDL", samples, || {
        let mut output_bytes = 0;
        for (name, source) in &corpus.files {
            let formatted = windows_rdl::formatter::format_named(name, source)
                .unwrap_or_else(|error| panic!("failed to format `{name}`: {error}"));
            output_bytes += formatted.len();
        }
        assert_eq!(black_box(output_bytes), corpus.bytes);
    });

    let dump_dir = output.join("dump");
    dump(&winmd, &dump_dir);
    let dumped = load_corpus(&dump_dir);
    let dumped_reader = build_reader(&dumped);

    let dump = measure("dump Windows.winmd", samples, || dump(&winmd, &dump_dir));
    let build = measure("build dumped WinRT RDL", samples, || {
        let bytes = dumped_reader
            .bytes("Windows")
            .unwrap_or_else(|error| panic!("failed to build dumped WinRT RDL: {error}"));
        black_box(bytes);
    });

    println!("RDL release benchmarks: {samples} samples, median shown, one warmup excluded");
    println!(
        "inputs: {} RDL files, {} bytes; Windows.winmd, {} bytes; dumped RDL, {} bytes",
        corpus.files.len(),
        corpus.bytes,
        winmd.len(),
        dumped.bytes
    );
    println!();
    println!(
        "{:<28} {:>10} {:>10} {:>10}",
        "workload", "median (s)", "min (s)", "max (s)"
    );
    for measurement in [check, validate, format, dump, build] {
        println!(
            "{:<28} {:>10.3} {:>10.3} {:>10.3}",
            measurement.name,
            measurement.median.as_secs_f64(),
            measurement.min.as_secs_f64(),
            measurement.max.as_secs_f64()
        );
    }
}

fn require_release() {
    #[cfg(debug_assertions)]
    panic!("benchmarks require a release build: cargo run -p tool_rdl_bench --release");
}

fn samples() -> usize {
    let mut args = std::env::args().skip(1);
    let mut samples = 5;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--samples" => {
                let value = args.next().unwrap_or_else(|| {
                    panic!("expected a positive integer after `--samples`");
                });
                samples = value
                    .parse()
                    .ok()
                    .filter(|value| *value > 0)
                    .unwrap_or_else(|| panic!("invalid sample count `{value}`"));
            }
            _ => panic!("unknown argument `{arg}`"),
        }
    }

    samples
}

fn build_reader(corpus: &Corpus) -> windows_rdl::Reader {
    let mut reader = windows_rdl::reader();
    reader.input_texts_named(
        corpus
            .files
            .iter()
            .map(|(name, source)| (name.as_str(), source.as_str())),
    );
    reader
}

fn dump(winmd: &[u8], output: &Path) {
    std::fs::create_dir_all(output)
        .unwrap_or_else(|error| panic!("failed to create `{}`: {error}", output.display()));
    windows_rdl::writer()
        .input_bytes(winmd)
        .split()
        .output(output)
        .write()
        .unwrap_or_else(|error| panic!("failed to dump Windows metadata: {error}"));
}

fn load_corpus(path: impl AsRef<Path>) -> Corpus {
    let path = path.as_ref();
    let mut paths = Vec::new();
    collect_rdl(path, &mut paths);
    paths.sort();

    let mut files = Vec::with_capacity(paths.len());
    let mut bytes = 0;
    for path in paths {
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read `{}`: {error}", path.display()));
        bytes += source.len();
        files.push((path.to_string_lossy().into_owned(), source));
    }

    assert!(
        !files.is_empty(),
        "no RDL files found under `{}`",
        path.display()
    );
    Corpus { files, bytes }
}

fn collect_rdl(path: &Path, paths: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().is_some_and(|extension| extension == "rdl") {
            paths.push(path.to_path_buf());
        }
        return;
    }

    let entries = std::fs::read_dir(path)
        .unwrap_or_else(|error| panic!("failed to read `{}`: {error}", path.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("failed to read directory entry: {error}"))
            .path();
        if path.is_dir() {
            collect_rdl(&path, paths);
        } else if path.extension().is_some_and(|extension| extension == "rdl") {
            paths.push(path);
        }
    }
}

fn measure(name: &'static str, samples: usize, mut operation: impl FnMut()) -> Measurement {
    operation();

    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        operation();
        durations.push(start.elapsed());
    }
    durations.sort_unstable();

    Measurement {
        name,
        median: durations[durations.len() / 2],
        min: durations[0],
        max: durations[durations.len() - 1],
    }
}
