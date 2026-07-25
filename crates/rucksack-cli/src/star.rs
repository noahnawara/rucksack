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

/// Ask once, ever, whether to star the repository.
///
/// Deliberately at the end of the first `unpack`: the user is back at a desk and has just watched
/// rucksack work, so it costs them nothing. Asking during `pack` would be asking someone a favour
/// while they are halfway out of the door, which is exactly what rucksack is supposed to stop
/// doing. Never asks when there is no terminal to answer from, so agents and scripts are unaffected.
pub fn offer_once(paths: &AppPaths, output: &Output) {
    let marker = paths.star_prompt_marker();
    if marker.exists() || !output.is_interactive() {
        return;
    }
    // Record the ask before making it, so a refusal is never asked twice.
    if atomic_write(&marker, b"asked\n", 0o600).is_err() {
        return;
    }
    match output.ask("That worked. Star rucksack on GitHub?") {
        Ok(true) => {
            if let Err(error) = star(output) {
                output.detail(format!("{error:#}"));
            }
        }
        Ok(false) => output.step("No problem. `rucksack star` if you change your mind."),
        Err(_) => {}
    }
}
