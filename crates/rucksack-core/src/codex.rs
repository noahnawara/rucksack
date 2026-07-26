use crate::system::{run_owned, which, CommandResult};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

/// Where the Codex installer keeps the standalone CLI, relative to the home directory.
///
/// Codex also ships inside ChatGPT.app, and that copy used to be the fallback when `codex` was not
/// on `PATH`. It cannot serve: `remote-control` refuses outright unless this managed install is
/// present, because its daemon starts and updates app-server from this fixed path. So the fallback
/// reported Codex as available and then failed every single time it was used — worse than reporting
/// it absent, since "found" is what makes rucksack try. Resolving the managed path directly answers
/// the question the bundled copy was standing in for, and answers it correctly.
const MANAGED_STANDALONE: &str = ".codex/packages/standalone/current/codex";

/// A Codex that can actually run `remote-control`, or nothing.
pub fn executable(home: &Path) -> Option<PathBuf> {
    which("codex").or_else(|| managed_standalone(home))
}

fn managed_standalone(home: &Path) -> Option<PathBuf> {
    let managed = home.join(MANAGED_STANDALONE);
    managed.is_file().then_some(managed)
}

pub fn remote_control_arguments(subcommand: &str) -> [String; 3] {
    [
        "remote-control".to_owned(),
        subcommand.to_owned(),
        "--json".to_owned(),
    ]
}

fn run(home: &Path, subcommand: &str) -> Result<CommandResult> {
    let executable = executable(home)
        .ok_or_else(|| anyhow!("codex was not found on PATH or at ~/{MANAGED_STANDALONE}"))?;
    run_owned(executable, &remote_control_arguments(subcommand))
}

pub fn start_remote_control(home: &Path) -> Result<CommandResult> {
    run(home, "start")
}

pub fn pair(home: &Path) -> Result<CommandResult> {
    run(home, "pair")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// The regression: a Codex that exists but cannot run `remote-control` is not a Codex.
    ///
    /// ChatGPT.app is on most of these Macs, so the fallback that accepted its bundled copy
    /// reported Codex present on every one of them — including the ones that had never installed
    /// the CLI, where every `remote-control` call then failed.
    #[test]
    fn only_the_managed_install_counts_as_codex() {
        let home = tempdir().unwrap();

        assert_eq!(managed_standalone(home.path()), None);

        let managed = home.path().join(MANAGED_STANDALONE);
        fs::create_dir_all(managed.parent().unwrap()).unwrap();
        fs::write(&managed, b"#!/bin/sh\n").unwrap();

        assert_eq!(managed_standalone(home.path()), Some(managed));
    }
}
