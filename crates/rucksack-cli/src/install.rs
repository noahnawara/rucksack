use crate::helper_client::HelperClient;
use crate::output::Output;
use anyhow::{anyhow, Context, Result};
use rucksack_core::system::run;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const HELPER_LABEL: &str = "io.rucksack.helper";
const HELPER_DESTINATION: &str = "/Library/PrivilegedHelperTools/io.rucksack.helper";
const PLIST_DESTINATION: &str = "/Library/LaunchDaemons/io.rucksack.helper.plist";
const HELPER_SOCKET: &str = "/var/run/rucksack-helper.sock";
const HELPER_LOG: &str = "/var/log/rucksack-helper.log";
const HELPER_STATE_DIRECTORY: &str = "/var/db/rucksack";
const HELPER_PLIST: &str = include_str!("../../../assets/launchd/io.rucksack.helper.plist");

/// Install the root helper, repairing whatever state it is in.
///
/// Reinstalling is safe from any state: `/usr/bin/install` overwrites unconditionally,
/// bootout/bootstrap/kickstart resets launchd, and a fresh helper verifies the sleep baseline
/// before it serves. So the only refusal left is a reachable helper that is holding a lease —
/// replacing that one would leave a Mac that cannot sleep.
pub fn install_helper(output: &Output) -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("The rucksack helper is supported only on macOS.");
    }

    if installed_helper_exists() {
        if let Ok(Some(status)) = HelperClient::default().status() {
            if status.active {
                anyhow::bail!(
                    "This Mac is already packed.\nRun `rucksack unpack` before replacing the helper."
                );
            }
        }
    }

    let cli = std::env::current_exe().context("Could not locate the rucksack executable.")?;
    let helper = sibling_helper(&cli)?;
    let staged_helper = stage_helper(&helper)?;

    output.step("Installing the power helper. macOS will ask for your password once.");

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
        if client.status().is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    Err(anyhow!(
        "The helper was installed but did not become reachable at /var/run/rucksack-helper.sock"
    ))
}

pub fn uninstall_helper() -> Result<()> {
    if !any_helper_file_exists() {
        return Ok(());
    }

    let client = HelperClient::default();
    let recovered = client.recover().context(
        "The helper is installed but unreachable. Refusing to remove it because rucksack cannot prove that the saved sleep baseline was restored.",
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

/// Whether the installed helper is byte-for-byte the one this rucksack would install.
///
/// `None` when the question cannot be answered — no helper installed yet, or no sibling binary to
/// compare against — because a check that cannot see both sides must not accuse either.
///
/// Bytes rather than versions. Two builds of the same tag carry the same version string and can
/// still be different binaries, which is exactly what happens when `helper install` runs before a
/// `cargo install --force` replaces the binary underneath it. A version comparison calls that a
/// match; the bytes do not.
pub fn installed_helper_matches_source() -> Option<bool> {
    let installed = Path::new(HELPER_DESTINATION);
    if !installed.exists() {
        return None;
    }
    let source = sibling_helper(&std::env::current_exe().ok()?).ok()?;
    let installed_size = fs::metadata(installed).ok()?.len();
    let source_size = fs::metadata(&source).ok()?.len();
    if installed_size != source_size {
        return Some(false);
    }
    Some(fs::read(installed).ok()? == fs::read(&source).ok()?)
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

/// Copy the helper somewhere root can install it from.
///
/// `O_NOFOLLOW` keeps a symlink from redirecting what gets installed as root.
fn stage_helper(helper: &Path) -> Result<NamedTempFile> {
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(helper)
        .with_context(|| {
            format!(
                "Could not open {}. Build rucksack-helper next to the rucksack binary first.",
                helper.display()
            )
        })?;

    let mut staged = NamedTempFile::new().context("Could not stage the helper.")?;
    io::copy(&mut source, &mut staged)
        .with_context(|| format!("Could not stage {}", helper.display()))?;
    staged
        .as_file()
        .sync_all()
        .context("Could not sync the staged helper.")?;
    Ok(staged)
}

fn bootout_helper_if_loaded() -> Result<()> {
    let target = format!("system/{HELPER_LABEL}");
    let result = run("/bin/launchctl", &["print", &target])?;
    if !result.success() {
        return Ok(());
    }
    sudo(&["/bin/launchctl", "bootout", &target])
}

fn sudo(args: &[&str]) -> Result<()> {
    let result = run("/usr/bin/sudo", args)?;
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
    Path::new(HELPER_DESTINATION).exists() && Path::new(PLIST_DESTINATION).exists()
}

/// True when any part of an installation is present, so a half-removed tree still cleans up.
fn any_helper_file_exists() -> bool {
    Path::new(HELPER_DESTINATION).exists() || Path::new(PLIST_DESTINATION).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn stages_a_snapshot_of_the_helper() {
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("rucksack-helper");
        fs::write(&helper, b"trusted helper").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();

        let staged = stage_helper(&helper).unwrap();

        assert_eq!(fs::read(staged.path()).unwrap(), b"trusted helper");
    }

    /// A symlink here would decide what gets installed as root.
    #[test]
    fn rejects_a_symlinked_helper_source() {
        let directory = tempfile::tempdir().unwrap();
        let helper = directory.path().join("rucksack-helper");
        fs::write(&helper, b"helper").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        let link = directory.path().join("rucksack-helper-link");
        symlink(&helper, &link).unwrap();

        assert!(stage_helper(&link).is_err());
    }
}
