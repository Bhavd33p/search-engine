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
| Redis | redis-server 8.8.0 on `127.0.0.1:6390` (loopback), persistence off |
| Code | commit `3452acf` (the binary that produced the results) |
| Date | 2026-11-01, runs started 15:22:41 UTC and 15:22:54 UTC |

`scripts/machine_info.sh` prints the machine rows. The machine was a normal
laptop session, not an isolated benchmark host.

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
redis-server --port 6390 --save '' --appendonly no --daemonize yes

./target/release/search-engine bench --name "Rust docs crawl" \
  --input data/rust-docs.jsonl \
  --queries bench/queries/rust-docs.txt \
  --work-dir data/bench/rust-docs \
  --redis-url redis://127.0.0.1:6390 --rounds 20 --build-runs 3 \
  --out bench/results/rust-docs.json

./target/release/search-engine bench --name "Cranfield" \
  --input data/cranfield/cran.all.1400 --format cranfield \
  --queries data/cranfield/cran.qry --query-format cranfield --any \
  --work-dir data/bench/cranfield \
  --redis-url redis://127.0.0.1:6390 --rounds 5 --build-runs 3 \
  --out bench/results/cranfield.json

./target/release/search-engine index --input data/cranfield/cran.all.1400 --format cranfield --index data/cranfield-index
./target/release/search-engine eval --index data/cranfield-index \
  --queries data/cranfield/cran.qry --qrels data/cranfield/cranqrel

redis-cli -p 6390 shutdown nosave
```

(`make bench` and `make eval` run the same thing against `REDIS_URL`.)

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

### Rust docs crawl

300 documents, 5128 distinct terms, 42696 postings, 152294 positions. Corpus file 1542301 bytes (1.47 MiB); analysed text 1487492 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.052 | 0.009 | 0.061 | 4946 |
| FixedU32 | 0.052 | 0.009 | 0.061 | 4935 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 315504 | 403000 | 1551783 | 4383 | 2068688 |
| FixedU32 | 971256 | 1062977 | 1551783 | 4387 | 2728688 |

VByte postings are 32.5% of the fixed-width u32 baseline (3.08× smaller) and 21.2% of the analysed text.

**Query latency** (39 queries, k = 10, BM25, query-language mode; warm rows are 20 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 39 | 421.2 | 708.9 | 710.4 | 443.8 |
| Warm, hot-term cache off | 780 | 150.1 | 409.6 | 450.9 | 180.6 |
| Warm, hot-term cache on | 780 | 129.1 | 378.7 | 404.9 | 153.7 |
| Warm, Redis result-cache hit | 780 | 57.0 | 71.3 | 79.1 | 51.3 |
| Baseline: linear scan (no index) | 39 | 48088.1 | 49148.7 | 49341.2 | 48197.7 |

Engine open in the cold child process (RocksDB open + metadata load): p50 928.6 µs, p99 1135.3 µs — not included in the cold row.
Hot-term cache after the warm run: 61 terms resident (~159660 bytes), 1682 hits / 82 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 39/39 queries.

### Cranfield

1400 documents, 5029 distinct terms, 86755 postings, 143789 positions. Corpus file 1644706 bytes (1.57 MiB); analysed text 1545666 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.045 | 0.010 | 0.055 | 25506 |
| FixedU32 | 0.044 | 0.011 | 0.055 | 25460 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 405063 | 488260 | 1635800 | 14485 | 2248068 |
| FixedU32 | 1289312 | 1377616 | 1635800 | 14488 | 3137446 |

VByte postings are 31.4% of the fixed-width u32 baseline (3.18× smaller) and 26.2% of the analysed text.

**Query latency** (225 queries, k = 10, BM25, free-text OR mode; warm rows are 5 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 225 | 428.2 | 504.3 | 547.0 | 432.8 |
| Warm, hot-term cache off | 1125 | 146.9 | 206.5 | 232.7 | 149.8 |
| Warm, hot-term cache on | 1125 | 109.9 | 140.2 | 154.2 | 111.1 |
| Warm, Redis result-cache hit | 1125 | 37.2 | 46.2 | 52.8 | 37.5 |
| Baseline: linear scan (no index) | 225 | 40467.6 | 41634.6 | 44080.4 | 40643.3 |

Engine open in the cold child process (RocksDB open + metadata load): p50 933.9 µs, p99 1172.6 µs — not included in the cold row.
Hot-term cache after the warm run: 684 terms resident (~2294920 bytes), 12798 hits / 840 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 225/225 queries.

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

* **Compression.** VByte with delta gaps makes posting lists 3.08× (Rust docs)
  and 3.18× (Cranfield) smaller than fixed-width `u32`s. The postings column
  family is 26.1% (Rust docs) and 29.7% (Cranfield) of the raw corpus file size.
* **The whole index is larger than the corpus** — 1.34× (Rust docs) and 1.37×
  (Cranfield) — because the `docs` column family stores every document's full
  text for snippets (75.0% and 72.8% of the VByte index directory). The inverted
  index itself is the small part.
* **Build time does not depend on the codec** here: 82–85% of it is analysing
  and inverting the text in memory (tokenising and stemming every word), and the
  RocksDB write, including encoding, takes 0.009–0.011 s either way. The Rust docs build is slower per document than Cranfield
  because its pages are longer (152,294 vs 143,789 positions in 300 vs 1,400 docs).
* **Cold vs warm.** At p50 a cold query takes 3.26× (Rust docs) and 3.90×
  (Cranfield) as long as a warm one with the hot-term cache on. Keeping decoded
  posting lists resident cuts warm p50 by 14.0% (Rust docs) and 25.2%
  (Cranfield) compared with re-reading and re-decoding them; Cranfield gains more
  because its long free-text queries touch more terms.
* **Redis.** A result-cache hit costs 57.0 µs (Rust docs) and 37.2 µs (Cranfield)
  at p50 on loopback — one round trip plus JSON decoding — independent of the
  query.
* **Index vs no index.** At p50 the warm indexed path (hot-term cache on) is
  372.4× (Rust docs) and 368.3× (Cranfield) faster than the linear scan, and
  even a cold query is 114.2× and 94.5× faster, while returning identical top-10
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
* One run on one laptop. Expect a few percent of run-to-run variation, more at
  p99, where 39 cold samples make p95/p99 the 2nd-highest and highest sample.
