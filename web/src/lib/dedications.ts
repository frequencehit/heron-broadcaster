// SPDX-License-Identifier: AGPL-3.0-or-later

// Dedications (docs/listener-interaction.md#dedications): what the rules
// say about a waiting request, and the reasons of a refusal.

import type { Schemas } from "../api";
import { type PlainKey, t } from "../i18n";

export type Dedication = Schemas["Dedication"];
export type DedicationList = Schemas["DedicationList"];
export type DedicationSettings = Schemas["DedicationSettings"];
export type Check = Schemas["DedicationCheck"];
export type Place = Schemas["DedicationPlace"];
export type Reason = Schemas["DedicationReason"];
export type DedicationStatus = Schemas["DedicationStatus"];

/** A check as a badge: its text and its tone. `time` writes a time of day. */
export function checkBadge(check: Check, time: (unixMs: number) => string): { text: string; tone: "good" | "warn" | "bad" } {
  const played = { minutes: Math.max(0, check.minutes_ago ?? 0), separation: check.separation_min ?? 0 };
  switch (check.kind) {
    case "artist_played":
      return { text: t("dedications.check.artist", played), tone: "warn" };
    case "title_played":
      return { text: t("dedications.check.title", played), tone: "warn" };
    case "track_played":
      return { text: t("dedications.check.track", played), tone: "warn" };
    case "in_queue":
      return { text: t("dedications.check.inQueue", { time: check.at_unix_ms ? time(check.at_unix_ms) : "…" }), tone: "warn" };
    case "restriction":
      return { text: t("dedications.check.restriction", { id: check.restriction_id ?? "" }), tone: "bad" };
    case "unplayable":
      return { text: t("dedications.check.unplayable"), tone: "bad" };
    default:
      return { text: t("dedications.check.kept"), tone: "good" };
  }
}

/** Whether a request can be accepted: nothing blocks it. */
export function acceptable(dedication: Dedication): boolean {
  return dedication.status === "pending" && !dedication.checks.some((check) => check.blocking);
}

/** The reasons an operator gives, `null` for none. */
export const REASONS: { value: Reason | null; label: PlainKey }[] = [
  { value: null, label: "dedications.reason.none" },
  { value: "just_played", label: "dedications.reason.just_played" },
  { value: "unavailable", label: "dedications.reason.unavailable" },
  { value: "inappropriate", label: "dedications.reason.inappropriate" },
];

/** A refusal's reason, Heron's own included. */
export function reasonLabel(reason: Reason | null | undefined): PlainKey {
  switch (reason) {
    case "just_played":
    case "unavailable":
    case "inappropriate":
    case "closed":
    case "removed":
      return `dedications.reason.${reason}`;
    default:
      return "dedications.reason.none";
  }
}
