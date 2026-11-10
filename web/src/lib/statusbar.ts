// SPDX-License-Identifier: AGPL-3.0-or-later

// What the status bar says (docs/web.md#status-bar): the decks, the
// listeners and their record, the next fixed time, the queue, the outputs
// and the machine's load, as short texts, following each user's settings.

import type { DeckState, OutputStatus, QueueEntry, Schemas, StationSummary, SystemLoad } from "../api";
import { locale, number } from "../i18n";
import { duration, timeZone, trackName } from "./format";
import { introFromCueIn } from "./intro";

export type StatusBarSettings = Schemas["StatusBarSettings"];
export type DeckTime = Schemas["DeckTime"];

/** A user's settings before they choose any: as the API fills them in. */
export const DEFAULT_STATUS_BAR: StatusBarSettings = {
  show: true,
  decks: true,
  on_air_only: false,
  history: false,
  history_type_id: 1,
  listeners: true,
  next_fixed: true,
  queue: false,
  outputs: false,
  system: true,
  station_time: false,
  time: "elapsed",
  intro_countdown: true,
  intro_blink: true,
  intro_seconds: 5,
  end_countdown: true,
  end_blink: false,
  end_seconds: 10,
  intro_color: null,
  track_color: null,
  hidden_stations: [],
  station_trouble: true,
};

/** The formats of the time on the decks, in the order the settings list them. */
export const DECK_TIMES: DeckTime[] = [
  "elapsed",
  "elapsed_total",
  "elapsed_remaining",
  "elapsed_remaining_total",
  "elapsed_total_remaining",
  "remaining",
  "remaining_total",
];

/** The time on a deck at `position` of a track that hands over at `total`: `1:23 / −3:49 / 5:12`. */
export function deckTime(format: DeckTime, position: number, total: number): string {
  const elapsed = duration(position);
  const all = duration(total);
  const left = `−${duration(Math.max(0, total - position))}`;
  const parts: Record<DeckTime, string[]> = {
    elapsed: [elapsed],
    elapsed_total: [elapsed, all],
    elapsed_remaining: [elapsed, left],
    elapsed_remaining_total: [elapsed, left, all],
    elapsed_total_remaining: [elapsed, all, left],
    remaining: [left],
    remaining_total: [left, all],
  };
  return parts[format].join(" / ");
}

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

/**
 * What the deck on air counts down to, in place of the time: the vocals
 * during the intro (`urgent` in its last seconds), or the next track in its
 * last seconds (always urgent).
 */
export interface Countdown {
  kind: "intro" | "end";
  /** Whole seconds left, rounded up: `−0:01` until it is reached. */
  seconds: number;
  urgent: boolean;
  /** Urgent and set to blink. */
  blink: boolean;
}

/** Which countdowns run, and for how many of their last seconds they are urgent. */
export type CountdownSettings = Pick<
  StatusBarSettings,
  "intro_countdown" | "intro_blink" | "intro_seconds" | "end_countdown" | "end_blink" | "end_seconds"
>;

export interface DeckSummary {
  /** `A` or `B`. */
  letter: string;
  status: DeckStatus;
  /** `▶`, `⏸` or `—`. */
  symbol: string;
  /** The track; empty for an empty deck. */
  text: string;
  /** The time, in the format chosen; empty for an empty deck. */
  time: string;
  /** On the deck on air only; `null` when nothing counts down. */
  countdown: Countdown | null;
  onAir: boolean;
  /** `null` for an empty deck. */
  badge: DeckBadge | null;
  /** How far the track has played, from 0 to 1; `null` for an empty deck or an unknown length. */
  progress: number | null;
  /** Where the vocals start, from 0 to 1 of the track; `null` without an intro or a length, or once they have started. */
  intro: number | null;
}

const SYMBOLS: Record<DeckStatus, string> = { playing: "▶", paused: "⏸", empty: "—" };

/** One of the two decks; `deck` is missing when the player does not list it. */
export function summarizeDeck(
  letter: "A" | "B",
  deck: DeckState | undefined,
  paused: boolean,
  positionMs: number | null,
  settings: Pick<StatusBarSettings, "time"> & CountdownSettings = DEFAULT_STATUS_BAR,
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
  // Past the vocals, one colour: the track's.
  const intro = vocals !== null && length > 0 && position < vocals ? Math.min(vocals / length, 1) : null;
  const nextStart = deck.entry?.next_start_ms ?? length;
  return {
    letter,
    status,
    symbol: SYMBOLS[status],
    text: trackName(track),
    time: deckTime(settings.time, position, nextStart),
    countdown: deck.on_air ? countdownOf(vocals, nextStart, position, settings) : null,
    onAir: deck.on_air,
    badge: deck.on_air ? "on_air" : "next",
    progress,
    intro,
  };
}

/**
 * What the deck on air counts down to at `position` (from the cue-in), as
 * `settings` say: the vocals at `vocals` while they have not started, else
 * the next track at `nextStart` in its last seconds.
 */
export function countdownOf(
  vocals: number | null,
  nextStart: number,
  position: number,
  settings: CountdownSettings = DEFAULT_STATUS_BAR,
): Countdown | null {
  const seconds = (ms: number) => Math.ceil(ms / 1000);
  if (settings.intro_countdown && vocals !== null && position < vocals) {
    const left = vocals - position;
    const urgent = left <= settings.intro_seconds * 1000;
    return { kind: "intro", seconds: seconds(left), urgent, blink: urgent && settings.intro_blink };
  }
  const left = nextStart - position;
  if (settings.end_countdown && nextStart > 0 && left > 0 && left <= settings.end_seconds * 1000) {
    return { kind: "end", seconds: seconds(left), urgent: true, blink: settings.end_blink };
  }
  return null;
}

