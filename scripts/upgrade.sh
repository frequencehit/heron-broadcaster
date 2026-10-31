#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Upgrades running stations without taking them off air: every service, the
# engines last, each by a hot handover from its running engine to a new one
# (docs/upgrading.md, docs/handover.md). Each station has its engine and its
# relay: heron-engine and heron-relay for the first, heron-engine-<code> and
# heron-relay-<code> for the others (docs/stations.md#engines).
#
#   scripts/upgrade.sh [--no-pull] [version]                # Docker Compose
#   scripts/upgrade.sh --swarm <stack> [--no-pull] [version] # Docker Swarm
#
# With a version, Compose's .env gets HERON_VERSION=<version>; in Swarm the
# Heron images of the stack get that tag. Run from the directory of
# docker-compose.yml.

set -eu

swarm=
pull=yes
version=
while [ $# -gt 0 ]; do
    case $1 in
    --swarm)
        swarm=${2:?--swarm needs the name of the stack}
        shift 2
        ;;
    --no-pull)
        pull=
        shift
        ;;
    -h | --help)
        sed -n '4,13p' "$0"
        exit 0
        ;;
    -*)
        echo "unknown option $1" >&2
        exit 2
        ;;
    *)
        version=$1
        shift
        ;;
    esac
done

say() {
    echo "==> $*"
}

fail() {
    echo "upgrade failed: $*" >&2
    exit 1
}

# Whether a service (or a Swarm service, after its stack's prefix) is a
# station's engine, or its relay.
is_engine() {
    case ${1##*_} in
    heron-engine | heron-engine-*) return 0 ;;
    *) return 1 ;;
    esac
}
is_relay() {
    case ${1##*_} in
    heron-relay | heron-relay-*) return 0 ;;
    *) return 1 ;;
    esac
}

# Docker Swarm: start-first updates of each engine do the handover.
if [ -n "$swarm" ]; then
    engines=
    for service in $(docker stack services --format '{{.Name}}' "$swarm"); do
        image=$(docker service inspect -f '{{.Spec.TaskTemplate.ContainerSpec.Image}}' "$service")
        image=${image%%@*}
        case $image in
        ghcr.io/*/heron-* | heron-*) ;;
        *) continue ;;
        esac
        if [ -n "$version" ]; then
            image="${image%:*}:$version"
        fi
        if [ -n "$pull" ]; then
            say "pulling $image"
            docker pull --quiet "$image" >/dev/null
        fi
        if is_engine "$service"; then
            engines="$engines $service=$image"
        elif is_relay "$service"; then
            # Holds the connections: left running (docs/upgrading.md).
            :
        else
            say "updating $service"
            docker service update --quiet --with-registry-auth --image "$image" "$service" >/dev/null
        fi
    done
    [ -n "$engines" ] || fail "no heron-engine service in stack $swarm"
    # heron-engine first, then the other stations' (sorted).
    for engine in $(echo "$engines" | tr ' ' '\n' | LC_ALL=C sort -t= -k1,1); do
        service=${engine%%=*}
        say "updating $service: the new engine takes over from the running one"
        docker service update --quiet --with-registry-auth --image "${engine#*=}" \
            --update-order start-first --update-failure-action rollback \
            --stop-grace-period 30s --force "$service" >/dev/null
    done
    say "done"
    exit 0
fi

# Docker Compose.
compose() {
    docker compose "$@"
}

if [ -n "$version" ]; then
    say "HERON_VERSION=$version in .env"
    if [ -f .env ] && grep -q '^HERON_VERSION=' .env; then
        sed "s/^HERON_VERSION=.*/HERON_VERSION=$version/" .env >.env.upgrade
        mv .env.upgrade .env
    else
        echo "HERON_VERSION=$version" >>.env
    fi
fi

if [ -n "$pull" ]; then
    say "pulling the images"
    compose pull --quiet
fi

# The relays hold the connections to the stream servers: left running.
# The engines are handed over one by one, after everything else.
others=
for service in $(compose ps --services --status running); do
    is_engine "$service" || is_relay "$service" || others="$others $service"
done
if [ -n "$others" ]; then
    say "upgrading$others"
    # shellcheck disable=SC2086
    compose up -d --no-deps --wait $others
fi

# Hands a station over from its running engine to a new one.
hand_over() {
    service=$1
    old=$(compose ps -q "$service")
    if [ -z "$old" ]; then
        say "$service is not running: starting it"
        compose up -d --no-deps --wait "$service"
        return
    fi
    [ "$(echo "$old" | wc -l)" -eq 1 ] || fail "more than one $service container runs"

    say "$service: starting the new engine next to the running one"
    compose up -d --no-deps --no-recreate --scale "$service=2" "$service"
    new=$(compose ps -q "$service" | grep -v "^$old$" || true)
    [ -n "$new" ] || fail "the new $service did not start"

    waited=0
    until [ "$(docker inspect -f '{{.State.Health.Status}}' "$new")" = healthy ]; do
        if [ "$waited" -ge 120 ] || [ "$(docker inspect -f '{{.State.Running}}' "$new")" != true ]; then
            docker logs --tail 20 "$new" >&2 || true
            docker rm -f "$new" >/dev/null
            fail "the new $service is not healthy: the running engine stays on air"
        fi
        sleep 2
        waited=$((waited + 2))
    done

    say "$service: handing over to the new engine"
    docker stop --time 30 "$old" >/dev/null
    docker rm "$old" >/dev/null

    waited=0
    until compose exec -T "$service" heron-engine ctl state >/dev/null 2>&1; do
        [ "$waited" -lt 30 ] || fail "the new $service does not answer"
        sleep 1
        waited=$((waited + 1))
    done
    say "$service: $(docker inspect -f '{{.Config.Image}}' "$new") on air"
}

# heron-engine first, then the other stations' (sorted).
engines=$(compose config --services | while read -r service; do
    if is_engine "$service"; then echo "$service"; fi
done | LC_ALL=C sort)
[ -n "$engines" ] || fail "no heron-engine service"
for service in $engines; do
    hand_over "$service"
done
say "done"
