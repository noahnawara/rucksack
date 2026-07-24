use super::{
    AdapterFileEvidence, AdapterFileStatus, ManagedFile, ManagedFileKind, Mutation, MANAGED_MARKER,
};
use crate::agent::AgentKind;
use crate::files::{atomic_write, backup_once, read_json, reject_symlink};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
#[cfg(test)]
use std::fs;
use std::path::Path;

pub fn command_hook(command: &str, status_message: Option<&str>, timeout_seconds: u64) -> Value {
    let mut value = json!({
        "type": "command",
        "command": command,
        "timeout": timeout_seconds
    });
    if let Some(status) = status_message {
        value["statusMessage"] = Value::String(status.to_owned());
    }
    value
}

pub fn wrapped_hook(command: &str, status_message: Option<&str>, timeout_seconds: u64) -> Value {
    json!({
        "hooks": [command_hook(command, status_message, timeout_seconds)]
    })
}

pub fn cursor_hook(command: &str, timeout_seconds: u64) -> Value {
    json!({
        "command": command,
        "timeout": timeout_seconds
    })
}

pub fn preflight_events(agent: AgentKind, path: &Path) -> Result<()> {
    reject_symlink(path)?;
    if !path.exists() {
        return Ok(());
    }
    let root = read_json::<Value>(path)?;
    validate_config(agent, path, &root)
}

pub fn preflight_uninstall_events(agent: AgentKind, path: &Path) -> Result<()> {
    preflight_events(agent, path)
}

pub fn install_events(
    agent: AgentKind,
    path: &Path,
    events: &[(&str, Value)],
    kind: ManagedFileKind,
) -> Result<Mutation> {
    preflight_events(agent, path)?;
    let existed = path.exists();
    let mut root = if existed {
        read_json::<Value>(path)?
    } else {
        json!({})
    };
    let mut changed = false;
    if agent == AgentKind::Cursor {
        let object = root
            .as_object_mut()
            .context("Hook configuration root must be a JSON object")?;
        match object.get("version") {
            None => {
                object.insert("version".to_owned(), Value::from(1));
                changed = true;
            }
            Some(Value::Number(number)) if number.as_u64() == Some(1) => {}
            Some(other) => anyhow::bail!(
                "Unsupported Cursor hooks version {other}; expected version 1 in {}",
                path.display()
            ),
        }
    }
    let hooks = ensure_object_field(&mut root, "hooks")?;

    let existing_events = hooks.keys().cloned().collect::<Vec<_>>();
    for event in existing_events {
        if events.iter().any(|(expected, _)| *expected == event) {
            continue;
        }
        let Some(array) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
            continue;
        };
        let next = array
            .iter()
            .filter_map(without_managed_hook)
            .collect::<Vec<_>>();
        if next.len() == array.len() {
            continue;
        }
        changed = true;
        if next.is_empty() {
            hooks.remove(&event);
        } else {
            *array = next;
        }
    }

    for (event, hook) in events {
        let array = hooks
            .entry((*event).to_owned())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .with_context(|| format!("hooks.{event} in {} is not an array", path.display()))?;
        let next = reconciled_entries(array, hook);
        if next != *array {
            *array = next;
            changed = true;
        }
    }

    let backup = if changed && existed {
        backup_once(path, MANAGED_MARKER)?
    } else {
        None
    };
    if changed {
        let mut bytes = serde_json::to_vec_pretty(&root)?;
        bytes.push(b'\n');
        atomic_write(path, &bytes, 0o600)?;
    }

    Ok(Mutation {
        file: ManagedFile {
            agent,
            path: path.to_path_buf(),
            created_by_rucksack: !existed,
            backup,
            kind,
        },
        changed,
    })
}

