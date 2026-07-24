use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const MAX_ANCHORED_FILE_BYTES: usize = 4 * 1024 * 1024;

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

#[derive(Debug)]
pub(crate) struct AnchoredFileContents {
    pub(crate) bytes: Vec<u8>,
    pub(crate) mode: u32,
}

#[cfg(unix)]
mod anchored {
    use super::AnchoredFileContents;
    use anyhow::{anyhow, Context, Result};
    use rustix::fs::{
        fchmod, fstat, mkdirat, open, openat, renameat, statat, unlinkat, AtFlags, FileType, Mode,
        OFlags, RawMode,
    };
    use rustix::io::Errno;
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::{Component, Path, PathBuf};
    use std::sync::Arc;
    use uuid::Uuid;

    const MAX_DIRECTORY_RETRIES: usize = 8;

    #[derive(Clone, Debug)]
    pub(crate) struct AnchoredRoot {
        directory: Arc<File>,
        path: PathBuf,
    }

    #[derive(Debug)]
    pub(crate) struct AnchoredFile {
        leaf: OsString,
        parent: Option<File>,
        parent_components: Vec<OsString>,
        path: PathBuf,
        root: Arc<File>,
        root_path: PathBuf,
    }

    enum DirectoryEntry {
        Directory(File),
        Missing,
        NotDirectory,
    }

    impl AnchoredRoot {
        pub(crate) fn open(path: &Path) -> Result<Self> {
            let directory = open_directory(path)
                .with_context(|| format!("Could not anchor directory {}", path.display()))?;
            Ok(Self {
                directory: Arc::new(directory),
                path: path.to_path_buf(),
            })
        }

        pub(crate) fn file(&self, path: &Path) -> Result<AnchoredFile> {
            let relative = path.strip_prefix(&self.path).with_context(|| {
                format!(
                    "Managed path {} is outside its trusted root {}",
                    path.display(),
                    self.path.display()
                )
            })?;
            let mut components: Vec<OsString> = Vec::new();
            for component in relative.components() {
                match component {
                    Component::CurDir => {}
                    Component::Normal(name) => components.push(name.to_os_string()),
                    _ => {
                        return Err(anyhow!(
                            "Managed path {} contains an unsupported component",
                            path.display()
                        ));
                    }
                }
            }
            let leaf = components.pop().ok_or_else(|| {
                anyhow!(
                    "Managed path {} does not name a file below {}",
                    path.display(),
                    self.path.display()
                )
            })?;
            let parent = open_parent(
                self.directory.as_ref(),
                &self.path,
                &components,
                false,
                path,
            )?;
            Ok(AnchoredFile {
                leaf,
                parent,
                parent_components: components,
                path: path.to_path_buf(),
                root: Arc::clone(&self.directory),
                root_path: self.path.clone(),
            })
        }
    }

    impl AnchoredFile {
        pub(crate) fn path(&self) -> &Path {
            &self.path
        }

