#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Chaos tests (M3): breaks a running Compose stack in the ways a station
# breaks, while listening to the stream, and checks each time that the air
# stays on or comes back by itself:
#
#   1. MariaDB stopped;
#   2. heron-api stopped, and the queue emptied: the fallback directory plays;
#   3. Icecast restarted;
#   4. the files of queued tracks deleted;
#   5. a silent file queued: the silence watchdog skips it;
#   6. heron-engine restarted, cleanly then killed: it resumes the track on air;
#   7. heron-engine upgraded (scripts/upgrade.sh): the new engine takes over
#      the source connection, listeners hear no gap.
#
#   docker compose --profile icecast up -d --wait
#   scripts/chaos-test.sh
#
# Needs tracks in the library and in the fallback directory, curl, and
# Docker (ffmpeg in a container listens to the stream). Takes about 10 min.

set -eu
export LC_ALL=C
cd "$(dirname "$0")/.."

# Settings from the environment, else from .env, else the defaults.
setting() {
    local key=$1
    local fallback=$2
    eval "value=\${$key:-}"
    [ -n "$value" ] || value=$(sed -n "s/^$key=//p" .env 2>/dev/null | tail -1)
    echo "${value:-$fallback}"
}
HERON_HTTP_PORT=$(setting HERON_HTTP_PORT 8000)
HERON_ICECAST_PORT=$(setting HERON_ICECAST_PORT 8010)
export HERON_HTTP_PORT HERON_ICECAST_PORT
icecast=http://localhost:$HERON_ICECAST_PORT
music=$(setting HERON_MUSIC_DIR ./data/music)
ffmpeg_image=linuxserver/ffmpeg:9.0.2
work=$(mktemp -d)
# The silent file of step 5: a name of its own, removed only once created.
silence=chaos-silence-$$.wav
silence_created=
failures=0

# Puts back the library files step 4 moved away. Fails, keeping them in the
# work directory, when one cannot be put back.
restore_deleted() {
    [ -d "$work/deleted" ] || return 0
    (cd "$work/deleted" && find . -type f) >"$work/to-restore"
    restore_failed=0
    while IFS= read -r path; do
        if [ -e "$music/$path" ] || ! mv "$work/deleted/$path" "$music/$path"; then
            echo "  cannot restore $music/$path" >&2
            restore_failed=1
        fi
    done <"$work/to-restore"
    [ "$restore_failed" -eq 0 ] && rm -rf "${work:?}/deleted"
}

