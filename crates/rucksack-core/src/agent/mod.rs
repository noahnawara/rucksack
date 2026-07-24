mod claude;
mod codex;
mod cursor;
mod json_hooks;

use crate::files::{
    atomic_write, atomic_write_json, backup_once, read_json_if_exists, reject_symlink,
    remove_if_exists,
};
use crate::paths::AppPaths;
use crate::policy::skill_document;
#[cfg(target_os = "macos")]
use crate::system::run_owned;
use crate::system::{processes, which, ProcessInfo};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub use claude::{
    claude_remote_preflight, claude_remote_server_command, claude_remote_user_instruction,
};
pub use codex::{codex_pair, codex_remote_start, codex_remote_stop};
pub use cursor::{activate_cursor_rule, deactivate_cursor_rule};

pub const MANAGED_MARKER: &str = "rucksack-managed";
const ADAPTER_MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Codex,
    Claude,
    Cursor,
}

impl AgentKind {
    pub const ALL: [AgentKind; 3] = [Self::Codex, Self::Claude, Self::Cursor];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Cursor => "Cursor",
        }
    }

    pub fn executable_names(self) -> &'static [&'static str] {
        match self {
            Self::Codex => &["codex"],
            Self::Claude => &["claude"],
            Self::Cursor => &["cursor-agent", "cursor"],
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
        })
    }
}

