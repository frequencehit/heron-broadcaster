// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { displayName, duckRamps, fadeGain, layout, levelAt, linerSlot, medleyName, moveTo, plannedLength, renderDay } from "./medley";

/** Down over 0.15 s before the liners, back up during them: as medleys were mixed before the placement could be chosen. */
const beforeAndDuring = { duck_attack_ms: 150, duck_attack_at: "outside", duck_release_at: "inside" } as const;

const parts = [
  { kind: "hook", from_ms: 10_000, to_ms: 17_000 },
  { kind: "liner", from_ms: 0, to_ms: 3_000 },
  { kind: "hook", from_ms: 5_000, to_ms: 12_000 },
];

describe("levelAt", () => {
  it("draws the fades, the ducking and the medley's fade-in and fade-out", () => {
    const parts = [
      { kind: "liner", from_ms: 0, to_ms: 1_000 },
      { kind: "hook", from_ms: 0, to_ms: 4_000 },
    ];
    const medley = layout(parts, {
      crossfade_ms: 0,
      liner_mode: "between",
      fade_in_ms: 1_000,
      fade_in_curve: "slow_then_fast",
      fade_out_ms: 1_000,
      fade_out_curve: "linear",
    });
    expect(medley.music_start_ms).toBe(1_000);
    // The liner is never faded but at its edges.
    expect(levelAt(medley, 0, 500)).toBe(1);
    // Halfway through the fade-in, slow then fast: a quarter.
    expect(levelAt(medley, 1, 1_500)).toBeCloseTo(0.25, 3);
    expect(levelAt(medley, 1, 3_000)).toBe(1);
    expect(levelAt(medley, 1, 4_500)).toBeCloseTo(0.5, 3);
    expect(levelAt(medley, 1, 9_000)).toBe(0);
    // Under a liner over the music: ducked.
    const over = layout(
      [
        { kind: "hook", from_ms: 10_000, to_ms: 18_000 },
        { kind: "liner", from_ms: 0, to_ms: 2_000 },
        { kind: "hook", from_ms: 30_000, to_ms: 36_000 },
      ],
      { crossfade_ms: 200, liner_mode: "over", duck_db: -20, ...beforeAndDuring },
    );
    expect(levelAt(over, 0, 8_700)).toBeCloseTo(0.1, 3);
    expect(levelAt(over, 0, 7_000)).toBe(1);
    // Back up over the liner's last 0.3 s: halfway 0.15 s before its end.
    expect(levelAt(over, 2, 10_000)).toBeCloseTo(0.1 + 0.9 * 0.5, 3);
    expect(levelAt(over, 2, 10_160)).toBe(1);
    expect(fadeGain("s_curve", 0.5)).toBeCloseTo(0.5, 6);
  });
});

describe("medleyName", () => {
  it("lists each artist once, in order", () => {
    expect(medleyName("2026-10-29", ["Helena", "Panic! At The Disco", "Mabel"])).toBe(
      "Medley 2026-10-29 (Helena - Panic! At The Disco - Mabel)",
    );
    expect(medleyName("2026-10-29", ["Mabel", null, " ", "mabel", "Helena"])).toBe("Medley 2026-10-29 (Mabel - Helena)");
    expect(medleyName("2026-11-02", [])).toBe("Medley 2026-11-02");
  });

  it("cuts a long list after the last name that fits", () => {
    const names = Array.from({ length: 30 }, (_, i) => `Artist number ${String(i + 1).padStart(2, "0")}`);
    const name = medleyName("2026-10-29", names);
    expect([...name].length).toBeLessThanOrEqual(249);
    expect(name.endsWith(" - …)")).toBe(true);
    const cut = medleyName("2026-10-29", ["x".repeat(400)]);
    expect([...cut]).toHaveLength(249);
    expect(cut.endsWith("x…)")).toBe(true);
  });
});

describe("renderDay", () => {
  it("is the day in the station's zone", () => {
    // 23:30 UTC on 31 October is already 1 November in Paris.
    const date = new Date(Date.UTC(2026, 9, 31, 23, 30));
    expect(renderDay(date, "UTC")).toBe("2026-10-31");
    expect(renderDay(date, "Europe/Paris")).toBe("2026-11-01");
  });
});

describe("displayName", () => {
  it("stands in until the first render", () => {
    expect(displayName({ id: 4, name: "Medley 2026-10 (A)" })).toBe("Medley 2026-10 (A)");
    expect(displayName({ id: 4, name: null })).toContain("4");
  });
});

describe("linerSlot", () => {
  const plan = [{ kind: "liner" }, { kind: "hook" }, { kind: "liner" }, { kind: "hook" }, { kind: "liner" }];
  it("is the start before every hook, the end after every hook", () => {
    expect(linerSlot(plan, 0)).toBe("start");
    expect(linerSlot(plan, 2)).toBe("between");
    expect(linerSlot(plan, 4)).toBe("end");
    expect(linerSlot(plan, plan.length)).toBe("end");
  });
});

describe("plannedLength", () => {
  it("overlaps parts by the crossfade", () => {
    expect(plannedLength(parts, { crossfade_ms: 500, liner_mode: "between" })).toBe(17_000 - 2 * 500);
    expect(plannedLength([], { crossfade_ms: 500, liner_mode: "over" })).toBe(0);
  });
});