# On any exit, the library is left as it was found.
cleanup() {
    status=$?
    if [ -n "$silence_created" ]; then
        rm -f -- "${music:?}/${silence:?}"
    fi
    if restore_deleted; then
        rm -rf "${work:?}"
    else
        echo "library files moved by the test are kept in $work/deleted" >&2
        status=1
    fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

ctl() {
    docker compose exec -T heron-engine heron-engine ctl "$@"
}

# One of the engine's metrics, read from inside the network, summed over
# its labels.
metric() {
    local name=$1
    docker compose exec -T heron-web wget -q -O - http://heron-engine:8081/metrics |
        awk -v name="$name" '$1 == name || index($1, name "{") == 1 { sum += $2; found = 1 }
            END { if (found) print sum }'
}

# Compose commands, without their progress lines.
compose() {
    docker compose --progress quiet "$@"
}

step() {
    current_step=$*
    printf '\n== %s\n' "$*"
}

pass() {
    printf '  PASS %s\n' "$*"
}

fail() {
    printf '  FAIL %s\n' "$*"
    # An annotation of the CI run, read without its logs: the step and the
    # check that failed.
    [ -z "${GITHUB_ACTIONS:-}" ] || printf '::error::chaos test, %s: %s\n' "${current_step:-}" "$*"
    failures=$((failures + 1))
}

# Waits up to $1 seconds for the command that follows to succeed.
wait_for() {
    timeout=$1
    shift
    start=$(date +%s)
    until "$@" >/dev/null 2>&1; do
        [ $(($(date +%s) - start)) -lt "$timeout" ] || return 1
        sleep 1
    done
}

on_icecast() {
    curl -fsS "$icecast/status-json.xsl" | grep -q '"listenurl"'
}

# Starts recording $1 seconds of the stream in the background.
start_listening() {
    listened=$1
    capture="$work/capture-$(date +%s).mp3"
    curl -s -m "$listened" "$icecast/live" >"$capture" &
    listener=$!
}

# Waits for the recording, then checks it: at least 90 % of the time
# requested, and no silence longer than $1 seconds. Prints the longest silence.
check_listening() {
    max_silence=$1
    wait "$listener" || true
    # The recording starts anywhere in an MP3 frame: probed, it is now and
    # then taken for raw video (VVC) and nothing is decoded. Say it is MP3.
    report=$(docker run --rm -i --entrypoint ffmpeg "$ffmpeg_image" -hide_banner -nostdin \
        -f mp3 -i pipe:0 -af silencedetect=noise=-50dB:duration=1 -f null - <"$capture" 2>&1 || true)
    heard=$(printf '%s\n' "$report" | tr '\r' '\n' | sed -n 's/.*time=\([0-9:.]*\).*/\1/p' | tail -1 |
        awk -F: '{ print $1 * 3600 + $2 * 60 + $3 }')
    longest=$(printf '%s\n' "$report" | sed -n 's/.*silence_duration: \([0-9.]*\).*/\1/p' |
        sort -n | tail -1)
    starts=$(printf '%s\n' "$report" | grep -c 'silence_start' || true)
    ends=$(printf '%s\n' "$report" | grep -c 'silence_end' || true)
    longest=${longest:-0}
    heard=${heard:-0}
    echo "  heard ${heard} s of ${listened} s, longest silence ${longest} s"
    if [ "$starts" -gt "$ends" ]; then
        fail "the stream was still silent at the end (heard ${heard} s of ${listened} s, longest silence ${longest} s)"
        return 1
    fi
    if awk -v heard="$heard" -v wanted="$listened" -v longest="$longest" -v max="$max_silence" \
        'BEGIN { exit !(heard >= wanted * 0.9 && longest <= max) }'; then
        return 0
    fi
    fail "the stream was interrupted (heard ${heard} s of ${listened} s, longest silence ${longest} s, ${max_silence} s allowed)"
    return 1
}

waiting_tracks() {
    ctl state | grep -cE '^(cued|  queued): #' || true
}

has_rotation_queue() {
    ctl state | grep -E '^  queued: #' | grep -q '(rotation)'
}

on_air_line() {
    ctl state | grep '(on air)'
}

# The track on air has played at least $1 s and has at least $2 s left:
# `A (on air)    10.2 /    21.0 s  #11 …`.
on_air_between() {
    min_played=$1
    min_left=$2
    on_air_line | awk -v played="$min_played" -v left="$min_left" \
        '{ found = 1; if ($4 + 0 >= played + 0 && $6 - $4 >= left + 0) ok = 1 } END { exit !(found && ok) }'
}

# When Icecast's source on /live connected: changes with a new connection.
source_started() {
    curl -fsS "$icecast/status-json.xsl" | tr ',' '\n' | sed -n 's/.*"stream_start_iso8601":"\([^"]*\)".*/\1/p' | head -1
}

# All in a function: the shell reads it whole before running, so editing
# this file during a run cannot break the run.
main() {
    step "Setup: an output to the local Icecast, the queue filled by the API"
    scripts/check-streaming.sh >/dev/null
    wait_for 60 has_rotation_queue || fail "autofill did not fill the queue"
    start_listening 15
    check_listening 1 && pass "on air"

    step "1. MariaDB stopped for 90 s"
    start_listening 100
    compose stop mariadb >/dev/null
    sleep 90
    compose start mariadb >/dev/null
    check_listening 1 && pass "the air stayed on without the database"
    if wait_for 120 sh -c "curl -fsS http://localhost:$HERON_HTTP_PORT/api/v1/status | grep -q '\"database\":\"ok\"'"; then
        pass "the API found the database again"
    else
        fail "the API did not recover its database"
    fi

    step "2. heron-api stopped, the queue emptied"
    compose stop heron-api >/dev/null
    start_listening 70
    ctl clear >/dev/null
    ctl skip >/dev/null
    sleep 10
    if on_air_line | grep -q '(fallback)'; then
        pass "the fallback directory is on air: $(on_air_line)"
    else
        fail "no fallback track on air: $(on_air_line || echo nothing)"
    fi
    check_listening 2 && pass "the air stayed on without the API"
    compose start heron-api >/dev/null
    if wait_for 120 has_rotation_queue; then
        pass "the API refilled the queue with rotation"
    else
        fail "the queue was not refilled"
    fi

    step "3. Icecast restarted"
    reconnects=$(metric heron_output_reconnects_total)
    compose restart icecast >/dev/null
    if wait_for 60 on_icecast; then
        pass "the engine reconnected (reconnections: ${reconnects:-0} -> $(metric heron_output_reconnects_total))"
    else
        fail "the engine did not reconnect to Icecast"
    fi
    start_listening 15
    check_listening 1 && pass "on air again"

    step "4. The files of the queued tracks deleted"
    paths=$(ctl state | sed -nE 's/^(cued|  queued): #.*\[(.*)\] \(rotation\)$/\2/p')
    count=$(printf '%s\n' "$paths" | grep -c . || true)
    printf '%s\n' "$paths" | while IFS= read -r path; do
        [ -n "$path" ] || continue
        mkdir -p "$work/deleted/$(dirname "$path")"
        mv "$music/$path" "$work/deleted/$path"
    done
    echo "  deleted $count queued files"
    start_listening 45
    for _ in 1 2 3 4; do
        ctl skip >/dev/null
        sleep 5
    done
    check_listening 3 && pass "missing files were skipped (or played from memory) without dead air"
    restore_deleted || fail "the deleted files could not all be restored"

    step "5. A silent file queued"
    skips=$(metric heron_engine_silence_skips_total)
    # Never touch a file of the library: refuse if the name is taken.
    if [ -e "$music/$silence" ]; then
        echo "$music/$silence exists: not overwriting it" >&2
        exit 1
    fi
    docker run --rm -v "$(cd "$music" && pwd):/out" --entrypoint ffmpeg "$ffmpeg_image" -loglevel error -nostdin -n \
        -f lavfi -i anullsrc=r=44100:cl=stereo:d=90 -c:a pcm_s16le "/out/$silence"
    silence_created=1
    ctl enqueue --next --no-analysis "$silence" >/dev/null
    ctl skip >/dev/null
    start_listening 50
    check_listening 25 || true
    if awk -v longest="$longest" 'BEGIN { exit !(longest >= 15 && longest <= 25) }'; then
        pass "the silence watchdog skipped the silent track after ${longest} s"
    else
        fail "expected one silence of about 20 s, got ${longest} s"
    fi
    echo "  silence skips: ${skips:-0} -> $(metric heron_engine_silence_skips_total)"
    rm -f -- "${music:?}/${silence:?}"
    silence_created=

    step "6. heron-engine restarted"
    for how in restart kill; do
        # A track that cannot end during the restart, and that the queue,
        # saved every 5 s, already holds as on air.
        wait_for 40 on_air_between 6 10 ||
            echo "  no track on air with 6 s played and 10 s left"
        before=$(on_air_line || true)
        entry=$(printf '%s\n' "$before" | sed -n 's/.*#\([0-9]*\) .*/\1/p')
        if [ "$how" = restart ]; then
            compose restart heron-engine >/dev/null
        else
            # No clean shutdown: the queue saved every 5 s is all there is.
            compose kill heron-engine >/dev/null
            compose start heron-engine >/dev/null
        fi
        start=$(date +%s)
        if wait_for 30 on_icecast; then
            pass "$how: back on Icecast after $(($(date +%s) - start)) s"
        else
            fail "$how: not back on Icecast after 30 s"
        fi
        after=$(on_air_line || true)
        echo "  before: $before"
        echo "  after:  $after"
        if [ -n "$entry" ] && printf '%s\n' "$after" | grep -q "#$entry "; then
            pass "$how: the track on air was resumed"
        else
            fail "$how: the track on air was not resumed"
        fi
        start_listening 15
        check_listening 1 && pass "$how: on air"
    done

    step "7. heron-engine upgraded: hot handover"
    wait_for 30 on_icecast || true
    before=$(on_air_line || true)
    entry=$(printf '%s\n' "$before" | sed -n 's/.*#\([0-9]*\) .*/\1/p')
    source_before=$(source_started)
    start_listening 60
    sleep 5
    if scripts/upgrade.sh --no-pull >"$work/upgrade.log" 2>&1; then
        pass "upgraded: $(tail -1 "$work/upgrade.log")"
    else
        fail "scripts/upgrade.sh failed:"
        sed 's/^/    /' "$work/upgrade.log"
    fi
    check_listening 1 && pass "the listener heard no gap"
    source_after=$(source_started)
    if [ -n "$source_before" ] && [ "$source_before" = "$source_after" ]; then
        pass "one source connection throughout (since $source_before)"
    else
        fail "the source connected again ($source_before -> $source_after)"
    fi
    after=$(on_air_line || true)
    echo "  before: $before"
    echo "  after:  $after"
    if [ -n "$entry" ] && printf '%s\n' "$after" | grep -q "#$entry "; then
        pass "the new engine plays on the track on air"
    elif [ -n "$after" ]; then
        pass "the new engine is on air (the track changed during the test)"
    else
        fail "nothing on air after the upgrade"
    fi
    engines=$(docker compose ps -q heron-engine | wc -l | tr -d ' ')
    [ "$engines" -eq 1 ] && pass "one engine left" || fail "$engines engines running"

    printf '\n'
    if [ "$failures" -gt 0 ]; then
        echo "$failures check(s) failed"
        exit 1
    fi
    echo "every check passed"
}

main "$@"
