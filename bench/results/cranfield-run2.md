### Cranfield

1400 documents, 5029 distinct terms, 86755 postings, 143789 positions. Corpus file 1644706 bytes (1.57 MiB); analysed text 1545666 bytes.

**Index build** (median of 3 runs)

| Postings codec | Analyse + invert (s) | Write to RocksDB (s) | Total (s) | Docs/sec |
|---|---:|---:|---:|---:|
| VByte | 0.046 | 0.010 | 0.056 | 24816 |
| FixedU32 | 0.045 | 0.010 | 0.055 | 25514 |

**Index size**

| Postings codec | Postings bytes | `postings` CF on disk | `docs` CF on disk | `meta` CF on disk | Index directory |
|---|---:|---:|---:|---:|---:|
| VByte | 405063 | 488260 | 1635800 | 14485 | 2248068 |
| FixedU32 | 1289312 | 1377616 | 1635800 | 14488 | 3137447 |

VByte postings are 31.4% of the fixed-width u32 baseline (3.18× smaller) and 26.2% of the analysed text.

**Query latency** (225 queries, k = 10, BM25, free-text OR mode; warm rows are 5 rounds over all queries)

| Path | Samples | p50 (µs) | p95 (µs) | p99 (µs) | Mean (µs) |
|---|---:|---:|---:|---:|---:|
| Cold: fresh process, empty caches | 225 | 536.1 | 638.9 | 687.7 | 539.0 |
| Warm, hot-term cache off | 1125 | 153.6 | 217.4 | 251.3 | 158.4 |
| Warm, hot-term cache on | 1125 | 116.9 | 157.3 | 257.3 | 127.7 |
| Warm, Redis result-cache hit | 1125 | 71.8 | 86.0 | 93.0 | 73.4 |
| Baseline: linear scan (no index) | 225 | 40697.4 | 42414.2 | 43768.7 | 40918.1 |

Engine open in the cold child process (RocksDB open + metadata load): p50 932.3 µs, p99 1432.8 µs — not included in the cold row.
Hot-term cache after the warm run: 684 terms resident (~2294920 bytes), 12798 hits / 840 misses.
Redis hit ratio during the timed warm-Redis rounds: 100.0%.
Baseline returned the same top-10 doc ids as the index for 225/225 queries.

