# Common tasks. Data lands in data/ (git-ignored).
REDIS_URL ?= redis://127.0.0.1:6379
BIN := target/release/search-engine

.PHONY: fmt lint test check build crawl cranfield index-rust-docs index-cranfield eval bench serve client docker-build compose-up clean-data

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

check:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings
	cargo test

build:
	cargo build --release

crawl: build
	./scripts/crawl_rust_docs.sh data/rust-docs.jsonl

cranfield:
	./scripts/download_cranfield.sh data/cranfield

index-rust-docs: build
	rm -rf data/rust-docs-index
	$(BIN) index --input data/rust-docs.jsonl --index data/rust-docs-index

index-cranfield: build
	rm -rf data/cranfield-index
	$(BIN) index --input data/cranfield/cran.all.1400 --format cranfield --index data/cranfield-index

eval: index-cranfield
	$(BIN) eval --index data/cranfield-index --queries data/cranfield/cran.qry --qrels data/cranfield/cranqrel

# `make bench RUN=2` writes bench/results/*-run2.{json,md}.
RUN ?= 1

bench: build
	mkdir -p bench/results
	rm -rf data/bench
	$(BIN) bench --name "Rust docs crawl" --input data/rust-docs.jsonl \
		--queries bench/queries/rust-docs.txt --work-dir data/bench/rust-docs \
		--redis-url $(REDIS_URL) --rounds 20 --build-runs 3 \
		--out bench/results/rust-docs-run$(RUN).json > bench/results/rust-docs-run$(RUN).md
	$(BIN) bench --name "Cranfield" --input data/cranfield/cran.all.1400 --format cranfield \
		--queries data/cranfield/cran.qry --query-format cranfield --any --work-dir data/bench/cranfield \
		--redis-url $(REDIS_URL) --rounds 5 --build-runs 3 \
		--out bench/results/cranfield-run$(RUN).json > bench/results/cranfield-run$(RUN).md
	cat bench/results/rust-docs-run$(RUN).md bench/results/cranfield-run$(RUN).md

serve: build
	$(BIN) serve --index data/rust-docs-index --addr 127.0.0.1:50051 --redis-url $(REDIS_URL)

client:
	cargo run --release --example grpc_client -- 'ownership "borrow checker"'

docker-build:
	docker build -t search-engine .

compose-up:
	docker compose up --build

clean-data:
	rm -rf data/bench data/rust-docs-index data/cranfield-index
