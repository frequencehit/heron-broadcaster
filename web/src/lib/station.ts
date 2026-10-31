// SPDX-License-Identifier: AGPL-3.0-or-later

// The station the pages show (docs/stations.md#web-ui). What belongs to a
// station is served under /api/v1/stations/{station_id}/…: the pages call
// those typed routes with `here()`, so a route the API does not serve per
// station does not compile.

import { useSyncExternalStore } from "react";
import type { Role, Schemas, User } from "../api";

export type StationInfo = Schemas["StationInfo"];
export type StationRoleName = Schemas["StationRoleName"];

/**
 * The path parameters of a station's route for the station the pages show:
 * `api.GET("/api/v1/stations/{station_id}/queue", { params: { path: here() } })`.
 * The pages only render once it is known.
 */
export function here(): { station_id: number };
export function here<P extends object>(path: P): P & { station_id: number };
export function here(path: object = {}): object {
  if (current === null) {
    throw new Error("no station chosen yet");
  }
  return { ...path, station_id: current };
}

/** The URL of a station route outside the typed client (WebSocket, downloads): `rest` under it. */
export function stationUrl(rest: string, id: number | null = current): string {
  return id === null ? `/api/v1/${rest}` : `/api/v1/stations/${id}/${rest}`;
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
