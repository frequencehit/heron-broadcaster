// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import type { DeckState, QueueEntry, Schemas } from "../api";
import {
  DEFAULT_STATUS_BAR,
  countdownOf,
  countdownText,
  deckTime,
  levelOf,
  listenersText,
  nextFixedTime,
  outputsOf,
  queueLength,
  recordOf,
  summarizeDeck,
  summarizeSystem,
  whenOf,
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
    expect(summarizeDeck("A", withIntro(), false, 12_500).countdown).toEqual({
      kind: "intro",
      seconds: 8,
      urgent: false,
      blink: false,
    });
    expect(summarizeDeck("B", withIntro({ on_air: false }), false, 0).countdown).toBeNull();
  });

  it("counts to the engine's transition, else to the end of the track", () => {
    expect(summarizeDeck("A", withIntro(), false, 230_000).countdown).toEqual({
      kind: "end",
      seconds: 6,
      urgent: true,
      blink: false,
    });
    expect(summarizeDeck("A", deck(), false, 235_000).countdown?.seconds).toBe(5);
  });

  it("turns urgent in the intro's last 5 seconds by default, blinking, and rounds up", () => {
    expect(countdownOf(20_000, 236_000, 14_999)).toEqual({ kind: "intro", seconds: 6, urgent: false, blink: false });
    expect(countdownOf(20_000, 236_000, 15_000)).toEqual({ kind: "intro", seconds: 5, urgent: true, blink: true });
    expect(countdownOf(20_000, 236_000, 19_999)).toEqual({ kind: "intro", seconds: 1, urgent: true, blink: true });
    expect(countdownOf(20_000, 236_000, 20_000)).toBeNull();
  });

  it("shows the end in its last 10 seconds only by default, without blinking, and not past it", () => {
    expect(countdownOf(null, 236_000, 226_000 - 1)).toBeNull();
    expect(countdownOf(null, 236_000, 226_000)).toEqual({ kind: "end", seconds: 10, urgent: true, blink: false });
    expect(countdownOf(null, 236_000, 236_000)).toBeNull();
    expect(countdownOf(null, 0, 0)).toBeNull();
  });

  it("follows the user's seconds, blinking and switches", () => {
    const settings = { ...DEFAULT_STATUS_BAR, intro_seconds: 8, intro_blink: false, end_seconds: 3, end_blink: true };
    expect(countdownOf(20_000, 236_000, 12_000, settings)).toEqual({ kind: "intro", seconds: 8, urgent: true, blink: false });
    expect(countdownOf(null, 236_000, 232_000, settings)).toBeNull();
    expect(countdownOf(null, 236_000, 233_000, settings)).toEqual({ kind: "end", seconds: 3, urgent: true, blink: true });
    const off = { ...DEFAULT_STATUS_BAR, intro_countdown: false, end_countdown: false };
    expect(countdownOf(20_000, 236_000, 1_000, off)).toBeNull();
    expect(countdownOf(20_000, 236_000, 235_000, off)).toBeNull();
    // The intro off, the end still counts.
    expect(countdownOf(20_000, 236_000, 230_000, { ...DEFAULT_STATUS_BAR, intro_countdown: false })?.kind).toBe("end");
  });

  it("has one colour past the vocals: no intro zone any more", () => {
    expect(summarizeDeck("A", withIntro(), false, 19_999).intro).not.toBeNull();
    expect(summarizeDeck("A", withIntro(), false, 20_000).intro).toBeNull();
  });

  it("writes the time in the format chosen, counting to the next track's start", () => {
    const time = (format: Schemas["DeckTime"]) =>
      summarizeDeck("A", withIntro(), false, 83_000, { ...DEFAULT_STATUS_BAR, time: format }).time;
    expect(time("elapsed")).toBe("1:23");
    expect(time("remaining_total")).toBe("−2:33 / 3:56");
    expect(deckTime("elapsed_total", 83_000, 312_000)).toBe("1:23 / 5:12");
    expect(deckTime("elapsed_remaining", 83_000, 312_000)).toBe("1:23 / −3:49");
    expect(deckTime("elapsed_remaining_total", 83_000, 312_000)).toBe("1:23 / −3:49 / 5:12");
    expect(deckTime("elapsed_total_remaining", 83_000, 312_000)).toBe("1:23 / 5:12 / −3:49");
    expect(deckTime("remaining", 83_000, 312_000)).toBe("−3:49");
    expect(deckTime("remaining", 400_000, 312_000)).toBe("−0:00");
  });

  it("puts the intro first when both would count", () => {
    expect(countdownOf(9_000, 10_000, 1_000)?.kind).toBe("intro");
  });

  it("writes the time left with a minus sign", () => {
    expect(countdownText({ kind: "intro", seconds: 7, urgent: false, blink: false })).toBe("−0:07");
  });
});

