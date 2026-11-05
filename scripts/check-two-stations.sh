#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Two stations on air from one Heron (M13), upgraded, and an encrypted backup
# restored while they stream (M12, M15). On a running Compose stack with the icecast
# profile, after scripts/check-streaming.sh (the first station streams to
# /live):
#
#   1. a second engine and relay are added (docs/upgrading.md#several-stations)
#      and a station `hits80` streams to /hits80;
#   2. scripts/upgrade.sh hands both engines over: each source keeps its
#      connection to Icecast;
#   3. backups are encrypted, and a configuration backup is restored through
#      the API with its key: both stations play on, their outputs unchanged
#      keep their connection.
#
#   docker compose --profile icecast up -d --wait
#   scripts/check-streaming.sh
#   scripts/check-two-stations.sh
#
# Needs curl and python3. Leaves the second station running.

# `check && pass || fail`: pass always succeeds.
# shellcheck disable=SC2015
set -eu
export LC_ALL=C
cd "$(dirname "$0")/.."

web=http://localhost:${HERON_HTTP_PORT:-8000}
icecast=http://localhost:${HERON_ICECAST_PORT:-8010}
api="$web/api/v1"
password=check-two-stations-password
work=$(mktemp -d)
jar="$work/cookies"
failures=0

# The second station's engine and relay, as the docs show, in a file of
# their own: every `docker compose` below, scripts/upgrade.sh's included,
# reads both files.
cat >"$work/two-stations.yml" <<'EOF'
services:
  heron-engine-hits80:
    extends: { file: docker-compose.yml, service: heron-engine }
    volumes:
      - ${HERON_MUSIC_DIR:-./data/music}:/music:ro
      - ${HERON_FALLBACK_DIR:-./data/fallback}:/var/lib/heron/fallback:ro
      - engine-hits80-state:/var/lib/heron/engine
      - engine-hits80-relay:/run/heron/relay
  heron-relay-hits80:
    extends: { file: docker-compose.yml, service: heron-relay }
    volumes:
      - engine-hits80-relay:/run/heron/relay
  heron-api:
    environment:
      HERON_ENGINE_URL_HITS80: http://heron-engine-hits80:50051

volumes:
  engine-hits80-state:
  engine-hits80-relay:
EOF
cp "$work/two-stations.yml" ./.two-stations.yml
trap 'rm -rf "${work:?}"' EXIT
export COMPOSE_FILE="docker-compose.yml:.two-stations.yml"
export COMPOSE_PROFILES=icecast

pass() { echo "  ok   $*"; }
fail() {
    echo "  FAIL $*"
    # An annotation of the CI run, read without the logs.
    [ -z "${GITHUB_ACTIONS:-}" ] || echo "::error::check-two-stations: $*"
    failures=$((failures + 1))
}
step() { printf '\n== %s\n' "$*"; }

call() {
    method=$1
    path=$2
    shift 2
    curl -fsS -b "$jar" -c "$jar" -X "$method" -H 'X-Requested-With: check-two-stations' \
        -H 'Content-Type: application/json' "$@" "$api$path"
}

sign_in() {
    call POST /auth/login -d "{\"username\": \"check-two-stations\", \"password\": \"$password\"}" >/dev/null
}

# A field of a JSON document on stdin: `json id`, `json 0.name`.
json() {
    local path=$1
    python3 -c '
import json, sys
value = json.load(sys.stdin)
for part in sys.argv[1].split("."):
    value = value[int(part)] if isinstance(value, list) else value[part]
print(value if not isinstance(value, (dict, list)) else json.dumps(value))
' "$path"
}

# When Icecast's source on a mount connected (empty when there is none):
# it changes with a new connection.
source_started() {
    local mount=$1
    curl -fsS "$icecast/status-json.xsl" | python3 -c '
import json, sys
sources = json.load(sys.stdin)["icestats"].get("source", [])
sources = sources if isinstance(sources, list) else [sources]
for source in sources:
    if source.get("listenurl", "").endswith(sys.argv[1]) and source.get("title"):
        print(source.get("stream_start_iso8601", ""))
' "$mount"
}

wait_for() {
    seconds=$1
    shift
    start=$(date +%s)
    until "$@"; do
        [ $(($(date +%s) - start)) -lt "$seconds" ] || return 1
        sleep 1
    done
}

on_mount() {
    local mount=$1
    [ -n "$(source_started "$mount")" ]
}

# Bytes of a mount in 5 s: a 128 kbps stream gives 80 kB, plus the burst.
streams() {
    local mount=$1
    bytes=$(curl -s -m 5 "$icecast$mount" | wc -c | tr -d ' ')
    [ "$bytes" -ge 70000 ] && pass "$mount streams ($bytes bytes in 5 s)" || fail "$mount: $bytes bytes in 5 s"
}

