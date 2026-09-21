use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "destiny-mcp", version, about = "Destiny MCP server")]
struct Cli;

fn main() {
    let _ = Cli::parse();
    if let Err(error) = destiny_app::startup() {
        eprintln!("{}", error);
        std::process::exit(1);
    }
}
