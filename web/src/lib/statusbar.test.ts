// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import type { DeckState } from "../api";
import {
  END_SOON_MS,
  countdownOf,
  countdownText,
  levelOf,
  listenersText,
  recordOf,
  summarizeDeck,
  summarizeSystem,
} from "./statusbar";

const deck = (over: Partial<DeckState> = {}, title = "Space Cowboy"): DeckState =>
  ({
    deck: "a",
    on_air: true,
    position_ms: 151_000,
    length_ms: 240_000,
    entry: { entry_id: 1, track: { artist: "Jamiroquai", title, path: "a/b.mp3" } },
    ...over,
  }) as DeckState;

describe("summarizeDeck", () => {
  it("shows the track and how far it has played", () => {
    expect(summarizeDeck("A", deck(), false, null)).toEqual({
      letter: "A",
      status: "playing",
      symbol: "▶",
      text: "Jamiroquai – Space Cowboy",
      time: "2:31",
      countdown: null,
      onAir: true,
      badge: "on_air",
      progress: 151_000 / 240_000,
      intro: null,
    });
  });

  it("keeps the progress between 0 and 1, and has none without a length", () => {
    expect(summarizeDeck("A", deck(), false, 300_000).progress).toBe(1);
    expect(summarizeDeck("A", deck(), false, -5).progress).toBe(0);
    expect(summarizeDeck("A", deck({ length_ms: 0 }), false, null).progress).toBeNull();
  });

  it("moves on from the position it is given", () => {
    expect(summarizeDeck("A", deck(), false, 65_000).time).toBe("1:05");
  });

  it("says paused for a deck the player has paused", () => {
    const summary = summarizeDeck("B", deck({ on_air: false }), true, null);
    expect([summary.status, summary.symbol, summary.onAir]).toEqual(["paused", "⏸", false]);
  });

  it("marks the deck listeners hear on air, and the other loaded one next", () => {
    expect(summarizeDeck("A", deck(), false, null).badge).toBe("on_air");
    expect(summarizeDeck("B", deck({ on_air: false, position_ms: 0 }), false, null).badge).toBe("next");
  });

  it("is empty without a track, or when the player does not list the deck", () => {
    const empty = {
      letter: "B",
      status: "empty",
      symbol: "—",
      text: "",
      time: "",
      countdown: null,
      onAir: false,
      badge: null,
      progress: null,
      intro: null,
    };
    expect(summarizeDeck("B", deck({ entry: null }), false, null)).toEqual(empty);
    expect(summarizeDeck("B", undefined, false, null)).toEqual(empty);
  });

  it("falls back on the file name without tags", () => {
    const untagged = deck({ entry: { entry_id: 2, track: { path: "incoming/jingle.mp3" } } as NonNullable<DeckState["entry"]> });
    expect(summarizeDeck("A", untagged, false, 0).text).toBe("jingle.mp3");
  });
});

