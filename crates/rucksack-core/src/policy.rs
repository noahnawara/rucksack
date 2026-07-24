use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Focus {
    #[default]
    Continue,
    Finish,
    Investigate,
    Review,
    LowPower,
}

impl fmt::Display for Focus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Continue => "continue",
            Self::Finish => "finish",
            Self::Investigate => "investigate",
            Self::Review => "review",
            Self::LowPower => "low-power",
        };
        f.write_str(value)
    }
}

impl FromStr for Focus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "continue" => Ok(Self::Continue),
            "finish" => Ok(Self::Finish),
            "investigate" | "research" => Ok(Self::Investigate),
            "review" => Ok(Self::Review),
            "low-power" | "low_power" | "battery" => Ok(Self::LowPower),
            other => Err(format!(
                "Unknown focus {other:?}; use continue, finish, investigate, review, or low-power"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyContext {
    pub focus: Focus,
    pub minutes_remaining: u64,
    pub battery_floor_percent: u8,
    pub project_name: Option<String>,
}

pub fn render_policy(context: &PolicyContext) -> String {
    let focus = match context.focus {
        Focus::Continue => {
            "Continue the current task and its stated acceptance criteria. Do not expand scope."
        }
        Focus::Finish => {
            "Prioritize finishing the current coherent unit of work and targeted validation. Do not start a new workstream."
        }
        Focus::Investigate => {
            "Prefer read-only investigation, evidence gathering, and a decision-ready recommendation. Make edits only when essential."
        }
        Focus::Review => {
            "Review the current changes for defects, missing tests, regressions, and risk. Keep edits narrow and corrective."
        }
        Focus::LowPower => {
            "Prefer low-CPU work: reading, reasoning, small edits, and targeted checks. Defer heavy builds and broad test suites."
        }
    };

    let project = context
        .project_name
        .as_deref()
        .map(|name| format!(" Project: {name}."))
        .unwrap_or_default();

    format!(
        r#"# Rucksack Commute Mode

The host Mac is closed or about to be closed, running on battery, and connected through a potentially unstable mobile network.{project}

## Objective

{focus}

## Working behavior

- Preserve momentum, but ask only questions that are truly blocking.
- For a non-blocking ambiguity, choose the least-destructive reversible assumption, state it briefly, and continue.
- Prefer small, reviewable, reversible changes. Keep a concise checkpoint in the conversation.
- Use bounded retries. If the same operation fails twice for the same reason, stop and report one clear next action.
- Surface immediately when waiting for approval, credentials, user input, or an external dependency.
- Run targeted tests first. Defer broad monorepo suites, Docker/VM workloads, local models, browser-heavy automation, large indexing jobs, and other heat-intensive work unless the user explicitly asks for them during this remote session.
- Do not broaden permissions, disable sandboxing, bypass approvals, or auto-approve tools.
- Do not deploy, publish, merge, release, rotate credentials, modify production, delete data, apply destructive database or infrastructure changes, or perform another irreversible action without explicit authorization in the current remote conversation.
- Do not create work merely to remain busy. When the useful bounded task is complete, summarize changes, validation, residual risk, and the next decision.

## Safety envelope

This commute lease has roughly {minutes} minutes remaining. The host will restore normal sleep at or before {floor}% battery or when thermal safety trips. Plan work so a clean checkpoint exists before that boundary.
"#,
        project = project,
        focus = focus,
        minutes = context.minutes_remaining,
        floor = context.battery_floor_percent,
    )
}

pub fn skill_document() -> &'static str {
    r#"---
name: commute-mode
description: Continue the current coding task safely while the host Mac is closed, on battery, and controlled remotely.
---

# Commute Mode

Adopt Rucksack's active Commute Mode policy. First read
`~/Library/Application Support/Rucksack/active-policy.json`; if it exists, follow the `policy`
field exactly. The Rucksack hook may also supply the same policy as session context. If neither
source contains an active policy, tell the user to run `rucksack leave`.

Keep normal permissions and sandboxing. Prefer reversible progress, targeted validation,
bounded retries, low thermal load, and explicit blocking-state reports. Do not perform
irreversible or production actions without explicit authorization in the current remote
conversation.
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_includes_safety_boundary() {
        let text = render_policy(&PolicyContext {
            focus: Focus::Finish,
            minutes_remaining: 42,
            battery_floor_percent: 15,
            project_name: Some("atlas".to_owned()),
        });
        assert!(text.contains("42 minutes"));
        assert!(text.contains("15%"));
        assert!(text.contains("Do not broaden permissions"));
    }
}
