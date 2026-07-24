use crate::helper_client::HelperClient;
use anyhow::{anyhow, Context, Result};
use rucksack_core::system::run_owned;
use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const HELPER_LABEL: &str = "io.rucksack.helper";
const HELPER_DESTINATION: &str = "/Library/PrivilegedHelperTools/io.rucksack.helper";
const PLIST_DESTINATION: &str = "/Library/LaunchDaemons/io.rucksack.helper.plist";
const HELPER_SOCKET: &str = "/var/run/rucksack-helper.sock";
const HELPER_LOG: &str = "/var/log/rucksack-helper.log";
const HELPER_STATE_DIRECTORY: &str = "/var/db/rucksack";
const HELPER_PLIST: &str = include_str!("../../../assets/launchd/io.rucksack.helper.plist");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperInstallationState {
    Absent,
    Complete,
    Partial,
}

pub fn install_helper() -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("The Rucksack helper is supported only on macOS");
    }
    if !cfg!(debug_assertions) {
        anyhow::bail!(
            "Release builds install the signed helper through the notarized Rucksack package. Reinstall the package from a tagged GitHub release."
        );
    }

    if helper_installation_state() == HelperInstallationState::Partial {
        anyhow::bail!(
            "A partial helper installation exists. Refusing to overwrite it without a signed package repair."
        );
    }

    if installed_helper_exists() {
        let client = HelperClient::default();
        let status = client.status().context(
            "An existing helper is installed but unreachable. Refusing to replace it without proving that no closed-lid lease is active. Run `rucksack recover`, then retry.",
        )?;
        let status = status.context("The installed helper returned no status")?;
        if status.active {
            anyhow::bail!(
                "A closed-lid lease is active. Run `rucksack unpack` before replacing the helper."
            );
        }
        if status.sleep_disabled != Some(0) {
            anyhow::bail!(
                "The installed helper cannot prove normal sleep is restored (SleepDisabled={:?}). Run `rucksack recover` before replacing it.",
                status.sleep_disabled
            );
        }
    }

    let cli = std::env::current_exe().context("Could not locate the rucksack executable")?;
    let helper = sibling_helper(&cli)?;
    let staged_helper = stage_development_helper(&helper)?;

    let mut staged_plist = NamedTempFile::new()?;
    std::io::Write::write_all(&mut staged_plist, HELPER_PLIST.as_bytes())?;
    staged_plist.as_file().sync_all()?;

    sudo(&[
        "/usr/bin/install",
        "-d",
        "-o",
        "root",
        "-g",
        "wheel",
        "-m",
        "0755",
        "/Library/PrivilegedHelperTools",
        "/Library/LaunchDaemons",
    ])?;
    sudo(&[
        "/usr/bin/install",
        "-d",
        "-o",
        "root",
        "-g",
        "wheel",
        "-m",
        "0700",
        "/var/db/rucksack",
    ])?;
    sudo(&[
        "/usr/bin/install",
        "-o",
        "root",
        "-g",
        "wheel",
        "-m",
        "0755",
        staged_helper.path().to_string_lossy().as_ref(),
        HELPER_DESTINATION,
    ])?;
    sudo(&[
        "/usr/bin/install",
        "-o",
        "root",
        "-g",
        "wheel",
        "-m",
        "0644",
        staged_plist.path().to_string_lossy().as_ref(),
        PLIST_DESTINATION,
    ])?;

    bootout_helper_if_loaded()?;
    sudo(&["/bin/launchctl", "bootstrap", "system", PLIST_DESTINATION])?;
    sudo(&[
        "/bin/launchctl",
        "kickstart",
        "-k",
        &format!("system/{HELPER_LABEL}"),
    ])?;

    let client = HelperClient::default();
    for _ in 0..20 {
        if client.is_available() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    Err(anyhow!(
        "The helper was installed but did not become reachable at /var/run/rucksack-helper.sock"
    ))
}