        pub(crate) fn read(&mut self) -> Result<Option<AnchoredFileContents>> {
            self.open_parent_if_available()?;
            let Some(parent) = self.parent.as_ref() else {
                return Ok(None);
            };
            match inspect_file_entry(parent, &self.leaf, &self.path)? {
                FileEntry::Missing => return Ok(None),
                FileEntry::Regular => {}
            }

            let descriptor = match openat(
                parent,
                &self.leaf,
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(descriptor) => descriptor,
                Err(Errno::NOENT) => return Ok(None),
                Err(Errno::LOOP) => {
                    return Err(anyhow!(
                        "Refusing to read symlinked managed file {}",
                        self.path.display()
                    ));
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Could not open {}", self.path.display()));
                }
            };
            let file = File::from(descriptor);
            let metadata = fstat(&file)
                .with_context(|| format!("Could not inspect open file {}", self.path.display()))?;
            if FileType::from_raw_mode(metadata.st_mode as _) != FileType::RegularFile {
                return Err(anyhow!(
                    "Refusing to read non-regular managed file {}",
                    self.path.display()
                ));
            }
            let mut bytes: Vec<u8> = Vec::new();
            file.take((super::MAX_ANCHORED_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .with_context(|| format!("Could not read {}", self.path.display()))?;
            if bytes.len() > super::MAX_ANCHORED_FILE_BYTES {
                return Err(anyhow!(
                    "Managed file {} exceeds the 4 MiB safety limit",
                    self.path.display()
                ));
            }
            Ok(Some(AnchoredFileContents {
                bytes,
                mode: (metadata.st_mode as u32) & 0o7777,
            }))
        }

        pub(crate) fn write_atomic(&mut self, bytes: &[u8], mode: u32) -> Result<()> {
            let path = self.path.clone();
            let leaf = self.leaf.clone();
            let requested_mode = managed_mode(mode)?;
            let parent = self.ensure_parent()?;
            let _ = inspect_file_entry(parent, &leaf, &path)?;

            let temporary_name =
                OsString::from(format!(".rucksack-{}.tmp", Uuid::new_v4().simple()));
            let descriptor = openat(
                parent,
                &temporary_name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                requested_mode,
            )
            .with_context(|| {
                format!(
                    "Could not create a temporary file beside {}",
                    path.display()
                )
            })?;
            let mut temporary_file = File::from(descriptor);
            let operation = (|| -> Result<()> {
                fchmod(&temporary_file, requested_mode)
                    .with_context(|| format!("Could not set permissions for {}", path.display()))?;
                temporary_file.write_all(bytes).with_context(|| {
                    format!("Could not write temporary file for {}", path.display())
                })?;
                temporary_file.sync_all().with_context(|| {
                    format!("Could not flush temporary file for {}", path.display())
                })?;
                let _ = inspect_file_entry(parent, &leaf, &path)?;
                renameat(parent, &temporary_name, parent, &leaf)
                    .with_context(|| format!("Could not replace {}", path.display()))?;
                parent
                    .sync_all()
                    .with_context(|| format!("Could not sync parent of {}", path.display()))
            })();

            if let Err(error) = operation {
                match unlinkat(parent, &temporary_name, AtFlags::empty()) {
                    Ok(()) | Err(Errno::NOENT) => return Err(error),
                    Err(cleanup_error) => {
                        return Err(anyhow!(
                            "{error:#}; could not remove temporary file for {}: {cleanup_error}",
                            path.display()
                        ));
                    }
                }
            }
            Ok(())
        }

        pub(crate) fn remove_if_exists(&mut self) -> Result<bool> {
            self.open_parent_if_available()?;
            let Some(parent) = self.parent.as_ref() else {
                return Ok(false);
            };
            match inspect_file_entry(parent, &self.leaf, &self.path)? {
                FileEntry::Missing => return Ok(false),
                FileEntry::Regular => {}
            }
            match unlinkat(parent, &self.leaf, AtFlags::empty()) {
                Ok(()) => {
                    parent.sync_all().with_context(|| {
                        format!("Could not sync parent of {}", self.path.display())
                    })?;
                    Ok(true)
                }
                Err(Errno::NOENT) => Ok(false),
                Err(error) => {
                    Err(error).with_context(|| format!("Could not remove {}", self.path.display()))
                }
            }
        }

        pub(crate) fn verify_parent_binding(&self) -> Result<()> {
            let live_root = open_directory(&self.root_path).with_context(|| {
                format!(
                    "Managed root {} changed while modifying {}",
                    self.root_path.display(),
                    self.path.display()
                )
            })?;
            verify_same_directory(self.root.as_ref(), &live_root, &self.root_path)?;

            let Some(expected_parent) = self.parent.as_ref() else {
                return Ok(());
            };
            let live_parent = open_parent(
                self.root.as_ref(),
                &self.root_path,
                &self.parent_components,
                false,
                &self.path,
            )?
            .ok_or_else(|| {
                anyhow!(
                    "Managed parent for {} changed while it was being modified",
                    self.path.display()
                )
            })?;
            verify_same_directory(expected_parent, &live_parent, &self.path)
        }

        fn open_parent_if_available(&mut self) -> Result<()> {
            if self.parent.is_none() {
                self.parent = open_parent(
                    self.root.as_ref(),
                    &self.root_path,
                    &self.parent_components,
                    false,
                    &self.path,
                )?;
            }
            Ok(())
        }

        fn ensure_parent(&mut self) -> Result<&File> {
            if self.parent.is_none() {
                self.parent = open_parent(
                    self.root.as_ref(),
                    &self.root_path,
                    &self.parent_components,
                    true,
                    &self.path,
                )?;
            }
            self.parent.as_ref().ok_or_else(|| {
                anyhow!(
                    "Could not create parent directory for {}",
                    self.path.display()
                )
            })
        }
    }

    enum FileEntry {
        Missing,
        Regular,
    }

    fn managed_mode(mode: u32) -> Result<Mode> {
        let raw_mode: RawMode = mode
            .try_into()
            .map_err(|_| anyhow!("Managed file mode {mode:o} is not supported on this platform"))?;
        Ok(Mode::from_raw_mode(raw_mode))
    }

    fn inspect_file_entry(parent: &File, leaf: &OsStr, path: &Path) -> Result<FileEntry> {
        let metadata = match statat(parent, leaf, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => metadata,
            Err(Errno::NOENT) => return Ok(FileEntry::Missing),
            Err(error) => {
                return Err(error).with_context(|| format!("Could not inspect {}", path.display()));
            }
        };
        match FileType::from_raw_mode(metadata.st_mode as _) {
            FileType::RegularFile => Ok(FileEntry::Regular),
            FileType::Symlink => Err(anyhow!(
                "Refusing to modify symlinked managed file {}",
                path.display()
            )),
            _ => Err(anyhow!(
                "Refusing to modify non-regular managed file {}",
                path.display()
            )),
        }
    }

    fn open_directory(path: &Path) -> Result<File> {
        let descriptor = open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let directory = File::from(descriptor);
        let metadata = fstat(&directory)?;
        if FileType::from_raw_mode(metadata.st_mode as _) != FileType::Directory {
            return Err(anyhow!("{} is not a directory", path.display()));
        }
        Ok(directory)
    }

    fn open_parent(
        root: &File,
        root_path: &Path,
        components: &[OsString],
        create_missing: bool,
        managed_path: &Path,
    ) -> Result<Option<File>> {
        let mut current = root.try_clone().with_context(|| {
            format!(
                "Could not retain anchored root {} for {}",
                root_path.display(),
                managed_path.display()
            )
        })?;
        let mut current_path = root_path.to_path_buf();
        for component in components {
            current_path.push(component);
            let mut next: Option<File> = None;
            for _ in 0..MAX_DIRECTORY_RETRIES {
                match open_directory_entry(&current, component, &current_path)? {
                    DirectoryEntry::Directory(directory) => {
                        next = Some(directory);
                        break;
                    }
                    DirectoryEntry::Missing if !create_missing => return Ok(None),
                    DirectoryEntry::NotDirectory if !create_missing => return Ok(None),
                    DirectoryEntry::NotDirectory => {
                        return Err(anyhow!(
                            "Managed parent component {} is not a directory",
                            current_path.display()
                        ));
                    }
                    DirectoryEntry::Missing => match mkdirat(&current, component, Mode::RWXU) {
                        Ok(()) | Err(Errno::EXIST) => {}
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!(
                                    "Could not create managed directory {}",
                                    current_path.display()
                                )
                            });
                        }
                    },
                }
            }
            current = next.ok_or_else(|| {
                anyhow!(
                    "Managed directory {} kept changing while opening {}",
                    current_path.display(),
                    managed_path.display()
                )
            })?;
        }
        Ok(Some(current))
    }

    fn open_directory_entry(
        parent: &File,
        name: &OsStr,
        display_path: &Path,
    ) -> Result<DirectoryEntry> {
        let metadata = match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => metadata,
            Err(Errno::NOENT) => return Ok(DirectoryEntry::Missing),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Could not inspect managed directory {}",
                        display_path.display()
                    )
                });
            }
        };
        match FileType::from_raw_mode(metadata.st_mode as _) {
            FileType::Symlink => {
                return Err(anyhow!(
                    "Refusing to traverse symlinked path {}",
                    display_path.display()
                ));
            }
            FileType::Directory => {}
            _ => return Ok(DirectoryEntry::NotDirectory),
        }

        match openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(descriptor) => Ok(DirectoryEntry::Directory(File::from(descriptor))),
            Err(Errno::NOENT) => Ok(DirectoryEntry::Missing),
            Err(Errno::NOTDIR) => Ok(DirectoryEntry::NotDirectory),
            Err(Errno::LOOP) => Err(anyhow!(
                "Refusing to traverse symlinked path {}",
                display_path.display()
            )),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "Could not open managed directory {}",
                    display_path.display()
                )
            }),
        }
    }

    fn verify_same_directory(expected: &File, actual: &File, path: &Path) -> Result<()> {
        let expected_metadata = fstat(expected)
            .with_context(|| format!("Could not inspect anchored directory {}", path.display()))?;
        let actual_metadata = fstat(actual)
            .with_context(|| format!("Could not inspect live directory {}", path.display()))?;
        if expected_metadata.st_dev != actual_metadata.st_dev
            || expected_metadata.st_ino != actual_metadata.st_ino
        {
            return Err(anyhow!(
                "Managed directory {} changed while it was being modified",
                path.display()
            ));
        }
        Ok(())
    }
}

