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

