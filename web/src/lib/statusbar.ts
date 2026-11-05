// SPDX-License-Identifier: AGPL-3.0-or-later

// What the status bar says (docs/web.md#status-bar): the decks, the
// listeners and the machine's load, as short texts.

import type { DeckState, SystemLoad } from "../api";
import { number } from "../i18n";
import { duration, trackName } from "./format";

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

export interface DeckSummary {
  /** `A` or `B`. */
  letter: string;
  status: DeckStatus;
  /** `▶`, `⏸` or `—`. */
  symbol: string;
  /** The track, with its elapsed time while one is loaded; empty for an empty deck. */
  text: string;
  onAir: boolean;
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
    return { letter, status: "empty", symbol: SYMBOLS.empty, text: "", onAir: false };
  }
  const status: DeckStatus = paused ? "paused" : "playing";
  const elapsed = duration(positionMs ?? deck.position_ms);
  return {
    letter,
    status,
    symbol: SYMBOLS[status],
    text: `${trackName(track)} ${elapsed}`,
    onAir: deck.on_air,
  };
}

/** The listeners' total; a gap (`–`) when no server could be read, never 0. */
export function listenersText(total: number | null | undefined): string {
  return total === null || total === undefined ? "–" : number(total);
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
