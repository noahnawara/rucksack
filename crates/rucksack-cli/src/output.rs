use serde::Serialize;
use serde_json::json;
use std::cell::Cell;
use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanEvent {
    Chapter(String),
    RucksackAction(String),
    UserAction(Vec<String>),
    Waiting {
        condition: String,
        continuation: String,
    },
    Fact(String),
    Warning(String),
}

#[derive(Debug, Clone)]
pub struct Output {
    verbose: bool,
    json: bool,
    color: bool,
    result_emitted: Cell<bool>,
}

impl Output {
    pub fn new(verbose: bool, json: bool) -> Self {
        let color = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() && !json;
        Self {
            verbose,
            json,
            color,
            result_emitted: Cell::new(false),
        }
    }

    pub fn json(&self) -> bool {
        self.json
    }

    pub fn story(&self, event: HumanEvent) {
        if self.json {
            for record in story_progress_records(&event) {
                println!("{record}");
            }
            return;
        }

        match event {
            HumanEvent::Chapter(message) => {
                println!();
                println!("{}", self.bold(&format!("🎒 {}", sentence(&message))));
                println!();
            }
            HumanEvent::RucksackAction(action) => {
                println!("{}", rucksack_action_line(&action));
            }
            HumanEvent::UserAction(instructions) => {
                println!();
                for (index, line) in user_action_lines(&instructions).iter().enumerate() {
                    if index == 0 {
                        println!("{}", self.bold(line));
                    } else {
                        println!("{line}");
                    }
                }
            }
            HumanEvent::Waiting {
                condition,
                continuation,
            } => {
                for line in waiting_lines(&condition, &continuation) {
                    println!("{line}");
                }
            }
            HumanEvent::Fact(message) => println!("{}", self.green(&fact_line(&message))),
            HumanEvent::Warning(message) => println!("{}", self.yellow(&warning_line(&message))),
        }
    }

    pub fn title(&self, subtitle: &str) {
        if self.json {
            self.emit_progress("title", subtitle);
            return;
        }
        println!();
        println!("{}", self.bold("rucksack"));
        println!("{subtitle}");
        println!();
    }

    pub fn section(&self, name: &str) {
        if self.json {
            self.emit_progress("section", name);
            return;
        }
        println!("{}", self.bold(name));
    }

    pub fn pass(&self, message: impl AsRef<str>) {
        if self.json {
            self.emit_progress("pass", message.as_ref());
        } else {
            println!("{} {}", self.green("✓"), message.as_ref())
        }
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        if self.json {
            self.emit_progress("warning", message.as_ref());
        } else {
            println!("{} {}", self.yellow("!"), message.as_ref())
        }
    }

    pub fn fail(&self, message: impl AsRef<str>) {
        if self.json {
            self.emit_progress("failure", message.as_ref());
        } else {
            eprintln!("{} {}", self.red("×"), message.as_ref());
        }
    }

    pub fn detail(&self, message: impl AsRef<str>) {
        if self.verbose {
            if self.json {
                self.emit_progress("detail", message.as_ref());
            } else {
                println!("  {}", message.as_ref())
            }
        }
    }

    pub fn blank(&self) {
        if !self.json {
            println!();
        }
    }

    pub fn plain(&self, message: impl AsRef<str>) {
        if self.json {
            self.emit_progress("message", message.as_ref());
        } else {
            println!("{}", message.as_ref())
        }
    }

    pub fn emit_json<T: Serialize>(&self, value: &T) -> anyhow::Result<()> {
        self.emit_json_result(value, true, None)
    }

    pub fn emit_json_failure<T: Serialize>(&self, value: &T, message: &str) -> anyhow::Result<()> {
        self.emit_json_result(value, false, Some(message))
    }

    fn emit_json_result<T: Serialize>(
        &self,
        value: &T,
        ok: bool,
        error_message: Option<&str>,
    ) -> anyhow::Result<()> {
        let record = json!({
            "schema_version": 1,
            "type": "result",
            "ok": ok,
            "data": serde_json::to_value(value)?,
            "error": error_message.map(|message| json!({ "message": message })),
        });
        serde_json::to_writer(io::stdout().lock(), &record)?;
        println!();
        self.result_emitted.set(true);
        Ok(())
    }

    pub fn finish_json(&self, command: &str) -> anyhow::Result<()> {
        if !self.json || self.result_emitted.get() {
            return Ok(());
        }
        self.emit_json(&json!({ "command": command }))
    }

    pub fn confirm(&self, prompt: &str, default_yes: bool) -> anyhow::Result<bool> {
        let mut stdin = io::stdin().lock();
        self.confirm_from(prompt, default_yes, &mut stdin)
    }

    fn confirm_from(
        &self,
        prompt: &str,
        default_yes: bool,
        reader: &mut impl BufRead,
    ) -> anyhow::Result<bool> {
        if self.json {
            anyhow::bail!("interactive confirmation is unavailable with --json");
        }
        let suffix = if default_yes { " [Y/n] " } else { " [y/N] " };
        print!("{prompt}{suffix}");
        io::stdout().flush()?;
        let line = read_response(reader)?;
        let value = line.trim().to_ascii_lowercase();
        if value.is_empty() {
            return Ok(default_yes);
        }
        Ok(matches!(value.as_str(), "y" | "yes"))
    }

