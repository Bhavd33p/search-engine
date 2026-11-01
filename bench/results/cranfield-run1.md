### Cranfield

1400 documents, 5029 distinct terms, 86755 postings, 143789 positions. Corpus file 1644706 bytes (1.57 MiB); analysed text 1545666 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.045 | 0.010 | 0.054 | 25824 |
| FixedU32 | 0.044 | 0.010 | 0.055 | 25641 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 405063 | 488260 | 1635800 | 14485 | 2248068 |
| FixedU32 | 1289312 | 1377616 | 1635800 | 14488 | 3137446 |

VByte postings are 31.4% of the fixed-width u32 baseline (3.18× smaller) and 26.2% of the analysed text.

**Query latency** (225 queries, k = 10, BM25, free-text OR mode; warm rows are 5 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 225 | 539.6 | 622.2 | 672.2 | 537.2 |
| Warm, hot-term cache off | 1125 | 151.2 | 212.4 | 239.5 | 153.7 |
| Warm, hot-term cache on | 1125 | 108.2 | 139.2 | 151.3 | 109.2 |
| Warm, Redis result-cache hit | 1125 | 70.9 | 86.5 | 95.5 | 72.7 |
| Baseline: linear scan (no index) | 225 | 41099.6 | 43644.3 | 45551.4 | 41406.4 |

Engine open in the cold child process (RocksDB open + metadata load): p50 937.7 µs, p99 1276.5 µs — not included in the cold row.
Hot-term cache after the warm run: 684 terms resident (~2294920 bytes), 12798 hits / 840 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 225/225 queries.

