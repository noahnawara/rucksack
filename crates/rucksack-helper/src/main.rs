mod lease;
mod power_events;
mod server;

fn main() {
    if let Err(error) = server::run() {
        eprintln!("rucksack-helper: {error:#}");
        std::process::exit(1);
    }
}
