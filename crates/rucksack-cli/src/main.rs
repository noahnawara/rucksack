mod app;
mod cli;
mod daemon;
mod flow;
mod helper_client;
mod install;
mod output;
mod star;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    if let Err(error) = app::run(cli) {
        eprint!("{}", output::render_error(&error));
        std::process::exit(1);
    }
}
