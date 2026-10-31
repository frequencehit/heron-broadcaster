# syntax=docker/dockerfile:1
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Builds every Rust binary once, then one small image per service.
# Targets: heron-engine, heron-api, heron-analyzer, heron-librarian.

FROM rust:1.99.1-slim-trixie AS builder
# LAME (MP3 encoding) is compiled from source by its build script (configure, make).
RUN apt-get update \
    && apt-get install -y --no-install-recommends g++ make \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
        --bin heron-engine --bin heron-api --bin heron-analyzer --bin heron-librarian \
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

FROM runtime AS heron-engine
USER root
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
# The commit the image is built from: the web UI checks it is its own.
ARG HERON_REVISION=
ENV HERON_REVISION=$HERON_REVISION
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
