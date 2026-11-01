### Rust docs crawl

300 documents, 5128 distinct terms, 42696 postings, 152294 positions. Corpus file 1542301 bytes (1.47 MiB); analysed text 1487492 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.051 | 0.009 | 0.061 | 4958 |
| FixedU32 | 0.052 | 0.009 | 0.061 | 4929 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 315504 | 403000 | 1551783 | 4383 | 2068688 |
| FixedU32 | 971256 | 1062977 | 1551783 | 4387 | 2728688 |

VByte postings are 32.5% of the fixed-width u32 baseline (3.08× smaller) and 21.2% of the analysed text.

**Query latency** (39 queries, k = 10, BM25, query-language mode; warm rows are 20 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 39 | 572.9 | 887.3 | 930.9 | 591.9 |
| Warm, hot-term cache off | 780 | 134.5 | 372.5 | 401.8 | 157.8 |
| Warm, hot-term cache on | 780 | 124.9 | 368.4 | 395.8 | 150.3 |
| Warm, Redis result-cache hit | 780 | 74.2 | 111.8 | 121.5 | 82.9 |
| Baseline: linear scan (no index) | 39 | 47709.2 | 50708.5 | 53642.4 | 48135.0 |

Engine open in the cold child process (RocksDB open + metadata load): p50 1036.4 µs, p99 1356.5 µs — not included in the cold row.
Hot-term cache after the warm run: 61 terms resident (~159660 bytes), 1682 hits / 82 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 39/39 queries.

