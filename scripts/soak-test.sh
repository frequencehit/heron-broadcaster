#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Soak test of the engine (M2): plays the library with crossfades to two
# Icecast servers (MP3 128k and 64k) for a given time, sampling the engine's
# metrics every minute, then checks there was no drift and no glitch.
#
#   scripts/soak-test.sh <library dir> <seconds> [work dir]
#
# Needs Docker (Icecast) and a release build: cargo build --release -p heron-engine
# Uses ports 18000, 18001 (Icecast), 50051 (gRPC) and 18081 (metrics).

set -eu
# Metrics use `.` as decimal point: awk must not parse them with the user's locale.
export LC_ALL=C

library=${1:?usage: soak-test.sh <library dir> <seconds> [work dir]}
duration=${2:?usage: soak-test.sh <library dir> <seconds> [work dir]}
work=${3:-$(mktemp -d)}
engine=${HERON_ENGINE_BIN:-target/release/heron-engine}
image=libretime/icecast:2.5.0

library=$(cd "$library" && pwd)
mkdir -p "$work/state"
csv="$work/metrics.csv"

# Container names unique to this run, so two runs never touch each other's.
server_a=heron-soak-$$-a
server_b=heron-soak-$$-b
autofill_pid=
engine_pid=
cleanup() {
    kill $autofill_pid $engine_pid 2>/dev/null || true
    docker rm -f "$server_a" "$server_b" >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

docker run -d --name "$server_a" -p 18000:8000 -e ICECAST_SOURCE_PASSWORD=hackme "$image" >/dev/null
docker run -d --name "$server_b" -p 18001:8000 -e ICECAST_SOURCE_PASSWORD=hackme "$image" >/dev/null

cat > "$work/outputs.json" <<'EOF'
{
  "encoder_profiles": [
    {"id": 1, "name": "MP3 128k", "encoder": "mp3", "bitrate_kbps": 128, "sample_rate": 44100, "channels": 2},
    {"id": 2, "name": "MP3 64k", "encoder": "mp3", "bitrate_kbps": 64, "sample_rate": 22050, "channels": 2}
  ],
  "outputs": [
    {"id": 1, "name": "soak-a", "kind": "icecast", "encoder_profile_id": 1,
     "config": {"host": "127.0.0.1", "port": 18000, "mount": "/live", "password": "hackme"}},
    {"id": 2, "name": "soak-b", "kind": "icecast", "encoder_profile_id": 2,
     "config": {"host": "127.0.0.1", "port": 18001, "mount": "/live", "password": "hackme"}}
  ]
}
EOF

export HERON_LIBRARY_DIR="$library" HERON_STATE_DIR="$work/state" HERON_FALLBACK_DIR="$work" \
    HERON_HTTP_ADDR=127.0.0.1:18081 HERON_GRPC_ADDR=127.0.0.1:50051 RUST_LOG=info
"$engine" > "$work/engine.log" 2>&1 &
engine_pid=$!
sleep 2
"$engine" ctl set-outputs "$work/outputs.json"
"$engine" ctl autofill --min 3 > "$work/autofill.log" 2>&1 &
autofill_pid=$!

metric() {
    local text=$1
    local key=$2
    printf "%s\n" "$text" | awk -v name="$key" '$1 == name { print $2 }'
}

echo "elapsed_s,rendered_s,drift_s,max_late_s,underruns,dropped_blocks,clock_resets,bytes_a,bytes_b,reconnects_a,reconnects_b" > "$csv"
start=$(date +%s)
while :; do
    # In the background, so a signal interrupts the wait at once.
    sleep 60 &
    wait $!
    elapsed=$(( $(date +%s) - start ))
    m=$(curl -fsS http://127.0.0.1:18081/metrics)
    echo "$elapsed,$(metric "$m" heron_engine_rendered_seconds),$(metric "$m" heron_engine_clock_drift_seconds),$(metric "$m" heron_engine_clock_max_late_seconds),$(metric "$m" heron_engine_underruns_total),$(metric "$m" heron_engine_dropped_blocks_total),$(metric "$m" heron_engine_clock_resets_total),$(metric "$m" 'heron_output_bytes_total{output="soak-a"}'),$(metric "$m" 'heron_output_bytes_total{output="soak-b"}'),$(metric "$m" 'heron_output_reconnects_total{output="soak-a"}'),$(metric "$m" 'heron_output_reconnects_total{output="soak-b"}')" >> "$csv"
    [ "$elapsed" -ge "$duration" ] && break
done

# Verdict: the clock kept time and nothing was lost.
awk -F, 'NR == 2 { first_drift = $3; first_a = $8; first_b = $9; first_t = $1 }
NR > 1 { last = $0; drift = $3; t = $1; a = $8; b = $9; late = ($4 > late ? $4 : late)
         underruns = $5; dropped = $6; resets = $7; rec = $10 + $11 }
END {
    span = t - first_t
    printf "duration %d s, drift moved %.6f s, worst lateness %.3f s\n", t, drift - first_drift, late
    printf "underruns %d, dropped blocks %d, clock resets %d, reconnects %d\n", underruns, dropped, resets, rec
    if (span > 0) printf "output A %.1f kbps, output B %.1f kbps\n", (a - first_a) * 8 / span / 1000, (b - first_b) * 8 / span / 1000
    ok = (underruns == 0 && dropped == 0 && resets == 0 && rec == 0 && (drift - first_drift) < 0.05 && (first_drift - drift) < 0.05)
    print (ok ? "PASS" : "FAIL")
    exit (ok ? 0 : 1)
}' "$csv"
echo "Details in $work"
