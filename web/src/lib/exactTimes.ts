// SPDX-License-Identifier: AGPL-3.0-or-later

// The attempts page (docs/rotation.md#the-attempts-page): how an exact time
// was met, how each attempt ends against its window, where its tracks fall
// on a timeline, and how it ranks against the attempt kept.

import type { Schemas } from "../api";
import { number, t } from "../i18n";
import { clockTime } from "./format";

export type Attempt = Schemas["Attempt"];
export type AttemptEnding = Schemas["AttemptEnding"];
export type CalculationSummary = Schemas["CalculationSummary"];
export type ExactTimeOfDay = Schemas["ExactTimeOfDay"];

/** A window: the exact time, the earliest start and the cut (Unix ms). */
export interface ExactWindow {
  at: number;
  from: number;
  cut: number;
}

/** How a status reads: its text and its colour. */
export type Tone = "good" | "warn" | "bad" | "muted";
export interface Status {
  label: string;
  tone: Tone;
}

/** Whole seconds, signed: `+3 s`, `−4 s`, `0 s`. */
export function signedSeconds(ms: number): string {
  const seconds = Math.round(ms / 1000);
  const value = number(Math.abs(seconds));
  return seconds > 0 ? `+${value} s` : seconds < 0 ? `−${value} s` : `${value} s`;
}

/** How long a faded ending cuts the track before: from the cut to where it would end (ms). */
export function cutMs(ending: AttemptEnding, window: ExactWindow): number {
  if (!ending.faded || ending.offset_ms === null || ending.offset_ms === undefined) {
    return 0;
  }
  return Math.max(0, window.at + ending.offset_ms - window.cut);
}

/** Lands: in the window, nothing faded out (its gap filled or not). It can be chosen by hand. */
export function lands(ending: AttemptEnding): boolean {
  return ending.in_window && !ending.faded;
}

/** How an attempt ends: `−4 s · in the window`, `+2 s · 9 s filled`, `faded 18 s`. */
export function endingStatus(ending: AttemptEnding, window: ExactWindow): Status {
  if (ending.offset_ms === null || ending.offset_ms === undefined) {
    return { label: t("exactTimes.notReached"), tone: "bad" };
  }
  if (ending.faded) {
    return {
      label: t("exactTimes.endsFaded", { seconds: Math.round(cutMs(ending, window) / 1000) }),
      tone: "bad",
    };
  }
  const offset = signedSeconds(ending.offset_ms);
  if (!ending.in_window) {
    return { label: t("exactTimes.endsOutside", { offset }), tone: "bad" };
  }
  if (ending.filling_ms > 0) {
    return {
      label: t("exactTimes.endsFilled", { offset, seconds: Math.round(ending.filling_ms / 1000) }),
      tone: "warn",
    };
  }
  return { label: t("exactTimes.endsInWindow", { offset }), tone: "good" };
}

/** How an exact time of the day was met, for its chip: `on time`, `early 12 s`, `faded 18 s`, `upcoming`. */
export function metStatus(exact: ExactTimeOfDay): Status {
  const met = exact.met;
  if (!met) {
    if (exact.upcoming) {
      return { label: t("exactTimes.upcoming"), tone: "muted" };
    }
    return { label: t("exactTimes.notMet"), tone: "muted" };
  }
  if (met.faded) {
    // By how much: the last calculation's ending, when known.
    const last = exact.calculations.at(-1);
    const window = windowOf(exact);
    const cut = last ? cutMs(last.ending, window) : 0;
    return {
      label: cut > 0 ? t("exactTimes.fadedBy", { seconds: Math.round(cut / 1000) }) : t("exactTimes.faded"),
      tone: "bad",
    };
  }
  const seconds = Math.round(Math.abs(met.offset_ms) / 1000);
  const tone: Tone = met.in_window ? "good" : "bad";
  if (met.met === "on_time") {
    return { label: t("exactTimes.onTime"), tone };
  }
  return {
    label: met.met === "early" ? t("exactTimes.earlyBy", { seconds }) : t("exactTimes.lateBy", { seconds }),
    tone: met.in_window ? "warn" : "bad",
  };
}

