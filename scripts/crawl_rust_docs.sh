#!/usr/bin/env bash
# Reproduces the demo corpus: 300 pages of The Rust Programming Language and
# Rust by Example from doc.rust-lang.org, whose robots.txt allows both paths.
# At one request per second this takes about five minutes.
set -euo pipefail

OUT="${1:-data/rust-docs.jsonl}"
mkdir -p "$(dirname "$OUT")"

cargo run --release -- crawl \
  --seed https://doc.rust-lang.org/book/ \
  --seed https://doc.rust-lang.org/rust-by-example/ \
  --exclude print.html \
  --max-pages 300 \
  --delay-ms 1000 \
  --concurrency 4 \
  --out "$OUT"