/** How long the waiting tracks last, crossfades included: each hands over at its next start. */
export function queueLength(queue: QueueEntry[]): number {
  return queue.reduce(
    (total, entry) => total + (entry.next_start_ms ?? entry.track.cue_out_ms - entry.track.cue_in_ms),
    0,
  );
}

/** The enabled outputs connected, of all those enabled, and the names of the others. */
export function outputsOf(outputs: OutputStatus[]): { connected: number; total: number; down: string[] } {
  const enabled = outputs.filter((output) => output.enabled);
  const down = enabled.filter((output) => output.state !== "connected").map((output) => output.name);
  return { connected: enabled.length - down.length, total: enabled.length, down };
}

/** Something at a fixed time: a show opening (or open), a top's show, a track at an exact time. */
export interface FixedTime {
  name: string;
  /** When it opens or starts; while open, when it closes. */
  at: number;
  open: boolean;
  /** The page a click opens. */
  link: string;
}

type ShowView = Schemas["ShowView"];
type TopView = Schemas["TopView"];

/**
 * The next fixed time, `now` in Unix ms: a show open now first (the one
 * closing first), else the first to come of the shows (on and enabled),
 * the tops and the tracks at an exact time.
 */
export function nextFixedTime(
  shows: readonly ShowView[],
  tops: readonly TopView[],
  queue: readonly QueueEntry[],
  now: number,
  showName: (show: ShowView) => string,
  trackLabel: (entry: QueueEntry) => string,
): FixedTime | null {
  const candidates: FixedTime[] = [];
  const add = (name: string, status: Schemas["ShowStatus"], link: string) => {
    if (status.open && status.closes_at_unix_ms != null) {
      candidates.push({ name, at: status.closes_at_unix_ms, open: true, link });
    } else if (status.next_opening_unix_ms != null && status.next_opening_unix_ms > now) {
      candidates.push({ name, at: status.next_opening_unix_ms, open: false, link });
    }
  };
  for (const show of shows) {
    if (show.module !== "likes" && show.settings.enabled) {
      add(showName(show), show.status, `/shows/${show.module}`);
    }
  }
  for (const top of tops) {
    if (top.settings.enabled) {
      add(top.settings.name, top.status, `/shows/top30/${top.id}`);
    }
  }
  for (const entry of queue) {
    const at = entry.track.start_at_unix_ms;
    if (at != null && at > now) {
      candidates.push({ name: trackLabel(entry), at, open: false, link: "/queue" });
    }
  }
  const open = candidates.filter((candidate) => candidate.open).sort((a, b) => a.at - b.at);
  const coming = candidates.filter((candidate) => !candidate.open).sort((a, b) => a.at - b.at);
  return open[0] ?? coming[0] ?? null;
}

/** When a fixed time comes, as the bar says it: in minutes within the hour, at a time today, else with the day. */
export function whenOf(at: number, now: number): { kind: "in"; minutes: number } | { kind: "at" | "day"; day: string } {
  const minutes = Math.ceil((at - now) / 60_000);
  if (minutes < 60) {
    return { kind: "in", minutes: Math.max(1, minutes) };
  }
  const zone = timeZone();
  const date = (unixMs: number) =>
    new Intl.DateTimeFormat("en-CA", { timeZone: zone, year: "numeric", month: "2-digit", day: "2-digit" }).format(unixMs);
  if (date(at) === date(now)) {
    return { kind: "at", day: "" };
  }
  return { kind: "day", day: new Intl.DateTimeFormat(locale(), { weekday: "short", timeZone: zone }).format(at) };
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

/** Why another station is in trouble; `null` when it is not. */
export type Trouble = "engine" | "nothing" | "outputs";

export function troubleOf(station: StationSummary): Trouble | null {
  if (!station.reachable) {
    return "engine";
  }
  if (!station.on_air) {
    return "nothing";
  }
  return station.outputs_down.length > 0 ? "outputs" : null;
}

/** Another station as its bar shows it: the deck on air and the time left, at `elapsed` ms after the message. */
export interface OtherStation {
  id: number;
  code: string;
  trouble: Trouble | null;
  outputsDown: string[];
  onAir: { letter: "A" | "B"; text: string; left: number; progress: number; paused: boolean } | null;
}

/**
 * The other stations the station button opens, as the settings say (the
 * hidden ones left out, unless in trouble and `station_trouble`), and how
 * many of those it counts are in trouble.
 */
export function otherStations(
  stations: readonly StationSummary[],
  settings: Pick<StatusBarSettings, "hidden_stations" | "station_trouble">,
  elapsed: number,
): { shown: OtherStation[]; inTrouble: number } {
  const shown: OtherStation[] = [];
  for (const station of stations) {
    const trouble = troubleOf(station);
    const hidden = settings.hidden_stations.includes(station.id);
    if (hidden && !(trouble && settings.station_trouble)) {
      continue;
    }
    const deck = station.on_air;
    let onAir: OtherStation["onAir"] = null;
    if (deck) {
      const position = Math.min(deck.position_ms + (station.paused ? 0 : elapsed), deck.next_start_ms);
      onAir = {
        letter: deck.deck === "b" ? "B" : "A",
        text: trackName(deck),
        left: Math.max(0, deck.next_start_ms - position),
        progress: deck.next_start_ms > 0 ? Math.min(1, position / deck.next_start_ms) : 0,
        paused: station.paused,
      };
    }
    shown.push({ id: station.id, code: station.code, trouble, outputsDown: station.outputs_down, onAir });
  }
  return { shown, inTrouble: shown.filter((station) => station.trouble).length };
}
