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
    if let Err(error) = destiny_app::startup() {
        eprintln!("{}", error);
        std::process::exit(1);
    }
}
