# syntax=docker/dockerfile:1

# ---- build ------------------------------------------------------------------
FROM rust:1.99-trixie AS build
# RocksDB's bindings are generated with bindgen, which needs libclang.
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY proto proto
COPY src src
RUN cargo build --release --locked --bin search-engine

# ---- runtime ----------------------------------------------------------------
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /home/search search \
    && mkdir /data && chown search:search /data
COPY --from=build /src/target/release/search-engine /usr/local/bin/search-engine
USER search
WORKDIR /data
VOLUME ["/data"]
EXPOSE 50051
ENTRYPOINT ["search-engine"]
CMD ["serve", "--index", "/data/index", "--addr", "0.0.0.0:50051"]