    pub fn input(&self, prompt: &str) -> anyhow::Result<String> {
        let mut stdin = io::stdin().lock();
        self.input_from(prompt, &mut stdin)
    }

    fn input_from(&self, prompt: &str, reader: &mut impl BufRead) -> anyhow::Result<String> {
        if self.json {
            anyhow::bail!("interactive text input is unavailable with --json");
        }
        print!("{prompt}");
        io::stdout().flush()?;
        let line = read_response(reader)?;
        Ok(line.trim().to_owned())
    }

    pub fn choose(&self, prompt: &str, options: &[String]) -> anyhow::Result<usize> {
        let mut stdin = io::stdin().lock();
        self.choose_from(prompt, options, &mut stdin)
    }

    fn choose_from(
        &self,
        prompt: &str,
        options: &[String],
        reader: &mut impl BufRead,
    ) -> anyhow::Result<usize> {
        if options.is_empty() {
            anyhow::bail!("no choices available");
        }
        if options.len() == 1 {
            return Ok(0);
        }
        if self.json {
            anyhow::bail!("an explicit --agent is required with --json");
        }
        println!("{prompt}");
        for (index, option) in options.iter().enumerate() {
            println!("{}. {}", index + 1, option);
        }
        loop {
            print!("Select [{}]: ", 1);
            io::stdout().flush()?;
            let line = read_response(reader)?;
            let value = line.trim();
            if value.is_empty() {
                return Ok(0);
            }
            if let Ok(index) = value.parse::<usize>() {
                if (1..=options.len()).contains(&index) {
                    return Ok(index - 1);
                }
            }
            println!("Enter a number between 1 and {}.", options.len());
        }
    }

    fn bold(&self, value: &str) -> String {
        self.paint("1", value)
    }

    fn green(&self, value: &str) -> String {
        self.paint("32", value)
    }

    fn yellow(&self, value: &str) -> String {
        self.paint("33", value)
    }

    fn red(&self, value: &str) -> String {
        self.paint("31", value)
    }

    fn paint(&self, code: &str, value: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{value}\x1b[0m")
        } else {
            value.to_owned()
        }
    }

    fn emit_progress(&self, level: &str, message: &str) {
        let record = json!({
            "schema_version": 1,
            "type": "progress",
            "level": level,
            "message": message,
        });
        println!("{record}");
    }
}

fn sentence(value: &str) -> String {
    let value = value.trim();
    if value.ends_with('.') {
        value.to_owned()
    } else {
        format!("{value}.")
    }
}

fn fragment(value: &str) -> &str {
    value.trim().strip_suffix('.').unwrap_or(value.trim())
}

fn rucksack_action_line(action: &str) -> String {
    sentence(&format!("→ rucksack is {}", fragment(action)))
}

fn user_action_lines(instructions: &[String]) -> Vec<String> {
    let mut lines = Vec::with_capacity(instructions.len() + 2);
    lines.push("your turn.".to_owned());
    lines.push(String::new());
    lines.extend(
        instructions
            .iter()
            .map(|instruction| sentence(&format!("→ {}", fragment(instruction)))),
    );
    lines
}

fn waiting_lines(condition: &str, continuation: &str) -> [String; 2] {
    [
        sentence(&format!(
            "→ rucksack is waiting for {}",
            fragment(condition)
        )),
        sentence(&format!("  ↳ {}", fragment(continuation))),
    ]
}

fn fact_line(message: &str) -> String {
    sentence(&format!("✓ {}", fragment(message)))
}

fn warning_line(message: &str) -> String {
    sentence(&format!("! {}", fragment(message)))
}

fn story_progress_records(event: &HumanEvent) -> Vec<serde_json::Value> {
    match event {
        HumanEvent::Chapter(message) => {
            vec![story_progress_record("chapter", "rucksack", message, None)]
        }
        HumanEvent::RucksackAction(message) => {
            vec![story_progress_record("action", "rucksack", message, None)]
        }
        HumanEvent::UserAction(instructions) => instructions
            .iter()
            .map(|message| story_progress_record("action", "user", message, None))
            .collect(),
        HumanEvent::Waiting {
            condition,
            continuation,
        } => vec![story_progress_record(
            "waiting",
            "rucksack",
            condition,
            Some(continuation.as_str()),
        )],
        HumanEvent::Fact(message) => {
            vec![story_progress_record("fact", "system", message, None)]
        }
        HumanEvent::Warning(message) => {
            vec![story_progress_record("warning", "system", message, None)]
        }
    }
}

fn story_progress_record(
    kind: &str,
    actor: &str,
    message: &str,
    continuation: Option<&str>,
) -> serde_json::Value {
    json!({
        "schema_version": 1,
        "type": "progress",
        "level": kind,
        "kind": kind,
        "actor": actor,
        "message": message,
        "continuation": continuation,
    })
}

