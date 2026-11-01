//! Benchmarks: index build time, index size and query latency, each against a baseline.
//!
//! * build: the VByte index vs the same index with fixed-width u32 postings;
//! * size: VByte postings vs fixed-width postings vs the raw corpus;
//! * latency, all through [`SearchService`] (parse → analyse → search → snippets):
//!   - cold: a fresh process per query — empty hot-term cache, a newly opened
//!     RocksDB (empty block cache) and, with Redis, an empty result-cache namespace.
//!     The OS page cache cannot be dropped without root, so index files may
//!     still be in memory;
//!   - warm, hot-term cache off: same process, every query seen before, LRU disabled;
//!   - warm, hot-term cache on: decoded posting lists resident in memory;
//!   - warm, Redis hit: whole results served from Redis;
//!   - baseline: linear scan over the in-memory corpus (BM25, ids only — no
//!     snippets or stored-field reads, which favours the baseline).
//!
//! Percentiles use the nearest-rank method on the raw samples.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::analysis::Analyzer;
use crate::baseline::LinearScan;
use crate::cache::hot_terms::HotTermStats;
use crate::cache::result_cache::{ResultCache, ResultCacheConfig};
use crate::corpus::Document;
use crate::engine::{Engine, EngineConfig, QueryMode};
use crate::index::builder::InMemoryIndex;
use crate::index::postings::Codec;
use crate::index::store::{IndexReader, write_index};
use crate::rank::RankerKind;
use crate::service::SearchService;

