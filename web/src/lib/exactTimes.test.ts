// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { t } from "../i18n";
import {
  type Attempt,
  type AttemptEnding,
  type CalculationSummary,
  type ExactTimeOfDay,
  blocks,
  calculationResult,
  chosenLabel,
  criteria,
  cutMs,
  defaultExactTime,
  endingStatus,
  filterAttempts,
  lands,
  latestBuilt,
  losesAt,
  metStatus,
  runHolds,
  runsOf,
  signedSeconds,
  spanOf,
  ticks,
  triggerLabel,
  verdict,
} from "./exactTimes";

const MIN = 60_000;
const AT = 13 * 60 * MIN;
// Soft: from 10 s before to 30 s after.
const WINDOW = { at: AT, from: AT - 10_000, cut: AT + 30_000 };

function ending(changes: Partial<AttemptEnding> = {}): AttemptEnding {
  return {
    offset_ms: -4_000,
    in_window: true,
    faded: false,
    filling_ms: 0,
    relaxed_steps: 0,
    age_ms: 4 * 86_400_000,
    ...changes,
  };
}

/** An attempt of tracks of these lengths (s) from 12:44. */
function attempt(number: number, rank: number, lengths: number[], changes: Partial<AttemptEnding> = {}): Attempt {
  let at = AT - 16 * MIN;
  const tracks = lengths.map((length, index) => {
    const track = {
      track_id: number * 100 + index,
      pick: index,
      slot: 20 + index,
      category_id: 1,
      start_unix_ms: at,
      next_ms: length * 1000,
      chosen: "slot" as const,
      step: 0,
    };
    at += length * 1000;
    return track;
  });
  return { number, batch: 1, seed: "42", rank, tracks, ending: ending({ offset_ms: at - AT, ...changes }) };
}

function calculation(changes: Partial<CalculationSummary>): CalculationSummary {
  return {
    id: 1,
    computed_at_unix_ms: AT - 20 * MIN,
    trigger: "first",
    follows_id: null,
    fills: 10,
    batches: 1,
    stop: "landed",
    duration_ms: 96,
    kept: 1,
    ending: ending(),
    by_hand: false,
    chosen: null,
    ...changes,
  };
}

function exactTime(changes: Partial<ExactTimeOfDay>): ExactTimeOfDay {
  return {
    at_unix_ms: AT,
    mode: "soft",
    window_from_unix_ms: WINDOW.from,
    window_cut_unix_ms: WINDOW.cut,
    clock_name: "Hour",
    slot: 0,
    met: null,
    planned: false,
    upcoming: true,
    calculations: [],
    ...changes,
  };
}

describe("a planned exact time", () => {
  it("says planned, not met, while its slot is only queued", () => {
    const met = {
      at_unix_ms: AT,
      mode: "soft" as const,
      before_s: 10,
      after_s: 30,
      met: "on_time" as const,
      offset_ms: 3_000,
      faded: false,
      in_window: true,
      cut_at_unix_ms: WINDOW.cut,
    };
    const planned = metStatus(exactTime({ met, planned: true, upcoming: true }));
    expect(planned.tone).toBe("muted");
    expect(planned.label).toBe(t("exactTimes.plannedStatus", { status: t("exactTimes.onTime") }));
    expect(metStatus(exactTime({ met, planned: false, upcoming: false })).label).toBe(t("exactTimes.onTime"));
  });
});

describe("signed seconds", () => {
  it("rounds and signs", () => {
    expect(signedSeconds(2_400)).toBe("+2 s");
    expect(signedSeconds(-4_000)).toBe("−4 s");
    expect(signedSeconds(300)).toBe("0 s");
  });
});

describe("endings", () => {
  it("says how an attempt ends against its window", () => {
    expect(endingStatus(ending(), WINDOW)).toEqual({ label: t("exactTimes.endsInWindow", { offset: "−4 s" }), tone: "good" });
    expect(endingStatus(ending({ offset_ms: 2_000, filling_ms: 9_000 }), WINDOW).tone).toBe("warn");
    expect(endingStatus(ending({ offset_ms: -40_000, in_window: false }), WINDOW)).toEqual({
      label: t("exactTimes.endsOutside", { offset: "−40 s" }),
      tone: "bad",
    });
    expect(endingStatus(ending({ offset_ms: null }), WINDOW).label).toBe(t("exactTimes.notReached"));
  });

  it("measures what a fade cuts, from the end of the window", () => {
    const faded = ending({ offset_ms: 48_000, in_window: false, faded: true });
    expect(cutMs(faded, WINDOW)).toBe(18_000);
    expect(endingStatus(faded, WINDOW)).toEqual({ label: t("exactTimes.endsFaded", { seconds: 18 }), tone: "bad" });
    expect(cutMs(ending(), WINDOW)).toBe(0);
  });

  it("lets only the attempts in their window be chosen, filled or not", () => {
    expect(lands(ending({ filling_ms: 9_000 }))).toBe(true);
    expect(lands(ending({ in_window: false, offset_ms: -40_000 }))).toBe(false);
    expect(lands(ending({ faded: true, in_window: false }))).toBe(false);
  });
});