pub fn verify_events(
    agent: AgentKind,
    path: &Path,
    events: &[(&str, Value)],
) -> AdapterFileEvidence {
    if let Err(error) = reject_symlink(path) {
        return AdapterFileEvidence::invalid(
            path,
            ManagedFileKind::Hooks,
            format!("Unsafe hook path: {error}"),
        );
    }
    if !path.exists() {
        return AdapterFileEvidence::missing(path, ManagedFileKind::Hooks);
    }

    let root = match read_json::<Value>(path) {
        Ok(root) => root,
        Err(error) => {
            return AdapterFileEvidence::invalid(
                path,
                ManagedFileKind::Hooks,
                format!("Could not read hook configuration: {error}"),
            );
        }
    };
    if let Err(error) = validate_config(agent, path, &root) {
        return AdapterFileEvidence::invalid(
            path,
            ManagedFileKind::Hooks,
            format!("Invalid hook configuration: {error}"),
        );
    }

    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else {
        return AdapterFileEvidence {
            path: path.to_path_buf(),
            kind: ManagedFileKind::Hooks,
            status: AdapterFileStatus::Unowned,
            exists: true,
            parsed: true,
            marker_present: false,
            schema_match: Some(false),
            binary_match: Some(false),
            detail: "The hook file parses but contains no Rucksack-owned hooks".to_owned(),
        };
    };
    let managed_count = hooks
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter(|entry| is_managed_hook(entry))
        .count();
    if managed_count == 0 {
        return AdapterFileEvidence {
            path: path.to_path_buf(),
            kind: ManagedFileKind::Hooks,
            status: AdapterFileStatus::Unowned,
            exists: true,
            parsed: true,
            marker_present: false,
            schema_match: Some(false),
            binary_match: Some(false),
            detail: "The hook file parses but contains no Rucksack-owned hooks".to_owned(),
        };
    }

    let expected_schema_is_current = events.iter().all(|(event, expected)| {
        hooks
            .get(*event)
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .filter(|entry| is_managed_hook(entry))
                    .count()
                    == 1
                    && entries
                        .iter()
                        .any(|entry| same_managed_hook_schema(entry, expected))
            })
    });
    let expected_names = events.iter().map(|(event, _)| *event).collect::<Vec<_>>();
    let has_obsolete_managed = hooks.iter().any(|(event, entries)| {
        !expected_names.contains(&event.as_str())
            && entries
                .as_array()
                .is_some_and(|entries| entries.iter().any(is_managed_hook))
    });
    let root_schema_is_current =
        agent != AgentKind::Cursor || root.get("version").and_then(Value::as_u64) == Some(1);
    let schema_match = root_schema_is_current
        && expected_schema_is_current
        && !has_obsolete_managed
        && managed_count == events.len();
    let expected_commands = events
        .iter()
        .flat_map(|(_, hook)| managed_commands(hook))
        .collect::<Vec<_>>();
    let actual_commands = hooks
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter(|entry| is_managed_hook(entry))
        .flat_map(managed_commands)
        .collect::<Vec<_>>();
    let binary_match = !actual_commands.is_empty()
        && actual_commands
            .iter()
            .all(|command| expected_commands.contains(command));

    if schema_match && binary_match {
        AdapterFileEvidence {
            path: path.to_path_buf(),
            kind: ManagedFileKind::Hooks,
            status: AdapterFileStatus::Current,
            exists: true,
            parsed: true,
            marker_present: true,
            schema_match: Some(true),
            binary_match: Some(true),
            detail: format!(
                "{} managed hooks match the current schema and binary",
                events.len()
            ),
        }
    } else {
        AdapterFileEvidence {
            path: path.to_path_buf(),
            kind: ManagedFileKind::Hooks,
            status: AdapterFileStatus::Outdated,
            exists: true,
            parsed: true,
            marker_present: true,
            schema_match: Some(schema_match),
            binary_match: Some(binary_match),
            detail: "Managed hooks use a stale binary path, stale schema, or duplicate entries"
                .to_owned(),
        }
    }
}

pub fn uninstall_events(agent: AgentKind, path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    preflight_uninstall_events(agent, path)?;
    let mut root = read_json::<Value>(path)?;
    let mut changed = false;

    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            let Some(array) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
                continue;
            };
            let next = array
                .iter()
                .filter_map(without_managed_hook)
                .collect::<Vec<_>>();
            changed |= next != *array;
            *array = next;
            if array.is_empty() {
                hooks.remove(&event);
            }
        }
    }
    if root
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty)
    {
        root.as_object_mut()
            .context("Hook configuration root must be a JSON object")?
            .remove("hooks");
    }

    if !changed {
        return Ok(false);
    }

    let mut bytes = serde_json::to_vec_pretty(&root)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes, 0o600)?;
    Ok(true)
}

