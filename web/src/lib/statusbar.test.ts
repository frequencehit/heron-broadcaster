// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import type { DeckState } from "../api";
import { levelOf, listenersText, summarizeDeck, summarizeSystem } from "./statusbar";

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
      text: "Jamiroquai – Space Cowboy 2:31",
      onAir: true,
    });
  });

  it("moves on from the position it is given", () => {
    expect(summarizeDeck("A", deck(), false, 65_000).text).toBe("Jamiroquai – Space Cowboy 1:05");
  });

  it("says paused for a deck the player has paused", () => {
    const summary = summarizeDeck("B", deck({ on_air: false }), true, null);
    expect([summary.status, summary.symbol, summary.onAir]).toEqual(["paused", "⏸", false]);
  });

  it("is empty without a track, or when the player does not list the deck", () => {
    const empty = { letter: "B", status: "empty", symbol: "—", text: "", onAir: false };
    expect(summarizeDeck("B", deck({ entry: null }), false, null)).toEqual(empty);
    expect(summarizeDeck("B", undefined, false, null)).toEqual(empty);
  });

  it("falls back on the file name without tags", () => {
    const untagged = deck({ entry: { entry_id: 2, track: { path: "incoming/jingle.mp3" } } as NonNullable<DeckState["entry"]> });
    expect(summarizeDeck("A", untagged, false, 0).text).toBe("jingle.mp3 0:00");
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