describe("exact times of the day", () => {
  it("says how each was met", () => {
    expect(metStatus(exactTime({})).label).toBe(t("exactTimes.upcoming"));
    const met = {
      at_unix_ms: AT,
      mode: "soft" as const,
      before_s: 10,
      after_s: 30,
      met: "on_time" as const,
      offset_ms: 400,
      in_window: true,
      faded: false,
      cut_at_unix_ms: WINDOW.cut,
    };
    expect(metStatus(exactTime({ met, upcoming: false }))).toEqual({ label: t("exactTimes.onTime"), tone: "good" });
    expect(metStatus(exactTime({ met: { ...met, met: "early", offset_ms: -12_000 }, upcoming: false })).label).toBe(
      t("exactTimes.earlyBy", { seconds: 12 }),
    );
    // Faded: by how much, from the last calculation.
    const faded = exactTime({
      met: { ...met, met: "late", offset_ms: 30_000, faded: true },
      upcoming: false,
      calculations: [calculation({ ending: ending({ offset_ms: 48_000, in_window: false, faded: true }) })],
    });
    expect(metStatus(faded)).toEqual({ label: t("exactTimes.fadedBy", { seconds: 18 }), tone: "bad" });
  });

  it("shows the one asked for, else the next ahead, else the last", () => {
    const past = exactTime({ at_unix_ms: AT - 60 * MIN, upcoming: false });
    const next = exactTime({});
    const later = exactTime({ at_unix_ms: AT + 30 * MIN });
    expect(defaultExactTime([past, next, later], null)).toBe(next);
    expect(defaultExactTime([past, next, later], past.at_unix_ms)).toBe(past);
    expect(defaultExactTime([past], null)).toBe(past);
    expect(defaultExactTime([], null)).toBeUndefined();
  });

  it("finds the latest calculation that built fills", () => {
    const list = [
      calculation({ id: 1 }),
      calculation({ id: 2, trigger: "queue_changed" }),
      calculation({ id: 3, trigger: "followed", follows_id: 2 }),
    ];
    expect(latestBuilt(list)?.id).toBe(2);
    expect(latestBuilt([])).toBeUndefined();
    expect(triggerLabel(list[1] as CalculationSummary)).toBe(t("exactTimes.triggerQueueChanged"));
    expect(triggerLabel(calculation({ trigger: "by_hand" }))).toBe(t("exactTimes.triggerByHand"));
    expect(triggerLabel(calculation({ trigger: "followed", by_hand: true }))).toBe(t("exactTimes.triggerFollowedByHand"));
  });

  it("says how a calculation ends in words, its sign spelled out", () => {
    const result = (changes: Partial<AttemptEnding>) => calculationResult(calculation({ ending: ending(changes) }), WINDOW);
    expect(result({ offset_ms: -4_000 })).toEqual({ label: t("exactTimes.resultEarly", { seconds: 4 }), tone: "good" });
    expect(result({ offset_ms: 300 })).toEqual({ label: t("exactTimes.resultOnTime"), tone: "good" });
    expect(result({ offset_ms: 3_000 }).label).toBe(t("exactTimes.resultLate", { seconds: 3 }));
    expect(result({ offset_ms: -2_000, filling_ms: 9_000 })).toEqual({
      label: t("exactTimes.resultFilled", { result: t("exactTimes.resultEarly", { seconds: 2 }), seconds: 9 }),
      tone: "warn",
    });
    expect(result({ offset_ms: -15_000, in_window: false })).toEqual({
      label: t("exactTimes.resultOutside", { result: t("exactTimes.resultEarly", { seconds: 15 }) }),
      tone: "bad",
    });
    // Faded: by how much the cut takes off the track before.
    expect(result({ offset_ms: 42_000, in_window: false, faded: true })).toEqual({
      label: t("exactTimes.resultFaded", { seconds: 12 }),
      tone: "bad",
    });
    expect(result({ offset_ms: null })).toEqual({ label: t("exactTimes.notReached"), tone: "bad" });
    expect(calculationResult(calculation({ trigger: "followed" }), WINDOW).label).toBe(t("exactTimes.resultFollowed"));
  });

  it("puts the picks that followed a fill on the card of its calculation, newest first", () => {
    const runs = runsOf([
      calculation({ id: 1 }),
      calculation({ id: 2, trigger: "followed", follows_id: 1 }),
      calculation({ id: 3, trigger: "followed", follows_id: 1 }),
      calculation({ id: 4, trigger: "queue_changed" }),
      calculation({ id: 5, trigger: "followed", follows_id: 4 }),
      // The one it follows is no longer kept: its picks together.
      calculation({ id: 6, trigger: "followed", follows_id: 99 }),
      calculation({ id: 7, trigger: "followed", follows_id: 99 }),
    ]);
    expect(runs.map((run) => [run.calculation.id, run.followed.map((followed) => followed.id)])).toEqual([
      [6, [7]],
      [4, [5]],
      [1, [2, 3]],
    ]);
    expect(runHolds(runs[2] as (typeof runs)[number], 3)).toBe(true);
    expect(runHolds(runs[2] as (typeof runs)[number], 4)).toBe(false);
    expect(runsOf([])).toEqual([]);
  });
});