export function windowOf(exact: {
  at_unix_ms?: number;
  exact_at_unix_ms?: number;
  window_from_unix_ms: number;
  window_cut_unix_ms: number;
}): ExactWindow {
  return {
    at: exact.exact_at_unix_ms ?? exact.at_unix_ms ?? 0,
    from: exact.window_from_unix_ms,
    cut: exact.window_cut_unix_ms,
  };
}

/** The calculation that built fills last (followed picks aside): the one whose fill the queue follows. */
export function latestBuilt(calculations: readonly CalculationSummary[]): CalculationSummary | undefined {
  return [...calculations].reverse().find((calculation) => calculation.trigger !== "followed");
}

/** The exact time to show first: the one asked for, else the next one ahead, else the last of the day. */
export function defaultExactTime(
  exactTimes: readonly ExactTimeOfDay[],
  asked: number | null,
): ExactTimeOfDay | undefined {
  return (
    exactTimes.find((exact) => exact.at_unix_ms === asked) ??
    exactTimes.find((exact) => exact.upcoming) ??
    exactTimes.at(-1)
  );
}

export type AttemptFilter = "all" | "in_window" | "faded";

export function filterAttempts(attempts: readonly Attempt[], filter: AttemptFilter): Attempt[] {
  switch (filter) {
    case "in_window":
      return attempts.filter((attempt) => attempt.ending.in_window);
    case "faded":
      return attempts.filter((attempt) => attempt.ending.faded);
    default:
      return [...attempts];
  }
}

/** A track on an attempt's timeline: where it starts and how wide it is (% of the span), and the share of it a fade cuts. */
export interface Block {
  index: number;
  left: number;
  width: number;
  /** From 0 (nothing cut) to 1: where in the block the cut falls. */
  keptShare: number;
}

/** The time a timeline shows: from the end of the queue to a little after the cut, or the last end. */
export interface Span {
  from: number;
  to: number;
}

export function spanOf(attempts: readonly Attempt[], from: number, window: ExactWindow): Span {
  let to = window.cut;
  for (const attempt of attempts) {
    const last = attempt.tracks.at(-1);
    if (last) {
      to = Math.max(to, last.start_unix_ms + last.next_ms);
    }
  }
  const length = Math.max(to - from, 1);
  return { from, to: to + length * 0.03 };
}

/** Where `unixMs` falls on the span, in %. */
export function position(span: Span, unixMs: number): number {
  return ((unixMs - span.from) / (span.to - span.from)) * 100;
}

export function blocks(attempt: Attempt, span: Span, window: ExactWindow): Block[] {
  return attempt.tracks.map((track, index) => {
    const end = track.start_unix_ms + track.next_ms;
    const left = position(span, track.start_unix_ms);
    const width = Math.max(position(span, end) - left, 0);
    const keptShare =
      end > window.cut && track.next_ms > 0 ? Math.max(0, (window.cut - track.start_unix_ms) / track.next_ms) : 1;
    return { index, left, width, keptShare };
  });
}

/** The ticks of the axis: whole minutes, 4 to 6 of them, and the exact time. */
export function ticks(span: Span, window: ExactWindow): { at: number; left: number; exact: boolean }[] {
  const minutes = (span.to - span.from) / 60_000;
  const step = [1, 2, 4, 5, 10, 15, 20, 30].find((step) => minutes / step <= 5) ?? 60;
  const stepMs = step * 60_000;
  const result: { at: number; left: number; exact: boolean }[] = [];
  for (let at = Math.ceil(span.from / stepMs) * stepMs; at < window.at - stepMs / 3; at += stepMs) {
    result.push({ at, left: position(span, at), exact: false });
  }
  result.push({ at: window.at, left: position(span, window.at), exact: true });
  return result;
}

