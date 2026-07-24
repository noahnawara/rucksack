use crate::agent::AgentKind;
use crate::files::{
    atomic_write_json, ensure_private_dir, read_json_if_exists, reject_symlink, with_advisory_lock,
};
use crate::paths::AppPaths;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ring::digest::{Context as DigestContext, SHA256};
use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const REMOTE_ONBOARDING_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct EvidenceBasis(String);

impl EvidenceBasis {
    pub fn from_parts(parts: &[&[u8]]) -> Self {
        let mut digest = DigestContext::new(&SHA256);
        for part in parts {
            digest.update(&(part.len() as u64).to_be_bytes());
            digest.update(part);
        }
        Self(hex_encode(digest.finish().as_ref()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(value: String) -> Result<Self> {
        if value.len() != 64
            || !value
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            anyhow::bail!("Evidence basis must be a 64-character lowercase SHA-256 digest");
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for EvidenceBasis {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceKind {
    Installation,
    Pairing,
    NativeTrust,
    PhoneVisibility,
}

impl EvidenceKind {
    fn required_source(self) -> EvidenceSource {
        match self {
            Self::Installation => EvidenceSource::Measured,
            Self::Pairing | Self::NativeTrust | Self::PhoneVisibility => {
                EvidenceSource::ConfirmedByUser
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceSource {
    Measured,
    ConfirmedByUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceInvalidationReason {
    ProviderInstallationChanged,
    AdapterChanged,
    AdapterRemoved,
    PairingReset,
    UserReset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceInvalidation {
    pub reason: EvidenceInvalidationReason,
    pub invalidated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub basis_sha256: EvidenceBasis,
    pub source: EvidenceSource,
    pub recorded_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalidation: Option<EvidenceInvalidation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRemoteOnboarding {
    pub agent: AgentKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation: Option<Evidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing: Option<Evidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_trust: Option<Evidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone_visibility: Option<Evidence>,
}

impl AgentRemoteOnboarding {
    pub fn evidence(&self, kind: EvidenceKind) -> Option<&Evidence> {
        match kind {
            EvidenceKind::Installation => self.installation.as_ref(),
            EvidenceKind::Pairing => self.pairing.as_ref(),
            EvidenceKind::NativeTrust => self.native_trust.as_ref(),
            EvidenceKind::PhoneVisibility => self.phone_visibility.as_ref(),
        }
    }

    fn evidence_mut(&mut self, kind: EvidenceKind) -> Option<&mut Evidence> {
        match kind {
            EvidenceKind::Installation => self.installation.as_mut(),
            EvidenceKind::Pairing => self.pairing.as_mut(),
            EvidenceKind::NativeTrust => self.native_trust.as_mut(),
            EvidenceKind::PhoneVisibility => self.phone_visibility.as_mut(),
        }
    }

    fn record(&mut self, kind: EvidenceKind, evidence: Evidence) {
        match kind {
            EvidenceKind::Installation => self.installation = Some(evidence),
            EvidenceKind::Pairing => self.pairing = Some(evidence),
            EvidenceKind::NativeTrust => self.native_trust = Some(evidence),
            EvidenceKind::PhoneVisibility => self.phone_visibility = Some(evidence),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteOnboardingRegistry {
    pub version: u32,
    pub agents: Vec<AgentRemoteOnboarding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceStatus {
    Missing,
    Current,
    BasisChanged,
    Invalidated {
        reason: EvidenceInvalidationReason,
        invalidated_at: DateTime<Utc>,
    },
}

impl RemoteOnboardingRegistry {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        validate_read_paths(paths)?;
        load_registry_file(&paths.remote_onboarding_file())
    }

    pub fn agent(&self, agent: AgentKind) -> Option<&AgentRemoteOnboarding> {
        self.agents.iter().find(|entry| entry.agent == agent)
    }

    pub fn evidence_status(
        &self,
        agent: AgentKind,
        kind: EvidenceKind,
        current_basis: &EvidenceBasis,
    ) -> EvidenceStatus {
        let Some(evidence) = self.agent(agent).and_then(|entry| entry.evidence(kind)) else {
            return EvidenceStatus::Missing;
        };
        if let Some(invalidation) = &evidence.invalidation {
            return EvidenceStatus::Invalidated {
                reason: invalidation.reason,
                invalidated_at: invalidation.invalidated_at,
            };
        }
        if evidence.basis_sha256 == *current_basis {
            EvidenceStatus::Current
        } else {
            EvidenceStatus::BasisChanged
        }
    }

    pub fn record_evidence(
        paths: &AppPaths,
        agent: AgentKind,
        kind: EvidenceKind,
        basis_sha256: EvidenceBasis,
        source: EvidenceSource,
        recorded_at: DateTime<Utc>,
    ) -> Result<Self> {
        validate_source(kind, source)?;
        update_registry(paths, |mut registry| {
            let evidence = Evidence {
                basis_sha256,
                source,
                recorded_at,
                invalidation: None,
            };
            match registry
                .agents
                .iter_mut()
                .find(|entry| entry.agent == agent)
            {
                Some(entry) => entry.record(kind, evidence),
                None => {
                    let mut entry = empty_agent(agent);
                    entry.record(kind, evidence);
                    registry.agents.push(entry);
                }
            }
            Ok(registry)
        })
    }

    pub fn invalidate_evidence(
        paths: &AppPaths,
        agent: AgentKind,
        kind: EvidenceKind,
        reason: EvidenceInvalidationReason,
        invalidated_at: DateTime<Utc>,
    ) -> Result<Self> {
        update_registry(paths, |mut registry| {
            if let Some(evidence) = registry
                .agents
                .iter_mut()
                .find(|entry| entry.agent == agent)
                .and_then(|entry| entry.evidence_mut(kind))
            {
                evidence.invalidation = Some(EvidenceInvalidation {
                    reason,
                    invalidated_at,
                });
            }
            Ok(registry)
        })
    }
}

fn empty_registry() -> RemoteOnboardingRegistry {
    RemoteOnboardingRegistry {
        version: REMOTE_ONBOARDING_VERSION,
        agents: Vec::new(),
    }
}

fn empty_agent(agent: AgentKind) -> AgentRemoteOnboarding {
    AgentRemoteOnboarding {
        agent,
        installation: None,
        pairing: None,
        native_trust: None,
        phone_visibility: None,
    }
}

fn validate_registry(registry: &RemoteOnboardingRegistry, path: &Path) -> Result<()> {
    if registry.version != REMOTE_ONBOARDING_VERSION {
        anyhow::bail!(
            "Unsupported remote onboarding version {} in {}; expected version {}",
            registry.version,
            path.display(),
            REMOTE_ONBOARDING_VERSION
        );
    }
    let mut agents = HashSet::new();
    for entry in &registry.agents {
        if !agents.insert(entry.agent) {
            anyhow::bail!(
                "Remote onboarding registry {} contains duplicate evidence for {}",
                path.display(),
                entry.agent
            );
        }
        for kind in [
            EvidenceKind::Installation,
            EvidenceKind::Pairing,
            EvidenceKind::NativeTrust,
            EvidenceKind::PhoneVisibility,
        ] {
            if let Some(evidence) = entry.evidence(kind) {
                validate_source(kind, evidence.source).with_context(|| {
                    format!(
                        "Invalid {} evidence for {} in {}",
                        evidence_kind_name(kind),
                        entry.agent,
                        path.display()
                    )
                })?;
            }
        }
    }
    Ok(())
}

fn validate_source(kind: EvidenceKind, source: EvidenceSource) -> Result<()> {
    let required = kind.required_source();
    if source != required {
        anyhow::bail!(
            "{} evidence must use source {}; observed {}",
            evidence_kind_name(kind),
            evidence_source_name(required),
            evidence_source_name(source)
        );
    }
    Ok(())
}

fn evidence_kind_name(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::Installation => "installation",
        EvidenceKind::Pairing => "pairing",
        EvidenceKind::NativeTrust => "native-trust",
        EvidenceKind::PhoneVisibility => "phone-visibility",
    }
}

fn evidence_source_name(source: EvidenceSource) -> &'static str {
    match source {
        EvidenceSource::Measured => "measured",
        EvidenceSource::ConfirmedByUser => "confirmed-by-user",
    }
}

fn load_registry_file(path: &Path) -> Result<RemoteOnboardingRegistry> {
    let Some(registry) = read_json_if_exists::<RemoteOnboardingRegistry>(path)? else {
        return Ok(empty_registry());
    };
    validate_registry(&registry, path)?;
    Ok(registry)
}

fn update_registry(
    paths: &AppPaths,
    operation: impl FnOnce(RemoteOnboardingRegistry) -> Result<RemoteOnboardingRegistry>,
) -> Result<RemoteOnboardingRegistry> {
    prepare_write_paths(paths)?;
    let registry_path = paths.remote_onboarding_file();
    let lock_path = registry_lock_path(&registry_path);
    with_advisory_lock(&lock_path, || {
        validate_read_paths(paths)?;
        validate_private_data_dir(&paths.data_dir)?;
        validate_existing_private_regular_file(&lock_path)?;

        let current = load_registry_file(&registry_path)?;
        let next = operation(current)?;
        validate_registry(&next, &registry_path)?;
        atomic_write_json(&registry_path, &next)?;
        validate_existing_private_regular_file(&registry_path)?;
        Ok(next)
    })
}

fn prepare_write_paths(paths: &AppPaths) -> Result<()> {
    if let Some(metadata) = validate_data_dir(&paths.data_dir)? {
        validate_data_dir_owner(&paths.data_dir, &metadata)?;
    }
    validate_state_files(paths)?;
    ensure_private_dir(&paths.data_dir)?;
    validate_private_data_dir(&paths.data_dir)?;
    validate_state_files(paths)
}

fn validate_read_paths(paths: &AppPaths) -> Result<()> {
    if validate_data_dir(&paths.data_dir)?.is_some() {
        validate_private_data_dir(&paths.data_dir)?;
    }
    validate_state_files(paths)
}

fn validate_state_files(paths: &AppPaths) -> Result<()> {
    let registry_path = paths.remote_onboarding_file();
    let lock_path = registry_lock_path(&registry_path);
    validate_existing_private_regular_file(&registry_path)?;
    validate_existing_private_regular_file(&lock_path)
}

fn validate_data_dir(path: &Path) -> Result<Option<fs::Metadata>> {
    reject_symlink(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Could not inspect data directory {}", path.display()));
        }
    };
    if !metadata.file_type().is_dir() {
        anyhow::bail!("rucksack data path {} is not a directory", path.display());
    }
    Ok(Some(metadata))
}

fn validate_existing_private_regular_file(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Could not inspect private file {}", path.display()));
        }
    };
    if !metadata.file_type().is_file() {
        anyhow::bail!(
            "Private state path {} is not a regular file",
            path.display()
        );
    }
    #[cfg(unix)]
    validate_private_unix_metadata(path, &metadata)?;
    Ok(())
}

#[cfg(unix)]
fn validate_private_unix_metadata(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let current_uid = unsafe { libc::geteuid() };
    if metadata.uid() != current_uid {
        anyhow::bail!(
            "Private state file {} is owned by uid {}; expected current uid {}",
            path.display(),
            metadata.uid(),
            current_uid
        );
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        anyhow::bail!(
            "Private state file {} has unsafe permissions {:o}; remove group/other access",
            path.display(),
            mode
        );
    }
    Ok(())
}

fn validate_private_data_dir(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Could not inspect data directory {}", path.display()))?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!("rucksack data path {} is not a directory", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        validate_data_dir_owner(path, &metadata)?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode != 0o700 {
            anyhow::bail!(
                "rucksack data directory {} has permissions {:o}; expected 700",
                path.display(),
                mode
            );
        }
    }
    Ok(())
}

fn validate_data_dir_owner(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let current_uid = unsafe { libc::geteuid() };
        if metadata.uid() != current_uid {
            anyhow::bail!(
                "rucksack data directory {} is owned by uid {}; expected current uid {}",
                path.display(),
                metadata.uid(),
                current_uid
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = metadata;
    }
    Ok(())
}

fn registry_lock_path(registry_path: &Path) -> PathBuf {
    registry_path.with_extension("lock")
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::atomic_write;
    use serde_json::{json, Value};
    use tempfile::tempdir;

    fn test_paths(root: &Path) -> AppPaths {
        let data_dir = root.join("data");
        let log_dir = root.join("logs");
        AppPaths {
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
        }
    }

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-24T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
            + chrono::Duration::minutes(minutes)
    }

    fn basis(label: &str) -> EvidenceBasis {
        EvidenceBasis::from_parts(&[label.as_bytes()])
    }

    fn write_registry_json(paths: &AppPaths, value: &Value) {
        ensure_private_dir(&paths.data_dir).unwrap();
        let bytes = serde_json::to_vec_pretty(value).unwrap();
        atomic_write(&paths.remote_onboarding_file(), &bytes, 0o600).unwrap();
    }

    #[test]
    fn missing_registry_loads_as_empty() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());

        let registry = RemoteOnboardingRegistry::load(&paths).unwrap();

        assert_eq!(registry.version, REMOTE_ONBOARDING_VERSION);
        assert!(registry.agents.is_empty());
        assert!(!paths.remote_onboarding_file().exists());
    }

    #[test]
    fn evidence_round_trips_and_reports_current_or_changed_basis() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let recorded_basis = basis("codex-installation");

        RemoteOnboardingRegistry::record_evidence(
            &paths,
            AgentKind::Codex,
            EvidenceKind::Installation,
            recorded_basis.clone(),
            EvidenceSource::Measured,
            at(0),
        )
        .unwrap();
        let loaded = RemoteOnboardingRegistry::load(&paths).unwrap();

        assert_eq!(
            loaded.evidence_status(
                AgentKind::Codex,
                EvidenceKind::Installation,
                &recorded_basis
            ),
            EvidenceStatus::Current
        );
        assert_eq!(
            loaded.evidence_status(
                AgentKind::Codex,
                EvidenceKind::Installation,
                &basis("replacement-installation")
            ),
            EvidenceStatus::BasisChanged
        );
    }

    #[test]
    fn future_versions_and_unknown_fields_are_rejected() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        write_registry_json(&paths, &json!({"version": 2, "agents": []}));
        let future_error = RemoteOnboardingRegistry::load(&paths).unwrap_err();
        assert!(future_error
            .to_string()
            .contains("Unsupported remote onboarding version 2"));

        write_registry_json(
            &paths,
            &json!({"version": 1, "agents": [], "unexpected": true}),
        );
        let unknown_error = RemoteOnboardingRegistry::load(&paths).unwrap_err();
        assert!(format!("{unknown_error:#}").contains("unexpected"));

        write_registry_json(
            &paths,
            &json!({
                "version": 1,
                "agents": [{
                    "agent": "codex",
                    "installation": {
                        "basis_sha256": basis("codex").as_str(),
                        "source": "measured",
                        "recorded_at": "2026-07-24T12:00:00Z",
                        "unexpected_nested": true
                    }
                }]
            }),
        );
        let nested_error = RemoteOnboardingRegistry::load(&paths).unwrap_err();
        assert!(format!("{nested_error:#}").contains("unexpected_nested"));
    }

    #[test]
    fn duplicate_agents_source_mismatches_and_invalid_digests_are_rejected() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let digest = basis("valid").as_str().to_owned();
        let installation = json!({
            "basis_sha256": digest,
            "source": "measured",
            "recorded_at": "2026-07-24T12:00:00Z"
        });
        write_registry_json(
            &paths,
            &json!({
                "version": 1,
                "agents": [
                    {"agent": "codex", "installation": installation},
                    {"agent": "codex"}
                ]
            }),
        );
        assert!(RemoteOnboardingRegistry::load(&paths)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));

        write_registry_json(
            &paths,
            &json!({
                "version": 1,
                "agents": [{
                    "agent": "codex",
                    "pairing": {
                        "basis_sha256": basis("pairing").as_str(),
                        "source": "measured",
                        "recorded_at": "2026-07-24T12:00:00Z"
                    }
                }]
            }),
        );
        let source_error = RemoteOnboardingRegistry::load(&paths).unwrap_err();
        assert!(format!("{source_error:#}")
            .contains("pairing evidence must use source confirmed-by-user"));

        write_registry_json(
            &paths,
            &json!({
                "version": 1,
                "agents": [{
                    "agent": "codex",
                    "installation": {
                        "basis_sha256": "ABC",
                        "source": "measured",
                        "recorded_at": "2026-07-24T12:00:00Z"
                    }
                }]
            }),
        );
        let digest_error = RemoteOnboardingRegistry::load(&paths).unwrap_err();
        assert!(format!("{digest_error:#}").contains("64-character lowercase SHA-256"));
    }

    #[test]
    fn invalidation_targets_only_one_agent_and_evidence_kind() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let codex_pairing = basis("codex-pairing");
        let claude_pairing = basis("claude-pairing");
        let codex_installation = basis("codex-installation");
        for (agent, kind, evidence_basis, source) in [
            (
                AgentKind::Codex,
                EvidenceKind::Pairing,
                codex_pairing.clone(),
                EvidenceSource::ConfirmedByUser,
            ),
            (
                AgentKind::Claude,
                EvidenceKind::Pairing,
                claude_pairing.clone(),
                EvidenceSource::ConfirmedByUser,
            ),
            (
                AgentKind::Codex,
                EvidenceKind::Installation,
                codex_installation.clone(),
                EvidenceSource::Measured,
            ),
        ] {
            RemoteOnboardingRegistry::record_evidence(
                &paths,
                agent,
                kind,
                evidence_basis,
                source,
                at(0),
            )
            .unwrap();
        }

        let registry = RemoteOnboardingRegistry::invalidate_evidence(
            &paths,
            AgentKind::Codex,
            EvidenceKind::Pairing,
            EvidenceInvalidationReason::PairingReset,
            at(1),
        )
        .unwrap();

        assert_eq!(
            registry.evidence_status(AgentKind::Codex, EvidenceKind::Pairing, &codex_pairing),
            EvidenceStatus::Invalidated {
                reason: EvidenceInvalidationReason::PairingReset,
                invalidated_at: at(1),
            }
        );
        assert_eq!(
            registry.evidence_status(AgentKind::Claude, EvidenceKind::Pairing, &claude_pairing),
            EvidenceStatus::Current
        );
        assert_eq!(
            registry.evidence_status(
                AgentKind::Codex,
                EvidenceKind::Installation,
                &codex_installation
            ),
            EvidenceStatus::Current
        );
    }

    #[cfg(unix)]
    #[test]
    fn writes_use_private_directory_file_and_lock_modes() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        RemoteOnboardingRegistry::record_evidence(
            &paths,
            AgentKind::Codex,
            EvidenceKind::Installation,
            basis("codex"),
            EvidenceSource::Measured,
            at(0),
        )
        .unwrap();

        let registry_path = paths.remote_onboarding_file();
        let lock_path = registry_lock_path(&registry_path);
        assert_eq!(
            fs::metadata(&paths.data_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(registry_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(lock_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn serialized_schema_contains_only_evidence_not_secrets() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        RemoteOnboardingRegistry::record_evidence(
            &paths,
            AgentKind::Cursor,
            EvidenceKind::PhoneVisibility,
            basis("cursor-phone-visibility"),
            EvidenceSource::ConfirmedByUser,
            at(0),
        )
        .unwrap();

        let bytes = fs::read(paths.remote_onboarding_file()).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let root = value.as_object().unwrap();
        assert_eq!(
            root.keys().map(String::as_str).collect::<HashSet<_>>(),
            HashSet::from(["version", "agents"])
        );
        let agent = value["agents"][0].as_object().unwrap();
        assert_eq!(
            agent.keys().map(String::as_str).collect::<HashSet<_>>(),
            HashSet::from(["agent", "phone_visibility"])
        );
        let evidence = agent["phone_visibility"].as_object().unwrap();
        assert_eq!(
            evidence.keys().map(String::as_str).collect::<HashSet<_>>(),
            HashSet::from(["basis_sha256", "source", "recorded_at"])
        );
        let serialized = String::from_utf8(bytes).unwrap();
        for forbidden in [
            "pairing_code",
            "provider_session_id",
            "confirmation_token",
            "\"secret\"",
        ] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_or_non_private_registry_paths_are_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        ensure_private_dir(&paths.data_dir).unwrap();
        let external = directory.path().join("external.json");
        atomic_write(&external, b"{\"version\":1,\"agents\":[]}", 0o600).unwrap();
        symlink(&external, paths.remote_onboarding_file()).unwrap();
        assert!(RemoteOnboardingRegistry::load(&paths)
            .unwrap_err()
            .to_string()
            .contains("symlinked"));

        fs::remove_file(paths.remote_onboarding_file()).unwrap();
        atomic_write(
            &paths.remote_onboarding_file(),
            b"{\"version\":1,\"agents\":[]}",
            0o600,
        )
        .unwrap();
        fs::set_permissions(
            paths.remote_onboarding_file(),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(RemoteOnboardingRegistry::load(&paths)
            .unwrap_err()
            .to_string()
            .contains("unsafe permissions"));

        fs::set_permissions(
            paths.remote_onboarding_file(),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        fs::set_permissions(&paths.data_dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(RemoteOnboardingRegistry::load(&paths)
            .unwrap_err()
            .to_string()
            .contains("expected 700"));
    }
}
