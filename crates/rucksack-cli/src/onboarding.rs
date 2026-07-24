use anyhow::{Context, Result};
use rucksack_core::agent::{AgentAdapterEvidence, AgentDetection, AgentKind};
use rucksack_core::onboarding::{
    EvidenceBasis, EvidenceKind, EvidenceStatus, RemoteOnboardingRegistry,
};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug)]
pub(crate) struct ProviderOnboarding {
    pub(crate) installation: EvidenceStatus,
    pub(crate) pairing: EvidenceStatus,
    pub(crate) native_trust: EvidenceStatus,
    pub(crate) phone_visibility: EvidenceStatus,
}

impl ProviderOnboarding {
    pub(crate) fn is_current(&self) -> bool {
        [
            &self.installation,
            &self.pairing,
            &self.native_trust,
            &self.phone_visibility,
        ]
        .into_iter()
        .all(|status| matches!(status, EvidenceStatus::Current))
    }

    pub(crate) fn needs_user_confirmation(&self, kind: EvidenceKind) -> bool {
        !matches!(self.status(kind), EvidenceStatus::Current)
    }

    pub(crate) fn status(&self, kind: EvidenceKind) -> &EvidenceStatus {
        match kind {
            EvidenceKind::Installation => &self.installation,
            EvidenceKind::Pairing => &self.pairing,
            EvidenceKind::NativeTrust => &self.native_trust,
            EvidenceKind::PhoneVisibility => &self.phone_visibility,
        }
    }

    pub(crate) fn detail(&self) -> String {
        format!(
            "installation={}, pairing={}, native_trust={}, phone_visibility={}",
            status_label(&self.installation),
            status_label(&self.pairing),
            status_label(&self.native_trust),
            status_label(&self.phone_visibility)
        )
    }
}

#[derive(Debug)]
pub(crate) struct ProviderOnboardingBases {
    pub(crate) installation: EvidenceBasis,
    pub(crate) pairing: EvidenceBasis,
    pub(crate) native_trust: EvidenceBasis,
    pub(crate) phone_visibility: EvidenceBasis,
}

impl ProviderOnboardingBases {
    pub(crate) fn basis(&self, kind: EvidenceKind) -> EvidenceBasis {
        match kind {
            EvidenceKind::Installation => self.installation.clone(),
            EvidenceKind::Pairing => self.pairing.clone(),
            EvidenceKind::NativeTrust => self.native_trust.clone(),
            EvidenceKind::PhoneVisibility => self.phone_visibility.clone(),
        }
    }
}

pub(crate) fn bases(
    detection: &AgentDetection,
    adapter: &AgentAdapterEvidence,
) -> Result<ProviderOnboardingBases> {
    if detection.kind != adapter.agent {
        anyhow::bail!(
            "Onboarding evidence mixed {} detection with {} adapter state",
            detection.kind,
            adapter.agent
        );
    }
    if !adapter.current {
        anyhow::bail!(
            "{} adapter must be current before remote onboarding",
            adapter.agent.display_name()
        );
    }

    let installation = installation_basis(detection)?;
    let native_trust = adapter_basis(adapter)?;
    let agent = detection.kind.to_string();
    let pairing =
        EvidenceBasis::from_parts(&[b"rucksack-pairing-attestation-v1", agent.as_bytes()]);
    let phone_visibility = EvidenceBasis::from_parts(&[
        b"rucksack-phone-visibility-attestation-v1",
        agent.as_bytes(),
    ]);

    Ok(ProviderOnboardingBases {
        installation,
        pairing,
        native_trust,
        phone_visibility,
    })
}

pub(crate) fn assess(
    registry: &RemoteOnboardingRegistry,
    agent: AgentKind,
    bases: &ProviderOnboardingBases,
) -> ProviderOnboarding {
    ProviderOnboarding {
        installation: registry.evidence_status(
            agent,
            EvidenceKind::Installation,
            &bases.installation,
        ),
        pairing: registry.evidence_status(agent, EvidenceKind::Pairing, &bases.pairing),
        native_trust: registry.evidence_status(
            agent,
            EvidenceKind::NativeTrust,
            &bases.native_trust,
        ),
        phone_visibility: registry.evidence_status(
            agent,
            EvidenceKind::PhoneVisibility,
            &bases.phone_visibility,
        ),
    }
}

pub(crate) fn status_label(status: &EvidenceStatus) -> &'static str {
    match status {
        EvidenceStatus::Missing => "missing",
        EvidenceStatus::Current => "current",
        EvidenceStatus::BasisChanged => "changed",
        EvidenceStatus::Invalidated { .. } => "invalidated",
    }
}

