#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Checks the buffer zone of a running Compose stack through the API:
# uploads a file in two chunks (sending the first one again, as after a
# dropped connection, is refused with where to carry on), waits until it
# and the files dropped into the incoming folder are ready, imports them
# and waits for the import. Needs jq.
#
#   scripts/check-buffer.sh <file to upload> <files expected ready> [web url]

set -eu

file=${1:?usage: check-buffer.sh <file to upload> <files expected ready> [web url]}
expected=${2:?}
web=${3:-http://localhost:${HERON_HTTP_PORT:-8000}}
api="$web/api/v1"
password=check-buffer-password
jar=$(mktemp)
part=$(mktemp)
trap 'rm -f "$jar" "$part"' EXIT

call() {
    method=$1
    path=$2
    shift 2
    curl -fsS -b "$jar" -c "$jar" -X "$method" -H 'X-Requested-With: check-buffer' \
        -H 'Content-Type: application/json' "$@" "$api$path"
}

put() {
    local offset=$1
    local file=$2
    curl -fsS -b "$jar" -X PUT -H 'X-Requested-With: check-buffer' \
        -H 'Content-Type: application/octet-stream' --data-binary "@$file" \
        "$api/staging/uploads/$id?offset=$offset"
}

within() {
    local limit=$1
    local message=$2
    if [ $(( $(date +%s) - start )) -ge "$limit" ]; then
        echo "$message" >&2
        call GET /staging >&2 || true
        exit 1
    fi
}

# Operators upload and import. The user may exist from an earlier run.
echo "$password" | docker compose exec -T heron-api heron-api user add check-buffer --role operator ||
    echo "$password" | docker compose exec -T heron-api heron-api user passwd check-buffer
call POST /auth/login -d "{\"username\": \"check-buffer\", \"password\": \"$password\"}" >/dev/null

group=$(call POST /staging/groups -d '{"name": "check-buffer"}' | jq -r .id)
size=$(wc -c <"$file" | tr -d ' ')
id=$(call POST /staging/uploads \
    -d "{\"group_id\": $group, \"name\": \"check-buffer/$(basename "$file")\", \"size\": $size}" | jq -r .id)
half=$((size / 2))
head -c "$half" "$file" >"$part"
put 0 "$part" >/dev/null
if put 0 "$part" >/dev/null 2>&1; then
    echo "a chunk out of place was accepted" >&2
    exit 1
fi
[ "$(call GET "/staging/uploads/$id" | jq -r .received)" = "$half" ]
tail -c +$((half + 1)) "$file" >"$part"
put "$half" "$part" | jq -e '.status == "identifying"' >/dev/null
echo "uploaded $size bytes in two chunks, resumed after a refused one"

start=$(date +%s)
while :; do
    ready=$(call GET /staging | jq -c '[.groups[].files[] | select(.status == "ready") | .id]')
    [ "$(echo "$ready" | jq length)" -ge "$expected" ] && break
    within 120 "fewer than $expected files ready in the buffer zone after 120 s"
    sleep 2
done
echo "ready in the buffer zone: $ready"

task=$(call POST /staging/import -d "{\"file_ids\": $ready}" | jq -r .id)
start=$(date +%s)
while :; do
    status=$(call GET "/library/tasks/$task" | jq -r .status)
    [ "$status" = done ] || [ "$status" = failed ] && break
    within 120 "the import did not finish in 120 s"
    sleep 1
done
report=$(call GET "/library/tasks/$task")
echo "$report" | jq '{status, error, report}'
[ "$status" = done ]
[ "$(echo "$report" | jq .report.done)" -eq "$expected" ]
