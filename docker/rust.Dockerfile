# syntax=docker/dockerfile:1
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Builds every Rust binary once, then one small image per service.
# Targets: heron-engine, heron-api, heron-analyzer, heron-librarian.

FROM rust:1.99.1-slim-trixie AS builder
# LAME (MP3 encoding) is compiled from source by its build script (configure, make).
# FFmpeg's development libraries and libclang (bindgen) for the engine's AAC-LC
# encoder (the `aac` feature), which links libavcodec dynamically.
RUN apt-get update \
    && apt-get install -y --no-install-recommends g++ make pkg-config clang libclang-dev \
        libavcodec-dev libavformat-dev libavutil-dev libswresample-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
# Two builds: cargo unifies features inside one, and only the engine links
# libavcodec (the other images have none).
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p heron-engine --features aac \
    && cargo build --release --locked -p heron-api -p heron-analyzer -p heron-librarian \
    && mkdir -p /out \
    && cp target/release/heron-engine target/release/heron-api target/release/heron-analyzer \
        target/release/heron-librarian /out/

FROM debian:trixie-slim AS runtime
LABEL org.opencontainers.image.source="https://github.com/dehy/heron-broadcaster" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"
# CA certificates for HTTPS: Sentry and metrics remote write in every
# service; AcoustID, MusicBrainz and the Cover Art Archive in the librarian.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --no-create-home heron
USER heron
# The commit and the date the image is built from: the web UI checks the
# commit is its own, and the About page shows both.
ARG HERON_REVISION=
ARG HERON_BUILD_DATE=
ENV HERON_REVISION=$HERON_REVISION \
    HERON_BUILD_DATE=$HERON_BUILD_DATE

FROM runtime AS heron-engine
USER root
# libavcodec: FFmpeg's native `aac` encoder, for AAC-LC (docs/outputs.md#built-in-aac).
# Debian's build has no fdk-aac.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libavcodec61 libavformat61 libswresample5 \
    && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /var/lib/heron/engine /var/lib/heron/fallback /run/heron/relay \
    && chown -R heron:heron /var/lib/heron /run/heron
USER heron
COPY --from=builder /out/heron-engine /usr/local/bin/heron-engine
ENV HERON_HTTP_ADDR=0.0.0.0:8081
EXPOSE 8081
HEALTHCHECK --interval=10s --timeout=5s --start-period=10s --retries=3 \
    CMD ["heron-engine", "healthcheck"]
ENTRYPOINT ["heron-engine"]

FROM runtime AS heron-api
# mariadb-dump: the database dump of each backup (docs/backups.md); the API
# compresses and encrypts it itself.
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends mariadb-client \
    && rm -rf /var/lib/apt/lists/*
USER heron
COPY --from=builder /out/heron-api /usr/local/bin/heron-api
ENV HERON_HTTP_ADDR=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=5s --start-period=60s --retries=3 \
    CMD ["heron-api", "healthcheck"]
ENTRYPOINT ["heron-api"]

FROM runtime AS heron-analyzer
COPY --from=builder /out/heron-analyzer /usr/local/bin/heron-analyzer
ENV HERON_HTTP_ADDR=0.0.0.0:8082
EXPOSE 8082
HEALTHCHECK --interval=10s --timeout=5s --start-period=60s --retries=3 \
    CMD ["heron-analyzer", "healthcheck"]
ENTRYPOINT ["heron-analyzer"]

FROM runtime AS heron-librarian
COPY --from=builder /out/heron-librarian /usr/local/bin/heron-librarian
ENV HERON_HTTP_ADDR=0.0.0.0:8083
EXPOSE 8083
HEALTHCHECK --interval=10s --timeout=5s --start-period=60s --retries=3 \
    CMD ["heron-librarian", "healthcheck"]
ENTRYPOINT ["heron-librarian"]
