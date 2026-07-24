use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

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
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("NO_COLOR", "1")
        .output()
        .expect("rucksack should run")
}

fn run_rucksack_in_home_with_input(arguments: &[&str], home: &Path, input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rucksack"))
        .args(arguments)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rucksack should start");
    child
        .stdin
        .take()
        .expect("stdin should be piped")
        .write_all(input.as_bytes())
        .expect("hook input should be written");
    child.wait_with_output().expect("rucksack should finish")
}

fn report_file(home: &Path) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/Rucksack/last-report.json")
    }
    #[cfg(not(target_os = "macos"))]
    {
        home.join(".local/share/Rucksack/last-report.json")
    }
}

fn active_policy_file(home: &Path) -> std::path::PathBuf {
    report_file(home)
        .parent()
        .expect("report path should have a parent")
        .join("active-policy.json")
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
    assert!(stdout.contains("\n  pack "));
    assert!(stdout.contains("\n  unpack "));
    assert!(stdout.contains("\n  report "));
    assert!(!stdout.contains("\n  leave "));
    assert!(!stdout.contains("\n  arrive "));
    assert!(!stdout.contains("\"type\":\"result\""));

    for removed_name in ["leave", "arrive"] {
        let removed_help = run_rucksack(&[removed_name, "--help"]);
        assert_eq!(removed_help.status.code(), Some(2));
        assert!(!removed_help.stderr.is_empty());
    }
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

#[test]
fn report_reads_manual_and_automatic_sessions_without_mutating_them() {
    for (end_kind, human_end_kind, release_reason) in [
        (
            "automatic",
            "automatic release",
            "commute route moved to ordinary Wi-Fi",
        ),
        ("unpack", "unpack", "user unpacked"),
    ] {
        let home = tempfile::tempdir().expect("temporary home should be created");
        let path = report_file(home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let fixture = serde_json::json!({
            "version": 1,
            "session_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "agent": "codex",
            "project_dir": "/workspace/atlas",
            "focus": "continue",
            "started_at": "2026-07-24T14:00:00Z",
            "released_at": "2026-07-24T14:05:00Z",
            "duration_seconds": 300,
            "end_kind": end_kind,
            "release_reason": release_reason,
            "start_battery_percent": 80,
            "end_battery_percent": 77,
            "expected_hotspot_ssid": "Noah",
            "route_interface": "en0",
            "route_gateway": "192.0.0.1",
            "network_reachable_at_end": true,
            "remote_confirmed_by_user": true,
            "mobile_data": {
                "status": "available",
                "usage": {
                    "interface": "en0",
                    "received_bytes": 3000000,
                    "sent_bytes": 500000,
                    "total_bytes": 3500000,
                    "measured_from": "2026-07-24T14:00:05Z",
                    "measured_to": "2026-07-24T14:04:59Z"
                }
            }
        });
        let bytes = serde_json::to_vec_pretty(&fixture).unwrap();
        fs::write(&path, &bytes).unwrap();

        let human = run_rucksack_in_home(&["report"], home.path());
        assert_eq!(human.status.code(), Some(0));
        assert!(human.stderr.is_empty());
        let human_stdout = String::from_utf8(human.stdout).unwrap();
        assert!(human_stdout.contains("🎒 trip report."));
        assert!(human_stdout.contains("codex worked in /workspace/atlas."));
        assert!(human_stdout.contains("the rucksack was packed for 5m 0s."));
        assert!(human_stdout.contains(human_end_kind));
        assert!(human_stdout.contains(release_reason));
        assert!(human_stdout.contains("estimated mobile data was 3.5 MB."));
        assert!(human_stdout.contains("this is not agent only usage or carrier billing."));

        let json = run_rucksack_in_home(&["--json", "report"], home.path());
        assert_eq!(json.status.code(), Some(0));
        assert!(json.stderr.is_empty());
        let json_stdout = String::from_utf8(json.stdout).unwrap();
        let records = json_stdout
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "result");
        assert_eq!(records[0]["ok"], true);
        assert_eq!(records[0]["data"]["version"], 1);
        assert_eq!(
            records[0]["data"]["session_id"],
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        );
        assert_eq!(records[0]["data"]["agent"], "codex");
        assert_eq!(records[0]["data"]["project_dir"], "/workspace/atlas");
        assert_eq!(records[0]["data"]["duration_seconds"], 300);
        assert_eq!(records[0]["data"]["end_kind"], end_kind);
        assert_eq!(records[0]["data"]["release_reason"], release_reason);
        assert_eq!(
            records[0]["data"]["mobile_data"]["usage"]["total_bytes"],
            3_500_000
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn missing_report_returns_one_actionable_json_error_without_creating_state() {
    let home = tempfile::tempdir().expect("temporary home should be created");
    let path = report_file(home.path());

    let output = run_rucksack_in_home(&["--json", "report"], home.path());

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let records = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["type"], "result");
    assert_eq!(records[0]["ok"], false);
    assert!(records[0]["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("rucksack pack")));
    assert!(!path.exists());
}

#[test]
fn provider_hooks_deliver_exact_policy_only_on_supported_context_events() {
    let policy_text = "lease nonce rucksack-e2e-42";
    for agent in ["codex", "claude"] {
        let home = tempfile::tempdir().expect("temporary home should be created");
        let path = active_policy_file(home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "session_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "agent": agent,
                "focus": "continue",
                "project_dir": "/workspace/atlas",
                "activated_at": "2026-07-24T10:00:00Z",
                "expires_at": "2099-07-24T11:00:00Z",
                "policy": policy_text
            }))
            .unwrap(),
        )
        .unwrap();

        let output = run_rucksack_in_home_with_input(
            &["hook", agent],
            home.path(),
            r#"{"hook_event_name":"UserPromptSubmit"}"#,
        );
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            payload,
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "UserPromptSubmit",
                    "additionalContext": policy_text
                }
            })
        );
    }

    let home = tempfile::tempdir().expect("temporary home should be created");
    let path = active_policy_file(home.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "session_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "agent": "cursor",
            "focus": "continue",
            "project_dir": "/workspace/atlas",
            "activated_at": "2026-07-24T10:00:00Z",
            "expires_at": "2099-07-24T11:00:00Z",
            "policy": policy_text
        }))
        .unwrap(),
    )
    .unwrap();

    let output = run_rucksack_in_home_with_input(
        &["hook", "cursor"],
        home.path(),
        r#"{"event_name":"sessionStart"}"#,
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