pub fn uninstall_helper() -> Result<()> {
    match helper_installation_state() {
        HelperInstallationState::Absent => return Ok(()),
        HelperInstallationState::Complete => {}
        HelperInstallationState::Partial => {
            anyhow::bail!(
                "A partial helper installation exists. Reinstall the signed package before uninstalling so Rucksack can prove the sleep baseline is restored."
            )
        }
    }

    let client = HelperClient::default();
    let recovered = client.recover().context(
        "The helper is installed but unreachable. Refusing to remove it because Rucksack cannot prove that the saved sleep baseline was restored.",
    )?;
    if let Some(status) = recovered.as_ref() {
        if status.active {
            anyhow::bail!(
                "The helper still reports an active lease after recovery; refusing removal"
            );
        }
        if status.sleep_disabled != Some(0) {
            anyhow::bail!(
                "The helper cannot prove SleepDisabled returned to 0; refusing removal: {:?}",
                status
            );
        }
    } else {
        anyhow::bail!("The helper returned no status after recovery; refusing removal");
    }

    bootout_helper_if_loaded()?;
    sudo(&["/bin/rm", "-f", PLIST_DESTINATION, HELPER_DESTINATION])?;
    sudo(&["/bin/rm", "-f", HELPER_SOCKET, HELPER_LOG])?;
    let _ = sudo(&["/bin/rmdir", HELPER_STATE_DIRECTORY]);
    Ok(())
}

pub fn helper_paths() -> (&'static str, &'static str) {
    (HELPER_DESTINATION, PLIST_DESTINATION)
}

fn sibling_helper(cli: &Path) -> Result<PathBuf> {
    let directory = cli
        .parent()
        .context("The rucksack executable has no parent directory")?;
    Ok(directory.join("rucksack-helper"))
}

fn stage_development_helper(helper: &Path) -> Result<NamedTempFile> {
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(helper)
        .with_context(|| {
            format!(
                "Could not open {} as a regular file. Build rucksack-helper next to the debug CLI before installation.",
                helper.display()
            )
        })?;
    let metadata = source.metadata().with_context(|| {
        format!(
            "Could not inspect the opened development helper {}",
            helper.display()
        )
    })?;
    if !metadata.is_file() {
        anyhow::bail!(
            "Refusing development helper source that is not a regular file: {}",
            helper.display()
        );
    }
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        anyhow::bail!(
            "Refusing a development helper that is not owned by the current user or is writable by its group or other users: {}",
            helper.display()
        );
    }

    let mut staged = NamedTempFile::new().context("Could not stage the development helper")?;
    io::copy(&mut source, &mut staged)
        .with_context(|| format!("Could not stage {}", helper.display()))?;
    staged
        .as_file()
        .sync_all()
        .context("Could not sync the staged development helper")?;
    Ok(staged)
}

fn bootout_helper_if_loaded() -> Result<()> {
    let target = format!("system/{HELPER_LABEL}");
    let result = run_owned("/bin/launchctl", &["print".to_owned(), target.clone()])?;
    if !result.success() {
        return Ok(());
    }
    sudo(&["/bin/launchctl", "bootout", &target])
}

fn sudo(args: &[&str]) -> Result<()> {
    let owned = args
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    let result = run_owned("/usr/bin/sudo", &owned)?;
    if result.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "sudo {} failed: {}",
            args.join(" "),
            result.combined_trimmed()
        ))
    }
}

pub fn installed_helper_exists() -> bool {
    helper_installation_state() == HelperInstallationState::Complete
}

fn helper_installation_state() -> HelperInstallationState {
    match (
        Path::new(HELPER_DESTINATION).exists(),
        Path::new(PLIST_DESTINATION).exists(),
    ) {
        (false, false) => HelperInstallationState::Absent,
        (true, true) => HelperInstallationState::Complete,
        _ => HelperInstallationState::Partial,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn stages_an_opened_private_helper_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("rucksack-helper");
        fs::write(&helper, b"trusted helper").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();

        let staged = stage_development_helper(&helper).unwrap();

        assert_eq!(fs::read(staged.path()).unwrap(), b"trusted helper");
    }

    #[test]
    fn rejects_symlinked_or_other_writable_helper_sources() {
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("rucksack-helper");
        fs::write(&helper, b"helper").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(stage_development_helper(&helper).is_err());

        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        let link = directory.path().join("rucksack-helper-link");
        symlink(&helper, &link).unwrap();
        assert!(stage_development_helper(&link).is_err());
    }
}
