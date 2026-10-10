use std::collections::BTreeSet;
use windows_clang::{
    ExtractionOptions, Input, extract, extract_partitioned_with_options, extract_with_options,
};

#[test]
fn parallel_extraction_preserves_input_order_and_output() {
    helpers::ensure_libclang();

    let scratch =
        std::env::temp_dir().join(format!("windows-clang-parallel-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(
        scratch.join("shared.h"),
        "typedef struct SHARED { unsigned value; } SHARED;\n",
    )
    .unwrap();
    for index in 0..4 {
        std::fs::write(
            scratch.join(format!("api{index}.h")),
            format!(
                "#include \"shared.h\"\n\
                 typedef struct API{index} {{ SHARED shared; }} API{index};\n"
            ),
        )
        .unwrap();
    }

    let inputs: Vec<_> = (0..4)
        .map(|index| {
            Input::new(
                format!("input-{index}.cpp"),
                format!("#include \"api{index}.h\"\n"),
            )
            .with_root_dirs([scratch.to_string_lossy().to_string()])
        })
        .collect();
    let include = format!("-I{}", scratch.display());
    let args = ["-x", "c++", include.as_str()];
    let serial = extract(inputs.clone(), &args).unwrap();
    let parallel =
        extract_with_options(inputs, &args, &ExtractionOptions::new().with_parallelism(4)).unwrap();

    assert_eq!(serial, parallel);
    assert_eq!(serial.included_files(), parallel.included_files());
    assert_eq!(
        serial.emit("Parallel").unwrap(),
        parallel.emit("Parallel").unwrap()
    );
    let observed_inputs: Vec<_> = parallel
        .included_files()
        .iter()
        .map(|file| file.input.clone())
        .collect();
    let expected_inputs: Vec<_> = (0..4)
        .flat_map(|index| {
            let input = format!("input-{index}.cpp");
            std::iter::repeat_n(input, 3)
        })
        .collect();
    assert_eq!(observed_inputs, expected_inputs);

    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn parallel_extraction_reports_errors_in_input_order() {
    helpers::ensure_libclang();

    let error = extract_with_options(
        [
            Input::new("first.cpp", "#error FIRST_FAILURE\n"),
            Input::new("second.cpp", "#error SECOND_FAILURE\n"),
        ],
        &["-x", "c++"],
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("FIRST_FAILURE"), "{error}");
    assert!(!error.contains("SECOND_FAILURE"), "{error}");
}

#[test]
fn partitioned_extraction_uses_bounded_parallel_parsing() {
    helpers::ensure_libclang();

    let snapshot = extract_partitioned_with_options(
        [
            Input::new(
                "first.h",
                "typedef struct FIRST { unsigned value; } FIRST;\n",
            )
            .partitioned("first")
            .with_root("first.h", "first", "Parallel.First"),
            Input::new(
                "second.h",
                "typedef struct SECOND { unsigned value; } SECOND;\n",
            )
            .partitioned("second")
            .with_root("second.h", "second", "Parallel.Second"),
        ],
        &["-x", "c++"],
        &ExtractionOptions::new().with_parallelism(2),
    )
    .unwrap();

    assert!(snapshot.facts().iter().any(|fact| fact.name == "FIRST"));
    assert!(snapshot.facts().iter().any(|fact| fact.name == "SECOND"));
    assert_eq!(snapshot.included_files()[0].input, "first.h");
    assert_eq!(snapshot.included_files()[1].input, "second.h");
}

#[test]
fn excluded_source_directories_keep_inclusion_provenance() {
    helpers::ensure_libclang();

    let scratch =
        std::env::temp_dir().join(format!("windows-clang-excluded-{}", std::process::id()));
    let resource = scratch.join("clang-resource").join("include");
    let sdk = scratch.join("sdk");
    std::fs::create_dir_all(&resource).unwrap();
    std::fs::create_dir_all(&sdk).unwrap();
    std::fs::write(
        resource.join("resource.h"),
        "#define RESOURCE_VALUE 1\n\
         typedef struct RESOURCE_ONLY { int value; } RESOURCE_ONLY;\n",
    )
    .unwrap();
    std::fs::write(
        sdk.join("api.h"),
        "typedef struct SDK_ONLY { unsigned value; } SDK_ONLY;\n",
    )
    .unwrap();

    let include_resource = format!("-I{}", resource.display());
    let include_sdk = format!("-I{}", sdk.display());
    let snapshot = extract(
        [Input::new(
            "aggregate.cpp",
            "#include \"resource.h\"\n#include \"api.h\"\n",
        )
        .with_root_dirs([scratch.to_string_lossy().to_string()])
        .with_excluded_source_dirs([resource.to_string_lossy().to_ascii_uppercase()])],
        &["-x", "c++", include_resource.as_str(), include_sdk.as_str()],
    )
    .unwrap();
    let rdl = snapshot.emit("Excluded").unwrap();

    assert!(rdl.contains("struct SDK_ONLY"), "{rdl}");
    assert!(!rdl.contains("RESOURCE_ONLY"), "{rdl}");
    assert!(!rdl.contains("RESOURCE_VALUE"), "{rdl}");
    let included: BTreeSet<_> = snapshot
        .included_files()
        .iter()
        .map(|file| file.path.to_ascii_lowercase())
        .collect();
    assert!(
        included.contains(
            &resource
                .join("resource.h")
                .to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase()
        ),
        "{included:?}"
    );

    std::fs::remove_dir_all(scratch).unwrap();
}
