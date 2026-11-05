#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Writes the web app's runtime configuration (/config.js) from the
# container's environment, JSON-encoded, when heron-web starts (run by the
# nginx image's entrypoint from /docker-entrypoint.d). It never fails: a bad
# optional setting must not keep the web interface down.

target=/usr/share/nginx/html/config.js

# A JSON string: control characters dropped, backslashes and quotes escaped.
json() {
    local value=$1
    printf '"%s"' "$(printf '%s' "$value" | tr -d '\000-\037' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"
}

if ! printf 'window.heronConfig = {"sentryDsn": %s, "sentryEnvironment": %s};\n' \
    "$(json "${HERON_WEB_SENTRY_DSN:-}")" \
    "$(json "${HERON_SENTRY_ENVIRONMENT:-production}")" >"$target"; then
    echo "heron-web: cannot write $target, the web app runs without Sentry" >&2
fi
exit 0
