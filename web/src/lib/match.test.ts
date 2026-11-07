// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { along, clock, left, outcome, sides, type MatchView } from "./match";

function match(change: Partial<MatchView>): MatchView {
  return {
    id: 1,
    status: "closed",
    category_id: 1,
    category_name: "Hits",
    vote_track: { track_id: 1, artist: "A", title: "During" },
    jingle: null,
    choices: [
      { choice: "a", track_id: 2, artist: "Cassia Bloom", title: "Neon Orchard", votes: 46, percent: 58 },
      { choice: "b", track_id: 3, artist: "Echo Parade", title: "Running Lights", votes: 33, percent: 42 },
    ],
    votes: 79,
    opened_at_unix_ms: 0,
    closes_at_unix_ms: 50_000,
    transition_at_unix_ms: 60_000,
    winner: "a",
    decided_by: "votes",
    closed_at_unix_ms: 50_000,
    ...change,
  };
}

describe("match", () => {
  it("counts down to the close", () => {
    expect(left(null, 0)).toBeNull();
    expect(left(10_000, 2_500)).toBe(7_500);
    expect(left(10_000, 12_000)).toBe(0);
    expect(clock(72_000)).toBe("1:12");
    expect(clock(4_100)).toBe("0:05");
    expect(clock(0)).toBe("0:00");
  });

  it("places a time on the vote's track", () => {
    expect(along(5, 0, 10)).toBe(50);
    expect(along(20, 0, 10)).toBe(100);
    expect(along(-1, 0, 10)).toBe(0);
    expect(along(1, 5, 5)).toBe(0);
  });

  it("tells how a match was decided", () => {
    expect(outcome(match({}))).toEqual({ text: "58 %", tone: "good" });
    expect(outcome(match({ decided_by: "tie" })).tone).toBe("muted");
    expect(outcome(match({ decided_by: "no_votes" })).tone).toBe("warn");
    expect(outcome(match({ status: "cancelled", winner: null, decided_by: null })).tone).toBe("muted");
    expect(sides(match({ winner: "b" })).winner?.title).toBe("Running Lights");
    expect(sides(match({ winner: null })).winner).toBeNull();
  });
});