/** The days an age in ms stands for, averaged over `tracks`: `3.1 d`. */
export function averageAge(ending: AttemptEnding, tracks: number): string {
  const days = ending.age_ms / Math.max(tracks, 1) / 86_400_000;
  return t("exactTimes.days", { days: number(days, { maximumFractionDigits: 1, minimumFractionDigits: 1 }) });
}

/** A second late counts as 3 early: ending before the exact time is better (rotation's own weight). */
const LATE_WEIGHT = 3;

function weighed(offset: number | null | undefined): number {
  if (offset === null || offset === undefined) {
    return Number.POSITIVE_INFINITY;
  }
  return offset > 0 ? offset * LATE_WEIGHT : -offset;
}

/** One of the criteria attempts are ranked by, for this attempt and the one kept. */
export interface Criterion {
  n: number;
  label: string;
  mine: string;
  kept: string;
}

/**
 * The criteria, in rotation's order: nothing faded, the end against the
 * exact time (in the window first), the filling, the rules relaxed, the
 * age of the tracks.
 */
export function criteria(attempt: Attempt, kept: Attempt, window: ExactWindow): Criterion[] {
  const faded = (ending: AttemptEnding) =>
    ending.faded ? t("exactTimes.fadedBy", { seconds: Math.round(cutMs(ending, window) / 1000) }) : t("exactTimes.yes");
  const end = (ending: AttemptEnding) => {
    if (ending.offset_ms === null || ending.offset_ms === undefined) {
      return t("exactTimes.notReached");
    }
    const offset = signedSeconds(ending.offset_ms);
    return ending.in_window ? offset : t("exactTimes.outsideShort", { offset });
  };
  const seconds = (ms: number) => `${number(Math.round(ms / 1000))} s`;
  return [
    { n: 1, label: t("exactTimes.criterionFaded"), mine: faded(attempt.ending), kept: faded(kept.ending) },
    {
      n: 2,
      label: t("exactTimes.criterionEnd", { time: clockTime(window.at) }),
      mine: end(attempt.ending),
      kept: end(kept.ending),
    },
    {
      n: 3,
      label: t("exactTimes.criterionFilling"),
      mine: seconds(attempt.ending.filling_ms),
      kept: seconds(kept.ending.filling_ms),
    },
    {
      n: 4,
      label: t("exactTimes.criterionRelaxed"),
      mine: number(attempt.ending.relaxed_steps),
      kept: number(kept.ending.relaxed_steps),
    },
    {
      n: 5,
      label: t("exactTimes.criterionAge"),
      mine: averageAge(attempt.ending, attempt.tracks.length),
      kept: averageAge(kept.ending, kept.tracks.length),
    },
  ];
}

/** The first criterion where `attempt` loses to `kept`, from 1; 0 when it does not lose (it is the one kept, or as good). */
export function losesAt(attempt: Attempt, kept: Attempt): number {
  // Not reaching the exact slot loses to anything, then fading out.
  const faded = (ending: AttemptEnding) =>
    (ending.offset_ms === null || ending.offset_ms === undefined ? 2 : 0) + (ending.faded ? 1 : 0);
  // Out of the window loses to any end in it.
  const end = (ending: AttemptEnding) => (ending.in_window ? 0 : 1e15) + weighed(ending.offset_ms);
  const a = attempt.ending;
  const b = kept.ending;
  const steps: [number, number][] = [
    [faded(a), faded(b)],
    [end(a), end(b)],
    [a.filling_ms, b.filling_ms],
    [a.relaxed_steps, b.relaxed_steps],
    [-a.age_ms, -b.age_ms],
  ];
  for (const [index, [mine, theirs]] of steps.entries()) {
    if (mine !== theirs) {
      return mine > theirs ? index + 1 : 0;
    }
  }
  return 0;
}

