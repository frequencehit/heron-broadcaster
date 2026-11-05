#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Generates a synthetic music library for import tests:
#   scripts/generate-test-library.sh <dir> <count>
# Files are laid out as Artist/Album/NN Title.ext, in MP3, M4A (AAC) and
# FLAC, with tags and silence before and after the music. Uses ffmpeg in
# Docker (multi-arch image), so nothing needs installing.

set -eu

dir=${1:?usage: generate-test-library.sh <dir> <count>}
count=${2:?usage: generate-test-library.sh <dir> <count>}
image=linuxserver/ffmpeg:9.0.2
scripts=$(cd "$(dirname "$0")/test-library" && pwd)

mkdir -p "$dir"
dir=$(cd "$dir" && pwd)

docker run --rm -v "$dir:/out" -v "$scripts:/scripts:ro" --entrypoint /bin/sh "$image" -c \
    "seq 1 $count | xargs -P \$(nproc) -n 1 sh /scripts/generate-track.sh"
echo "Generated $count tracks in $dir"