describe("attempts", () => {
  const kept = attempt(12, 1, [241, 268, 225, 222]);
  const filled = attempt(27, 3, [268, 300, 210, 169, 9], { filling_ms: 9_000 });
  const faded = attempt(7, 5, [255, 290, 240, 233], { offset_ms: 58_000, in_window: false, faded: true });

  it("filters those in the window, and those faded", () => {
    const all = [kept, filled, faded];
    expect(filterAttempts(all, "all")).toHaveLength(3);
    expect(filterAttempts(all, "in_window").map((a) => a.number)).toEqual([12, 27]);
    expect(filterAttempts(all, "faded").map((a) => a.number)).toEqual([7]);
  });

  it("lays its tracks on a timeline, the part a fade cuts marked", () => {
    const span = spanOf([kept, faded], AT - 16 * MIN, WINDOW);
    expect(span.from).toBe(AT - 16 * MIN);
    expect(span.to).toBeGreaterThan(AT + 58_000);
    const laid = blocks(faded, span, WINDOW);
    expect(laid).toHaveLength(4);
    expect(laid[0]?.left).toBe(0);
    for (const [index, block] of laid.entries()) {
      const next = laid[index + 1];
      if (next) {
        expect(next.left).toBeCloseTo(block.left + block.width, 6);
      }
    }
    // The last one crosses the cut: 233 s from 12:57:05, cut at 13:00:30.
    expect(laid[3]?.keptShare).toBeCloseTo(205 / 233, 3);
    expect(laid[2]?.keptShare).toBe(1);
    expect(blocks(kept, span, WINDOW).every((block) => block.keptShare === 1)).toBe(true);
  });

  it("puts whole minutes on the axis, and the exact time", () => {
    const span = spanOf([kept], AT - 16 * MIN, WINDOW);
    const axis = ticks(span, WINDOW);
    expect(axis.at(-1)).toMatchObject({ at: AT, exact: true });
    expect(axis.length).toBeGreaterThanOrEqual(4);
    expect(axis.length).toBeLessThanOrEqual(7);
    for (const tick of axis.slice(0, -1)) {
      expect(tick.at % MIN).toBe(0);
      expect(tick.left).toBeGreaterThanOrEqual(0);
    }
  });

  it("ranks criterion by criterion against the one kept", () => {
    expect(losesAt(kept, kept)).toBe(0);
    expect(losesAt(faded, kept)).toBe(1);
    // As close, but filled.
    expect(losesAt(filled, kept)).toBe(3);
    const closer = attempt(3, 2, [241, 268, 225, 222], { filling_ms: 4_000 });
    expect(losesAt(closer, kept)).toBe(3);
    const relaxed = attempt(4, 2, [241, 268, 225, 222], { relaxed_steps: 1 });
    expect(losesAt(relaxed, kept)).toBe(4);
    const recent = attempt(5, 2, [241, 268, 225, 222], { age_ms: 86_400_000 });
    expect(losesAt(recent, kept)).toBe(5);
    // Late weighs three times early: +2 s loses to −4 s.
    const late = attempt(6, 2, [241, 268, 225, 228]);
    expect(late.ending.offset_ms).toBe(2_000);
    expect(losesAt(late, kept)).toBe(2);
    expect(verdict(kept, kept).label).toBe(t("exactTimes.verdictKept"));
    expect(verdict(faded, kept)).toEqual({ label: t("exactTimes.verdictFaded"), tone: "bad" });
    expect(verdict(closer, kept).label).toBe(t("exactTimes.verdictFilling", { seconds: 4, kept: 0, number: "12" }));
  });

  it("lists the criteria with both values", () => {
    const rows = criteria(faded, kept, WINDOW);
    expect(rows.map((row) => row.n)).toEqual([1, 2, 3, 4, 5]);
    expect(rows[0]).toMatchObject({ mine: t("exactTimes.fadedBy", { seconds: 28 }), kept: t("exactTimes.yes") });
    expect(rows[1]?.kept).toBe("−4 s");
    expect(rows[4]?.kept).toBe(t("exactTimes.days", { days: "1.0" }));
  });

  it("says how each track was chosen", () => {
    const track = kept.tracks[0];
    if (!track) {
      throw new Error("no track");
    }
    expect(chosenLabel(track)).toBe(t("exactTimes.chosenSlot"));
    expect(chosenLabel({ ...track, chosen: "fit_search" })).toBe(t("exactTimes.chosenSearch"));
    expect(chosenLabel({ ...track, chosen: "filler", next_ms: 9_000 })).toBe(t("exactTimes.chosenFiller", { seconds: 9 }));
    expect(chosenLabel({ ...track, chosen: "medley", parts_left_out: [3, 4] })).toBe(
      t("exactTimes.chosenMedleyShortened", { count: 2 }),
    );
  });
});
