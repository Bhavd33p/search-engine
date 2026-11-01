### Cranfield

1400 documents, 5029 distinct terms, 86755 postings, 143789 positions. Corpus file 1644706 bytes (1.57 MiB); analysed text 1545666 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.049 | 0.010 | 0.059 | 23723 |
| FixedU32 | 0.045 | 0.010 | 0.055 | 25239 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 405063 | 488260 | 1635800 | 14485 | 2248068 |
| FixedU32 | 1289312 | 1377616 | 1635800 | 14488 | 3137445 |

VByte postings are 31.4% of the fixed-width u32 baseline (3.18× smaller) and 26.2% of the analysed text.

**Query latency** (225 queries, k = 10, BM25, free-text OR mode; warm rows are 5 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 225 | 527.0 | 644.2 | 832.4 | 571.0 |
| Warm, hot-term cache off | 1125 | 169.4 | 247.7 | 284.9 | 171.3 |
| Warm, hot-term cache on | 1125 | 111.0 | 143.2 | 157.6 | 112.4 |
| Warm, Redis result-cache hit | 1125 | 74.6 | 89.8 | 98.4 | 76.1 |
| Baseline: linear scan (no index) | 225 | 41164.2 | 44758.6 | 48613.0 | 41735.0 |

Engine open in the cold child process (RocksDB open + metadata load): p50 953.5 µs, p99 1264.6 µs — not included in the cold row.
Hot-term cache after the warm run: 684 terms resident (~2294920 bytes), 12798 hits / 840 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 225/225 queries.

