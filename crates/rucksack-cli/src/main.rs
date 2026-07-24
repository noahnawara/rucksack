mod app;
mod cli;
mod daemon;
mod doctor;
mod flow;
mod helper_client;
mod hooks;
mod install;
mod onboarding;
mod output;
mod report;

use clap::Parser;

fn main() {
    let json = std::env::args_os().any(|argument| argument == "--json");
    let cli = match cli::Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) if json && error.use_stderr() => {
            let exit_code = error.exit_code();
            let error = anyhow::Error::msg(error.to_string().trim_end().to_owned());
            if let Err(serialization_error) = output::emit_json_error(&error) {
                eprintln!("× Could not serialize the error: {serialization_error:#}");
            }
            std::process::exit(exit_code);
        }
        Err(error) => error.exit(),
    };

    if let Err(error) = app::run(cli) {
        if json {
            if !output::is_json_failure_emitted(&error) {
                if let Err(serialization_error) = output::emit_json_error(&error) {
                    eprintln!("× Could not serialize the error: {serialization_error:#}");
                }
            }
        } else {
            eprintln!();
            eprintln!("rucksack stopped.");
            eprintln!();
            eprintln!("{error:#}");
        }
        std::process::exit(1);
    }
}
