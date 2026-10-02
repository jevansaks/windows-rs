use std::path::{Path, PathBuf};
use windows_clang::{Input, Snapshot, extract};

fn scratch(name: &str) -> PathBuf {
    let scratch = std::env::temp_dir().join(format!(
        "windows-clang-inclusions-{name}-{}",
        std::process::id()
    ));
    if scratch.exists() {
        std::fs::remove_dir_all(&scratch).unwrap();
    }
    std::fs::create_dir_all(&scratch).unwrap();
    scratch
}

fn normalized(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn paths_for_input<'a>(snapshot: &'a Snapshot, input: &str) -> Vec<&'a str> {
    snapshot
        .included_files()
        .iter()
        .filter(|file| file.input == input)
        .map(|file| file.path.as_str())
        .collect()
}

#[test]
fn included_files_record_direct_and_transitive_headers() {
    helpers::ensure_libclang();

    let scratch = scratch("transitive");
    let main = scratch.join("main.cpp");
    let direct = scratch.join("direct.h");
    let transitive = scratch.join("transitive.h");
    let skipped = scratch.join("skipped.h");
    std::fs::write(&main, "").unwrap();
    std::fs::write(&direct, "#include \"transitive.h\"\n").unwrap();
    std::fs::write(&transitive, "// no declarations\n").unwrap();
    std::fs::write(&skipped, "// not visited\n").unwrap();

    let input = normalized(&main);
    let include = format!("-I{}", scratch.display());
    let snapshot = extract(
        [Input::new(
            &input,
            "#include \"direct.h\"\n#if 0\n#include \"skipped.h\"\n#endif\n",
        )],
        &["-x", "c++", &include],
    )
    .unwrap();
    let paths = paths_for_input(&snapshot, &input);

    assert!(paths.contains(&normalized(&direct).as_str()), "{paths:#?}");
    assert!(
        paths.contains(&normalized(&transitive).as_str()),
        "{paths:#?}"
    );
    assert!(
        !paths.contains(&normalized(&skipped).as_str()),
        "{paths:#?}"
    );
    assert!(
        snapshot.facts().iter().all(|fact| {
            fact.spelling.file != normalized(&direct)
                && fact.spelling.file != normalized(&transitive)
        }),
        "{}",
        snapshot.dump()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn included_files_preserve_each_input_identity() {
    helpers::ensure_libclang();

    let scratch = scratch("inputs");
    let shared = scratch.join("shared.h");
    let first = scratch.join("z-first.cpp");
    let second = scratch.join("a-second.cpp");
    std::fs::write(&shared, "typedef int SHARED;\n").unwrap();
    std::fs::write(&first, "").unwrap();
    std::fs::write(&second, "").unwrap();

    let first = normalized(&first);
    let second = normalized(&second);
    let shared = normalized(&shared);
    let include = format!("-I{}", scratch.display());
    let snapshot = extract(
        [
            Input::new(&first, "#include \"shared.h\"\n"),
            Input::new(&second, "#include \"shared.h\"\n"),
        ],
        &["-x", "c++", &include],
    )
    .unwrap();

    let shared_inputs: Vec<_> = snapshot
        .included_files()
        .iter()
        .filter(|file| file.path == shared)
        .map(|file| file.input.as_str())
        .collect();
    assert_eq!(shared_inputs, [&first, &second]);
    assert!(
        snapshot
            .included_files()
            .iter()
            .skip_while(|file| file.input == first)
            .all(|file| file.input == second),
        "{:#?}",
        snapshot.included_files()
    );

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn included_files_are_normalized_deduplicated_and_deterministic() {
    helpers::ensure_libclang();

    let scratch = scratch("deterministic");
    let main = scratch.join("main.cpp");
    let alpha = scratch.join("Alpha.h");
    let zeta = scratch.join("zeta.h");
    std::fs::write(&main, "").unwrap();
    std::fs::write(&alpha, "// alpha\n").unwrap();
    std::fs::write(&zeta, "// zeta\n").unwrap();

    let input = normalized(&main);
    let source =
        "#include \"zeta.h\"\n#include \"Alpha.h\"\n#include \"zeta.h\"\n#define VALUE 1\n";
    let include = format!("-I{}", scratch.display());
    let first = extract([Input::new(&input, source)], &["-x", "c++", &include]).unwrap();
    let second = extract([Input::new(&input, source)], &["-x", "c++", &include]).unwrap();

    assert_eq!(first.included_files(), second.included_files());
    assert!(
        first
            .included_files()
            .iter()
            .all(|file| !file.path.contains('\\') && !file.path.contains(".__clang_eval.cpp")),
        "{:#?}",
        first.included_files()
    );
    let headers: Vec<_> = paths_for_input(&first, &input)
        .into_iter()
        .filter(|path| path.ends_with(".h"))
        .collect();
    assert_eq!(headers, [normalized(&alpha), normalized(&zeta)]);

    std::fs::remove_dir_all(scratch).unwrap();
}
