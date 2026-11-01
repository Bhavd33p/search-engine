use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "search-engine",
    version,
    about = "Full-text search engine with a compressed positional inverted index"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Crawl a website politely and write a JSONL corpus.
    Crawl,
    /// Build an inverted index from a corpus.
    Index,
    /// Run a query against an index.
    Search,
    /// Serve the index over gRPC.
    Serve,
    /// Benchmark index build, index size and query latency.
    Bench,
    /// Measure retrieval quality on the Cranfield collection.
    Eval,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Crawl | Command::Index | Command::Search | Command::Serve | Command::Bench | Command::Eval => {
            bail!("not implemented yet")
        }
    }
}
