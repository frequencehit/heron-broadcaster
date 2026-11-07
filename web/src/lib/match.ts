// SPDX-License-Identifier: AGPL-3.0-or-later

// The match (docs/listener-interaction.md#match): the countdown to the
// close of its vote, and how a match was decided.

import type { Schemas } from "../api";
import { t } from "../i18n";

export type MatchView = Schemas["MatchView"];
export type MatchSettings = Schemas["MatchSettings"];
export type MatchHistory = Schemas["MatchHistory"];
export type MatchSide = Schemas["MatchSide"];

/** Milliseconds left until `closes`, never below 0; `null` without a close. */
export function left(closes: number | null | undefined, now: number): number | null {
  if (closes === null || closes === undefined) {
    return null;
  }
  return Math.max(0, closes - now);
}

/** A countdown as `m:ss`, rounded up to the second. */
export function clock(ms: number): string {
  const seconds = Math.ceil(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** Where `at` lies between `from` and `to`, 0 to 100. */
export function along(at: number, from: number, to: number): number {
  if (to <= from) {
    return 0;
  }
  return Math.min(100, Math.max(0, ((at - from) / (to - from)) * 100));
}

/** How a match ended, as a badge: its text and its tone. */
export function outcome(match: MatchView): { text: string; tone: "good" | "warn" | "muted" } {
  if (match.status === "cancelled") {
    return { text: t("match.outcome.cancelled"), tone: "muted" };
  }
  const winner = match.choices.find((choice) => choice.choice === match.winner);
  switch (match.decided_by) {
    case "tie":
      return { text: t("match.outcome.tie"), tone: "muted" };
    case "no_votes":
      return { text: t("match.outcome.noVotes"), tone: "warn" };
    case "early":
      return { text: t("match.outcome.early", { percent: winner?.percent ?? 0 }), tone: "good" };
    default:
      return { text: `${winner?.percent ?? 0} %`, tone: "good" };
  }
}

/** The winner and the loser of a decided match. */
export function sides(match: MatchView): { winner: MatchView["choices"][number] | null; loser: MatchView["choices"][number] | null } {
  const winner = match.choices.find((choice) => choice.choice === match.winner) ?? null;
  const loser = match.choices.find((choice) => choice.choice !== match.winner) ?? null;
  return winner ? { winner, loser } : { winner: null, loser: null };
}