main() {
    step "Setup: a second engine, and the station hits80 on /hits80"
    echo "$password" | docker compose exec -T heron-api heron-api user add check-two-stations --role admin ||
        echo "$password" | docker compose exec -T heron-api heron-api user passwd check-two-stations
    docker compose up -d --wait --wait-timeout 300 >/dev/null 2>&1 || docker compose up -d --wait --wait-timeout 300
    wait_for 60 curl -fsS "$api/status" -o /dev/null || fail "the API did not come back"
    sign_in
    station=$(call GET /stations | python3 -c '
import json, sys
print(next((s["id"] for s in json.load(sys.stdin) if s["code"] == "hits80"), ""))')
    if [ -z "$station" ]; then
        station=$(call POST /stations -d '{"name": "Hits 80", "code": "hits80"}' | json id)
    fi
    info=$(call GET "/stations/$station")
    echo "  station $station: $(echo "$info" | json engine_variable), configured $(echo "$info" | json engine_configured)"
    [ "$(echo "$info" | json engine_configured)" = True ] && pass "its engine is configured" || fail "its engine is not configured"
    profile=$(call GET /encoder-profiles | json 0.id)
    call POST "/stations/$station/outputs" -d "{\"name\": \"hits80\", \"kind\": \"icecast\", \"encoder_profile_id\": $profile,
        \"config\": {\"host\": \"icecast\", \"port\": 8000, \"mount\": \"/hits80\", \"password\": \"hackme\"}}" >/dev/null ||
        echo "  output already configured"
    if wait_for 90 on_mount /hits80 && wait_for 30 on_mount /live; then
        pass "both stations on air"
    else
        fail "not both on air: /live '$(source_started /live)', /hits80 '$(source_started /hits80)'"
        call GET "/stations/$station/engine" || true
    fi
    streams /live
    streams /hits80

    step "Both engines upgraded: each hands over to a new one"
    live_before=$(source_started /live)
    hits_before=$(source_started /hits80)
    if scripts/upgrade.sh --no-pull >"$work/upgrade.log" 2>&1; then
        pass "upgraded"
    else
        fail "scripts/upgrade.sh failed"
    fi
    sed 's/^/    /' "$work/upgrade.log"
    grep -q "heron-engine-hits80: .* on air" "$work/upgrade.log" && pass "hits80's engine handed over" ||
        fail "hits80's engine was not handed over"
    wait_for 30 on_mount /live || true
    wait_for 30 on_mount /hits80 || true
    [ "$(source_started /live)" = "$live_before" ] && pass "/live kept its connection ($live_before)" ||
        fail "/live connected again ($live_before -> $(source_started /live))"
    [ "$(source_started /hits80)" = "$hits_before" ] && pass "/hits80 kept its connection ($hits_before)" ||
        fail "/hits80 connected again ($hits_before -> $(source_started /hits80))"
    for engine in heron-engine heron-engine-hits80; do
        count=$(docker compose ps -q "$engine" | wc -l | tr -d ' ')
        [ "$count" -eq 1 ] && pass "one $engine" || fail "$count $engine containers"
    done

    step "An encrypted backup restored while both stations stream"
    sign_in
    key=$(call POST /backups/key -d '{}' | json key)
    call POST /backups/key/confirm -d "{\"key\": \"$key\"}" >/dev/null &&
        pass "backups encrypted" || fail "encryption was not turned on"
    name=$(call POST /backups | json name)
    [ "$(call GET /backups | json 0.encrypted)" = True ] && pass "backed up, encrypted: $name" ||
        fail "$name is not encrypted"
    live_before=$(source_started /live)
    hits_before=$(source_started /hits80)
    if call POST "/backups/$name/restore" >/dev/null 2>&1; then
        fail "restored without its key"
    else
        pass "its key is needed"
    fi
    report=$(call POST "/backups/$name/restore" -d "{\"key\": \"$key\"}") && pass "restored with its key" ||
        fail "the restore failed"
    echo "$report" | python3 -c '
import json, sys
report = json.load(sys.stdin)
print("    restored:", ", ".join(f"{k} {v}" for k, v in report["restored"].items() if v))
print("    left out:", report["left_out"] or "nothing")'
    if call GET /auth/me >/dev/null 2>&1; then
        fail "the session survived the restore"
    else
        pass "everyone signs in again"
    fi
    sign_in
    # The stations are started again and their engines get the configuration.
    sleep 20
    [ "$(source_started /live)" = "$live_before" ] && pass "/live kept its connection" ||
        fail "/live connected again ($live_before -> $(source_started /live))"
    [ "$(source_started /hits80)" = "$hits_before" ] && pass "/hits80 kept its connection" ||
        fail "/hits80 connected again ($hits_before -> $(source_started /hits80))"
    streams /live
    streams /hits80

    printf '\n'
    if [ "$failures" -gt 0 ]; then
        echo "$failures check(s) failed"
        exit 1
    fi
    echo "every check passed"
}

main
