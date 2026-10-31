// SPDX-License-Identifier: AGPL-3.0-or-later

// The station the pages show (docs/stations.md#web-ui). What belongs to a
// station is served under /api/v1/stations/{id}/…: requests the pages make
// to the single-station routes (/api/v1/queue…) are sent there for the
// chosen station, so the pages keep their typed routes.

import { useSyncExternalStore } from "react";
import type { Role, Schemas, User } from "../api";

export type StationInfo = Schemas["StationInfo"];
export type StationRoleName = Schemas["StationRoleName"];

/** The first path segments, under /api/v1, of what is per station (the API's list). */
const PER_STATION = [
  "engine",
  "live",
  "playout",
  "player",
  "queue",
  "history",
  "clocks",
  "schedule",
  "schedules",
  "rotation",
  "outputs",
  "webhooks",
  "webhook-deliveries",
  "listeners",
];
/** The settings groups each station has its own of. */
const STATION_SETTINGS = ["playout", "eq", "queue", "rotation", "station", "medleys"];

/** Whether a path under /api/v1 (`queue/12`) is one of a station's. */
export function perStation(rest: string): boolean {
  const [first, second] = rest.split("/");
  if (first === "settings") {
    return second !== undefined && STATION_SETTINGS.includes(second.split("?")[0] ?? "");
  }
  return PER_STATION.includes((first ?? "").split("?")[0] ?? "");
}

/** `url` for station `id` when it is one of a station's routes; unchanged otherwise. */
export function onStation(url: string, id: number | null): string {
  if (id === null) {
    return url;
  }
  const match = /^(.*?\/api\/v1\/)(.*)$/.exec(url);
  if (!match) {
    return url;
  }
  const [, prefix, rest] = match;
  if (rest === undefined || rest.startsWith("stations/") || !perStation(rest)) {
    return url;
  }
  return `${prefix}stations/${id}/${rest}`;
}

/**
 * `request` sent to station `id`'s route, with its body; `undefined` when it
 * is not one of a station's routes. `new Request(url, request)` would lose
 * the body: browsers refuse a stream body, or send none.
 */
export async function toStation(request: Request, id: number | null): Promise<Request | undefined> {
  const url = onStation(request.url, id);
  if (url === request.url) {
    return undefined;
  }
  const body = request.method === "GET" || request.method === "HEAD" ? null : await request.arrayBuffer();
  return new Request(url, {
    method: request.method,
    headers: request.headers,
    body,
    credentials: request.credentials,
    cache: request.cache,
    redirect: request.redirect,
    referrer: request.referrer,
    signal: request.signal,
  });
}

// The chosen station, kept per user in the browser.
let current: number | null = null;
let userId: number | null = null;
const listeners = new Set<() => void>();

function key(user: number): string {
  return `heron.station.${user}`;
}

/** The station the pages show; `null` before the stations are known. */
export function currentStation(): number | null {
  return current;
}

/** Shows `id`'s pages, and keeps the choice for the user. */
export function chooseStation(id: number | null) {
  if (id === current) {
    return;
  }
  current = id;
  if (userId !== null && id !== null) {
    try {
      localStorage.setItem(key(userId), String(id));
    } catch {
      // Storage unavailable: the choice lasts until the page reloads.
    }
  }
  for (const listener of listeners) {
    listener();
  }
}

/** The station to show among `stations`: the one chosen last, or the first. */
export function initialStation(user: number, stations: StationInfo[]): number | null {
  userId = user;
  let stored: number | null = null;
  try {
    stored = Number(localStorage.getItem(key(user))) || null;
  } catch {
    stored = null;
  }
  const ids = stations.map((station) => station.id);
  if (stored !== null && ids.includes(stored)) {
    return stored;
  }
  return firstStation(stations)?.id ?? null;
}

/** The first station: the one an existing Heron became (the lowest ID). */
export function firstStation(stations: StationInfo[]): StationInfo | undefined {
  return [...stations].sort((a, b) => a.id - b.id)[0];
}

export function useStation(): number | null {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => current,
  );
}

/** The user's role on a station: a shared admin is admin on every one. */
export function stationRole(user: User, stationId: number | null): Role | undefined {
  if (user.role === "admin") {
    return "admin";
  }
  if (stationId === null) {
    return user.role;
  }
  const role = user.station_roles.find((r) => r.station_id === stationId)?.role;
  return role === undefined || role === "none" ? undefined : role;
}

/** Stations' codes, in their order, for badges. */
export function codes(ids: number[], stations: StationInfo[]): string[] {
  return stations.filter((station) => ids.includes(station.id)).map((station) => station.code);
}
