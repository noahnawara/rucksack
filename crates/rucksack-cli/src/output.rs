use std::io::{self, IsTerminal};

/// Everything rucksack says to the person running it.
///
/// Four verbs, one line each, no blank lines: `pack` is a two-second command and its whole output
/// should fit in a glance. `detail` is the escape hatch for the probes and paths behind a step,
/// and only `--verbose` shows it.
#[derive(Debug, Clone)]
pub struct Output {
    verbose: bool,
    color: bool,
}

impl Output {
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            color: io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    /// What just happened.
    pub fn step(&self, message: impl AsRef<str>) {
        println!("{}", message.as_ref());
    }

    /// The verdict the reader is waiting for. Always the last line.
    pub fn done(&self, message: impl AsRef<str>) {
        println!("{}", self.paint(BOLD, message.as_ref()));
    }

    /// Something the user should know, which did not stop rucksack.
    pub fn warn(&self, message: impl AsRef<str>) {
        println!("{} {}", self.paint(YELLOW, "!"), message.as_ref());
    }

    pub fn detail(&self, message: impl AsRef<str>) {
        if self.verbose {
            println!("  {}", message.as_ref());
        }
    }

    fn paint(&self, code: &str, value: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{value}\x1b[0m")
        } else {
            value.to_owned()
        }
    }
}

const BOLD: &str = "1";
const YELLOW: &str = "33";

/// Render a failure as "what stopped" then "what to do about it".
///
/// Messages carry their own next action on a second line, so the only formatting here is the
/// marker and a two-space indent that keeps the action visually attached to its cause.
pub fn render_error(error: &anyhow::Error) -> String {
    let mut rendered = String::new();
    for (index, line) in format!("{error:#}").lines().enumerate() {
        rendered.push_str(if index == 0 { "× " } else { "  " });
        rendered.push_str(line);
        rendered.push('\n');
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_leads_with_the_cause_and_indents_the_next_action() {
        let error = anyhow::anyhow!("The battery is at 12%.\nPlug in, then try again.");

        assert_eq!(
            render_error(&error),
            "× The battery is at 12%.\n  Plug in, then try again.\n"
        );
    }
}
