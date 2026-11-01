use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use search_engine::analysis::Analyzer;
use search_engine::cache::result_cache::{ResultCache, ResultCacheConfig};
use search_engine::corpus;
use search_engine::crawler::{self, CrawlConfig};
use search_engine::engine::{Engine, EngineConfig, QueryMode};
use search_engine::grpc;
use search_engine::index::builder::InMemoryIndex;
use search_engine::index::postings::Codec;
use search_engine::index::store;
use search_engine::rank::{Bm25, RankerKind};
use search_engine::service::SearchService;
use tracing::info;
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
    Crawl(CrawlArgs),
    /// Build an inverted index from a corpus.
    Index(IndexArgs),
    /// Run a query against an index.
    Search(SearchArgs),
    /// Serve the index over gRPC.
    Serve(ServeArgs),
    /// Benchmark index build, index size and query latency.
    Bench,
    /// Measure retrieval quality on the Cranfield collection.
    Eval,
}

#[derive(Args)]
struct CrawlArgs {
    /// Start URL(s). The crawl stays on their host, under their directory.
    #[arg(long = "seed", required = true)]
    seeds: Vec<url::Url>,
    /// Output corpus (JSONL).
    #[arg(long)]
    out: PathBuf,
    /// Stop after saving this many pages.
    #[arg(long, default_value_t = 300)]
    max_pages: usize,
    /// Minimum milliseconds between requests to the same host (raised to the
    /// site's Crawl-delay if that is longer).
    #[arg(long, default_value_t = 1000)]
    delay_ms: u64,
    /// Fetches in flight at once (still throttled per host).
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
    /// Skip URLs containing this substring (repeatable).
    #[arg(long = "exclude")]
    excludes: Vec<String>,
    #[arg(long, default_value = crawler::DEFAULT_USER_AGENT)]
    user_agent: String,
    /// Pages larger than this are skipped.
    #[arg(long, default_value_t = 5 * 1024 * 1024)]
    max_body_bytes: usize,
    #[arg(long, default_value_t = 30)]
    timeout_secs: u64,
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

#[derive(Args)]
struct SearchArgs {
    /// Index directory.
    #[arg(long)]
    index: PathBuf,
    /// Query text, e.g. `rust -unsafe "memory safety"`.
    #[arg(required = true, num_args = 1..)]
    query: Vec<String>,
    /// Number of results.
    #[arg(short, long, default_value_t = 10)]
    k: usize,
    #[arg(long, value_enum, default_value_t = RankerKind::Bm25)]
    ranker: RankerKind,
    /// Treat the query as free text (OR of all words) instead of the query language.
    #[arg(long)]
    any: bool,
    #[command(flatten)]
    engine: EngineArgs,
    #[command(flatten)]
    cache: CacheArgs,
    /// Print results as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ServeArgs {
    /// Index directory.
    #[arg(long)]
    index: PathBuf,
    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:50051")]
    addr: SocketAddr,
    #[command(flatten)]
    engine: EngineArgs,
    #[command(flatten)]
    cache: CacheArgs,
}

#[derive(Args, Clone, Copy)]
struct EngineArgs {
    /// BM25 term-frequency saturation.
    #[arg(long, default_value_t = 1.2)]
    bm25_k1: f64,
    /// BM25 length normalisation (0 = none, 1 = full).
    #[arg(long, default_value_t = 0.75)]
    bm25_b: f64,
    /// Posting lists kept decoded in memory (LRU, counted in terms); 0 disables.
    #[arg(long, default_value_t = 10_000)]
    hot_terms: usize,
}

impl EngineArgs {
    fn engine_config(self) -> EngineConfig {
        EngineConfig {
            bm25: Bm25 {
                k1: self.bm25_k1,
                b: self.bm25_b,
            },
            hot_term_capacity: self.hot_terms,
        }
    }
}

#[derive(Args, Clone)]
struct CacheArgs {
    /// Redis URL for the query-result cache, e.g. redis://127.0.0.1:6379. Omit to disable.
    #[arg(long, env = "REDIS_URL")]
    redis_url: Option<String>,
    /// Seconds a cached result lives.
    #[arg(long, default_value_t = 300)]
    cache_ttl_secs: u64,
    /// Prefix for cache keys.
    #[arg(long, default_value = "se")]
    cache_namespace: String,
}

impl CacheArgs {
    async fn connect(&self) -> Option<ResultCache> {
        let url = self.redis_url.clone()?;
        ResultCache::connect(ResultCacheConfig {
            url,
            ttl: Duration::from_secs(self.cache_ttl_secs),
            namespace: self.cache_namespace.clone(),
        })
        .await
    }
}

fn query_mode(any: bool) -> QueryMode {
    if any { QueryMode::AnyTerms } else { QueryMode::Boolean }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Index(args) => run_index(args),
        Command::Search(args) => run_search(args).await,
        Command::Serve(args) => run_serve(args).await,
        Command::Crawl(args) => run_crawl(args).await,
        Command::Bench | Command::Eval => {
            bail!("not implemented yet")
        }
    }
}

async fn run_crawl(args: CrawlArgs) -> Result<()> {
    let out = args.out.clone();
    let config = CrawlConfig {
        seeds: args.seeds,
        max_pages: args.max_pages,
        delay: Duration::from_millis(args.delay_ms),
        concurrency: args.concurrency,
        user_agent: args.user_agent,
        excludes: args.excludes,
        max_body_bytes: args.max_body_bytes,
        request_timeout: Duration::from_secs(args.timeout_secs),
    };
    let started = Instant::now();
    let report = crawler::crawl(config, &out).await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!(
        "wrote {} pages to {} in {:.1}s",
        report.pages_written,
        out.display(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
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

async fn run_search(args: SearchArgs) -> Result<()> {
    let engine = Engine::open(&args.index, args.engine.engine_config())
        .with_context(|| format!("opening index {}", args.index.display()))?;
    let service = SearchService::new(Arc::new(engine), args.cache.connect().await);
    let text = args.query.join(" ");
    let response = service.search(&text, query_mode(args.any), args.k, args.ranker).await?;
    let results = &response.results;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }
    let noun = if results.total_matches == 1 { "match" } else { "matches" };
    let source = if response.cache_hit { "redis cache" } else { "index" };
    println!(
        "{} {noun} in {:.2} ms ({}, from {source})",
        results.total_matches,
        response.took.as_secs_f64() * 1000.0,
        args.ranker
    );
    for (rank, hit) in results.hits.iter().enumerate() {
        println!("\n{:>2}. [{:.4}] {}", rank + 1, hit.score, hit.title);
        if !hit.url.is_empty() {
            println!("    {}", hit.url);
        }
        println!("    {}", hit.snippet);
    }
    Ok(())
}

async fn run_serve(args: ServeArgs) -> Result<()> {
    let engine = Engine::open(&args.index, args.engine.engine_config())
        .with_context(|| format!("opening index {}", args.index.display()))?;
    let stats = engine.stats().clone();
    let service = Arc::new(SearchService::new(Arc::new(engine), args.cache.connect().await));
    let listener = tokio::net::TcpListener::bind(args.addr)
        .await
        .with_context(|| format!("binding {}", args.addr))?;
    info!(
        addr = %listener.local_addr()?,
        docs = stats.num_docs,
        terms = stats.num_terms,
        index_version = %stats.index_version,
        "gRPC server listening"
    );
    grpc::serve(listener, service, grpc::shutdown_signal()).await?;
    info!("shut down cleanly");
    Ok(())
}
