// SPDX-License-Identifier: AGPL-3.0-or-later

// Dayparts and daypart separation (docs/rotation.md#daypart-separation): a
// station's day in 4 dayparts of 6 hours or 6 of 4 hours, named A, B, C…;
// a track played in one does not play in the N that follow it.

import { locale, t } from "../i18n";

/** The divisions offered. */
export const DAYPART_COUNTS = [4, 6] as const;

/** The longest daypart separation: 6 with 4 dayparts, 10 with 6. */
export function maxSeparation(count: number): number {
  return count === 6 ? 10 : 6;
}

/** `A`, `B`… */
export function daypartLetter(index: number): string {
  return String.fromCharCode(65 + Math.max(0, Math.min(index, 25)));
}

/** `06:00–11:59`. */
export function daypartHours(count: number, index: number): string {
  const length = 24 / count;
  const pad = (hour: number) => String(hour).padStart(2, "0");
  return `${pad(index * length)}:00–${pad((index + 1) * length - 1)}:59`;
}

/** `A 00:00–05:59 · B 06:00–11:59 · …` */
export function daypartsList(count: number): string {
  return Array.from({ length: count }, (_, index) => `${daypartLetter(index)} ${daypartHours(count, index)}`).join(" · ");
}

/** Where a daypart stands after a play: played, kept out, or free. */
export type PartState = "played" | "kept" | "free";

/**
 * The days of an example, today first: each daypart's state after a play in
 * today's `played`. Enough days (2 to 4) to show where the track can play
 * again.
 */
export function exampleDays(count: number, separation: number, played: number): PartState[][] {
  const state = (ordinal: number): PartState => {
    const after = ordinal - played;
    if (after === 0) {
      return "played";
    }
    return after > 0 && after <= separation ? "kept" : "free";
  };
  const days = Math.min(4, Math.max(2, Math.ceil((played + separation + 2) / count)));
  return Array.from({ length: days }, (_, day) =>
    Array.from({ length: count }, (_, index) => state(day * count + index)),
  );
}

/** The label of a day of the example: `Today`, `Tomorrow`, `In 2 days`. */
export function dayLabel(day: number): string {
  if (day === 0) {
    return t("dayparts.today");
  }
  return day === 1 ? t("dayparts.tomorrow") : t("dayparts.inDays", { days: day });
}

/** A daypart `ordinal` dayparts after today's A: `C`, `tomorrow's A`. */
export function partName(count: number, ordinal: number): string {
  const day = Math.floor(ordinal / count);
  const part = daypartLetter(ordinal % count);
  if (day === 0) {
    return part;
  }
  return day === 1 ? t("dayparts.partTomorrow", { part }) : t("dayparts.partInDays", { part, days: day });
}

function list(items: string[]): string {
  return new Intl.ListFormat(locale(), { type: "conjunction" }).format(items);
}

/**
 * What a separation does to a track played in today's `played`: `Played in
 * A, it does not play in B; at the earliest in C. It can play again later
 * in A itself: …`.
 */
export function describeSeparation(count: number, separation: number, played = 0): string {
  if (separation <= 0) {
    return t("dayparts.off");
  }
  const kept = Array.from({ length: separation }, (_, k) => partName(count, played + k + 1));
  const name = daypartLetter(played);
  const sentence = t("dayparts.sentence", {
    played: name,
    kept: list(kept),
    again: partName(count, played + separation + 1),
  });
  return separation < count ? `${sentence} ${t("dayparts.sameDaypart", { played: name })}` : sentence;
}
