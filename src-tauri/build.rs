use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../native-helper/Package.swift");
    println!("cargo:rerun-if-changed=../native-helper/Sources/CarlaNativeHelper/main.swift");
    println!("cargo:rerun-if-changed=../scripts/transcription_runtime.py");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_native_helper();
        ensure_python_runtime();
        stage_ffmpeg();
    }

    tauri_build::build()
}

fn build_native_helper() {
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let swift_configuration = if profile == "release" {
        "release"
    } else {
        "debug"
    };
    let helper_dir = PathBuf::from("../native-helper");

    let status = Command::new("swift")
        .current_dir(&helper_dir)
        .args([
            "build",
            "-c",
            swift_configuration,
            "--product",
            "CarlaNativeHelper",
        ])
        .status()
        .expect("failed to execute swift build for CarlaNativeHelper");

    assert!(status.success(), "swift build failed for CarlaNativeHelper");

    let source = helper_dir
        .join(".build")
        .join(swift_configuration)
        .join("CarlaNativeHelper");
    let destination = PathBuf::from("bin/CarlaNativeHelper");
    copy_executable(&source, &destination);
}

fn copy_executable(source: &Path, destination: &Path) {
    let parent = destination
        .parent()
        .expect("bin output path must have a parent directory");
    fs::create_dir_all(parent).expect("failed to create src-tauri/bin");
    fs::copy(source, destination).unwrap_or_else(|error| {
        panic!(
            "failed to copy CarlaNativeHelper from {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(destination)
            .expect("failed to stat copied CarlaNativeHelper")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(destination, permissions)
            .expect("failed to set executable bit on CarlaNativeHelper");
    }
}

fn ensure_python_runtime() {
    let venv_python = PathBuf::from("../.venv/bin/python");
    if !venv_python.exists() {
        let status = Command::new("uv")
            .args(["venv", "../.venv"])
            .status()
            .expect("failed to create Python virtualenv with uv");
        assert!(status.success(), "uv venv failed");
    }

    let status = Command::new(&venv_python)
        .args(["-c", "import mlx_whisper, whisper"])
        .status()
        .expect("failed to validate Python transcription runtime");

    if !status.success() {
        let install = Command::new("uv")
            .args([
                "pip",
                "install",
                "--python",
                "../.venv/bin/python",
                "mlx-whisper",
                "openai-whisper",
            ])
            .status()
            .expect("failed to install transcription runtime packages");
        assert!(
            install.success(),
            "uv pip install failed for transcription runtime"
        );
    }
}

fn stage_ffmpeg() {
    let source = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".local/bin/ffmpeg"))
        .filter(|path| path.exists())
        .or_else(|| {
            let path = PathBuf::from("/opt/homebrew/bin/ffmpeg");
            path.exists().then_some(path)
        })
        .unwrap_or_else(|| panic!("ffmpeg not found; install it before building Carla"));

    copy_executable(&source, &PathBuf::from("bin/ffmpeg"));
}
