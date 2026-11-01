# Benchmarks

Every number below was produced by the commands in this file and copied from the
raw outputs committed in [`bench/results/`](bench/results). Nothing is rounded up
or estimated. Re-running will give somewhat different latencies (see
[Caveats](#caveats)).

## Setup

| | |
|---|---|
| Machine | Apple M5 Pro, 18 cores, 24 GiB RAM (MacBook Pro) |
| OS | macOS 26.4 (arm64) |
| Rust | rustc 1.99.1 (58cf8f9c3 2026-10-13), `--release` profile |
| Redis | redis-server 8.8.0 started for the benchmark on `127.0.0.1:6417` (loopback), persistence off, otherwise default config |
| Code | commit `c9fa462` (the binary that produced the results) |
| Date | 2026-11-01, three back-to-back runs between 15:28:19 and 15:29:22 UTC; Cranfield evaluation at 15:23 UTC |

`scripts/machine_info.sh` prints the machine rows. The machine was a normal
laptop session with other work running (load average ≈ 2–5), not an isolated
benchmark host.

An earlier recording (commit `07fc048`) was discarded: its Redis server on port
6390 turned out to belong to another process rather than the one started for the
benchmark. Every number in this file comes from the runs below.

### Corpora

| Corpus | Source | Documents | File size |
|---|---|---:|---:|
| Rust docs crawl | 300 pages crawled by this project's crawler from *The Rust Programming Language* (114 pages) and *Rust by Example* (186 pages) on doc.rust-lang.org, `print.html` excluded | 300 | 1,542,301 bytes |
| Cranfield | `cran.all.1400` from the University of Glasgow IR test collections | 1,400 | 1,644,706 bytes |

The crawl took 302.0 s for 302 requests (one request per second, the politeness
delay), fetched and obeyed `robots.txt`, and hit 0 HTTP errors; 2 fetched pages
were skipped because their text was empty or an exact duplicate of a saved page.
Reproduce it with `scripts/crawl_rust_docs.sh`.

### Commands

```sh
cargo build --release
redis-server --port 6417 --save '' --appendonly no --daemonize yes

./target/release/search-engine bench --name "Rust docs crawl" \
  --input data/rust-docs.jsonl \
  --queries bench/queries/rust-docs.txt \
  --work-dir data/bench/rust-docs \
  --redis-url redis://127.0.0.1:6417 --rounds 20 --build-runs 3 \
  --out bench/results/rust-docs-run1.json      # run1, run2, run3

./target/release/search-engine bench --name "Cranfield" \
  --input data/cranfield/cran.all.1400 --format cranfield \
  --queries data/cranfield/cran.qry --query-format cranfield --any \
  --work-dir data/bench/cranfield \
  --redis-url redis://127.0.0.1:6417 --rounds 5 --build-runs 3 \
  --out bench/results/cranfield-run1.json      # run1, run2, run3

./target/release/search-engine index --input data/cranfield/cran.all.1400 --format cranfield --index data/cranfield-index
./target/release/search-engine eval --index data/cranfield-index \
  --queries data/cranfield/cran.qry --qrels data/cranfield/cranqrel

redis-cli -p 6417 shutdown nosave
```

Each `bench` invocation needs a fresh `--work-dir` (`rm -rf data/bench` between
runs). `make bench` and `make eval` run the same commands against `REDIS_URL`.

## What is measured

**Index build.** Each corpus is indexed three times per posting codec; the
median is reported. "Analyse + invert" is tokenising, stop-word removal,
stemming and building the in-memory posting lists. "Write" is encoding the
postings, writing all three RocksDB column families in batches, flushing and
compacting. Reading the corpus file is not included. Docs/sec = documents ÷
median total.

**Index size.** *Postings bytes* is the sum of the encoded posting-list values.
The baseline is the same postings written as fixed-width little-endian `u32`s
(`--codec fixed32`): a real second index, built and measured, not an estimate.
The column-family columns are RocksDB's `rocksdb.total-sst-files-size`; the last
column is every file in the index directory. RocksDB's own block compression is
off for every column family, so the sizes show the effect of our encoding only.

**Query latency.** Every row except the baseline goes through
`SearchService::search` — the same path the gRPC server uses: parse, analyse,
Redis lookup (when enabled), boolean/phrase evaluation, BM25 scoring, top-k,
stored-field reads and snippets. It is timed in-process, so gRPC/network time is
not included. k = 10, BM25 (k1 = 1.2, b = 0.75).

| Row | Meaning |
|---|---|
| Cold | A **fresh process per query** (`bench` spawns `search-engine bench-cold-query`): new RocksDB handle (empty block cache), empty hot-term cache, and a Redis key namespace that has never been written, so the query misses Redis and then stores its result. Process start and opening the index are excluded and reported separately. |
| Warm, hot-term cache off | One process; every query has run once before; the decoded-posting-list cache is disabled, so each term is read from RocksDB (block cache warm) and VByte-decoded again. |
| Warm, hot-term cache on | As above with the LRU of decoded posting lists enabled: posting lists of terms seen before are already in memory. |
| Warm, Redis hit | Result cache enabled and populated; every timed query is answered from Redis (the hit ratio is printed). |
| Baseline | No index: for each query, re-analyse every document of the in-memory corpus, test the boolean/phrase structure against it and score with BM25. It returns ids only (no stored fields, no snippets), which favours the baseline. It must return the same top-k as the index; agreement is printed. |

Percentiles use the nearest-rank method over all samples. Warm rows run one
untimed pass over the queries, then `--rounds` timed passes.

The Rust-docs queries ([`bench/queries/rust-docs.txt`](bench/queries/rust-docs.txt))
are 39 hand-written queries covering single terms, implicit AND, AND/OR/NOT,
parentheses and phrases. The Cranfield queries are the collection's 225
natural-language queries run as free text (OR of all words).

## Results

All tables below are run 1 of 3. [Run-to-run variation](#run-to-run-variation) lists the other two.

### Rust docs crawl

300 documents, 5128 distinct terms, 42696 postings, 152294 positions. Corpus file 1542301 bytes (1.47 MiB); analysed text 1487492 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.050 | 0.008 | 0.058 | 5185 |
| FixedU32 | 0.049 | 0.009 | 0.057 | 5246 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 315504 | 403000 | 1551783 | 4383 | 2068688 |
| FixedU32 | 971256 | 1062977 | 1551783 | 4387 | 2728687 |

VByte postings are 32.5% of the fixed-width u32 baseline (3.08× smaller) and 21.2% of the analysed text.

**Query latency** (39 queries, k = 10, BM25, query-language mode; warm rows are 20 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 39 | 521.5 | 737.7 | 770.3 | 525.7 |
| Warm, hot-term cache off | 780 | 134.1 | 373.4 | 415.5 | 158.0 |
| Warm, hot-term cache on | 780 | 124.5 | 361.8 | 387.1 | 148.2 |
| Warm, Redis result-cache hit | 780 | 72.7 | 110.2 | 116.9 | 81.9 |
| Baseline: linear scan (no index) | 39 | 46135.7 | 48255.0 | 48533.9 | 46302.8 |

Engine open in the cold child process (RocksDB open + metadata load): p50 916.9 µs, p99 1199.9 µs — not included in the cold row.
Hot-term cache after the warm run: 61 terms resident (~159660 bytes), 1682 hits / 82 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 39/39 queries.

### Cranfield

1400 documents, 5029 distinct terms, 86755 postings, 143789 positions. Corpus file 1644706 bytes (1.57 MiB); analysed text 1545666 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.045 | 0.009 | 0.054 | 25778 |
| FixedU32 | 0.043 | 0.009 | 0.052 | 26830 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 405063 | 488260 | 1635800 | 14485 | 2248068 |
| FixedU32 | 1289312 | 1377616 | 1635800 | 14488 | 3137446 |

VByte postings are 31.4% of the fixed-width u32 baseline (3.18× smaller) and 26.2% of the analysed text.

**Query latency** (225 queries, k = 10, BM25, free-text OR mode; warm rows are 5 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 225 | 513.3 | 593.7 | 628.9 | 514.9 |
| Warm, hot-term cache off | 1125 | 144.3 | 200.6 | 225.2 | 146.7 |
| Warm, hot-term cache on | 1125 | 111.0 | 145.2 | 159.6 | 112.6 |
| Warm, Redis result-cache hit | 1125 | 71.5 | 85.2 | 93.5 | 73.0 |
| Baseline: linear scan (no index) | 225 | 39614.8 | 41312.8 | 42024.0 | 39819.5 |

Engine open in the cold child process (RocksDB open + metadata load): p50 919.1 µs, p99 1248.4 µs — not included in the cold row.
Hot-term cache after the warm run: 684 terms resident (~2294920 bytes), 12798 hits / 840 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 225/225 queries.

### Run-to-run variation

The full benchmark was run three times back to back (raw outputs:
`bench/results/*-run{1,2,3}.{json,md}`). Build times are medians of 3 builds
within each run; latencies are p50 in µs.

| Corpus | Run | Build VByte (s) | Build fixed32 (s) | Cold | Warm, cache off | Warm, cache on | Redis hit | Linear scan | Baseline agreement |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Rust docs | 1 | 0.058 | 0.057 | 521.5 | 134.1 | 124.5 | 72.7 | 46135.7 | 39/39 |
| Rust docs | 2 | 0.059 | 0.061 | 499.7 | 132.8 | 124.1 | 71.7 | 45819.0 | 39/39 |
| Rust docs | 3 | 0.059 | 0.059 | 496.3 | 131.9 | 124.7 | 71.4 | 46393.8 | 39/39 |
| Cranfield | 1 | 0.054 | 0.052 | 513.3 | 144.3 | 111.0 | 71.5 | 39614.8 | 225/225 |
| Cranfield | 2 | 0.052 | 0.052 | 513.5 | 147.8 | 107.6 | 73.8 | 39450.7 | 225/225 |
| Cranfield | 3 | 0.053 | 0.053 | 516.4 | 146.4 | 109.1 | 69.9 | 39816.6 | 225/225 |

### Retrieval quality on Cranfield

All 225 queries, run as free text (OR of every analysed word), top 1000 retrieved
per query.

| Ranker | Queries | Recall@10 | Recall@100 | MAP | nDCG@10 |
|--------|--------:|----------:|-----------:|----:|--------:|
| bm25 | 225 | 0.4010 | 0.7510 | 0.3135 | 0.3729 |
| tfidf | 225 | 0.3971 | 0.7473 | 0.2946 | 0.3523 |

How the labels are used (`src/cranfield.rs`, `src/eval.rs`):

* a document is **relevant** to a query if `cranqrel` gives it grade 1–4; each
  query's single undocumented `-1` row is ignored;
* `cranqrel` numbers queries 1–225 by their position in `cran.qry` (whose own ids
  skip numbers), so queries are matched by position;
* **nDCG@10** uses graded gains: grade 1 ("complete answer") = 4 … grade 4
  ("minimum interest") = 1, with a log2(rank + 1) discount, normalised by the
  ideal ordering of all judged documents;
* **MAP** is mean average precision over the top 1000;
* documents are indexed as title + abstract. Cranfield abstracts begin by
  repeating the title, so title words count twice — a mild title boost that
  applies equally to both rankers;
* no parameter was tuned on these queries: BM25 uses the textbook k1 = 1.2,
  b = 0.75.

## Reading the numbers

All figures here are from run 1; the variation table shows the other runs agree.

* **Compression.** VByte with delta gaps makes posting lists 3.08× (Rust docs)
  and 3.18× (Cranfield) smaller than fixed-width `u32`s. The postings column
  family is 26.1% (Rust docs) and 29.7% (Cranfield) of the raw corpus file size.
* **The whole index is larger than the corpus** — 1.34× (Rust docs) and 1.37×
  (Cranfield) — because the `docs` column family stores every document's full
  text for snippets (75.0% and 72.8% of the VByte index directory). The inverted
  index itself is the small part.
* **Build time does not depend on the codec** here: 82–86% of it is analysing
  and inverting the text in memory (tokenising and stemming every word), and the
  RocksDB write, including encoding, takes about 0.008–0.009 s either way. The
  fixed32 build is within 0.0021 s of the VByte build in every run, sometimes
  faster and sometimes slower. The Rust docs build is slower per document than Cranfield because
  its pages are longer (152,294 vs 143,789 positions in 300 vs 1,400 documents).
* **Cold vs warm.** At p50 a cold query takes 4.19× (Rust docs) and 4.62×
  (Cranfield) as long as a warm one with the hot-term cache on.
* **Hot-term residency.** Keeping decoded posting lists in memory cuts warm p50
  by 7.1% (Rust docs) and 23.1% (Cranfield) compared with reading and decoding
  them from RocksDB's (already warm) block cache each time. Cranfield gains more
  because its long free-text queries touch many more terms per query; on the
  Rust docs queries most of the warm time goes to loading stored documents and
  building snippets, which this cache does not cover.
* **Redis.** A result-cache hit costs 72.7 µs (Rust docs) and 71.5 µs
  (Cranfield) at p50 on loopback — a GET round trip plus decoding the cached
  JSON — 41.6% and 35.6% less than the warm in-process path. On a corpus this
  small the engine is already fast, so the cache matters most for expensive
  queries and for sharing results between several engine processes.
* **Index vs no index.** At p50 the warm indexed path (hot-term cache on) is
  370.4× (Rust docs) and 356.8× (Cranfield) faster than the linear scan, and
  even a cold query is 88.5× and 77.2× faster, while returning identical top-10
  results.
* **BM25 vs TF-IDF.** BM25 is ahead on every metric, most clearly on MAP
  (0.3135 vs 0.2946) and nDCG@10 (0.3729 vs 0.3523); Recall@100 is close
  (0.7510 vs 0.7473).

## Caveats

* Both corpora are small (≈1.5 MB of text) and fit in memory several times
  over, so these numbers say nothing about behaviour when the index does not fit
  in RAM.
* **The OS page cache cannot be dropped without root.** "Cold" means a new
  process, a new RocksDB instance and empty application caches; the index files
  themselves were very likely still in the macOS page cache.
* Latencies are single-threaded, one query at a time, measured inside the
  process. They do not include gRPC serialisation or network time, and say
  nothing about throughput under concurrent load.
* Redis ran on the same machine; a networked Redis adds its round-trip time to
  both the "Redis hit" row and every cache miss.
* Three runs on one laptop. Between runs, p50s moved by at most 5.5% (Cranfield
  Redis hit) and 5.1% (Rust docs cold); every other row moved by 3.2% or less. With 39 cold samples, p95 and p99 are simply the 2nd-highest and highest
  sample.