describe("the intro and the countdowns", () => {
  const withIntro = (over: Partial<DeckState> = {}, intro: number | null = 21_000) =>
    deck({
      entry: {
        entry_id: 3,
        next_start_ms: 236_000,
        track: { artist: "Daft Punk", title: "One More Time", path: "x.mp3", cue_in_ms: 1_000, cue_out_ms: 241_000, intro_ms: intro },
      } as NonNullable<DeckState["entry"]>,
      ...over,
    });

  it("places the vocals from the cue-in, on both decks", () => {
    expect(summarizeDeck("A", withIntro(), false, 0).intro).toBeCloseTo(20_000 / 240_000);
    expect(summarizeDeck("B", withIntro({ on_air: false }), false, 0).intro).toBeCloseTo(20_000 / 240_000);
  });

  it("has no intro without one, before the cue-in or without a length", () => {
    expect(summarizeDeck("A", withIntro({}, null), false, 0).intro).toBeNull();
    expect(summarizeDeck("A", withIntro({}, 500), false, 0).intro).toBeNull();
    expect(summarizeDeck("A", withIntro({ length_ms: 0 }), false, 0).intro).toBeNull();
  });

  it("counts down on the deck on air only", () => {
    expect(summarizeDeck("A", withIntro(), false, 12_500).countdown).toEqual({ kind: "intro", seconds: 8, urgent: false });
    expect(summarizeDeck("B", withIntro({ on_air: false }), false, 0).countdown).toBeNull();
  });

  it("counts to the engine's transition, else to the end of the track", () => {
    expect(summarizeDeck("A", withIntro(), false, 230_000).countdown).toEqual({ kind: "end", seconds: 6, urgent: true });
    expect(summarizeDeck("A", deck(), false, 235_000).countdown).toEqual({ kind: "end", seconds: 5, urgent: true });
  });

  it("turns urgent in the intro's last 5 seconds and rounds up", () => {
    expect(countdownOf(20_000, 236_000, 14_999)).toEqual({ kind: "intro", seconds: 6, urgent: false });
    expect(countdownOf(20_000, 236_000, 15_000)).toEqual({ kind: "intro", seconds: 5, urgent: true });
    expect(countdownOf(20_000, 236_000, 19_999)).toEqual({ kind: "intro", seconds: 1, urgent: true });
    expect(countdownOf(20_000, 236_000, 20_000)).toBeNull();
  });

  it("shows the end in its last 10 seconds only, and not past it", () => {
    expect(countdownOf(null, 236_000, 236_000 - END_SOON_MS - 1)).toBeNull();
    expect(countdownOf(null, 236_000, 236_000 - END_SOON_MS)).toEqual({ kind: "end", seconds: 10, urgent: true });
    expect(countdownOf(null, 236_000, 236_000)).toBeNull();
    expect(countdownOf(null, 0, 0)).toBeNull();
  });

  it("puts the intro first when both would count", () => {
    expect(countdownOf(9_000, 10_000, 1_000)?.kind).toBe("intro");
  });

  it("writes the time left with a minus sign", () => {
    expect(countdownText({ kind: "intro", seconds: 7, urgent: false })).toBe("−0:07");
  });
});

describe("listenersText", () => {
  it("is a gap, never zero, when no server could be read", () => {
    expect(listenersText(null)).toBe("–");
    expect(listenersText(undefined)).toBe("–");
    expect(listenersText(0)).toBe("0");
    expect(listenersText(1234)).toMatch(/^1.234$|^1,234$|^1 234$|^1 234$/);
  });
});

describe("levelOf", () => {
  it("warns from 80 % and alerts from 95 %", () => {
    expect([0, 79.9, 80, 94.9, 95, 100].map(levelOf)).toEqual(["ok", "ok", "warn", "warn", "bad", "bad"]);
  });
});

describe("summarizeSystem", () => {
  it("rounds the percentages and writes the load average with one decimal", () => {
    const summary = summarizeSystem({ cpu_percent: 12.4, mem_percent: 40.5, load_avg: [0.4, 0.55, 1], cores: 4 });
    expect(summary.cpu).toBe("12%");
    expect(summary.mem).toBe("41%");
    expect(summary.load).toMatch(/^0[.,]4 0[.,][56] 1[.,]0$/);
    expect([summary.cpuLevel, summary.memLevel, summary.loadLevel]).toEqual(["ok", "ok", "ok"]);
  });

  it("reads the load against the cores", () => {
    const busy = (cores: number) =>
      summarizeSystem({ cpu_percent: 96, mem_percent: 85, load_avg: [3.4, 2, 1], cores });
    expect(busy(4).loadLevel).toBe("warn");
    expect(busy(2).loadLevel).toBe("bad");
    expect(busy(16).loadLevel).toBe("ok");
    expect(busy(0).loadLevel).toBe("bad");
    expect([busy(4).cpuLevel, busy(4).memLevel]).toEqual(["bad", "warn"]);
  });
});

describe("recordOf", () => {
  it("gives the record's listeners and its day, and nothing before any", () => {
    expect(recordOf({ listeners: 1280, day: "2026-10-25" })).toEqual({
      listeners: (1280).toLocaleString("en"),
      day: "Oct 25, 2026",
    });
    expect(recordOf(null)).toBeNull();
    expect(recordOf(undefined)).toBeNull();
  });
});
