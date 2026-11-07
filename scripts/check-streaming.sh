#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Checks that a running Compose stack goes on air through the API: creates
# an admin, signs in, adds an output to the `icecast` service, lets the
# queue manager fill the queue, listens to the stream and reads the play
# history.
#
#   docker compose --profile icecast up -d --wait
#   scripts/check-streaming.sh [web url] [icecast url]

set -eu

web=${1:-http://localhost:${HERON_HTTP_PORT:-8000}}
icecast=${2:-http://localhost:${HERON_ICECAST_PORT:-8010}}
api="$web/api/v1"
password=check-streaming-password
jar=$(mktemp)
trap 'rm -f "$jar"' EXIT

call() {
    method=$1
    path=$2
    shift 2
    curl -fsS -b "$jar" -c "$jar" -X "$method" -H 'X-Requested-With: check-streaming' \
        -H 'Content-Type: application/json' "$@" "$api$path"
}

# The user may exist from an earlier run.
echo "$password" | docker compose exec -T heron-api heron-api user add check-streaming --role admin ||
    echo "$password" | docker compose exec -T heron-api heron-api user passwd check-streaming
call POST /auth/login -d "{\"username\": \"check-streaming\", \"password\": \"$password\"}" >/dev/null

profile=$(call GET /encoder-profiles | tr '{' '\n' | grep '"name":"MP3 128k"' | sed 's/.*"id":\([0-9]*\).*/\1/')
# The stream server, then the output sending to it (docs/outputs.md#stream-servers).
call POST /stream-servers -d '{"name": "check-streaming", "host": "icecast", "port": 8000, "source_password": "hackme"}' \
    >/dev/null || echo "stream server already configured"
server=$(call GET /stream-servers | tr '{' '\n' | grep '"name":"check-streaming"' | sed 's/.*"id":\([0-9]*\).*/\1/' | head -n 1)
call POST /outputs -d "{\"name\": \"check-streaming\", \"kind\": \"icecast\", \"encoder_profile_id\": $profile,
    \"server_id\": $server, \"config\": {\"mount\": \"/live\"}}" >/dev/null ||
    echo "output already configured"

start=$(date +%s)
until curl -fsS "$icecast/status-json.xsl" | grep -q '"title"'; do
    if [ $(( $(date +%s) - start )) -ge 60 ]; then
        echo "no stream with a title on $icecast after 60 s" >&2
        call GET /engine >&2 || true
        call GET /player >&2 || true
        exit 1
    fi
    sleep 1
done
curl -fsS "$icecast/status-json.xsl" | tr ',' '\n' | grep -E '"(title|audio_info|bitrate)"'

# 5 s of a 128 kbps stream is 80 kB, plus the server's burst.
bytes=$(curl -s -m 5 "$icecast/live" | wc -c)
echo "received $bytes bytes in 5 s"
[ "$bytes" -ge 70000 ]

call GET /queue | grep -q '"source":"rotation"'
call GET /history | grep -q '"started_at_unix_ms"'
echo "queue filled by autofill, plays recorded"
