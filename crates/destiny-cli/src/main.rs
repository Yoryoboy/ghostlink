use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "destiny-cli",
    version,
    about = "Destiny inventory command line interface"
)]
struct Cli;

fn main() {
    let _ = Cli::parse();
    let _ = destiny_app::foundation_ready();
}
