### Rust docs crawl

300 documents, 5128 distinct terms, 42696 postings, 152294 positions. Corpus file 1542301 bytes (1.47 MiB); analysed text 1487492 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.053 | 0.010 | 0.063 | 4782 |
| FixedU32 | 0.053 | 0.009 | 0.062 | 4831 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 315504 | 403000 | 1551783 | 4383 | 2068688 |
| FixedU32 | 971256 | 1062977 | 1551783 | 4387 | 2728689 |

VByte postings are 32.5% of the fixed-width u32 baseline (3.08× smaller) and 21.2% of the analysed text.

**Query latency** (39 queries, k = 10, BM25, query-language mode; warm rows are 20 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 39 | 537.6 | 802.2 | 1019.7 | 552.8 |
| Warm, hot-term cache off | 780 | 137.5 | 391.4 | 418.1 | 161.8 |
| Warm, hot-term cache on | 780 | 129.6 | 392.5 | 434.7 | 161.8 |
| Warm, Redis result-cache hit | 780 | 85.5 | 114.8 | 375.3 | 96.0 |
| Baseline: linear scan (no index) | 39 | 48403.6 | 51435.9 | 52926.1 | 48580.0 |

Engine open in the cold child process (RocksDB open + metadata load): p50 959.4 µs, p99 1369.7 µs — not included in the cold row.
Hot-term cache after the warm run: 61 terms resident (~159660 bytes), 1682 hits / 82 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 39/39 queries.

