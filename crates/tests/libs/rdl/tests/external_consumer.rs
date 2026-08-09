use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn write_winmd(name: &str, source: &str) -> PathBuf {
    let path = Path::new(env!("OUT_DIR")).join(format!("external_{name}.winmd"));
    windows_rdl::reader()
        .input_text(source)
        .output(&path)
        .write()
        .unwrap();
    path
}

fn run(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "command failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn external_consumers_accept_win32_and_winrt_metadata() {
    let win32 = write_winmd(
        "win32",
        r#"
#[win32]
mod Test {
    #[library("test.dll")]
    extern fn GetPoint() -> Point;

    struct Point {
        x: i32,
        y: i32,
    }
}
"#,
    );
    let winrt = write_winmd(
        "winrt",
        r#"
#[winrt]
mod Test {
    interface IWidget {
        fn Name(&self) -> String;
    }

    class Widget {
        #[default]
        IWidget,
    }
}
"#,
    );

    let scratch = Path::new(env!("OUT_DIR")).join("external_consumer");
    let consumer = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("external_consumer")
        .join("ExternalConsumer.csproj");
    let application = scratch.join("app");
    run(Command::new("dotnet")
        .arg("build")
        .arg(&consumer)
        .arg("--configuration")
        .arg("Release")
        .arg("--nologo")
        .arg("--output")
        .arg(&application)
        .arg(format!(
            "-p:BaseIntermediateOutputPath={}\\",
            scratch.join("obj").display()
        ))
        .arg(format!(
            "-p:MSBuildProjectExtensionsPath={}\\",
            scratch.join("obj").display()
        )));
    let output = run(Command::new("dotnet")
        .arg(application.join("ExternalConsumer.dll"))
        .arg(&win32)
        .arg(&winrt));
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "external_win32.winmd\n\
         field:Test.Point.x\n\
         field:Test.Point.y\n\
         kind:WindowsMetadata\n\
         method:Test.Apis.GetPoint\n\
         type:Test.Apis\n\
         type:Test.Point\n\
         external_winrt.winmd\n\
         kind:WindowsMetadata\n\
         method:Test.IWidget.Name\n\
         method:Test.Widget.Name\n\
         type:Test.IWidget\n\
         type:Test.Widget\n"
    );

    let projection = scratch.join("cppwinrt");
    std::fs::create_dir_all(&projection).unwrap();
    run(Command::new("cppwinrt.exe")
        .arg("-input")
        .arg(&winrt)
        .arg("-output")
        .arg(&projection));
    let header = std::fs::read_to_string(projection.join("winrt").join("Test.h")).unwrap();
    assert!(header.contains("consume_Test_IWidget"));
    assert!(header.contains("winrt::Test::Widget"));
}