fn ensure_object_field<'a>(root: &'a mut Value, key: &str) -> Result<&'a mut Map<String, Value>> {
    let object = root
        .as_object_mut()
        .context("Hook configuration root must be a JSON object")?;
    let value = object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    value
        .as_object_mut()
        .with_context(|| format!("{key} must be a JSON object"))
}

fn validate_config(agent: AgentKind, path: &Path, root: &Value) -> Result<()> {
    let object = root
        .as_object()
        .context("Hook configuration root must be a JSON object")?;
    if agent == AgentKind::Cursor {
        match object.get("version") {
            None => {}
            Some(Value::Number(number)) if number.as_u64() == Some(1) => {}
            Some(other) => anyhow::bail!(
                "Unsupported Cursor hooks version {other}; expected version 1 in {}",
                path.display()
            ),
        }
    }

    let Some(hooks) = object.get("hooks") else {
        return Ok(());
    };
    let hooks = hooks
        .as_object()
        .with_context(|| format!("hooks in {} must be a JSON object", path.display()))?;
    for (event, entries) in hooks {
        if !entries.is_array() {
            anyhow::bail!("hooks.{event} in {} is not an array", path.display());
        }
    }
    Ok(())
}

fn reconciled_entries(entries: &[Value], expected: &Value) -> Vec<Value> {
    let insertion_index = entries
        .iter()
        .take_while(|entry| !is_managed_hook(entry))
        .count();
    let mut next = entries
        .iter()
        .filter_map(without_managed_hook)
        .collect::<Vec<_>>();
    next.insert(insertion_index.min(next.len()), expected.clone());
    next
}

fn without_managed_hook(value: &Value) -> Option<Value> {
    if !is_managed_hook(value) {
        return Some(value.clone());
    }
    let Value::Object(values) = value else {
        return Some(value.clone());
    };
    if values
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_managed_command)
    {
        return None;
    }

    let Some(hooks) = values.get("hooks").and_then(Value::as_array) else {
        return Some(value.clone());
    };
    let next_hooks = hooks
        .iter()
        .filter_map(without_managed_hook)
        .collect::<Vec<_>>();
    if next_hooks.is_empty() {
        return None;
    }
    let mut next = values.clone();
    next.insert("hooks".to_owned(), Value::Array(next_hooks));
    Some(Value::Object(next))
}

fn is_managed_hook(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(is_managed_hook),
        Value::Object(values) => {
            values
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(is_managed_command)
                || values
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| hooks.iter().any(is_managed_hook))
        }
        _ => false,
    }
}

fn managed_commands(value: &Value) -> Vec<&str> {
    match value {
        Value::Array(values) => values.iter().flat_map(managed_commands).collect(),
        Value::Object(values) => {
            let mut commands = values
                .get("command")
                .and_then(Value::as_str)
                .filter(|command| is_managed_command(command))
                .into_iter()
                .collect::<Vec<_>>();
            if let Some(hooks) = values.get("hooks") {
                commands.extend(managed_commands(hooks));
            }
            commands
        }
        _ => Vec::new(),
    }
}

fn same_managed_hook_schema(actual: &Value, expected: &Value) -> bool {
    let mut actual = actual.clone();
    let mut expected = expected.clone();
    redact_managed_commands(&mut actual);
    redact_managed_commands(&mut expected);
    actual == expected
}

fn redact_managed_commands(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(redact_managed_commands),
        Value::Object(values) => {
            if let Some(command) = values.get_mut("command") {
                if command.as_str().is_some_and(is_managed_command) {
                    *command = Value::String(format!("<binary> # {MANAGED_MARKER}"));
                }
            }
            if let Some(hooks) = values.get_mut("hooks") {
                redact_managed_commands(hooks);
            }
        }
        _ => {}
    }
}

