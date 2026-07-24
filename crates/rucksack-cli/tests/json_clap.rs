use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn run_rucksack(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rucksack"))
        .args(arguments)
        .env("NO_COLOR", "1")
        .output()
        .expect("rucksack should run")
}

fn run_rucksack_in_home(arguments: &[&str], home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rucksack"))
        .args(arguments)
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .output()
        .expect("rucksack should run")
}

#[test]
fn json_parser_failure_emits_one_terminal_result_with_clap_exit_code() {
    let output = run_rucksack(&["--json", "setup", "--hotspot", "Noah", "--usb"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);

    let result = serde_json::from_str::<Value>(lines[0]).expect("stdout should contain JSON");
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["type"], "result");
    assert_eq!(result["ok"], false);
    assert!(result["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("cannot be used with '--usb'")));
}

#[test]
fn human_parser_failure_keeps_clap_output_and_exit_code() {
    let output = run_rucksack(&["setup", "--hotspot", "Noah", "--usb"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());

    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("cannot be used with '--usb'"));
}

#[test]
fn json_flag_keeps_help_human_readable() {
    let output = run_rucksack(&["--json", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("Usage: rucksack [OPTIONS] [COMMAND]"));
    assert!(!stdout.contains("\"type\":\"result\""));
}

#[test]
fn json_flag_keeps_version_human_readable() {
    let output = run_rucksack(&["--json", "--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.starts_with("rucksack "));
    assert!(!stdout.contains("\"type\":\"result\""));
}

#[test]
fn setup_rejects_control_characters_before_writing_components() {
    let home = tempfile::tempdir().expect("temporary home should be created");
    let output = run_rucksack_in_home(
        &[
            "setup",
            "--yes",
            "--no-helper",
            "--hotspot",
            "Noah\u{1b}[2J",
        ],
        home.path(),
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(!home.path().join(".codex").exists());
    assert!(!home.path().join(".claude").exists());
    assert!(!home.path().join(".cursor").exists());
    assert!(!home
        .path()
        .join("Library/Application Support/Rucksack")
        .exists());
}
