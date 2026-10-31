# syntax=docker/dockerfile:1
# SPDX-License-Identifier: AGPL-3.0-or-later

FROM node:22-alpine AS build
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --ignore-scripts
COPY web/ ./
# The commit the image is built from: compared with the API's.
ARG HERON_REVISION=
RUN HERON_REVISION=$HERON_REVISION npm run build

FROM nginx:1.29-alpine AS heron-web
LABEL org.opencontainers.image.source="https://github.com/dehy/heron-broadcaster" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"
COPY docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /web/dist /usr/share/nginx/html
# Writes /config.js from HERON_WEB_SENTRY_DSN… when the container starts.
COPY --chmod=755 docker/web-config.sh /docker-entrypoint.d/40-heron-config.sh
HEALTHCHECK --interval=10s --timeout=5s --start-period=5s --retries=3 \
    CMD wget -q -O /dev/null http://127.0.0.1/healthz || exit 1