fn installation_basis(detection: &AgentDetection) -> Result<EvidenceBasis> {
    let installation_path = installation_path(detection);
    let canonical = installation_path.canonicalize().with_context(|| {
        format!(
            "Could not resolve the {} installation at {}",
            detection.kind.display_name(),
            installation_path.display()
        )
    })?;
    let metadata = fs::metadata(&canonical).with_context(|| {
        format!(
            "Could not inspect the {} installation at {}",
            detection.kind.display_name(),
            canonical.display()
        )
    })?;
    let modified = metadata
        .modified()
        .with_context(|| {
            format!(
                "Could not read the modified time for {}",
                canonical.display()
            )
        })?
        .duration_since(UNIX_EPOCH)
        .with_context(|| {
            format!(
                "The modified time for {} predates the Unix epoch",
                canonical.display()
            )
        })?;
    let agent = detection.kind.to_string();
    let path = canonical.to_string_lossy();
    let size = metadata.len().to_string();
    let modified_seconds = modified.as_secs().to_string();
    let modified_nanos = modified.subsec_nanos().to_string();
    Ok(EvidenceBasis::from_parts(&[
        b"rucksack-provider-installation-v1",
        agent.as_bytes(),
        path.as_bytes(),
        size.as_bytes(),
        modified_seconds.as_bytes(),
        modified_nanos.as_bytes(),
    ]))
}

fn installation_path(detection: &AgentDetection) -> PathBuf {
    detection.executable.clone().unwrap_or_else(|| {
        if detection.kind == AgentKind::Cursor {
            PathBuf::from("/Applications/Cursor.app")
        } else {
            PathBuf::from(
                detection
                    .kind
                    .executable_names()
                    .first()
                    .copied()
                    .unwrap_or_default(),
            )
        }
    })
}

fn adapter_basis(adapter: &AgentAdapterEvidence) -> Result<EvidenceBasis> {
    #[derive(Serialize)]
    struct AdapterBasis<'a> {
        agent: AgentKind,
        files: Vec<AdapterFileBasis<'a>>,
    }

    #[derive(Serialize)]
    struct AdapterFileBasis<'a> {
        path: &'a Path,
        kind: rucksack_core::agent::ManagedFileKind,
        status: rucksack_core::agent::AdapterFileStatus,
        schema_match: Option<bool>,
        binary_match: Option<bool>,
    }

    let projection = AdapterBasis {
        agent: adapter.agent,
        files: adapter
            .files
            .iter()
            .map(|file| AdapterFileBasis {
                path: &file.path,
                kind: file.kind,
                status: file.status,
                schema_match: file.schema_match,
                binary_match: file.binary_match,
            })
            .collect(),
    };
    let bytes = serde_json::to_vec(&projection)?;
    Ok(EvidenceBasis::from_parts(&[
        b"rucksack-native-trust-v1",
        bytes.as_slice(),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rucksack_core::agent::{
        AdapterFileEvidence, AdapterFileStatus, ManagedFileKind, ProjectMatch,
    };
    use tempfile::tempdir;

    fn detection(executable: &Path) -> AgentDetection {
        AgentDetection {
            kind: AgentKind::Codex,
            installed: true,
            executable: Some(executable.to_path_buf()),
            running: true,
            matching_pids: vec![42],
            project_match: ProjectMatch::Matched,
            project_matching_pids: vec![42],
            observed_working_directories: Vec::new(),
            detail: "test".to_owned(),
        }
    }

    fn adapter(path: &Path) -> AgentAdapterEvidence {
        AgentAdapterEvidence {
            agent: AgentKind::Codex,
            current: true,
            files: vec![AdapterFileEvidence {
                path: path.to_path_buf(),
                kind: ManagedFileKind::Hooks,
                status: AdapterFileStatus::Current,
                exists: true,
                parsed: true,
                marker_present: true,
                schema_match: Some(true),
                binary_match: Some(true),
                detail: "current".to_owned(),
            }],
        }
    }

    #[test]
    fn unchanged_provider_and_adapter_produce_stable_bases() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("codex");
        let hook = directory.path().join("hooks.json");
        fs::write(&executable, "binary").unwrap();
        fs::write(&hook, "hooks").unwrap();

        let first = bases(&detection(&executable), &adapter(&hook)).unwrap();
        let second = bases(&detection(&executable), &adapter(&hook)).unwrap();

        assert_eq!(first.installation, second.installation);
        assert_eq!(first.pairing, second.pairing);
        assert_eq!(first.native_trust, second.native_trust);
        assert_eq!(first.phone_visibility, second.phone_visibility);
    }

    #[test]
    fn adapter_detail_text_does_not_change_the_native_trust_basis() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("codex");
        let hook = directory.path().join("hooks.json");
        fs::write(&executable, "binary").unwrap();
        fs::write(&hook, "hooks").unwrap();
        let mut changed_detail = adapter(&hook);
        changed_detail.files[0].detail = "different diagnostic wording".to_owned();

        let first = bases(&detection(&executable), &adapter(&hook)).unwrap();
        let second = bases(&detection(&executable), &changed_detail).unwrap();

        assert_eq!(first.native_trust, second.native_trust);
    }
}