impl FromStr for AgentKind {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "codex" | "openai" => Ok(Self::Codex),
            "claude" | "claude-code" | "anthropic" => Ok(Self::Claude),
            "cursor" => Ok(Self::Cursor),
            other => Err(format!(
                "Unknown agent {other:?}; use codex, claude, or cursor"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDetection {
    pub kind: AgentKind,
    pub installed: bool,
    pub executable: Option<PathBuf>,
    pub running: bool,
    pub matching_pids: Vec<u32>,
    pub project_match: ProjectMatch,
    pub project_matching_pids: Vec<u32>,
    pub observed_working_directories: Vec<ProcessWorkingDirectory>,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectMatch {
    NotRequested,
    NotRunning,
    Matched,
    Different,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessWorkingDirectory {
    pub pid: u32,
    pub path: PathBuf,
    pub authoritative_for_project: bool,
}

pub fn detect(kind: AgentKind, project_dir: Option<&Path>) -> Result<AgentDetection> {
    let executable = kind.executable_names().iter().find_map(|name| which(name));
    let processes = processes()?;
    let cursor_app_exists =
        kind == AgentKind::Cursor && Path::new("/Applications/Cursor.app").exists();
    Ok(detect_from_inventory(
        kind,
        project_dir,
        executable,
        cursor_app_exists,
        &processes,
        process_working_directory,
    ))
}

fn detect_from_inventory(
    kind: AgentKind,
    project_dir: Option<&Path>,
    executable: Option<PathBuf>,
    cursor_app_exists: bool,
    processes: &[ProcessInfo],
    mut working_directory: impl FnMut(u32) -> Option<PathBuf>,
) -> AgentDetection {
    let matching_processes = processes
        .iter()
        .filter(|process| process_matches(kind, process))
        .collect::<Vec<_>>();
    let matching_pids = matching_processes
        .iter()
        .map(|process| process.pid)
        .collect::<Vec<_>>();
    let observed_working_directories = matching_processes
        .iter()
        .filter_map(|process| {
            working_directory(process.pid).map(|path| ProcessWorkingDirectory {
                pid: process.pid,
                path,
                authoritative_for_project: working_directory_is_authoritative(kind, process),
            })
        })
        .collect::<Vec<_>>();
    let normalized_project = project_dir.map(normalize_path);
    let project_matching_pids = normalized_project
        .as_ref()
        .map(|project| {
            observed_working_directories
                .iter()
                .filter(|evidence| working_directory_matches_project(&evidence.path, project))
                .map(|evidence| evidence.pid)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let project_match = match (project_dir, matching_pids.is_empty()) {
        (None, _) => ProjectMatch::NotRequested,
        (Some(_), true) => ProjectMatch::NotRunning,
        (Some(_), false) if !project_matching_pids.is_empty() => ProjectMatch::Matched,
        (Some(_), false)
            if observed_working_directories.len() == matching_pids.len()
                && observed_working_directories
                    .iter()
                    .all(|evidence| evidence.authoritative_for_project) =>
        {
            ProjectMatch::Different
        }
        (Some(_), false) => ProjectMatch::Unknown,
    };

    let installed = executable.is_some() || cursor_app_exists;
    let running = !matching_pids.is_empty();
    let detail = detection_detail(kind, installed, running, project_match);

    AgentDetection {
        kind,
        installed,
        executable,
        running,
        matching_pids,
        project_match,
        project_matching_pids,
        observed_working_directories,
        detail,
    }
}

fn detection_detail(
    kind: AgentKind,
    installed: bool,
    running: bool,
    project_match: ProjectMatch,
) -> String {
    let name = kind.display_name();
    match (installed, running, project_match) {
        (_, true, ProjectMatch::Matched) => format!("{name} is running for this project"),
        (_, true, ProjectMatch::Different) => {
            format!("{name} is running, but its working directory is a different project")
        }
        (_, true, ProjectMatch::Unknown) => {
            format!("{name} is running; its project association could not be verified")
        }
        (true, true, _) => format!("{name} is installed and running"),
        (false, true, _) => format!("{name} is running but its CLI is not on PATH"),
        (true, false, _) => format!("{name} is installed but no matching process is running"),
        (false, false, _) => format!("{name} was not found"),
    }
}

fn process_matches(kind: AgentKind, process: &ProcessInfo) -> bool {
    let executable = executable_name(&process.command);
    let argument_executable = process
        .arguments
        .split_whitespace()
        .next()
        .map(executable_name);
    let arguments = process.arguments.to_ascii_lowercase();
    match kind {
        AgentKind::Codex => {
            executable == "codex"
                || (argument_executable.as_deref() == Some("codex")
                    && !arguments.contains("/frameworks/codex")
                    && !arguments.contains("/computer-use/"))
        }
        AgentKind::Claude => {
            executable == "claude"
                || (argument_executable.as_deref() == Some("claude")
                    && !arguments.contains("/claude.app/"))
        }
        AgentKind::Cursor => {
            executable == "cursor"
                || executable == "cursor-agent"
                || executable.starts_with("cursor helper")
                || argument_executable.as_deref() == Some("cursor-agent")
                || arguments.contains("/cursor.app/contents/macos/cursor")
        }
    }
}

fn executable_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase()
}

fn working_directory_is_authoritative(kind: AgentKind, process: &ProcessInfo) -> bool {
    match kind {
        AgentKind::Codex => !process
            .arguments
            .to_ascii_lowercase()
            .contains(" app-server"),
        AgentKind::Claude => true,
        AgentKind::Cursor => false,
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn working_directory_matches_project(working_directory: &Path, project: &Path) -> bool {
    normalize_path(working_directory).starts_with(project)
}

#[cfg(target_os = "macos")]
fn process_working_directory(pid: u32) -> Option<PathBuf> {
    let executable = Path::new("/usr/sbin/lsof");
    if !executable.is_file() {
        return None;
    }
    let args = vec![
        "-a".to_owned(),
        "-p".to_owned(),
        pid.to_string(),
        "-d".to_owned(),
        "cwd".to_owned(),
        "-Fn".to_owned(),
    ];
    let result = run_owned(executable, &args).ok()?;
    if !result.success() {
        return None;
    }
    result
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix('n'))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "linux")]
fn process_working_directory(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_working_directory(_pid: u32) -> Option<PathBuf> {
    None
}

pub fn detect_all(project_dir: Option<&Path>) -> Vec<AgentDetection> {
    AgentKind::ALL
        .iter()
        .map(|kind| {
            detect(*kind, project_dir)
                .unwrap_or_else(|error| failed_detection(*kind, project_dir, error))
        })
        .collect()
}

fn failed_detection(
    kind: AgentKind,
    project_dir: Option<&Path>,
    error: anyhow::Error,
) -> AgentDetection {
    let executable = kind.executable_names().iter().find_map(|name| which(name));
    let cursor_app_exists =
        kind == AgentKind::Cursor && Path::new("/Applications/Cursor.app").exists();
    AgentDetection {
        kind,
        installed: executable.is_some() || cursor_app_exists,
        executable,
        running: false,
        matching_pids: Vec::new(),
        project_match: if project_dir.is_some() {
            ProjectMatch::Unknown
        } else {
            ProjectMatch::NotRequested
        },
        project_matching_pids: Vec::new(),
        observed_working_directories: Vec::new(),
        detail: format!("{} process detection failed: {error}", kind.display_name()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterManifest {
    pub version: u32,
    pub installed_at: DateTime<Utc>,
    pub rucksack_binary: PathBuf,
    pub files: Vec<ManagedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedFile {
    pub agent: AgentKind,
    pub path: PathBuf,
    pub created_by_rucksack: bool,
    pub backup: Option<PathBuf>,
    pub kind: ManagedFileKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagedFileKind {
    Hooks,
    Skill,
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterFileStatus {
    Current,
    Missing,
    Invalid,
    Unowned,
    Outdated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterFileEvidence {
    pub path: PathBuf,
    pub kind: ManagedFileKind,
    pub status: AdapterFileStatus,
    pub exists: bool,
    pub parsed: bool,
    pub marker_present: bool,
    pub schema_match: Option<bool>,
    pub binary_match: Option<bool>,
    pub detail: String,
}

impl AdapterFileEvidence {
    fn missing(path: &Path, kind: ManagedFileKind) -> Self {
        Self {
            path: path.to_path_buf(),
            kind,
            status: AdapterFileStatus::Missing,
            exists: false,
            parsed: false,
            marker_present: false,
            schema_match: None,
            binary_match: None,
            detail: "Expected adapter file is missing".to_owned(),
        }
    }

    fn invalid(path: &Path, kind: ManagedFileKind, detail: String) -> Self {
        Self {
            path: path.to_path_buf(),
            kind,
            status: AdapterFileStatus::Invalid,
            exists: path.exists(),
            parsed: false,
            marker_present: false,
            schema_match: None,
            binary_match: None,
            detail,
        }
    }

    pub fn is_current(&self) -> bool {
        self.status == AdapterFileStatus::Current
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAdapterEvidence {
    pub agent: AgentKind,
    pub current: bool,
    pub files: Vec<AdapterFileEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterVerificationReport {
    pub rucksack_binary: PathBuf,
    pub current: bool,
    pub agents: Vec<AgentAdapterEvidence>,
}

impl AdapterManifest {
    pub fn load(paths: &AppPaths) -> Result<Option<Self>> {
        let manifest = read_json_if_exists::<Self>(&paths.adapter_manifest_file)?;
        if let Some(manifest) = &manifest {
            if manifest.version != ADAPTER_MANIFEST_VERSION {
                anyhow::bail!(
                    "Unsupported adapter manifest version {} in {}; expected version {}",
                    manifest.version,
                    paths.adapter_manifest_file.display(),
                    ADAPTER_MANIFEST_VERSION
                );
            }
        }
        Ok(manifest)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        atomic_write_json(&paths.adapter_manifest_file, self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterInstallReport {
    pub manifest: AdapterManifest,
    pub changed: Vec<PathBuf>,
    pub unchanged: Vec<PathBuf>,
    pub verification: AdapterVerificationReport,
}

pub fn install_adapters(
    paths: &AppPaths,
    rucksack_binary: &Path,
    agents: &[AgentKind],
) -> Result<AdapterInstallReport> {
    let existing = AdapterManifest::load(paths)?;
    let agents = agents_to_install(paths, agents, existing.as_ref());
    for agent in &agents {
        preflight_install(*agent, paths)?;
    }

    let mut files = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged = Vec::new();

    for agent in &agents {
        let mutations = match agent {
            AgentKind::Codex => codex::install(paths, rucksack_binary)?,
            AgentKind::Claude => claude::install(paths, rucksack_binary)?,
            AgentKind::Cursor => cursor::install(paths, rucksack_binary)?,
        };
        for mutation in mutations {
            if mutation.changed {
                changed.push(mutation.file.path.clone());
            } else {
                unchanged.push(mutation.file.path.clone());
            }
            files.push(mutation.file);
        }
    }

    if let Some(existing) = existing {
        for old in existing.files {
            if let Some(new) = files.iter_mut().find(|new| new.path == old.path) {
                new.created_by_rucksack |= old.created_by_rucksack;
                if new.backup.is_none() {
                    new.backup = old.backup;
                }
            } else if is_expected_managed_file(paths, &old) {
                files.push(old);
            }
        }
    }

    let manifest = AdapterManifest {
        version: ADAPTER_MANIFEST_VERSION,
        installed_at: Utc::now(),
        rucksack_binary: rucksack_binary.to_path_buf(),
        files,
    };
    manifest.save(paths)?;
    let verification = verify_adapters(paths, rucksack_binary, &agents);
    Ok(AdapterInstallReport {
        manifest,
        changed,
        unchanged,
        verification,
    })
}

pub fn remove_adapters(paths: &AppPaths, agents: &[AgentKind]) -> Result<Vec<PathBuf>> {
    let manifest = AdapterManifest::load(paths)?;
    for agent in agents {
        preflight_uninstall(*agent, paths)?;
    }
    let created_hook_files = manifest
        .as_ref()
        .map(|manifest| {
            manifest
                .files
                .iter()
                .filter(|file| {
                    agents.contains(&file.agent)
                        && file.created_by_rucksack
                        && file.kind == ManagedFileKind::Hooks
                        && file.path == expected_hook_path(paths, file.agent)
                })
                .map(|file| file.path.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut changed = Vec::new();

    for agent in agents {
        let agent_changed = match agent {
            AgentKind::Codex => codex::uninstall(paths)?,
            AgentKind::Claude => claude::uninstall(paths)?,
            AgentKind::Cursor => cursor::uninstall(paths)?,
        };
        changed.extend(agent_changed);
    }

    for path in created_hook_files {
        if empty_managed_hook_config(&path)? {
            remove_if_exists(&path)?;
            if !changed.contains(&path) {
                changed.push(path);
            }
        }
    }

    if let Some(mut manifest) = manifest {
        manifest.files.retain(|file| !agents.contains(&file.agent));
        if manifest.files.is_empty() {
            remove_if_exists(&paths.adapter_manifest_file)?;
        } else {
            manifest.save(paths)?;
        }
    }
    Ok(changed)
}

pub fn verify_adapter(
    paths: &AppPaths,
    rucksack_binary: &Path,
    agent: AgentKind,
) -> AgentAdapterEvidence {
    match agent {
        AgentKind::Codex => codex::verify(paths, rucksack_binary),
        AgentKind::Claude => claude::verify(paths, rucksack_binary),
        AgentKind::Cursor => cursor::verify(paths, rucksack_binary),
    }
}

pub fn verify_adapters(
    paths: &AppPaths,
    rucksack_binary: &Path,
    agents: &[AgentKind],
) -> AdapterVerificationReport {
    let agents = deduplicate_agents(agents)
        .into_iter()
        .map(|agent| verify_adapter(paths, rucksack_binary, agent))
        .collect::<Vec<_>>();
    AdapterVerificationReport {
        rucksack_binary: rucksack_binary.to_path_buf(),
        current: agents.iter().all(|agent| agent.current),
        agents,
    }
}

fn agents_to_install(
    paths: &AppPaths,
    requested: &[AgentKind],
    existing: Option<&AdapterManifest>,
) -> Vec<AgentKind> {
    let mut agents = requested.to_vec();
    if let Some(existing) = existing {
        let previously_managed = deduplicate_agents(
            &existing
                .files
                .iter()
                .map(|file| file.agent)
                .collect::<Vec<_>>(),
        )
        .into_iter()
        .filter(|agent| {
            verify_adapter(paths, &existing.rucksack_binary, *agent)
                .files
                .iter()
                .any(|file| file.marker_present)
        });
        agents.extend(previously_managed);
    }
    deduplicate_agents(&agents)
}

fn expected_hook_path(paths: &AppPaths, agent: AgentKind) -> &Path {
    match agent {
        AgentKind::Codex => &paths.codex_hooks,
        AgentKind::Claude => &paths.claude_settings,
        AgentKind::Cursor => &paths.cursor_hooks,
    }
}

fn is_expected_managed_file(paths: &AppPaths, file: &ManagedFile) -> bool {
    match (file.agent, file.kind) {
        (AgentKind::Codex, ManagedFileKind::Hooks) => file.path == paths.codex_hooks,
        (AgentKind::Codex, ManagedFileKind::Skill) => file.path == paths.codex_skill,
        (AgentKind::Claude, ManagedFileKind::Hooks) => file.path == paths.claude_settings,
        (AgentKind::Claude, ManagedFileKind::Skill) => file.path == paths.claude_skill,
        (AgentKind::Cursor, ManagedFileKind::Hooks) => file.path == paths.cursor_hooks,
        _ => false,
    }
}

fn deduplicate_agents(agents: &[AgentKind]) -> Vec<AgentKind> {
    let mut seen = HashSet::new();
    agents
        .iter()
        .copied()
        .filter(|agent| seen.insert(*agent))
        .collect()
}

fn preflight_install(agent: AgentKind, paths: &AppPaths) -> Result<()> {
    match agent {
        AgentKind::Codex => codex::preflight_install(paths),
        AgentKind::Claude => claude::preflight_install(paths),
        AgentKind::Cursor => cursor::preflight_install(paths),
    }
}

fn preflight_uninstall(agent: AgentKind, paths: &AppPaths) -> Result<()> {
    match agent {
        AgentKind::Codex => codex::preflight_uninstall(paths),
        AgentKind::Claude => claude::preflight_uninstall(paths),
        AgentKind::Cursor => cursor::preflight_uninstall(paths),
    }
}

fn managed_skill_text() -> String {
    format!("<!-- {MANAGED_MARKER} -->\n{}", skill_document())
}

fn managed_skill_prefix() -> String {
    format!("<!-- {MANAGED_MARKER} -->\n")
}

pub(crate) fn preflight_managed_skill(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    if !path.exists() {
        return Ok(());
    }
    let current = fs::read_to_string(path)
        .with_context(|| format!("Could not read managed skill {}", path.display()))?;
    if !current.starts_with(&managed_skill_prefix()) {
        anyhow::bail!(
            "Refusing to overwrite unowned skill {}. Move it or choose a different skill name.",
            path.display()
        );
    }
    Ok(())
}

pub(crate) fn preflight_remove_managed_skill(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    if path.exists() {
        fs::read_to_string(path)
            .with_context(|| format!("Could not read managed skill {}", path.display()))?;
    }
    Ok(())
}

pub(crate) fn install_managed_skill(agent: AgentKind, path: &Path) -> Result<Mutation> {
    preflight_managed_skill(path)?;
    let existed = path.exists();
    let managed = managed_skill_text();
    let current = if existed {
        fs::read_to_string(path)
            .with_context(|| format!("Could not read managed skill {}", path.display()))?
    } else {
        String::new()
    };
    let changed = current != managed;
    let backup = if changed && existed {
        backup_once(path, MANAGED_MARKER)?
    } else {
        None
    };
    if changed {
        atomic_write(path, managed.as_bytes(), 0o600)?;
    }
    Ok(Mutation {
        file: ManagedFile {
            agent,
            path: path.to_path_buf(),
            created_by_rucksack: !existed,
            backup,
            kind: ManagedFileKind::Skill,
        },
        changed,
    })
}

pub(crate) fn verify_managed_skill(path: &Path) -> AdapterFileEvidence {
    if let Err(error) = reject_symlink(path) {
        return AdapterFileEvidence::invalid(
            path,
            ManagedFileKind::Skill,
            format!("Unsafe skill path: {error}"),
        );
    }
    if !path.exists() {
        return AdapterFileEvidence::missing(path, ManagedFileKind::Skill);
    }
    let current = match fs::read_to_string(path) {
        Ok(current) => current,
        Err(error) => {
            return AdapterFileEvidence::invalid(
                path,
                ManagedFileKind::Skill,
                format!("Could not read managed skill: {error}"),
            );
        }
    };
    let marker_present = current.starts_with(&managed_skill_prefix());
    if !marker_present {
        return AdapterFileEvidence {
            path: path.to_path_buf(),
            kind: ManagedFileKind::Skill,
            status: AdapterFileStatus::Unowned,
            exists: true,
            parsed: true,
            marker_present: false,
            schema_match: Some(false),
            binary_match: None,
            detail: "The skill exists but is not owned by rucksack".to_owned(),
        };
    }
    let schema_match = current == managed_skill_text();
    AdapterFileEvidence {
        path: path.to_path_buf(),
        kind: ManagedFileKind::Skill,
        status: if schema_match {
            AdapterFileStatus::Current
        } else {
            AdapterFileStatus::Outdated
        },
        exists: true,
        parsed: true,
        marker_present,
        schema_match: Some(schema_match),
        binary_match: None,
        detail: if schema_match {
            "Managed skill content is current".to_owned()
        } else {
            "Managed skill content is outdated".to_owned()
        },
    }
}

pub(crate) fn remove_managed_skill(path: &Path) -> Result<bool> {
    preflight_remove_managed_skill(path)?;
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("Could not read managed skill {}", path.display()))?;
    if !text.starts_with(&managed_skill_prefix()) {
        return Ok(false);
    }
    remove_if_exists(path)?;
    Ok(true)
}

fn empty_managed_hook_config(path: &Path) -> Result<bool> {
    let Some(root) = read_json_if_exists::<serde_json::Value>(path)? else {
        return Ok(false);
    };
    let Some(object) = root.as_object() else {
        return Ok(false);
    };
    Ok(object.iter().all(|(key, value)| match key.as_str() {
        "version" => value.as_u64() == Some(1),
        "hooks" => value.as_object().is_some_and(serde_json::Map::is_empty),
        _ => false,
    }))
}

pub(crate) fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("'{}'", text.replace('\'', "'\\''"))
}

pub(crate) struct Mutation {
    pub file: ManagedFile,
    pub changed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::read_json;
    use serde_json::{json, Value};
    use tempfile::{tempdir, TempDir};

    struct TestPaths {
        _directory: TempDir,
        paths: AppPaths,
    }

    fn test_paths() -> TestPaths {
        let directory = tempdir().unwrap();
        let root = directory.path();
        let data_dir = root.join("data");
        let log_dir = data_dir.join("logs");
        TestPaths {
            paths: AppPaths {
                home: root.to_path_buf(),
                data_dir: data_dir.clone(),
                config_file: data_dir.join("config.toml"),
                session_file: data_dir.join("session.json"),
                report_file: data_dir.join("last-report.json"),
                policy_file: data_dir.join("active-policy.json"),
                adapter_manifest_file: data_dir.join("adapters.json"),
                log_dir: log_dir.clone(),
                daemon_log: log_dir.join("daemon.log"),
                codex_hooks: root.join(".codex/hooks.json"),
                codex_skill: root.join(".agents/skills/commute-mode/SKILL.md"),
                claude_settings: root.join(".claude/settings.json"),
                claude_skill: root.join(".claude/skills/commute-mode/SKILL.md"),
                cursor_hooks: root.join(".cursor/hooks.json"),
            },
            _directory: directory,
        }
    }

    fn write_json(path: &Path, value: &Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = serde_json::to_vec_pretty(value).unwrap();
        bytes.push(b'\n');
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn detection_uses_process_cwd_without_requiring_project_in_arguments() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let nested = project.join("crates/core");
        fs::create_dir_all(&nested).unwrap();
        let inventory = vec![ProcessInfo {
            pid: 41,
            command: "/opt/tools/codex".to_owned(),
            arguments: "codex resume".to_owned(),
        }];

        let detection = detect_from_inventory(
            AgentKind::Codex,
            Some(&project),
            Some(PathBuf::from("/opt/tools/codex")),
            false,
            &inventory,
            |_| Some(nested.clone()),
        );

        assert!(detection.running);
        assert_eq!(detection.matching_pids, vec![41]);
        assert_eq!(detection.project_match, ProjectMatch::Matched);
        assert_eq!(detection.project_matching_pids, vec![41]);
        assert!(detection.detail.contains("for this project"));
    }

    #[test]
    fn detection_does_not_claim_a_project_without_cwd_evidence() {
        let inventory = vec![ProcessInfo {
            pid: 42,
            command: "claude".to_owned(),
            arguments: "claude --resume".to_owned(),
        }];

        let detection = detect_from_inventory(
            AgentKind::Claude,
            Some(Path::new("/workspace/project")),
            None,
            false,
            &inventory,
            |_| None,
        );

        assert!(detection.running);
        assert_eq!(detection.project_match, ProjectMatch::Unknown);
        assert!(detection.detail.contains("could not be verified"));
    }

    #[test]
    fn generic_agent_process_is_not_detected_as_cursor() {
        let inventory = vec![ProcessInfo {
            pid: 43,
            command: "agent".to_owned(),
            arguments: "agent /workspace/project".to_owned(),
        }];

        let detection = detect_from_inventory(
            AgentKind::Cursor,
            Some(Path::new("/workspace/project")),
            None,
            false,
            &inventory,
            |_| Some(PathBuf::from("/workspace/project")),
        );

        assert!(!detection.running);
        assert_eq!(detection.project_match, ProjectMatch::NotRunning);
        assert!(!AgentKind::Cursor.executable_names().contains(&"agent"));
    }

    #[test]
    fn macos_truncated_process_command_uses_argv_only_for_agent_identity() {
        let codex = ProcessInfo {
            pid: 44,
            command: "/Applications/Ch".to_owned(),
            arguments:
                "/Applications/ChatGPT.app/Contents/Resources/codex app-server --listen stdio://"
                    .to_owned(),
        };
        let renderer = ProcessInfo {
            pid: 45,
            command: "/Applications/Ch".to_owned(),
            arguments: "/Applications/ChatGPT.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer).app/Contents/MacOS/Codex (Renderer)"
                .to_owned(),
        };
        let claude_desktop = ProcessInfo {
            pid: 46,
            command: "/Applications/Cl".to_owned(),
            arguments: "/Applications/Claude.app/Contents/MacOS/Claude".to_owned(),
        };

        assert!(process_matches(AgentKind::Codex, &codex));
        assert!(!process_matches(AgentKind::Codex, &renderer));
        assert!(!process_matches(AgentKind::Claude, &claude_desktop));
    }

    #[test]
    fn codex_app_server_cwd_does_not_make_an_unrelated_project_claim() {
        let inventory = vec![ProcessInfo {
            pid: 47,
            command: "/Applications/Ch".to_owned(),
            arguments:
                "/Applications/ChatGPT.app/Contents/Resources/codex app-server --listen stdio://"
                    .to_owned(),
        }];

        let detection = detect_from_inventory(
            AgentKind::Codex,
            Some(Path::new("/workspace/project")),
            None,
            false,
            &inventory,
            |_| Some(PathBuf::from("/")),
        );

        assert!(detection.running);
        assert_eq!(detection.project_match, ProjectMatch::Unknown);
        assert!(!detection.observed_working_directories[0].authoritative_for_project);
    }

    #[test]
    fn adapter_relocation_updates_every_managed_agent_and_preserves_user_hooks() {
        let fixture = test_paths();
        let paths = &fixture.paths;
        write_json(
            &paths.codex_hooks,
            &json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "./user-stop"}]}]}}),
        );
        write_json(
            &paths.claude_settings,
            &json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "./user-stop"}]}]}}),
        );
        write_json(
            &paths.cursor_hooks,
            &json!({"version": 1, "hooks": {"stop": [{"command": "./user-stop"}]}}),
        );
        let old_binary = Path::new("/Applications/Rucksack-old.app/Contents/MacOS/rucksack");
        let new_binary = Path::new("/Applications/Rucksack.app/Contents/MacOS/rucksack");

        let first = install_adapters(paths, old_binary, &AgentKind::ALL).unwrap();
        assert!(first.verification.current);
        let stale = verify_adapters(paths, new_binary, &AgentKind::ALL);
        assert!(!stale.current);
        for agent in &stale.agents {
            let hooks = agent
                .files
                .iter()
                .find(|file| file.kind == ManagedFileKind::Hooks)
                .unwrap();
            assert_eq!(hooks.status, AdapterFileStatus::Outdated);
            assert_eq!(hooks.schema_match, Some(true));
            assert_eq!(hooks.binary_match, Some(false));
        }

        let relocated = install_adapters(paths, new_binary, &[AgentKind::Cursor]).unwrap();
        assert!(relocated.verification.current);
        assert_eq!(relocated.verification.agents.len(), AgentKind::ALL.len());
        for path in [
            &paths.codex_hooks,
            &paths.claude_settings,
            &paths.cursor_hooks,
        ] {
            let text = fs::read_to_string(path).unwrap();
            assert!(text.contains(new_binary.to_str().unwrap()));
            assert!(!text.contains(old_binary.to_str().unwrap()));
            assert!(text.contains("./user-stop"));
        }
        let codex: Value = read_json(&paths.codex_hooks).unwrap();
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PermissionRequest",
            "PostToolUse",
            "Stop",
            "SessionEnd",
        ] {
            assert!(codex["hooks"].get(event).is_some(), "missing {event}");
        }
        assert_eq!(codex["hooks"]["SessionEnd"][0]["hooks"][0]["timeout"], 3);
        let claude: Value = read_json(&paths.claude_settings).unwrap();
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "Notification",
            "PermissionRequest",
            "PostToolUse",
            "Stop",
            "SessionEnd",
        ] {
            assert!(claude["hooks"].get(event).is_some(), "missing {event}");
        }
        let cursor: Value = read_json(&paths.cursor_hooks).unwrap();
        for event in [
            "sessionStart",
            "beforeSubmitPrompt",
            "beforeShellExecution",
            "afterShellExecution",
            "afterFileEdit",
            "stop",
            "sessionEnd",
        ] {
            assert!(cursor["hooks"].get(event).is_some(), "missing {event}");
        }

        remove_adapters(paths, &AgentKind::ALL).unwrap();
        assert!(!paths.codex_skill.exists());
        assert!(!paths.claude_skill.exists());
        assert!(!paths.adapter_manifest_file.exists());
        for path in [
            &paths.codex_hooks,
            &paths.claude_settings,
            &paths.cursor_hooks,
        ] {
            let text = fs::read_to_string(path).unwrap();
            assert!(text.contains("./user-stop"));
            assert!(!text.contains(MANAGED_MARKER));
        }
    }

    #[test]
    fn later_agent_preflight_failure_prevents_earlier_mutations() {
        let fixture = test_paths();
        let paths = &fixture.paths;
        fs::create_dir_all(paths.claude_skill.parent().unwrap()).unwrap();
        fs::write(
            &paths.claude_skill,
            "<!-- user file mentioning rucksack-managed -->\n",
        )
        .unwrap();

        let error = install_adapters(
            paths,
            Path::new("/usr/local/bin/rucksack"),
            &[AgentKind::Codex, AgentKind::Claude],
        )
        .unwrap_err();

        assert!(error.to_string().contains("unowned skill"));
        assert!(!paths.codex_hooks.exists());
        assert!(!paths.codex_skill.exists());
        assert!(!paths.adapter_manifest_file.exists());
    }

    #[test]
    fn adapter_verification_reads_expected_files_without_a_manifest() {
        let fixture = test_paths();
        let paths = &fixture.paths;
        let binary = Path::new("/usr/local/bin/rucksack");
        cursor::install(paths, binary).unwrap();

        assert!(!paths.adapter_manifest_file.exists());
        let evidence = verify_adapter(paths, binary, AgentKind::Cursor);
        assert!(evidence.current);
        assert_eq!(evidence.files.len(), 1);
        assert!(evidence.files[0].exists);
        assert!(evidence.files[0].parsed);
        assert!(evidence.files[0].marker_present);
        assert_eq!(evidence.files[0].schema_match, Some(true));
        assert_eq!(evidence.files[0].binary_match, Some(true));
    }

    #[test]
    fn manifest_paths_cannot_expand_adapter_removal_scope() {
        let fixture = test_paths();
        let paths = &fixture.paths;
        let victim = paths.home.join("unrelated.json");
        write_json(&victim, &json!({}));
        AdapterManifest {
            version: ADAPTER_MANIFEST_VERSION,
            installed_at: Utc::now(),
            rucksack_binary: PathBuf::from("/usr/local/bin/rucksack"),
            files: vec![ManagedFile {
                agent: AgentKind::Codex,
                path: victim.clone(),
                created_by_rucksack: true,
                backup: None,
                kind: ManagedFileKind::Hooks,
            }],
        }
        .save(paths)
        .unwrap();

        remove_adapters(paths, &[AgentKind::Codex]).unwrap();

        assert!(victim.exists());
        assert!(!paths.adapter_manifest_file.exists());
    }

    #[test]
    fn skill_ownership_requires_the_exact_leading_marker() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("SKILL.md");
        fs::write(
            &path,
            "# User skill\n\nThis text mentions rucksack-managed incidentally.\n",
        )
        .unwrap();

        assert!(preflight_managed_skill(&path).is_err());
        assert!(!remove_managed_skill(&path).unwrap());
        assert!(path.exists());
        let evidence = verify_managed_skill(&path);
        assert_eq!(evidence.status, AdapterFileStatus::Unowned);
        assert!(!evidence.marker_present);
    }
}
