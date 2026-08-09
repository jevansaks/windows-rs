use std::io::Write;
use std::process::{Command, Stdio};

fn scratch(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("riddle_cli_{}_{}", std::process::id(), name))
}

fn riddle() -> Command {
    Command::new(env!("CARGO_BIN_EXE_riddle"))
}

#[test]
fn check_and_build_valid_rdl() {
    let dir = scratch("valid");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    let output = dir.join("api.winmd");
    std::fs::write(&input, "#[win32] mod Test { struct Value { field: i32 } }").unwrap();

    assert!(
        riddle()
            .arg("check")
            .arg(&input)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        riddle()
            .arg("build")
            .arg(&input)
            .arg("--out")
            .arg(&output)
            .status()
            .unwrap()
            .success()
    );
    assert!(output.is_file());
}

#[test]
fn build_emits_static_global_function_signature() {
    let dir = scratch("global_function");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    let output = dir.join("api.winmd");
    std::fs::write(
        &input,
        "#[win32] mod Test { #[library(\"test.dll\")] extern fn GetValue(value: i32) -> i32; }",
    )
    .unwrap();

    assert!(
        riddle()
            .args(["build", "--no-default"])
            .arg(&input)
            .arg("--out")
            .arg(&output)
            .status()
            .unwrap()
            .success()
    );

    let index = windows_metadata::reader::Index::read(&output).unwrap();
    let method = index
        .expect("Test", "Apis")
        .methods()
        .find(|method| method.name() == "GetValue")
        .unwrap();
    assert!(
        method
            .flags()
            .contains(windows_metadata::MethodAttributes::Static)
    );
    assert!(
        !method
            .signature(&[])
            .flags
            .contains(windows_metadata::MethodCallAttributes::HASTHIS)
    );
}

