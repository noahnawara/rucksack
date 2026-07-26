use crate::output::Output;
use anyhow::{Context, Result};
use rucksack_core::files::atomic_write;
use rucksack_core::system::{run, which};
use rucksack_core::AppPaths;

const REPOSITORY: &str = "noahnawara/rucksack";
const REPOSITORY_URL: &str = "https://github.com/noahnawara/rucksack";

/// Star the repository on GitHub.
///
/// Uses the `gh` CLI the user already signed in to, so rucksack never sees a token and never asks
/// for one. Without `gh`, it opens the page and lets them click the button themselves.
pub fn star(output: &Output) -> Result<()> {
    let Some(gh) = which("gh") else {
        return open_in_browser(output);
    };
    let endpoint = format!("/user/starred/{REPOSITORY}");
    let result = run(gh, &["api", "--method", "PUT", &endpoint]).context("Could not run `gh`.")?;
    if result.success() {
        output.done("Starred. Thank you.");
        return Ok(());
    }
    output.detail(result.combined_trimmed());
    open_in_browser(output)
}

fn open_in_browser(output: &Output) -> Result<()> {
    let result = run("/usr/bin/open", &[REPOSITORY_URL]).context("Could not open a browser.")?;
    if !result.success() {
        output.done(format!("Star it here: {REPOSITORY_URL}"));
        return Ok(());
    }
    output.done("Opened GitHub — hit the star button.");
    Ok(())
}

/// Say once, ever, that starring is a thing — and leave the asking to whoever has a conversation.
///
/// rucksack cannot ask. Nobody is watching its output: an agent ran it, captured the text, and will
/// summarise it. An earlier version asked on the terminal and gated the question on a TTY, which
/// meant it never once fired for a real user, because agents have no TTY and agents are how this
/// gets run. Worse, ungating it would have printed `[Y/n]` into a pipe, read end-of-file, taken that
/// for "no", and recorded a permanent refusal without a human ever seeing the question.
///
/// So the division of labour is: rucksack states the fact, exactly once, at the end of the first
/// completed trip — the user is back at a desk and has just watched it work. The skill turns it into
/// a real question in the agent's own dialog, where there are buttons rather than a line of terminal
/// output to skim past. `rucksack star` does the work when they say yes.
///
/// Recorded before it is printed, so a conversation that dies mid-ask costs one missed mention
/// rather than asking forever.
pub fn mention_once(paths: &AppPaths, output: &Output) {
    let marker = paths.star_prompt_marker();
    if marker.exists() || atomic_write(&marker, b"mentioned\n", 0o600).is_err() {
        return;
    }
    output.step("First trip done. If rucksack earned it: `rucksack star`.");
}
