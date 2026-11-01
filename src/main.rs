use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use search_engine::analysis::Analyzer;
use search_engine::corpus;
use search_engine::index::builder::InMemoryIndex;
use search_engine::index::postings::Codec;
use search_engine::index::store;
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
    Index(IndexArgs),
    /// Run a query against an index.
    Search,
    /// Serve the index over gRPC.
    Serve,
    /// Benchmark index build, index size and query latency.
    Bench,
    /// Measure retrieval quality on the Cranfield collection.
    Eval,
}

#[derive(Args)]
struct IndexArgs {
    /// Corpus to index (JSONL, one document per line).
    #[arg(long)]
    input: PathBuf,
    /// Directory to create the index in.
    #[arg(long)]
    index: PathBuf,
    /// Posting list encoding.
    #[arg(long, value_enum, default_value_t = Codec::VByte)]
    codec: Codec,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Index(args) => run_index(args),
        Command::Crawl | Command::Search | Command::Serve | Command::Bench | Command::Eval => {
            bail!("not implemented yet")
        }
    }
}

fn run_index(args: IndexArgs) -> Result<()> {
    let started = Instant::now();
    let documents =
        corpus::read_jsonl(&args.input).with_context(|| format!("reading corpus {}", args.input.display()))?;
    let read_secs = started.elapsed().as_secs_f64();

    let analyzer = Analyzer::new();
    let started = Instant::now();
    let index = InMemoryIndex::build(&analyzer, documents);
    let build_secs = started.elapsed().as_secs_f64();

    let started = Instant::now();
    let (stats, report) = store::write_index(&args.index, &index, args.codec)
        .with_context(|| format!("writing index to {}", args.index.display()))?;
    let write_secs = started.elapsed().as_secs_f64();

    println!("indexed {} documents into {}", stats.num_docs, args.index.display());
    println!("  terms              {}", stats.num_terms);
    println!("  postings           {}", report.num_postings);
    println!("  positions          {}", report.num_positions);
    println!("  avg doc length     {:.1} terms", stats.avg_doc_len());
    println!("  postings bytes     {} ({:?})", report.postings_bytes, args.codec);
    println!("  as fixed u32       {}", report.fixed_u32_postings_bytes);
    println!("  stored docs bytes  {}", report.docs_bytes);
    println!("  index version      {}", stats.index_version);
    println!("  read {read_secs:.3}s, analyse+invert {build_secs:.3}s, write {write_secs:.3}s");
    Ok(())
}
