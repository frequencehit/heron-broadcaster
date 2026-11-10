// SPDX-License-Identifier: AGPL-3.0-or-later

// What the status bar says (docs/web.md#status-bar): the decks, the
// listeners and their record, and the machine's load, as short texts.

import type { DeckState, Schemas, SystemLoad } from "../api";
import { locale, number } from "../i18n";
import { duration, trackName } from "./format";
import { INTRO_SOON_MS, introFromCueIn } from "./intro";

export type Level = "ok" | "warn" | "bad";

/** From where the CPU and memory turn to the warning and the alert colour. */
export const WARN_PERCENT = 80;
export const BAD_PERCENT = 95;

export function levelOf(percent: number): Level {
  if (percent >= BAD_PERCENT) {
    return "bad";
  }
  return percent >= WARN_PERCENT ? "warn" : "ok";
}

export type DeckStatus = "playing" | "paused" | "empty";

/** `on_air` for the deck listeners hear, `next` for the other one with a track loaded. */
export type DeckBadge = "on_air" | "next";

/** From how long before the next track starts its countdown shows. */
export const END_SOON_MS = 10_000;

/**
 * What the deck on air counts down to, in place of the elapsed time: the
 * vocals during the intro (`urgent` in its last seconds), or the next track
 * in its last seconds.
 */
export interface Countdown {
  kind: "intro" | "end";
  /** Whole seconds left, rounded up: `−0:01` until it is reached. */
  seconds: number;
  urgent: boolean;
}

export interface DeckSummary {
  /** `A` or `B`. */
  letter: string;
  status: DeckStatus;
  /** `▶`, `⏸` or `—`. */
  symbol: string;
  /** The track; empty for an empty deck. */
  text: string;
  /** The elapsed time; empty for an empty deck. */
  time: string;
  /** On the deck on air only; `null` when nothing counts down. */
  countdown: Countdown | null;
  onAir: boolean;
  /** `null` for an empty deck. */
  badge: DeckBadge | null;
  /** How far the track has played, from 0 to 1; `null` for an empty deck or an unknown length. */
  progress: number | null;
  /** Where the vocals start, from 0 to 1 of the track; `null` without an intro or a length. */
  intro: number | null;
}

const SYMBOLS: Record<DeckStatus, string> = { playing: "▶", paused: "⏸", empty: "—" };

/** One of the two decks; `deck` is missing when the player does not list it. */
export function summarizeDeck(
  letter: "A" | "B",
  deck: DeckState | undefined,
  paused: boolean,
  positionMs: number | null,
): DeckSummary {
  const track = deck?.entry?.track;
  if (!deck || !track) {
    return {
      letter,
      status: "empty",
      symbol: SYMBOLS.empty,
      text: "",
      time: "",
      countdown: null,
      onAir: false,
      badge: null,
      progress: null,
      intro: null,
    };
  }
  const status: DeckStatus = paused ? "paused" : "playing";
  const position = positionMs ?? deck.position_ms;
  const length = deck.length_ms;
  const progress = length > 0 ? Math.min(Math.max(position / length, 0), 1) : null;
  const vocals = introFromCueIn(track);
  const intro = vocals !== null && length > 0 ? Math.min(vocals / length, 1) : null;
  return {
    letter,
    status,
    symbol: SYMBOLS[status],
    text: trackName(track),
    time: duration(position),
    countdown: deck.on_air ? countdownOf(vocals, deck.entry?.next_start_ms ?? length, position) : null,
    onAir: deck.on_air,
    badge: deck.on_air ? "on_air" : "next",
    progress,
    intro,
  };
}

/**
 * What the deck on air counts down to at `position` (from the cue-in): the
 * vocals at `vocals` while they have not started, else the next track at
 * `nextStart` in its last seconds.
 */
export function countdownOf(vocals: number | null, nextStart: number, position: number): Countdown | null {
  const seconds = (ms: number) => Math.ceil(ms / 1000);
  if (vocals !== null && position < vocals) {
    const left = vocals - position;
    return { kind: "intro", seconds: seconds(left), urgent: left <= INTRO_SOON_MS };
  }
  const left = nextStart - position;
  if (nextStart > 0 && left > 0 && left <= END_SOON_MS) {
    return { kind: "end", seconds: seconds(left), urgent: true };
  }
  return null;
}

/** A countdown as the bar writes it: `−0:07`. */
export function countdownText(countdown: Countdown): string {
  return `−${duration(countdown.seconds * 1000)}`;
}

/** The listeners' total; a gap (`–`) when no server could be read, never 0. */
export function listenersText(total: number | null | undefined): string {
  return total === null || total === undefined ? "–" : number(total);
}

/** The station's record: its listeners, and its day for people (`25 Oct 2026`); `null` before any. */
export function recordOf(
  record: Schemas["ListenersRecord"] | null | undefined,
): { listeners: string; day: string } | null {
  if (!record) {
    return null;
  }
  // The day is a station day already: shown as it is, not moved to another zone.
  const parsed = new Date(`${record.day}T00:00:00Z`);
  const day = Number.isNaN(parsed.getTime())
    ? record.day
    : new Intl.DateTimeFormat(locale(), { dateStyle: "medium", timeZone: "UTC" }).format(parsed);
  return { listeners: number(record.listeners), day };
}

export interface SystemSummary {
  cpu: string;
  mem: string;
  load: string;
  cpuLevel: Level;
  memLevel: Level;
  /** Above 1 per core, the machine has more to do than it can. */
  loadLevel: Level;
}

function load1(value: number): string {
  return number(value, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
}

export function summarizeSystem(system: SystemLoad): SystemSummary {
  const cores = Math.max(system.cores, 1);
  const perCore = (system.load_avg[0] ?? 0) / cores;
  return {
    cpu: `${Math.round(system.cpu_percent)}%`,
    mem: `${Math.round(system.mem_percent)}%`,
    load: system.load_avg.map(load1).join(" "),
    cpuLevel: levelOf(system.cpu_percent),
    memLevel: levelOf(system.mem_percent),
    loadLevel: levelOf(perCore * 100),
  };
}
