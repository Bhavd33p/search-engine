### Rust docs crawl

300 documents, 5128 distinct terms, 42696 postings, 152294 positions. Corpus file 1542301 bytes (1.47 MiB); analysed text 1487492 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.052 | 0.009 | 0.062 | 4867 |
| FixedU32 | 0.052 | 0.010 | 0.062 | 4843 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 315504 | 403000 | 1551783 | 4383 | 2068688 |
| FixedU32 | 971256 | 1062977 | 1551783 | 4387 | 2728688 |

VByte postings are 32.5% of the fixed-width u32 baseline (3.08× smaller) and 21.2% of the analysed text.

**Query latency** (39 queries, k = 10, BM25, query-language mode; warm rows are 20 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 39 | 519.2 | 756.7 | 809.3 | 532.8 |
| Warm, hot-term cache off | 780 | 138.6 | 380.3 | 409.9 | 161.4 |
| Warm, hot-term cache on | 780 | 143.2 | 385.0 | 459.3 | 166.1 |
| Warm, Redis result-cache hit | 780 | 70.8 | 117.6 | 129.8 | 81.4 |
| Baseline: linear scan (no index) | 39 | 48943.5 | 52935.2 | 52981.8 | 49416.6 |

Engine open in the cold child process (RocksDB open + metadata load): p50 933.1 µs, p99 1277.8 µs — not included in the cold row.
Hot-term cache after the warm run: 61 terms resident (~159660 bytes), 1682 hits / 82 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 39/39 queries.

