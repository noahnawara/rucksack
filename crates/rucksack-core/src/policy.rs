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
            "Prioritize finishing the current coherent unit of work and run all validation it requires. Do not start a new workstream."
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
    let workload = if context.focus == Focus::LowPower {
        "The user explicitly selected low-power focus. Prefer reading, reasoning, small edits, and targeted checks; defer heavy builds, Docker/VM work, broad suites, and large indexing."
    } else {
        "Run every workload required by the current task, including builds, broad test suites, Docker/VM work, browser automation, and indexing. Commute Mode adds no workload restriction."
    };

    format!(
        r#"# rucksack Commute Mode

The host Mac is closed or about to be closed, running on battery, and connected through a potentially unstable mobile network.{project}

## Objective

{focus}

## Working behavior

- Preserve momentum, but ask only questions that are truly blocking.
- For a non-blocking ambiguity, make a reasonable assumption, state it briefly, and continue.
- Keep a concise checkpoint in the conversation.
- Use bounded retries. If the same operation fails twice for the same reason, stop and report one clear next action.
- Surface immediately when waiting for approval, credentials, user input, or an external dependency.
- {workload}
- Do not create work merely to remain busy. When the useful bounded task is complete, summarize changes, validation, residual risk, and the next decision.

## Safety envelope

This commute lease has roughly {minutes} minutes remaining. The host will restore normal sleep at or before {floor}% battery or when thermal safety trips. Plan work so a clean checkpoint exists before that boundary.
"#,
        project = project,
        focus = focus,
        workload = workload,
        minutes = context.minutes_remaining,
        floor = context.battery_floor_percent,
    )
}

pub fn skill_document() -> &'static str {
    include_str!("../../../assets/adapters/shared/commute-mode/SKILL.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_preserves_full_workload() {
        for focus in [
            Focus::Continue,
            Focus::Finish,
            Focus::Investigate,
            Focus::Review,
        ] {
            let text = render_policy(&PolicyContext {
                focus,
                minutes_remaining: 42,
                battery_floor_percent: 15,
                project_name: Some("atlas".to_owned()),
            });
            assert!(text.contains("42 minutes"), "focus={focus}");
            assert!(text.contains("15%"), "focus={focus}");
            assert!(text.contains("Docker/VM work"), "focus={focus}");
            assert!(
                text.contains("Commute Mode adds no workload restriction"),
                "focus={focus}"
            );
            assert!(!text.contains("defer heavy"), "focus={focus}");
            for removed_clause in [
                "broaden permissions",
                "disable sandboxing",
                "bypass approvals",
                "auto-approve tools",
                "Do not deploy",
                "modify production",
                "irreversible action",
                "least-destructive",
                "Prefer small, reviewable, reversible",
            ] {
                assert!(!text.contains(removed_clause), "focus={focus}");
            }
        }

        let skill = skill_document();
        assert!(!skill.contains("permission"));
        assert!(!skill.contains("irreversible"));
    }

    #[test]
    fn low_power_focus_is_the_only_workload_restriction() {
        let text = render_policy(&PolicyContext {
            focus: Focus::LowPower,
            minutes_remaining: 42,
            battery_floor_percent: 15,
            project_name: None,
        });

        assert!(text.contains("explicitly selected low-power focus"));
        assert!(text.contains("defer heavy builds"));
        assert!(!text.contains("Commute Mode adds no workload restriction"));
    }
}