fn is_managed_command(command: &str) -> bool {
    command.trim_end().ends_with(&format!("# {MANAGED_MARKER}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn cursor_install_adds_schema_and_preserves_user_hooks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "hooks": {
                    "stop": [{"command": "./user-stop.sh"}]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let event = cursor_hook("'/tmp/rucksack' hook cursor # rucksack-managed", 5);
        let result = install_events(
            AgentKind::Cursor,
            &path,
            &[("stop", event)],
            ManagedFileKind::Hooks,
        )
        .unwrap();
        assert!(result.changed);

        let root: Value = read_json(&path).unwrap();
        assert_eq!(root["version"], 1);
        let stop = root["hooks"]["stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert!(stop.iter().any(is_managed_hook));
        assert!(stop
            .iter()
            .any(|entry| entry["command"] == "./user-stop.sh"));
    }

    #[test]
    fn uninstall_removes_only_managed_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "hooks": {
                    "stop": [
                        {"command": "./user-stop.sh"},
                        {"command": "'/tmp/rucksack' hook cursor # rucksack-managed"}
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(uninstall_events(AgentKind::Cursor, &path).unwrap());
        let root: Value = read_json(&path).unwrap();
        assert_eq!(root["version"], 1);
        assert_eq!(root["hooks"]["stop"].as_array().unwrap().len(), 1);
        assert_eq!(root["hooks"]["stop"][0]["command"], "./user-stop.sh");
    }

    #[test]
    fn reinstall_replaces_managed_hook_and_removes_obsolete_event() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        let old = cursor_hook("'/old/rucksack' hook cursor # rucksack-managed", 5);
        let obsolete = cursor_hook("'/old/rucksack' hook cursor # rucksack-managed", 5);
        install_events(
            AgentKind::Cursor,
            &path,
            &[("stop", old), ("afterAgentResponse", obsolete)],
            ManagedFileKind::Hooks,
        )
        .unwrap();

        let current = cursor_hook("'/new/rucksack' hook cursor # rucksack-managed", 5);
        install_events(
            AgentKind::Cursor,
            &path,
            &[("stop", current.clone())],
            ManagedFileKind::Hooks,
        )
        .unwrap();

        let root: Value = read_json(&path).unwrap();
        assert_eq!(root["hooks"]["stop"], json!([current]));
        assert!(root["hooks"].get("afterAgentResponse").is_none());
    }

    #[test]
    fn ownership_requires_marker_on_command_field() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "hooks": {
                    "Stop": [{
                        "statusMessage": "rucksack-managed",
                        "hooks": [{"type": "command", "command": "./user-stop.sh"}]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert!(!uninstall_events(AgentKind::Codex, &path).unwrap());
        let root: Value = read_json(&path).unwrap();
        assert_eq!(
            root["hooks"]["Stop"][0]["hooks"][0]["command"],
            "./user-stop.sh"
        );
    }

    #[test]
    fn nested_user_hook_survives_managed_reinstall_and_uninstall() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "hooks": {
                    "Stop": [{
                        "matcher": "",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "'/old/rucksack' hook codex # rucksack-managed",
                                "timeout": 5
                            },
                            {
                                "type": "command",
                                "command": "./user-stop.sh"
                            }
                        ]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let current = wrapped_hook("'/new/rucksack' hook codex # rucksack-managed", None, 5);

        install_events(
            AgentKind::Codex,
            &path,
            &[("Stop", current)],
            ManagedFileKind::Hooks,
        )
        .unwrap();
        let installed: Value = read_json(&path).unwrap();
        let installed_text = serde_json::to_string(&installed).unwrap();
        assert!(installed_text.contains("/new/rucksack"));
        assert!(!installed_text.contains("/old/rucksack"));
        assert!(installed_text.contains("./user-stop.sh"));

        assert!(uninstall_events(AgentKind::Codex, &path).unwrap());
        let uninstalled: Value = read_json(&path).unwrap();
        let uninstalled_text = serde_json::to_string(&uninstalled).unwrap();
        assert!(!uninstalled_text.contains(MANAGED_MARKER));
        assert!(uninstalled_text.contains("./user-stop.sh"));
        assert_eq!(uninstalled["hooks"]["Stop"][0]["matcher"], "");
    }
}
