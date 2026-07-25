use crate::system::{run_owned, which, CommandResult};
use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Codex ships inside ChatGPT.app as well as as a standalone CLI.
///
/// Only the standalone install puts `codex` on `PATH`, so resolving through `PATH` alone reports
/// Codex as absent on Macs where Remote Control is installed and working.
const BUNDLED_EXECUTABLE: &str = "/Applications/ChatGPT.app/Contents/Resources/codex";

pub fn executable() -> Option<PathBuf> {
    which("codex").or_else(|| {
        let bundled = PathBuf::from(BUNDLED_EXECUTABLE);
        bundled.is_file().then_some(bundled)
    })
}

pub fn remote_control_arguments(subcommand: &str) -> [String; 3] {
    [
        "remote-control".to_owned(),
        subcommand.to_owned(),
        "--json".to_owned(),
    ]
}

fn run(subcommand: &str) -> Result<CommandResult> {
    let executable = executable()
        .ok_or_else(|| anyhow!("codex was not found on PATH or in {BUNDLED_EXECUTABLE}"))?;
    run_owned(executable, &remote_control_arguments(subcommand))
}

pub fn start_remote_control() -> Result<CommandResult> {
    run("start")
}

pub fn pair() -> Result<CommandResult> {
    run("pair")
}