// The same cases as heron_core::medley's tests: the two layouts must agree.
describe("layout", () => {
  const over = { crossfade_ms: 200, liner_mode: "over", ...beforeAndDuring };
  const hooks = [
    { kind: "hook", from_ms: 10_000, to_ms: 18_000 },
    { kind: "liner", from_ms: 0, to_ms: 2_000 },
    { kind: "hook", from_ms: 30_000, to_ms: 36_000 },
  ];

  it("puts liners over the music between whole hooks, half each", () => {
    const { parts: [first, liner, second] = [], length_ms } = layout(hooks, over);
    expect([first?.from_ms, first?.to_ms]).toEqual([10_000, 19_250]);
    expect([second?.from_ms, second?.to_ms, second?.at_ms]).toEqual([28_900, 36_000, 9_050]);
    expect(liner?.at_ms).toBe(8_150);
    expect(length_ms).toBe(9_050 + 7_100);
  });

  it("shares the gap as the liner says, within the files", () => {
    const shifted = hooks.map((part, i) => (i === 1 ? { ...part, shift_ms: 500 } : part));
    const moved = layout(shifted, over).parts;
    expect([moved[0]?.to_ms, moved[2]?.from_ms, moved[1]?.at_ms]).toEqual([19_750, 29_400, 8_150]);
    const short = hooks.map((part, i) => (i === 0 ? { ...part, duration_ms: 18_500 } : part));
    const cut = layout(short, over).parts;
    expect([cut[0]?.to_ms, cut[2]?.from_ms]).toEqual([18_500, 28_150]);
  });

  it("moves parts: overlaps more or less, liners over the music move over it", () => {
    const between = { crossfade_ms: 500, liner_mode: "between" };
    const moved = (by: number) => {
      const parts = [
        { kind: "hook", from_ms: 0, to_ms: 1_000 },
        { kind: "liner", from_ms: 0, to_ms: 1_000, move_ms: by },
        { kind: "hook", from_ms: 0, to_ms: 1_000 },
      ];
      const placed = layout(parts, between).parts;
      return [placed[1]?.at_ms, placed[2]?.at_ms];
    };
    expect(moved(-200)).toEqual([300, 800]);
    expect(moved(600)[0]).toBe(1_000);
    expect(moved(-5_000)[0]).toBe(0);
    const shifted = hooks.map((part, i) => (i === 1 ? { ...part, move_ms: 300 } : part));
    expect(layout(shifted, over).parts[1]?.at_ms).toBe(8_450);
  });

  it("finds the move that puts a part where it was dropped", () => {
    const between = { crossfade_ms: 500, liner_mode: "between" };
    const parts = [
      { kind: "hook", from_ms: 0, to_ms: 1_000 },
      { kind: "hook", from_ms: 0, to_ms: 1_000 },
    ];
    expect(moveTo(parts, between, 1, 300)).toBe(-200);
    expect(moveTo(parts, between, 1, 2_000)).toBe(500);
    expect(moveTo(parts, between, 1, -50)).toBe(-500);
    expect(moveTo(parts, between, 0, 400)).toBe(0);
    expect(moveTo(hooks, over, 1, 8_450)).toBe(300);
    expect(moveTo(hooks, over, 1, -100)).toBe(-8_150);
  });

  it("goes down and comes back up where chosen", () => {
    // Across the liner's start and end: the spans stay clear of the ramps' halves.
    const across = { crossfade_ms: 200, liner_mode: "over", duck_db: -20, duck_attack_at: "across" } as const;
    const medley = layout(hooks, across);
    const [, liner, second] = medley.parts;
    expect(liner?.at_ms).toBe(8_150);
    const lineEnd = (liner?.at_ms ?? 0) + 2_000;
    expect((second?.at_ms ?? 0) + 30_000 - (second?.from_ms ?? 0)).toBe(lineEnd + 150);
    const halfway = (1 + 0.1) / 2;
    expect(levelAt(medley, 0, 8_150)).toBeCloseTo(halfway, 2);
    expect(levelAt(medley, 2, lineEnd)).toBeCloseTo(halfway, 2);
    expect(levelAt(medley, 0, 7_990)).toBe(1);
    expect(levelAt(medley, 2, lineEnd + 160)).toBe(1);
    // Outside, inside, and inside a liner too short for both.
    const outside = { attack_ms: 300, attack_at: "outside", release_ms: 300, release_at: "outside" } as const;
    expect(duckRamps(outside, 1_000, 2_000)).toEqual([700, 1_000, 2_000, 2_300]);
    const inside = { ...outside, attack_at: "inside", release_at: "inside" } as const;
    expect(duckRamps(inside, 1_000, 2_000)).toEqual([1_000, 1_300, 1_700, 2_000]);
    expect(duckRamps(inside, 1_000, 1_400)).toEqual([1_000, 1_200, 1_200, 1_400]);
    expect(layout(hooks, { ...across, duck_attack_at: "inside", duck_release_at: "inside" }).parts[1]?.at_ms).toBe(8_000);
  });

  it("extends one hook for a liner at the start or the end", () => {
    const { parts: [opening, hook, closing] = [] } = layout(
      [
        { kind: "liner", from_ms: 0, to_ms: 2_000 },
        { kind: "hook", from_ms: 5_000, to_ms: 9_000 },
        { kind: "liner", from_ms: 0, to_ms: 1_000 },
      ],
      over,
    );
    expect([hook?.at_ms, hook?.from_ms, hook?.to_ms]).toEqual([0, 3_000, 10_150]);
    expect(opening?.at_ms).toBe(0);
    expect(closing?.at_ms).toBe(9_000 - 3_000 + 150);
  });
});
