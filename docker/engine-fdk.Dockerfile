# SPDX-License-Identifier: AGPL-3.0-or-later
#
# heron-engine with an ffmpeg built with libfdk_aac, for AAC+ (HE-AAC)
# outputs through the external FFmpeg encoder (docs/outputs.md).
#
# NOT REDISTRIBUTABLE: this ffmpeg is built with --enable-nonfree. Build this
# image on your own station, for your own use; never push it to a registry.
# Heron's CI never builds it. See docker-compose.fdk.yml.

ARG HERON_ENGINE_IMAGE=ghcr.io/dehy/heron-engine:latest

FROM debian:trixie-slim AS ffmpeg
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates git cmake make gcc g++ pkg-config nasm \
    && rm -rf /var/lib/apt/lists/*
COPY docker/build-ffmpeg-fdk.sh /usr/local/bin/build-ffmpeg-fdk.sh
ARG FDK_AAC_VERSION=v2.0.3
ARG FFMPEG_VERSION=n7.1.1
RUN FDK_AAC_VERSION=$FDK_AAC_VERSION FFMPEG_VERSION=$FFMPEG_VERSION \
    build-ffmpeg-fdk.sh /build /opt/ffmpeg-fdk

FROM ${HERON_ENGINE_IMAGE} AS engine-fdk
LABEL org.opencontainers.image.description="heron-engine with a locally built, non-redistributable ffmpeg (libfdk_aac). Do not publish."
# Static: nothing else to install in the engine's image.
COPY --from=ffmpeg /opt/ffmpeg-fdk/bin/ffmpeg /usr/local/bin/ffmpeg-fdk
ENV HERON_FFMPEG=/usr/local/bin/ffmpeg-fdk
