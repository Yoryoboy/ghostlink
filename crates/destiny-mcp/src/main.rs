use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "destiny-mcp", version, about = "Destiny MCP server")]
struct Cli;

fn main() {
    let _ = Cli::parse();
    let _ = destiny_app::foundation_ready();
}
