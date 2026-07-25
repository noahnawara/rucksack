use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("Could not create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("Could not secure {}", path.display()))?;
    }
    Ok(())
}

pub fn reject_symlink(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(anyhow!(
                "Refusing to modify symlinked path {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn ensure_parent_exists(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::create_dir_all(path).with_context(|| format!("Could not create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("Could not secure new directory {}", path.display()))?;
    }
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    reject_symlink(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    ensure_parent_exists(parent)?;

    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("Could not create a temporary file in {}", parent.display()))?;
    temp.write_all(bytes)
        .with_context(|| format!("Could not write temporary file for {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(mode))
            .with_context(|| format!("Could not set permissions for {}", path.display()))?;
    }

    temp.as_file_mut()
        .sync_all()
        .with_context(|| format!("Could not flush temporary file for {}", path.display()))?;
    let persisted = temp
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Could not replace {}", path.display()))?;
    persisted
        .sync_all()
        .with_context(|| format!("Could not flush persisted file {}", path.display()))?;
    sync_parent(parent)?;
    Ok(())
}

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes, 0o600)
}

pub fn atomic_write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = toml::to_string_pretty(value)?;
    atomic_write(path, text.as_bytes(), 0o600)
}

pub fn with_advisory_lock<T>(path: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    reject_symlink(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    ensure_parent_exists(parent)?;

    let mut options = fs::OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("Could not open state lock {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error)
                    .with_context(|| format!("Could not lock state file {}", path.display()));
            }
        }
        operation()
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        let _ = operation;
        anyhow::bail!("Advisory state locking is unsupported on this platform")
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    reject_symlink(path)?;
    let file = File::open(path).with_context(|| format!("Could not open {}", path.display()))?;
    serde_json::from_reader(file)
        .with_context(|| format!("Could not parse JSON in {}", path.display()))
}

pub fn read_json_if_exists<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    read_json(path).map(Some)
}

pub fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    reject_symlink(path)?;
    let mut file =
        File::open(path).with_context(|| format!("Could not open {}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    toml::from_str(&text).with_context(|| format!("Could not parse TOML in {}", path.display()))
}

pub fn backup_once(path: &Path, marker: &str) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    reject_symlink(path)?;
    let parent = path.parent().context("File has no parent directory")?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("File name is not valid UTF-8")?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup = parent.join(format!("{file_name}.{marker}.{stamp}.bak"));
    fs::copy(path, &backup).with_context(|| {
        format!(
            "Could not back up {} to {}",
            path.display(),
            backup.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))?;
    }
    Ok(Some(backup))
}

pub fn remove_if_exists(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    match fs::remove_file(path) {
        Ok(()) => {
            let parent = path
                .parent()
                .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
            sync_parent(parent)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Could not remove {}", path.display())),
    }
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<()> {
    File::open(parent)
        .with_context(|| format!("Could not open directory {} for syncing", parent.display()))?
        .sync_all()
        .with_context(|| format!("Could not sync directory {}", parent.display()))
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<()> {
    Ok(())
}

pub fn append_line(path: &Path, line: &str) -> Result<()> {
    let parent = path.parent().context("Log path has no parent directory")?;
    ensure_private_dir(parent)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    let mut file = options
        .open(path)
        .with_context(|| format!("Could not open log {}", path.display()))?;
    writeln!(file, "{line}")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn atomic_write_and_remove_complete_successfully() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");

        atomic_write(&path, b"{\"active\":true}\n", 0o600).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"active\":true}\n");

        atomic_write(&path, b"{\"active\":false}\n", 0o600).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"active\":false}\n");

        remove_if_exists(&path).unwrap();
        assert!(!path.exists());
    }
}
