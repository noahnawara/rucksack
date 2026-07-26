//! What the `rucksack` binary promises its users.
//!
//! Everything here is read-only: taking a real lease disables system sleep, so that belongs in
//! `scripts/e2e.sh`, which restores it. These tests must be safe to run anywhere, including CI.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn rucksack(arguments: &[&str], home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rucksack"))
        .args(arguments)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("NO_COLOR", "1")
        .output()
        .expect("rucksack should run")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout should be UTF-8")
}

fn home() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary home")
}

/// Where the binary looks for its state under the `HOME` these tests hand it.
///
/// A test that plants a session file has to plant it where rucksack reads from, and that is not the
/// same place on the Linux runner as it is on a Mac. Writing to the wrong one leaves the test
/// passing against a rucksack that never opened the file.
fn state_directory(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Rucksack")
    } else {
        home.join(".local/share/Rucksack")
    }
}

#[test]
fn help_lists_five_verbs_and_nothing_else() {
    let home = home();
    let output = rucksack(&["--help"], home.path());

    assert_eq!(output.status.code(), Some(0));
    let help = stdout(&output);
    for verb in ["pack", "status", "unpack", "pair", "helper"] {
        assert!(help.contains(&format!("\n  {verb} ")), "{verb} missing");
    }
    for gone in ["setup", "doctor", "report", "recover", "adapters", "hook"] {
        assert!(!help.contains(&format!("\n  {gone} ")), "{gone} came back");
    }
    assert!(!help.contains("--json"));
}

#[test]
fn version_prints_a_version() {
    let home = home();
    let output = rucksack(&["--version"], home.path());

    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).starts_with("rucksack "));
}

/// Nothing in `status` may create state or hold a lease.
#[test]
fn status_on_a_fresh_mac_is_one_line_and_writes_nothing() {
    let home = home();
    let output = rucksack(&["status"], home.path());

    assert_eq!(output.status.code(), Some(0));
    let reported = stdout(&output);
    assert!(reported.contains("Not packed"), "{reported}");
    assert!(reported.lines().count() <= 2, "{reported}");
    assert!(!home
        .path()
        .join("Library/Application Support/Rucksack")
        .exists());
}

#[test]
fn contradictory_arguments_are_refused_before_anything_happens() {
    let home = home();

    for arguments in [
        vec!["pack", "--hotspot", "Noah", "--usb"],
        vec!["pack", "--for", "0m"],
        vec!["pack", "--for", "48h"],
        vec!["pack", "--for", "soon"],
    ] {
        let output = rucksack(&arguments, home.path());
        assert_eq!(output.status.code(), Some(2), "{arguments:?} was accepted");
    }
    assert!(!home
        .path()
        .join("Library/Application Support/Rucksack")
        .exists());
}

/// The flags that turned packing into a maze must stay gone.
#[test]
fn pack_asks_no_questions() {
    let home = home();
    let help = stdout(&rucksack(&["pack", "--help"], home.path()));

    for gone in [
        "--yes",
        "--agent",
        "--focus",
        "--allow-unverified-ssid",
        "--allow-unverified-remote",
    ] {
        assert!(!help.contains(gone), "{gone} came back");
    }
    for kept in ["--hotspot", "--usb", "--for", "--require-remote"] {
        assert!(help.contains(kept), "{kept} went missing");
    }
}

/// A config file from an older release must never break the command someone just typed.
#[test]
fn a_config_from_an_older_release_still_loads() {
    let home = home();
    let directory = home.path().join("Library/Application Support/Rucksack");
    std::fs::create_dir_all(&directory).expect("state directory");
    std::fs::write(
        directory.join("config.toml"),
        "version = 1\n\
         default_agent = \"codex\"\n\
         \n\
         [hotspot]\n\
         ssid = \"Noah\"\n\
         require_verified_ssid = true\n\
         probe_timeout_seconds = 6\n\
         \n\
         [session]\n\
         duration_minutes = 1440\n\
         focus = \"continue\"\n\
         idle_grace_seconds = 900\n\
         \n\
         [safety]\n\
         minimum_start_battery_percent = 35\n\
         warn_battery_percent = 20\n\
         sleep_battery_percent = 15\n",
    )
    .expect("legacy config");

    let output = rucksack(&["status"], home.path());

    assert_eq!(output.status.code(), Some(0));
    assert!(!stdout(&output).contains("unknown field"));
}

/// Session state rucksack cannot parse must not stop it reporting, or block `unpack`.
#[test]
fn unreadable_session_state_never_dead_ends() {
    let home = home();
    let directory = home.path().join("Library/Application Support/Rucksack");
    std::fs::create_dir_all(&directory).expect("state directory");
    std::fs::write(directory.join("session.json"), "{ this is not json").expect("broken state");

    let reported = rucksack(&["status"], home.path());
    assert_eq!(reported.status.code(), Some(0));
    assert!(stdout(&reported).contains("Not packed"));
}

/// Recorded intent is not protection, and `status` must never confuse the two.
///
/// This is the session file left behind by the failure it is named for: phase "active", hours still
/// on the clock, and a single heartbeat from long before, written by a watcher that then died. The
/// helper dropped the lease ninety seconds later, and `status` went on printing
/// `Packed · en0 · battery 100% · 23h 53m left` — the one sentence that gets a lid closed on
/// unprotected work.
#[test]
fn a_session_whose_watcher_died_is_never_reported_as_packed() {
    let home = home();
    let directory = state_directory(home.path());
    std::fs::create_dir_all(&directory).expect("state directory");
    std::fs::write(
        directory.join("session.json"),
        r#"{
          "version": 2,
          "id": "3f1a1f7a-2a5f-4f2e-9f7a-1b2c3d4e5f60",
          "lease_id": "8c2d4e6f-1a3b-4c5d-8e9f-0a1b2c3d4e5f",
          "phase": "active",
          "started_at": "2026-07-26T19:17:46Z",
          "expires_at": "2099-01-01T00:00:00Z",
          "ended_at": null,
          "last_heartbeat_at": "2026-07-26T19:18:19Z",
          "daemon_pid": 58895,
          "hotspot": null,
          "route_interface": "en0",
          "battery_percent": 100,
          "online": true,
          "last_event": null,
          "release_reason": null
        }"#,
    )
    .expect("a session the watcher stopped writing to");

    let reported = rucksack(&["status"], home.path());

    assert_eq!(reported.status.code(), Some(0));
    let reported = stdout(&reported);
    assert!(!reported.contains("Packed ·"), "{reported}");
    assert!(reported.contains("Not packed"), "{reported}");
    assert!(reported.contains("rucksack pack"), "{reported}");
}
