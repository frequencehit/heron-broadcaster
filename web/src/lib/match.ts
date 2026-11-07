// SPDX-License-Identifier: AGPL-3.0-or-later

// The match (docs/listener-interaction.md#match): the countdown to the
// close of its vote, and how a match was decided.

import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { type QueueEntry, type Schemas, api, unwrap } from "../api";
import { t } from "../i18n";
import { useModule } from "./modules";
import { here, useStation } from "./station";

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

/**
 * The queue entries locked by the match waiting or open: its vote's track,
 * A, B, and the announcement linked to the vote's track. Only the match
 * takes them out, or cancelling it (docs/listener-interaction.md#on-air).
 */
export function lockedEntries(match: MatchView | null | undefined, waiting: readonly QueueEntry[]): Set<number> {
  if (!match || (match.status !== "queued" && match.status !== "open")) {
    return new Set();
  }
  const locked = new Set(match.entry_ids);
  for (const entry of waiting) {
    if (entry.linked_to !== null && entry.linked_to !== undefined && locked.has(entry.linked_to)) {
      locked.add(entry.entry_id);
    }
  }
  return locked;
}

/** The match waiting or open, followed every 2 seconds. */
export function useCurrentMatch(enabled = true) {
  const station = useStation();
  return useQuery({
    queryKey: ["match", "current", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/match/current", { params: { path: here() } })),
    enabled: enabled && station !== null,
    refetchInterval: 2_000,
  });
}

/** The waiting entries the match in progress locks; none while the module is off. */
export function useLockedEntries(waiting: readonly QueueEntry[]): Set<number> {
  const on = useModule("match");
  const current = useCurrentMatch(on);
  const match = on ? current.data : null;
  return useMemo(() => lockedEntries(match, waiting), [match, waiting]);
}
