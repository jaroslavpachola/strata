use clap::Parser;

/// strata: a structured, local data store in two layers.
#[derive(Parser)]
#[command(name = "strata", version)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