#[cfg(not(unix))]
mod anchored {
    use super::{atomic_write, reject_symlink, remove_if_exists, AnchoredFileContents};
    use anyhow::{anyhow, Context, Result};
    use std::fs::{self, File};
    use std::io::Read;
    use std::path::{Component, Path, PathBuf};

    #[derive(Clone, Debug)]
    pub(crate) struct AnchoredRoot {
        path: PathBuf,
    }

    #[derive(Debug)]
    pub(crate) struct AnchoredFile {
        path: PathBuf,
        root: PathBuf,
    }

    impl AnchoredRoot {
        pub(crate) fn open(path: &Path) -> Result<Self> {
            reject_symlink(path)?;
            if !path.is_dir() {
                return Err(anyhow!("{} is not a directory", path.display()));
            }
            Ok(Self {
                path: path.to_path_buf(),
            })
        }

        pub(crate) fn file(&self, path: &Path) -> Result<AnchoredFile> {
            validate_components(&self.path, path)?;
            Ok(AnchoredFile {
                path: path.to_path_buf(),
                root: self.path.clone(),
            })
        }
    }

    impl AnchoredFile {
        pub(crate) fn path(&self) -> &Path {
            &self.path
        }

        pub(crate) fn read(&mut self) -> Result<Option<AnchoredFileContents>> {
            validate_components(&self.root, &self.path)?;
            let file = match File::open(&self.path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Could not read {}", self.path.display()));
                }
            };
            let mut bytes: Vec<u8> = Vec::new();
            file.take((super::MAX_ANCHORED_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .with_context(|| format!("Could not read {}", self.path.display()))?;
            if bytes.len() > super::MAX_ANCHORED_FILE_BYTES {
                return Err(anyhow!(
                    "Managed file {} exceeds the 4 MiB safety limit",
                    self.path.display()
                ));
            }
            Ok(Some(AnchoredFileContents { bytes, mode: 0o600 }))
        }

        pub(crate) fn write_atomic(&mut self, bytes: &[u8], mode: u32) -> Result<()> {
            validate_components(&self.root, &self.path)?;
            atomic_write(&self.path, bytes, mode)
        }

        pub(crate) fn remove_if_exists(&mut self) -> Result<bool> {
            validate_components(&self.root, &self.path)?;
            if !self.path.exists() {
                return Ok(false);
            }
            remove_if_exists(&self.path)?;
            Ok(true)
        }

        pub(crate) fn verify_parent_binding(&self) -> Result<()> {
            validate_components(&self.root, &self.path)
        }
    }

    fn validate_components(root: &Path, path: &Path) -> Result<()> {
        let relative = path.strip_prefix(root).with_context(|| {
            format!(
                "Managed path {} is outside its trusted root {}",
                path.display(),
                root.display()
            )
        })?;
        reject_symlink(root)?;
        let mut current = root.to_path_buf();
        for component in relative.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(name) => {
                    current.push(name);
                    reject_symlink(&current)?;
                }
                _ => {
                    return Err(anyhow!(
                        "Managed path {} contains an unsupported component",
                        path.display()
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(crate) use anchored::{AnchoredFile, AnchoredRoot};

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