#[test]
fn expand_shows_lowered_winrt_abi() {
    let dir = scratch("expand");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    let metadata = dir.join("api.winmd");
    std::fs::write(
        &input,
        "#[winrt] mod Test {
            delegate fn ChangedHandler(sender: Object);
            #[Windows::Foundation::Metadata::ExclusiveTo(Value)]
            interface IValue {
                Name: String;
                event Changed: ChangedHandler;
                #[overload(Get)]
                #[default_overload]
                fn Get(&self, value: i32);
                #[overload(GetWithString)]
                fn Get(&self, value: String);
            }
            interface IValueStatics {
                fn Create(&self) -> Value;
            }
            #[Windows::Foundation::Metadata::Activatable(1)]
            #[Windows::Foundation::Metadata::Static(IValueStatics, 1)]
            class Value {
                IValue,
            }
        }
        #[winrt] mod Windows {
            mod Foundation {
                mod Metadata {
                    attribute ActivatableAttribute {
                        fn(version: u32);
                    }
                    attribute ExclusiveToAttribute {
                        fn(r#type: Type);
                    }
                    attribute StaticAttribute {
                        fn(r#type: Type, version: u32);
                    }
                }
            }
        }",
    )
    .unwrap();

    let output = riddle()
        .args(["expand", "--no-default"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("type Test.IValue category=Interface"));
    assert!(stdout.contains("method get_Name() -> String"));
    assert!(stdout.contains("method put_Name(value: String) -> void"));
    assert!(stdout.contains("method add_Changed(handler: Test.ChangedHandler)"));
    assert!(
        stdout.contains("method remove_Changed(token: Windows.Foundation.EventRegistrationToken)")
    );
    assert!(stdout.contains("method GetWithString(value: String) -> void"));
    assert!(stdout.contains("projected=Get"));
    assert!(stdout.contains("property Name: String"));
    assert!(stdout.contains("event Changed: Test.ChangedHandler"));
    assert!(stdout.contains("type Test.Value category=Class"));
    assert!(stdout.contains("method .ctor() -> void flags=0x1886 impl=0x0003 call=0x20"));
    assert!(stdout.contains(
        "method GetWithString(value: String) -> void flags=0x01e6 impl=0x0003 call=0x20"
    ));
    assert!(stdout.contains("method Create() -> Test.Value flags=0x0096 impl=0x0003 call=0x00"));

    assert!(
        riddle()
            .args(["build", "--no-default"])
            .arg(&input)
            .arg("--out")
            .arg(&metadata)
            .status()
            .unwrap()
            .success()
    );
    let dumped = riddle().arg("dump").arg(&metadata).output().unwrap();
    assert!(dumped.status.success());
    assert_eq!(stdout.as_bytes(), dumped.stdout);
}

#[test]
fn validate_accepts_valid_metadata_directory() {
    let dir = scratch("validate_valid");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.winmd");
    let mut file = windows_metadata::writer::File::new("test");
    file.TypeDef(
        "Test",
        "Value",
        windows_metadata::writer::TypeDefOrRef::default(),
        windows_metadata::TypeAttributes::Public,
    );
    std::fs::write(&input, file.into_stream()).unwrap();

    assert!(
        riddle()
            .args(["validate", "--no-default", "--profile", "common"])
            .arg(&dir)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn validation_profile_is_validate_only() {
    let invalid = riddle()
        .args(["validate", "--profile", "other", "api.winmd"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("unknown validation profile `other`")
    );

    let wrong_command = riddle()
        .args(["check", "--profile", "winrt", "api.rdl"])
        .output()
        .unwrap();
    assert_eq!(wrong_command.status.code(), Some(2));
    assert!(
        String::from_utf8(wrong_command.stderr)
            .unwrap()
            .contains("`--profile` is only valid with `riddle validate`")
    );
}

#[test]
fn validate_reports_metadata_rows() {
    let dir = scratch("validate_invalid");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.winmd");
    let mut file = windows_metadata::writer::File::new("test");
    for _ in 0..2 {
        file.TypeDef(
            "Test",
            "Value",
            windows_metadata::writer::TypeDefOrRef::default(),
            windows_metadata::TypeAttributes::Public,
        );
    }
    std::fs::write(&input, file.into_stream()).unwrap();

    let output = riddle()
        .args(["validate", "--no-default"])
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error: duplicate type `Test.Value`"));
    assert!(stderr.contains(&input.to_string_lossy().to_string()));
    assert!(stderr.contains("metadata row TypeDef["));
    assert!(stderr.contains("related metadata row TypeDef["));
}

#[test]
fn validate_rejects_invalid_metadata_file() {
    let dir = scratch("validate_bad_file");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.winmd");
    std::fs::write(&input, b"not metadata").unwrap();

    let output = riddle().arg("validate").arg(&input).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error: invalid metadata"));
    assert!(stderr.contains(&input.to_string_lossy().to_string()));
}

#[test]
fn invalid_rdl_uses_terminal_diagnostic() {
    let dir = scratch("invalid");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(&input, "#[win32] mod Test { struct Value { field: } }").unwrap();

    let output = riddle().arg("check").arg(&input).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error:"));
    assert!(stderr.contains(&input.to_string_lossy().to_string()));
    assert!(stderr.contains(" --> "));
    assert!(stderr.contains(" | "));
    assert!(stderr.contains('^'));
}

#[test]
fn short_diagnostics_use_one_line_per_error() {
    let dir = scratch("short_diagnostics");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(&input, "#[win32] mod Test { struct Value { field: } }").unwrap();

    let output = riddle()
        .args(["check", "--format", "short", "--color", "never"])
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with(&input.to_string_lossy().to_string()));
    assert!(stderr.contains(": error:"));
    assert!(!stderr.contains(" | "));
    assert!(stderr.ends_with("error: aborting due to 1 previous error\n"));
}

#[test]
fn json_diagnostics_are_machine_readable() {
    let dir = scratch("json_diagnostics");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(&input, "#[win32] mod Test { struct Value { field: } }").unwrap();

    let output = riddle()
        .args(["check", "--format", "json", "--color", "always"])
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["summary"]["errors"], 1);
    assert_eq!(value["summary"]["warnings"], 0);
    assert_eq!(value["diagnostics"][0]["severity"], "error");
    assert_eq!(
        value["diagnostics"][0]["source"]["name"],
        input.to_string_lossy().as_ref()
    );
    assert!(!output.stderr.contains(&0x1b));
}

#[test]
fn color_policy_controls_ansi_output() {
    let dir = scratch("color_diagnostics");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(&input, "#[win32] mod Test { struct Value { field: } }").unwrap();

    let always = riddle()
        .args(["check", "--color", "always"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(always.stderr.contains(&0x1b));

    let never = riddle()
        .args(["check", "--color", "never"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(!never.stderr.contains(&0x1b));
}

#[test]
fn invalid_arguments_use_exit_code_two() {
    let output = riddle().arg("build").arg("api.rdl").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("requires `--out <path>`")
    );

    let format = riddle()
        .args(["check", "--format", "other", "api.rdl"])
        .output()
        .unwrap();
    assert_eq!(format.status.code(), Some(2));
    assert!(
        String::from_utf8(format.stderr)
            .unwrap()
            .contains("unknown diagnostic format `other`")
    );

    let color = riddle()
        .args(["check", "--color", "sometimes", "api.rdl"])
        .output()
        .unwrap();
    assert_eq!(color.status.code(), Some(2));
    assert!(
        String::from_utf8(color.stderr)
            .unwrap()
            .contains("unknown color policy `sometimes`")
    );
}

#[test]
fn duplicate_diagnostic_renders_both_labels() {
    let dir = scratch("duplicate");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(
        &input,
        "#[win32]\nmod Test {\n    struct Value {}\n    struct Value {}\n}\n",
    )
    .unwrap();

    let output = riddle().arg("check").arg(&input).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error[RDL0001]:"));
    assert!(stderr.contains("first declared here"));
    assert!(stderr.matches(" --> ").count() >= 2);
    assert!(stderr.contains('^'));
    assert!(stderr.contains('-'));
}

#[test]
fn check_renders_every_independent_error() {
    let dir = scratch("multiple_errors");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(
        &input,
        "#[win32]\nmod Test {\n\
         struct First { value: i32, value: i32, }\n\
         struct Second { value: i32, value: i32, }\n\
         }\n",
    )
    .unwrap();

    let output = riddle().arg("check").arg(&input).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.matches("error[RDL0001]:").count(), 2);
    assert!(stderr.contains("aborting due to 2 previous errors"));
}

#[test]
fn check_renders_warnings_without_failing() {
    let dir = scratch("warnings");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(
        &input,
        "use Other::Unused;\n\
         #[win32] mod Test { struct Value {} }\n\
         #[win32] mod Other { struct Unused {} }\n",
    )
    .unwrap();

    let output = riddle()
        .args(["check", "--no-default", "--color", "never"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("warning[RDL0006]: unused import"));
    assert!(stderr.ends_with("warning: 1 warning emitted\n"));
}

#[test]
fn check_reports_finalized_metadata_validation() {
    let dir = scratch("metadata_validation");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(
        &input,
        "#[win32]\nmod Test {\n\
         attribute MarkerAttribute { fn(); Value: i32, }\n\
         #[Marker(Value = 1, Value = 2)]\n\
         struct Item {}\n\
         }\n",
    )
    .unwrap();

    let output = riddle().arg("check").arg(&input).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error[RDL0001]:"));
    assert!(stderr.contains("duplicate named field argument `Value`"));
    assert!(stderr.contains(&input.to_string_lossy().to_string()));
    assert!(stderr.contains("metadata row Attribute["));
    assert!(stderr.contains('^'));
}

#[test]
fn check_reports_invalid_overload_metadata() {
    let dir = scratch("overload_validation");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(
        &input,
        "#[winrt]\nmod Test {\n\
         interface IValue {\n\
         #[overload(GetFirst)]\n\
         fn Get(&self, value: i32);\n\
         #[overload(GetSecond)]\n\
         fn Get(&self, value: i32);\n\
         }\n\
         }\n",
    )
    .unwrap();

    let output = riddle()
        .args(["check", "--no-default"])
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("error[RDL0001]:"));
    assert!(stderr.contains("duplicate overload signature `Get`"));
    assert!(stderr.contains("first declared here"));
    assert!(stderr.matches(" --> ").count() >= 2);
}

#[test]
fn check_accepts_standard_input() {
    let mut child = riddle()
        .args(["check", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"#[win32] mod Test { struct Value {} }")
        .unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn build_accepts_directory_and_reference_inputs() {
    let dir = scratch("references");
    let dependency_dir = dir.join("dependency");
    let source_dir = dir.join("source");
    std::fs::create_dir_all(&dependency_dir).unwrap();
    std::fs::create_dir_all(&source_dir).unwrap();
    let dependency = dir.join("dependency.winmd");
    let output = dir.join("api.winmd");
    std::fs::write(
        dependency_dir.join("dependency.rdl"),
        "#[win32] mod Dependency { struct Value { field: i32 } }",
    )
    .unwrap();
    std::fs::write(
        source_dir.join("api.rdl"),
        "#[win32] mod Test { struct UsesValue { value: Dependency::Value } }",
    )
    .unwrap();

    assert!(
        riddle()
            .args(["build", "--no-default"])
            .arg(&dependency_dir)
            .arg("--out")
            .arg(&dependency)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        riddle()
            .args(["build", "--no-default", "--reference"])
            .arg(&dependency)
            .arg(&source_dir)
            .arg("--out")
            .arg(&output)
            .status()
            .unwrap()
            .success()
    );
    assert!(output.is_file());
}

#[test]
fn fmt_checks_and_updates_files_with_comments() {
    let dir = scratch("fmt");
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("api.rdl");
    std::fs::write(
        &input,
        "/// API\n#[win32] mod Test { struct Value { field:i32, // Field\n} }",
    )
    .unwrap();

    let check = riddle()
        .args(["fmt", "--check"])
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
    assert!(
        String::from_utf8(check.stderr)
            .unwrap()
            .contains("needs formatting")
    );

    assert!(riddle().arg("fmt").arg(&input).status().unwrap().success());
    let formatted = std::fs::read_to_string(&input).unwrap();
    assert!(formatted.contains("/// API"));
    assert!(formatted.contains("field: i32, // Field"));
    assert!(
        riddle()
            .args(["fmt", "--check"])
            .arg(&input)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn fmt_does_not_modify_any_file_when_one_is_invalid() {
    let dir = scratch("fmt_invalid");
    std::fs::create_dir_all(&dir).unwrap();
    let valid = dir.join("a.rdl");
    let invalid = dir.join("b.rdl");
    let original = "#[win32] mod Test { struct Value { field:i32 } }";
    std::fs::write(&valid, original).unwrap();
    std::fs::write(&invalid, "#[win32] mod Test { struct Broken { field: } }").unwrap();

    let output = riddle().arg("fmt").arg(&dir).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(std::fs::read_to_string(valid).unwrap(), original);
}

#[test]
fn fmt_writes_standard_input_to_standard_output() {
    let mut child = riddle()
        .args(["fmt", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"#[win32] mod Test { struct Value {} }")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "#[win32]\nmod Test {\n    struct Value {}\n}\n"
    );
}
