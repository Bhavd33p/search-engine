### Rust docs crawl

300 documents, 5128 distinct terms, 42696 postings, 152294 positions. Corpus file 1542301 bytes (1.47 MiB); analysed text 1487492 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.051 | 0.009 | 0.060 | 4997 |
| FixedU32 | 0.052 | 0.009 | 0.061 | 4932 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 315504 | 403000 | 1551783 | 4383 | 2068688 |
| FixedU32 | 971256 | 1062977 | 1551783 | 4387 | 2728688 |

VByte postings are 32.5% of the fixed-width u32 baseline (3.08× smaller) and 21.2% of the analysed text.

**Query latency** (39 queries, k = 10, BM25, query-language mode; warm rows are 20 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 39 | 518.5 | 759.8 | 760.0 | 532.1 |
| Warm, hot-term cache off | 780 | 135.4 | 372.1 | 404.8 | 157.1 |
| Warm, hot-term cache on | 780 | 124.5 | 369.4 | 410.6 | 151.1 |
| Warm, Redis result-cache hit | 780 | 75.5 | 103.3 | 112.5 | 81.4 |
| Baseline: linear scan (no index) | 39 | 47542.4 | 50338.5 | 52508.0 | 47829.4 |

Engine open in the cold child process (RocksDB open + metadata load): p50 937.9 µs, p99 1043.7 µs — not included in the cold row.
Hot-term cache after the warm run: 61 terms resident (~159660 bytes), 1682 hits / 82 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 39/39 queries.

