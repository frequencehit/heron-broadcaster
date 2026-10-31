// SPDX-License-Identifier: AGPL-3.0-or-later

// The live WebSocket (/api/v1/live): decks, queue, outputs, positions,
// levels, track changes and the deck EQ, kept in one store that components read slices
// of. Reconnects with a growing delay when the connection drops.

import { useSyncExternalStore } from "react";
import type { DeckPosition, EqSettings, Levels, LiveMessage, PlayerState, TrackEvent } from "../api";
import { stationUrl } from "./station";

export interface Positions {
  decks: DeckPosition[];
  /** `performance.now()` when they arrived, to move them on between messages. */
  receivedAt: number;
}

export interface LiveState {
  connected: boolean;
  /** `undefined` before the first message, `null` while the engine is unreachable. */
  player: PlayerState | null | undefined;
  positions: Positions | null;
  levels: Levels | null;
  /** The most recent track changes, newest first. */
  events: TrackEvent[];
  /** The deck EQ as last changed while connected, `null` before. */
  eq: EqSettings | null;
}

const MAX_EVENTS = 20;
const MAX_DELAY_MS = 10_000;

const EMPTY: LiveState = {
  connected: false,
  player: undefined,
  positions: null,
  levels: null,
  events: [],
  eq: null,
};
let state: LiveState = EMPTY;
const listeners = new Set<() => void>();
let socket: WebSocket | null = null;
let retry: ReturnType<typeof setTimeout> | null = null;
let delay = 500;
let wanted = false;

function set(changes: Partial<LiveState>) {
  state = { ...state, ...changes };
  for (const listener of listeners) {
    listener();
  }
}

function handle(message: LiveMessage) {
  switch (message.type) {
    case "state":
      set({ player: message.player ?? null });
      break;
    case "positions":
      set({ positions: { decks: message.decks, receivedAt: performance.now() } });
      break;
    case "levels":
      set({ levels: message.levels });
      break;
    case "track":
      set({ events: [message.event, ...state.events].slice(0, MAX_EVENTS) });
      break;
    case "eq":
      set({ eq: message.settings });
      break;
  }
}

function connect() {
  if (!wanted || socket) {
    return;
  }
  const scheme = window.location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${scheme}://${window.location.host}${stationUrl("live")}`);
  socket = ws;
  // A socket closed by stopLive (another station, or signed out) may still
  // deliver a message or its close: they are not this connection's.
  ws.onopen = () => {
    if (socket !== ws) {
      return;
    }
    delay = 500;
    set({ connected: true });
  };
  ws.onmessage = (event) => {
    if (socket !== ws) {
      return;
    }
    try {
      handle(JSON.parse(String(event.data)) as LiveMessage);
    } catch {
      // A message this version does not know: ignored.
    }
  };
  ws.onclose = () => {
    if (socket !== ws) {
      return;
    }
    socket = null;
    // Changes made while disconnected are not known: read them again.
    set({ connected: false, levels: null, eq: null });
    if (wanted) {
      retry = setTimeout(connect, delay);
      delay = Math.min(delay * 2, MAX_DELAY_MS);
    }
  };
}

/** Opens the connection (once signed in). */
export function startLive() {
  wanted = true;
  connect();
}

/** Closes it (signed out). */
export function stopLive() {
  wanted = false;
  if (retry) {
    clearTimeout(retry);
    retry = null;
  }
  const closing = socket;
  socket = null;
  closing?.close();
  // The pages show nothing of the last station until the next one answers.
  set(EMPTY);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The live state now, outside React. */
export function liveState(): LiveState {
  return state;
}

/** A slice of the live state; re-renders only when it changes. */
export function useLive<T>(select: (state: LiveState) => T): T {
  return useSyncExternalStore(subscribe, () => select(state));
}

/**
 * Where a deck is now, moved on from the last positions message unless
 * the player is paused.
 */
export function positionNow(
  positions: Positions | null,
  deck: string,
  paused: boolean,
  now = performance.now(),
): { position_ms: number; length_ms: number } | null {
  const found = positions?.decks.find((d) => d.deck === deck);
  if (!positions || !found) {
    return null;
  }
  const elapsed = paused ? 0 : Math.max(0, now - positions.receivedAt);
  return {
    position_ms: Math.min(found.position_ms + elapsed, found.length_ms),
    length_ms: found.length_ms,
  };
}