/** Why an attempt ranks where it does against the one kept: the first criterion it loses at. */
export function verdict(attempt: Attempt, kept: Attempt): Status {
  if (attempt.number === kept.number) {
    return { label: t("exactTimes.verdictKept"), tone: "good" };
  }
  const at = losesAt(attempt, kept);
  const keptNumber = String(kept.number);
  switch (at) {
    case 1:
      return { label: t("exactTimes.verdictFaded"), tone: "bad" };
    case 2: {
      const mine = attempt.ending.offset_ms;
      const theirs = kept.ending.offset_ms;
      return {
        label: t("exactTimes.verdictEnd", {
          mine: mine === null || mine === undefined ? "–" : signedSeconds(mine),
          kept: theirs === null || theirs === undefined ? "–" : signedSeconds(theirs),
          number: keptNumber,
        }),
        tone: attempt.ending.in_window ? "muted" : "bad",
      };
    }
    case 3:
      return {
        label: t("exactTimes.verdictFilling", {
          seconds: Math.round(attempt.ending.filling_ms / 1000),
          kept: Math.round(kept.ending.filling_ms / 1000),
          number: keptNumber,
        }),
        tone: "warn",
      };
    case 4:
      return { label: t("exactTimes.verdictRelaxed", { number: keptNumber }), tone: "muted" };
    case 5:
      return { label: t("exactTimes.verdictAge", { number: keptNumber }), tone: "muted" };
    default:
      return { label: t("exactTimes.verdictTie", { number: keptNumber }), tone: "muted" };
  }
}

/** Why fills were built, or that the pick followed the fill kept. */
export function triggerLabel(calculation: CalculationSummary): string {
  switch (calculation.trigger) {
    case "first":
      return t("exactTimes.triggerFirst");
    case "restart":
      return t("exactTimes.triggerRestart");
    case "queue_changed":
      return t("exactTimes.triggerQueueChanged");
    case "hand_dropped":
      return t("exactTimes.triggerHandDropped");
    case "automatic":
      return t("exactTimes.triggerAutomatic");
    default:
      return calculation.by_hand ? t("exactTimes.triggerFollowedByHand") : t("exactTimes.triggerFollowed");
  }
}

/** Why building stopped: `the best one lands`, `200 fills`, `3 s spent`. */
export function stopLabel(calculation: CalculationSummary): string {
  switch (calculation.stop) {
    case "landed":
      return t("exactTimes.stopLanded");
    case "max_fills":
      return t("exactTimes.stopMaxFills", { count: calculation.fills });
    case "time":
      return t("exactTimes.stopTime");
    default:
      return "";
  }
}

/** The result in the list of calculations: `−4 s`, `followed`, `faded`. */
export function calculationResult(calculation: CalculationSummary): Status {
  const ending = calculation.ending;
  if (calculation.trigger === "followed") {
    return { label: t("exactTimes.followed"), tone: "good" };
  }
  if (ending.offset_ms === null || ending.offset_ms === undefined) {
    return { label: t("exactTimes.notReached"), tone: "bad" };
  }
  if (ending.faded || !ending.in_window) {
    return { label: signedSeconds(ending.offset_ms), tone: "bad" };
  }
  return { label: signedSeconds(ending.offset_ms), tone: ending.filling_ms > 0 ? "warn" : "good" };
}

/** How a track of an attempt was chosen: `least recently played`, `fit search`, `medley, 2 parts left out`. */
export function chosenLabel(track: Schemas["AttemptTrack"]): string {
  switch (track.chosen) {
    case "fit_search":
      return t("exactTimes.chosenSearch");
    case "kept":
      return t("exactTimes.chosenKept");
    case "filler":
      return t("exactTimes.chosenFiller", { seconds: Math.round(track.next_ms / 1000) });
    case "medley": {
      const parts = track.parts_left_out?.length ?? 0;
      return parts > 0 ? t("exactTimes.chosenMedleyShortened", { count: parts }) : t("exactTimes.chosenMedley");
    }
    default:
      return t("exactTimes.chosenSlot");
  }
}

/** The fit search's numbers in a row: `34 · 51 · 48`. */
export function joined(values: readonly number[]): string {
  return values.map((value) => number(value)).join(" · ");
}