fn read_response(reader: &mut impl BufRead) -> anyhow::Result<String> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        anyhow::bail!("standard input closed before an interactive response was received");
    }
    Ok(line)
}

#[derive(Debug)]
pub struct JsonFailureEmitted;

impl fmt::Display for JsonFailureEmitted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("machine-readable failure result was already emitted")
    }
}

impl std::error::Error for JsonFailureEmitted {}

pub fn is_json_failure_emitted(error: &anyhow::Error) -> bool {
    error.downcast_ref::<JsonFailureEmitted>().is_some()
}

pub fn emit_json_error(error: &anyhow::Error) -> anyhow::Result<()> {
    let causes = error
        .chain()
        .skip(1)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let record = json!({
        "schema_version": 1,
        "type": "result",
        "ok": false,
        "data": null,
        "error": {
            "message": error.to_string(),
            "causes": causes,
        },
    });
    serde_json::to_writer(io::stdout().lock(), &record)?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn confirm_rejects_eof_and_preserves_blank_line_default() {
        let output = Output::new(false, false);
        let mut eof = Cursor::new(b"".as_slice());
        let error = output
            .confirm_from("Continue?", true, &mut eof)
            .unwrap_err();
        assert!(error.to_string().contains("standard input closed"));

        let mut blank = Cursor::new(b"\n".as_slice());
        assert!(output.confirm_from("Continue?", true, &mut blank).unwrap());
    }

    #[test]
    fn input_rejects_eof_and_preserves_blank_line() {
        let output = Output::new(false, false);
        let mut eof = Cursor::new(b"".as_slice());
        let error = output.input_from("Value: ", &mut eof).unwrap_err();
        assert!(error.to_string().contains("standard input closed"));

        let mut blank = Cursor::new(b"\n".as_slice());
        assert_eq!(output.input_from("Value: ", &mut blank).unwrap(), "");
    }

    #[test]
    fn choose_rejects_eof_and_preserves_blank_line_default() {
        let output = Output::new(false, false);
        let options = vec!["One".to_owned(), "Two".to_owned()];
        let mut eof = Cursor::new(b"".as_slice());
        let error = output
            .choose_from("Choose:", &options, &mut eof)
            .unwrap_err();
        assert!(error.to_string().contains("standard input closed"));

        let mut blank = Cursor::new(b"\n".as_slice());
        assert_eq!(
            output.choose_from("Choose:", &options, &mut blank).unwrap(),
            0
        );
    }

    #[test]
    fn story_renderer_owns_action_grammar() {
        assert_eq!(
            rucksack_action_line("checking the active agent"),
            "→ rucksack is checking the active agent."
        );
        assert_eq!(
            rucksack_action_line("checking the active agent."),
            "→ rucksack is checking the active agent."
        );
    }

    #[test]
    fn user_turn_is_immediately_attached_to_its_instructions() {
        assert_eq!(
            user_action_lines(&[
                "unplug this mac while the lid is open".to_owned(),
                "return here".to_owned(),
            ]),
            vec![
                "your turn.",
                "",
                "→ unplug this mac while the lid is open.",
                "→ return here.",
            ]
        );
    }

    #[test]
    fn waiting_copy_names_condition_and_automatic_continuation() {
        assert_eq!(
            waiting_lines("battery power", "packing will continue automatically"),
            [
                "→ rucksack is waiting for battery power.",
                "↳ packing will continue automatically.",
            ]
        );
    }

    #[test]
    fn facts_and_warnings_have_scannable_markers() {
        assert_eq!(
            fact_line("internet is reachable"),
            "✓ internet is reachable."
        );
        assert_eq!(
            warning_line("internet is unavailable"),
            "! internet is unavailable."
        );
    }

    #[test]
    fn story_json_records_keep_actor_and_emoji_out_of_messages() {
        let records = story_progress_records(&HumanEvent::Chapter("packing up".to_owned()));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["actor"], "rucksack");
        assert_eq!(records[0]["kind"], "chapter");
        assert_eq!(records[0]["message"], "packing up");
        assert!(!records[0].to_string().contains('🎒'));
    }

    #[test]
    fn waiting_record_names_the_automatic_continuation() {
        let records = story_progress_records(&HumanEvent::Waiting {
            condition: "battery power".to_owned(),
            continuation: "packing will continue automatically".to_owned(),
        });
        assert_eq!(records[0]["actor"], "rucksack");
        assert_eq!(records[0]["kind"], "waiting");
        assert_eq!(
            records[0]["continuation"],
            "packing will continue automatically"
        );
    }

    #[test]
    fn user_actions_are_distinct_machine_events() {
        let records = story_progress_records(&HumanEvent::UserAction(vec![
            "unplug this mac while the lid is open".to_owned(),
            "return here".to_owned(),
        ]));
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|record| record["actor"] == "user"));
        assert!(records.iter().all(|record| record["kind"] == "action"));
    }
}