pub struct BenchConfig {
    pub corpus_name: String,
    pub corpus_path: PathBuf,
    pub documents: Vec<Document>,
    pub queries: Vec<String>,
    pub mode: QueryMode,
    pub k: usize,
    pub rounds: usize,
    pub build_runs: usize,
    pub work_dir: PathBuf,
    pub redis_url: Option<String>,
    /// The binary to spawn for cold-query measurements (normally this one).
    pub executable: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatencySummary {
    pub samples: usize,
    pub p50_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub mean_us: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildMeasurement {
    pub codec: Codec,
    pub runs: usize,
    /// Median over runs of analyse + invert time (in memory).
    pub median_analyse_secs: f64,
    /// Median over runs of RocksDB write + flush + compaction time.
    pub median_write_secs: f64,
    pub median_total_secs: f64,
    pub docs_per_sec: f64,
    pub postings_bytes: u64,
    pub fixed_u32_postings_bytes: u64,
    pub stored_docs_bytes: u64,
    pub sst_bytes_by_column_family: Vec<(String, u64)>,
    pub index_dir_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Latencies {
    pub cold: LatencySummary,
    pub cold_engine_open: LatencySummary,
    pub warm_hot_terms_off: LatencySummary,
    pub warm_hot_terms_on: LatencySummary,
    pub warm_redis_hit: Option<LatencySummary>,
    pub baseline_linear_scan: LatencySummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub corpus: String,
    pub num_docs: u32,
    pub corpus_file_bytes: u64,
    /// Title + body bytes of all documents (what gets analysed).
    pub text_bytes: u64,
    pub num_terms: u64,
    pub num_postings: u64,
    pub num_positions: u64,
    pub builds: Vec<BuildMeasurement>,
    pub queries: usize,
    pub rounds: usize,
    pub k: usize,
    pub any_terms: bool,
    pub latency: Latencies,
    pub hot_terms_after_warm: HotTermStats,
    pub redis_hit_ratio: Option<f64>,
    /// Queries whose top-k doc ids were identical in the index and the baseline.
    pub baseline_agreement: (usize, usize),
}

/// What one cold-query child process prints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColdSample {
    pub open_us: f64,
    pub search_us: f64,
    pub cache_hit: bool,
}

pub async fn run(config: BenchConfig) -> Result<BenchReport> {
    ensure!(!config.queries.is_empty(), "no benchmark queries");
    if config.work_dir.exists() {
        bail!(
            "work dir {} already exists; remove it or pick another",
            config.work_dir.display()
        );
    }
    std::fs::create_dir_all(&config.work_dir)?;
    let analyzer = Analyzer::new();

    let mut builds = Vec::new();
    let mut vbyte_dir = PathBuf::new();
    let mut counts = (0, 0, 0);
    for codec in [Codec::VByte, Codec::FixedU32] {
        let (measurement, dir, terms, postings, positions) = measure_build(&config, &analyzer, codec)?;
        if codec == Codec::VByte {
            vbyte_dir = dir;
            counts = (terms, postings, positions);
        }
        builds.push(measurement);
    }

    let mode_flag = config.mode == QueryMode::AnyTerms;
    let cold_namespace = unique_namespace("bench-cold");
    let mut cold = Vec::new();
    let mut cold_open = Vec::new();
    for query in &config.queries {
        let sample = cold_query_in_child(&config, &vbyte_dir, query, &cold_namespace)?;
        cold.push(sample.search_us);
        cold_open.push(sample.open_us);
    }

    let warm_hot_terms_off = {
        let engine = Engine::open(
            &vbyte_dir,
            EngineConfig {
                hot_term_capacity: 0,
                ..EngineConfig::default()
            },
        )?;
        let service = SearchService::new(Arc::new(engine), None);
        measure_warm(&service, &config).await?.0
    };
    let (warm_hot_terms_on, hot_terms_after_warm) = {
        let engine = Engine::open(
            &vbyte_dir,
            EngineConfig {
                hot_term_capacity: 1_000_000,
                ..EngineConfig::default()
            },
        )?;
        let service = SearchService::new(Arc::new(engine), None);
        let (latency, _) = measure_warm(&service, &config).await?;
        (latency, service.engine().hot_term_stats())
    };
    let (warm_redis_hit, redis_hit_ratio) = match &config.redis_url {
        Some(url) => {
            let cache = ResultCache::connect(ResultCacheConfig {
                url: url.clone(),
                ttl: Duration::from_secs(3600),
                namespace: unique_namespace("bench-warm"),
            })
            .await
            .context("Redis URL given but Redis is unreachable")?;
            let engine = Engine::open(&vbyte_dir, EngineConfig::default())?;
            let service = SearchService::new(Arc::new(engine), Some(cache));
            let (latency, hit_ratio) = measure_warm(&service, &config).await?;
            (Some(latency), Some(hit_ratio))
        }
        None => (None, None),
    };

    let engine = Engine::open(&vbyte_dir, EngineConfig::default())?;
    let scan = LinearScan::new(&analyzer, &config.documents, Default::default());
    let mut baseline = Vec::new();
    let mut agreed = 0;
    for text in &config.queries {
        let query = engine.analyze(text, config.mode)?;
        let started = Instant::now();
        let scanned = scan.search(&query, config.k);
        baseline.push(micros(started.elapsed()));
        let indexed = engine.rank(&query, config.k, RankerKind::Bm25)?.docs;
        if indexed.iter().map(|d| d.doc_id).eq(scanned.iter().map(|d| d.doc_id)) {
            agreed += 1;
        }
    }

    Ok(BenchReport {
        corpus: config.corpus_name.clone(),
        num_docs: config.documents.len() as u32,
        corpus_file_bytes: std::fs::metadata(&config.corpus_path)?.len(),
        text_bytes: config
            .documents
            .iter()
            .map(|d| (d.title.len() + d.body.len()) as u64)
            .sum(),
        num_terms: counts.0,
        num_postings: counts.1,
        num_positions: counts.2,
        builds,
        queries: config.queries.len(),
        rounds: config.rounds,
        k: config.k,
        any_terms: mode_flag,
        latency: Latencies {
            cold: summarize(cold),
            cold_engine_open: summarize(cold_open),
            warm_hot_terms_off,
            warm_hot_terms_on,
            warm_redis_hit,
            baseline_linear_scan: summarize(baseline),
        },
        hot_terms_after_warm,
        redis_hit_ratio,
        baseline_agreement: (agreed, config.queries.len()),
    })
}

type BuildResult = (BuildMeasurement, PathBuf, u64, u64, u64);

fn measure_build(config: &BenchConfig, analyzer: &Analyzer, codec: Codec) -> Result<BuildResult> {
    let name = match codec {
        Codec::VByte => "vbyte",
        Codec::FixedU32 => "fixed32",
    };
    let (mut analyse, mut write, mut total) = (Vec::new(), Vec::new(), Vec::new());
    let mut last = None;
    for run in 0..config.build_runs.max(1) {
        let dir = config.work_dir.join(format!("{name}-run{run}"));
        let documents = config.documents.clone(); // cloned outside the timed region
        let started = Instant::now();
        let index = InMemoryIndex::build(analyzer, documents);
        let analysed = started.elapsed();
        let (_, report) = write_index(&dir, &index, codec)?;
        let finished = started.elapsed();
        analyse.push(analysed.as_secs_f64());
        write.push((finished - analysed).as_secs_f64());
        total.push(finished.as_secs_f64());
        if let Some((previous_dir, _)) = last.replace((dir, report)) {
            std::fs::remove_dir_all(previous_dir)?;
        }
    }
    let Some((dir, report)) = last else {
        bail!("no build runs");
    };
    let reader = IndexReader::open(&dir)?;
    let sst_bytes_by_column_family = reader
        .column_family_sizes()?
        .into_iter()
        .map(|(name, size)| (name.to_string(), size))
        .collect();
    drop(reader);
    let median_total_secs = median(&mut total);
    let measurement = BuildMeasurement {
        codec,
        runs: config.build_runs.max(1),
        median_analyse_secs: median(&mut analyse),
        median_write_secs: median(&mut write),
        median_total_secs,
        docs_per_sec: config.documents.len() as f64 / median_total_secs,
        postings_bytes: report.postings_bytes,
        fixed_u32_postings_bytes: report.fixed_u32_postings_bytes,
        stored_docs_bytes: report.docs_bytes,
        sst_bytes_by_column_family,
        index_dir_bytes: dir_size(&dir)?,
    };
    Ok((
        measurement,
        dir,
        report.num_terms,
        report.num_postings,
        report.num_positions,
    ))
}

fn cold_query_in_child(config: &BenchConfig, index: &Path, query: &str, namespace: &str) -> Result<ColdSample> {
    let mut command = Command::new(&config.executable);
    command
        .arg("bench-cold-query")
        .arg("--index")
        .arg(index)
        .arg("-k")
        .arg(config.k.to_string());
    if config.mode == QueryMode::AnyTerms {
        command.arg("--any");
    }
    if let Some(url) = &config.redis_url {
        command
            .arg("--redis-url")
            .arg(url)
            .arg("--cache-namespace")
            .arg(namespace);
    }
    command.arg("--").arg(query);
    let output = command.output().context("spawning cold-query child")?;
    ensure!(
        output.status.success(),
        "cold-query child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sample: ColdSample = serde_json::from_slice(&output.stdout).context("parsing cold-query child output")?;
    ensure!(!sample.cache_hit, "cold query unexpectedly hit the result cache");
    Ok(sample)
}

/// Body of the hidden `bench-cold-query` command: one query in a fresh process.
pub async fn cold_query(
    index: &Path,
    query: &str,
    mode: QueryMode,
    k: usize,
    cache: Option<ResultCacheConfig>,
) -> Result<ColdSample> {
    let started = Instant::now();
    let engine = Engine::open(index, EngineConfig::default())?;
    let open_us = micros(started.elapsed());
    let cache = match cache {
        Some(config) => Some(ResultCache::connect(config).await.context("Redis unreachable")?),
        None => None,
    };
    let service = SearchService::new(Arc::new(engine), cache);
    let started = Instant::now();
    let response = service.search(query, mode, k, RankerKind::Bm25).await?;
    Ok(ColdSample {
        open_us,
        search_us: micros(started.elapsed()),
        cache_hit: response.cache_hit,
    })
}

/// One untimed pass to warm caches, then `rounds` timed passes. Returns the
/// latency summary and the fraction of timed queries answered from Redis.
async fn measure_warm(service: &SearchService, config: &BenchConfig) -> Result<(LatencySummary, f64)> {
    for query in &config.queries {
        service.search(query, config.mode, config.k, RankerKind::Bm25).await?;
    }
    let mut samples = Vec::with_capacity(config.queries.len() * config.rounds);
    let mut hits = 0usize;
    for _ in 0..config.rounds {
        for query in &config.queries {
            let started = Instant::now();
            let response = service.search(query, config.mode, config.k, RankerKind::Bm25).await?;
            samples.push(micros(started.elapsed()));
            hits += usize::from(response.cache_hit);
        }
    }
    let hit_ratio = hits as f64 / samples.len().max(1) as f64;
    Ok((summarize(samples), hit_ratio))
}

pub fn summarize(mut samples: Vec<f64>) -> LatencySummary {
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    let nearest_rank = |p: f64| -> f64 {
        if n == 0 {
            return 0.0;
        }
        let rank = ((p / 100.0) * n as f64).ceil() as usize;
        samples[rank.clamp(1, n) - 1]
    };
    LatencySummary {
        samples: n,
        p50_us: nearest_rank(50.0),
        p95_us: nearest_rank(95.0),
        p99_us: nearest_rank(99.0),
        mean_us: if n == 0 {
            0.0
        } else {
            samples.iter().sum::<f64>() / n as f64
        },
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    match values.len() {
        0 => 0.0,
        n if n % 2 == 1 => values[n / 2],
        n => (values[n / 2 - 1] + values[n / 2]) / 2.0,
    }
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000.0
}

fn unique_namespace(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{prefix}-{nanos}")
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total += if metadata.is_dir() {
            dir_size(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(total)
}

/// Renders the report as the Markdown tables used in BENCHMARKS.md.
pub fn to_markdown(report: &BenchReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let mib = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
    let _ = writeln!(out, "### {}\n", report.corpus);
    let _ = writeln!(
        out,
        "{} documents, {} distinct terms, {} postings, {} positions. Corpus file {} bytes ({:.2} MiB); analysed text {} bytes.\n",
        report.num_docs,
        report.num_terms,
        report.num_postings,
        report.num_positions,
        report.corpus_file_bytes,
        mib(report.corpus_file_bytes),
        report.text_bytes
    );
    let _ = writeln!(
        out,
        "**Index build** (median of {} runs)\n",
        report.builds.first().map_or(0, |b| b.runs)
    );
    let _ = writeln!(
        out,
        "| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|");
    for build in &report.builds {
        let _ = writeln!(
            out,
            "| {:?} | {:.3} | {:.3} | {:.3} | {:.0} |",
            build.codec,
            build.median_analyse_secs,
            build.median_write_secs,
            build.median_total_secs,
            build.docs_per_sec
        );
    }
    let _ = writeln!(out, "\n**Index size**\n");
    let _ = writeln!(
        out,
        "| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|");
    for build in &report.builds {
        let cf = |name: &str| {
            build
                .sst_bytes_by_column_family
                .iter()
                .find(|(n, _)| n == name)
                .map_or(0, |(_, size)| *size)
        };
        let _ = writeln!(
            out,
            "| {:?} | {} | {} | {} | {} | {} |",
            build.codec,
            build.postings_bytes,
            cf("postings"),
            cf("docs"),
            cf("meta"),
            build.index_dir_bytes
        );
    }
    if let (Some(vbyte), Some(fixed)) = (
        report.builds.iter().find(|b| b.codec == Codec::VByte),
        report.builds.iter().find(|b| b.codec == Codec::FixedU32),
    ) {
        let _ = writeln!(
            out,
            "\nVByte postings are {:.1}% of the fixed-width u32 baseline ({:.2}× smaller) and {:.1}% of the analysed text.",
            100.0 * vbyte.postings_bytes as f64 / fixed.postings_bytes as f64,
            fixed.postings_bytes as f64 / vbyte.postings_bytes as f64,
            100.0 * vbyte.postings_bytes as f64 / report.text_bytes as f64
        );
    }
    let _ = writeln!(
        out,
        "\n**Query latency** ({} queries, k = {}, BM25, {} mode; warm rows are {} rounds over all queries)\n",
        report.queries,
        report.k,
        if report.any_terms {
            "free-text OR"
        } else {
            "query-language"
        },
        report.rounds
    );
    let _ = writeln!(out, "| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |");
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|");
    let latency = &report.latency;
    let mut rows = vec![
        ("Cold: fresh process, empty caches", &latency.cold),
        ("Warm, hot-term cache off", &latency.warm_hot_terms_off),
        ("Warm, hot-term cache on", &latency.warm_hot_terms_on),
    ];
    if let Some(redis) = &latency.warm_redis_hit {
        rows.push(("Warm, Redis result-cache hit", redis));
    }
    rows.push(("Baseline: linear scan (no index)", &latency.baseline_linear_scan));
    for (name, summary) in rows {
        let _ = writeln!(
            out,
            "| {name} | {} | {:.1} | {:.1} | {:.1} | {:.1} |",
            summary.samples, summary.p50_us, summary.p95_us, summary.p99_us, summary.mean_us
        );
    }
    let _ = writeln!(
        out,
        "\nEngine open in the cold child process (RocksDB open + metadata load): p50 {:.1} µs, p99 {:.1} µs — not included in the cold row.",
        latency.cold_engine_open.p50_us, latency.cold_engine_open.p99_us
    );
    let hot = &report.hot_terms_after_warm;
    let _ = writeln!(
        out,
        "Hot-term cache after the warm run: {} terms resident (~{} bytes), {} hits / {} misses.",
        hot.resident_terms, hot.resident_bytes, hot.hits, hot.misses
    );
    if let Some(ratio) = report.redis_hit_ratio {
        let _ = writeln!(
            out,
            "Redis hit ratio during the timed warm-Redis rounds: {:.1}%.",
            ratio * 100.0
        );
    }
    let _ = writeln!(
        out,
        "Baseline returned the same top-{} doc ids as the index for {}/{} queries.",
        report.k, report.baseline_agreement.0, report.baseline_agreement.1
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let summary = summarize(samples);
        assert_eq!((summary.p50_us, summary.p95_us, summary.p99_us), (50.0, 95.0, 99.0));
        assert_eq!(summary.mean_us, 50.5);

        let small = summarize(vec![3.0, 1.0, 2.0]);
        assert_eq!((small.p50_us, small.p95_us, small.p99_us), (2.0, 3.0, 3.0));
        assert_eq!(summarize(Vec::new()).samples, 0);
    }

    #[test]
    fn median_of_odd_and_even_counts() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), 2.5);
    }
}
