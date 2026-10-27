#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Builds a minimal, static ffmpeg with libfdk_aac, for the external FFmpeg
# encoder (docs/outputs.md#external-ffmpeg-encoder): audio from a pipe in,
# AAC, HE-AAC (AAC+) and HE-AAC v2 out, as ADTS. Nothing else.
#
# The result is NOT redistributable (--enable-nonfree): build it for your own
# station, never publish it. Used by docker/engine-fdk.Dockerfile.
#
# Usage: build-ffmpeg-fdk.sh <work dir> <install prefix>
set -eu

work=$1
prefix=$2
FDK_AAC_VERSION=${FDK_AAC_VERSION:-v2.0.3}
FFMPEG_VERSION=${FFMPEG_VERSION:-n7.1.1}
jobs=$(nproc 2>/dev/null || echo 2)

mkdir -p "$work" "$prefix"
cd "$work"

[ -d fdk-aac ] || git clone --quiet --depth 1 --branch "$FDK_AAC_VERSION" https://github.com/mstorsjo/fdk-aac
cmake -S fdk-aac -B fdk-aac/build -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF \
    -DBUILD_PROGRAMS=OFF -DCMAKE_INSTALL_PREFIX="$prefix" -DCMAKE_POSITION_INDEPENDENT_CODE=ON >/dev/null
cmake --build fdk-aac/build --parallel "$jobs" >/dev/null
cmake --install fdk-aac/build >/dev/null

[ -d ffmpeg ] || git clone --quiet --depth 1 --branch "$FFMPEG_VERSION" https://github.com/FFmpeg/FFmpeg ffmpeg
cd ffmpeg
PKG_CONFIG_PATH="$prefix/lib/pkgconfig:$prefix/lib64/pkgconfig" ./configure \
    --prefix="$prefix" \
    --pkg-config-flags=--static \
    --extra-cflags="-I$prefix/include" \
    --extra-ldflags="-L$prefix/lib -L$prefix/lib64 -static" \
    --extra-libs="-lpthread -lm" \
    --enable-static --disable-shared \
    --enable-nonfree --enable-libfdk-aac \
    --disable-autodetect --disable-everything --disable-doc --disable-debug \
    --disable-network --disable-ffplay --disable-ffprobe \
    --enable-ffmpeg \
    --enable-swresample \
    --enable-protocol=pipe \
    --enable-demuxer=pcm_f32le \
    --enable-decoder=pcm_f32le \
    --enable-encoder=libfdk_aac,aac \
    --enable-muxer=adts \
    --enable-filter=aresample,aformat,anull,abuffer,abuffersink,format >/dev/null
make -j"$jobs" >/dev/null
make install >/dev/null
"$prefix/bin/ffmpeg" -hide_banner -encoders | grep -q libfdk_aac
echo "ffmpeg with libfdk_aac in $prefix/bin/ffmpeg"