describe("the queue and the outputs", () => {
  const entry = (cueIn: number, cueOut: number, next: number | null = null) =>
    ({ entry_id: 1, next_start_ms: next, track: { path: "x.mp3", cue_in_ms: cueIn, cue_out_ms: cueOut } }) as QueueEntry;

  it("adds up the waiting tracks, each handing over at its next start", () => {
    expect(queueLength([])).toBe(0);
    expect(queueLength([entry(1_000, 181_000), entry(0, 200_000, 195_000)])).toBe(375_000);
  });

  it("counts the enabled outputs connected, and names the others", () => {
    const output = (name: string, enabled: boolean, state: Schemas["OutputConnection"]) =>
      ({ id: 1, name, enabled, state, bytes_sent: 0, dropped_chunks: 0 }) as Schemas["OutputStatus"];
    expect(
      outputsOf([output("MP3", true, "connected"), output("AAC", true, "reconnecting"), output("Old", false, "stopped")]),
    ).toEqual({ connected: 1, total: 2, down: ["AAC"] });
  });
});

describe("the next fixed time", () => {
  const NOW = Date.UTC(2026, 10, 10, 14, 0);
  const status = (open: boolean, at: number): Schemas["ShowStatus"] =>
    open ? { open, closes_at_unix_ms: at, next_opening_unix_ms: null } : { open, next_opening_unix_ms: at };
  const show = (module: "match" | "dedications", enabled: boolean, s: Schemas["ShowStatus"]) =>
    ({ module, settings: { enabled, website: { title: "" } }, status: s }) as unknown as Schemas["ShowView"];
  const top = (name: string, enabled: boolean, s: Schemas["ShowStatus"]) =>
    ({ id: 7, settings: { name, enabled }, status: s }) as unknown as Schemas["TopView"];
  const exact = (at: number) => ({ entry_id: 3, track: { path: "x.mp3", title: "News", start_at_unix_ms: at } }) as QueueEntry;
  const next = (shows: Schemas["ShowView"][], tops: Schemas["TopView"][], queue: QueueEntry[]) =>
    nextFixedTime(shows, tops, queue, NOW, (s) => s.module, (e) => e.track.title ?? "");

  it("takes the first to come of the shows, tops and exact times", () => {
    expect(
      next(
        [show("match", true, status(false, NOW + 3_600_000))],
        [top("Top 30", true, status(false, NOW + 1_800_000))],
        [exact(NOW + 2_400_000)],
      ),
    ).toEqual({ name: "Top 30", at: NOW + 1_800_000, open: false, link: "/shows/top30/7" });
    expect(next([], [], [exact(NOW + 600_000)])).toEqual({ name: "News", at: NOW + 600_000, open: false, link: "/queue" });
  });

  it("puts a show open now first, and leaves out what is not enabled", () => {
    expect(
      next(
        [show("match", true, status(true, NOW + 3_600_000)), show("dedications", false, status(true, NOW + 60_000))],
        [top("Top 30", true, status(false, NOW + 60_000))],
        [],
      ),
    ).toEqual({ name: "match", at: NOW + 3_600_000, open: true, link: "/shows/match" });
    expect(next([], [top("Off", false, status(false, NOW + 60_000))], [exact(NOW - 1)])).toBeNull();
  });

  it("says when: in minutes within the hour, at a time today, else with the day", () => {
    expect(whenOf(NOW + 12 * 60_000 - 1, NOW)).toEqual({ kind: "in", minutes: 12 });
    expect(whenOf(NOW + 10_000, NOW)).toEqual({ kind: "in", minutes: 1 });
    expect(whenOf(NOW + 2 * 3_600_000, NOW).kind).toBe("at");
    expect(whenOf(NOW + 3 * 86_400_000, NOW).kind).toBe("day");
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
