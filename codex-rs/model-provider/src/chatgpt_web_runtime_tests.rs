use std::fs;

use tempfile::TempDir;

use super::latest_launcher_in;
#[cfg(windows)]
use super::launcher_command;

#[test]
fn latest_launcher_in_prefers_the_highest_installed_version() {
    let temp = TempDir::new().expect("temp dir");
    let versions = temp.path().join("versions");
    let older = versions.join("6.9.0-win32-x64").join("bin");
    let newer = versions.join("6.10.0-win32-x64").join("bin");
    fs::create_dir_all(&older).expect("older dir");
    fs::create_dir_all(&newer).expect("newer dir");

    let launcher = if cfg!(windows) {
        "codex-chatgpt-web.cmd"
    } else {
        "codex-chatgpt-web"
    };
    fs::write(older.join(launcher), "").expect("older launcher");
    fs::write(newer.join(launcher), "").expect("newer launcher");

    assert_eq!(latest_launcher_in(temp.path()), Some(newer.join(launcher)));
}

#[test]
fn latest_launcher_in_ignores_versions_without_a_launcher() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("versions").join("9.0.0").join("bin"))
        .expect("incomplete version");

    assert_eq!(latest_launcher_in(temp.path()), None);
}

#[cfg(windows)]
#[test]
fn windows_launcher_command_preserves_quoted_batch_path() {
    let temp = TempDir::new().expect("temp dir");
    let dir = temp.path().join("launcher with spaces");
    fs::create_dir_all(&dir).expect("launcher dir");
    let launcher = dir.join("codex-chatgpt-web.cmd");
    fs::write(
        &launcher,
        "@echo off\r\nif \"%1\"==\"serve\" exit /b 37\r\nexit /b 99\r\n",
    )
    .expect("launcher");

    let status = launcher_command(&launcher)
        .status()
        .expect("launcher command should start");

    assert_eq!(status.code(), Some(37));
}
