// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Schemas } from "../api";
import { t } from "../i18n";
import { clockTime } from "../lib/format";
import { navigate } from "../lib/router";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent, user } from "../test/render";
import { ExactTimes } from "./ExactTimes";

const MIN = 60_000;
// An hour ahead: its attempts can still be chosen.
const AT = Math.ceil(Date.now() / (60 * MIN)) * 60 * MIN + 60 * MIN;
const FROM = AT - 16 * MIN;

type Attempt = Schemas["Attempt"];

function attempt(number: number, rank: number, lengths: number[], ending: Partial<Attempt["ending"]> = {}): Attempt {
  let at = FROM;
  const tracks = lengths.map((length, index) => {
    const track = {
      track_id: index + 1,
      pick: index,
      slot: 20 + index,
      category_id: 3,
      start_unix_ms: at,
      next_ms: length * 1000,
      chosen: "slot" as const,
      step: index === 1 && number === 18 ? 3 : 0,
    };
    at += length * 1000;
    return track;
  });
  return {
    number,
    batch: 1,
    seed: "6869878787",
    rank,
    tracks,
    ending: {
      offset_ms: at - AT,
      in_window: true,
      faded: false,
      filling_ms: 0,
      relaxed_steps: 0,
      age_ms: 0,
      ...ending,
    },
    search: { slots: [21, 22, 23], options: [34, 51, 48], capped: false, combinations: 18_240, landing: 212, played_again: 4, found: number === 12 },
  };
}

const KEPT = attempt(12, 1, [241, 268, 225, 222]);
const OTHER = attempt(3, 2, [251, 233, 262, 214]);
const FADED = attempt(7, 3, [255, 290, 240, 233], { in_window: false, faded: true });

const SUMMARY: Schemas["CalculationSummary"] = {
  id: 40,
  computed_at_unix_ms: AT - 19 * MIN,
  trigger: "queue_changed",
  follows_id: null,
  fills: 10,
  batches: 1,
  stop: "landed",
  duration_ms: 412,
  kept: 12,
  ending: KEPT.ending,
  by_hand: false,
  chosen: null,
};

function detail(changes: Partial<Schemas["CalculationDetail"]> = {}): Schemas["CalculationDetail"] {
  return {
    ...SUMMARY,
    exact_at_unix_ms: AT,
    mode: "soft",
    window_from_unix_ms: AT - 10_000,
    window_cut_unix_ms: AT + 30_000,
    clock_name: "Hour",
    exact_slot: 0,
    from_unix_ms: FROM,
    latest: true,
    can_choose: true,
    attempts: [KEPT, OTHER, FADED],
    tracks: [
      { id: 1, artist: "Haddaway", title: "What Is Love" },
      { id: 2, artist: "Mylène Farmer", title: "Désenchantée" },
      { id: 3, artist: "Corona", title: "The Rhythm of the Night" },
      { id: 4, artist: "Eels", title: "Novocaine For The Soul" },
    ],
    categories: [{ id: 3, name: "Hits 90", color: "#2f6fd6" }],
    ...changes,
  };
}

function show(
  options: {
    choice?: Schemas["HandChoice"] | null;
    role?: "viewer" | "admin";
    calculation?: Partial<Schemas["CalculationDetail"]>;
    path?: string;
  } = {},
) {
  chooseStation(1);
  navigate(options.path ?? "/exact-times");
  answer({
    "GET /api/v1/stations/{station_id}/exact-times": {
      date: "2026-11-08",
      today: "2026-11-08",
      time_zone: "UTC",
      exact_times: [
        {
          at_unix_ms: AT,
          mode: "soft",
          window_from_unix_ms: AT - 10_000,
          window_cut_unix_ms: AT + 30_000,
          clock_name: "Hour",
          slot: 0,
          met: null,
          planned: false,
          upcoming: true,
          calculations: [
            { ...SUMMARY, id: 39, trigger: "first", computed_at_unix_ms: AT - 30 * MIN },
            SUMMARY,
            { ...SUMMARY, id: 41, trigger: "followed", follows_id: 40, fills: 0, batches: 0, stop: "followed", kept: null },
          ],
        },
      ],
      choice: options.choice ?? null,
    },
    "GET /api/v1/stations/{station_id}/exact-times/calculations/{id}": detail(options.calculation),
    "POST /api/v1/stations/{station_id}/exact-times/calculations/{id}/attempts/{n}/use": undefined,
    "DELETE /api/v1/stations/{station_id}/exact-times/choice": undefined,
    "POST /api/v1/stations/{station_id}/exact-times/recalculate": { id: 42 },
  });
  renderApp(<ExactTimes />, user(options.role ?? "admin"));
}

describe("ExactTimes", () => {
  it("shows the exact time, its calculations and the attempts of the latest, best first", async () => {
    show();
    const attempts = await screen.findByRole("heading", { name: /^Attempts of the .* calculation, best first$/ });
    const card = attempts.closest("section") as HTMLElement;
    // The kept one is marked, the others by rank.
    expect(await within(card).findByText(t("exactTimes.kept"))).toBeTruthy();
    expect(within(card).getByText("#3")).toBeTruthy();
    expect(within(card).getByText(t("exactTimes.endsFaded", { seconds: 28 }))).toBeTruthy();
    // The latest calculation, why it was made, how it ends, and the pick
    // that followed it on its card.
    expect(screen.getAllByText(t("exactTimes.triggerQueueChanged")).length).toBeGreaterThan(0);
    expect(screen.getByText(t("exactTimes.calculatedAt", { time: clockTime(SUMMARY.computed_at_unix_ms) }))).toBeTruthy();
    expect(screen.getAllByText(t("exactTimes.resultEarly", { seconds: 4 })).length).toBeGreaterThan(0);
    expect(
      screen.getByText(t("exactTimes.followedFor", { count: 1, time: clockTime(SUMMARY.computed_at_unix_ms) })),
    ).toBeTruthy();
    expect(screen.queryByText(t("exactTimes.triggerFollowed"))).toBeNull();
    // The detail of the one kept: its tracks and fit search.
    expect(await screen.findByRole("heading", { name: t("exactTimes.attemptTitle", { number: "12", rank: 1 }) })).toBeTruthy();
    expect(screen.getAllByText("What Is Love").length).toBeGreaterThan(0);
    expect(screen.getByText("34 · 51 · 48")).toBeTruthy();
    expect(screen.getByText(t("exactTimes.verdictKept"))).toBeTruthy();
    expect(screen.getByText(t("exactTimes.playingAuto"))).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("exactTimes.use") })).toBeNull();
  });

  it("shows the latest calculation only, the earlier ones behind a button above it", async () => {
    show();
    const earlier = await screen.findByRole("button", { name: t("exactTimes.earlier", { count: 1 }) });
    expect(earlier.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText(t("exactTimes.triggerFirst"))).toBeNull();
    fireEvent.click(earlier);
    expect(earlier.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText(t("exactTimes.triggerFirst"))).toBeTruthy();
    expect(earlier.textContent).toBe(t("exactTimes.hideEarlier"));
  });

  it("shows an earlier calculation a link asks for", async () => {
    show({ path: `/exact-times?at=${AT}&calculation=39` });
    expect(await screen.findByText(t("exactTimes.triggerFirst"))).toBeTruthy();
    const earlier = screen.getByRole("button", { name: t("exactTimes.hideEarlier") });
    expect(earlier.getAttribute("aria-expanded")).toBe("true");
    const card = screen.getByText(t("exactTimes.triggerFirst")).closest("button") as HTMLElement;
    expect(card.getAttribute("aria-pressed")).toBe("true");
  });

  it("recalculates the next exact time by hand", async () => {
    show();
    fireEvent.click(await screen.findByRole("button", { name: t("exactTimes.recalculate") }));
    await waitFor(() => expect(sent("POST /api/v1/stations/{station_id}/exact-times/recalculate")).toHaveLength(1));
    expect(sent("POST /api/v1/stations/{station_id}/exact-times/recalculate")[0]?.options).toMatchObject({
      params: { path: { station_id: 1 }, query: { exact_at_unix_ms: AT } },
    });
  });

  it("uses another attempt that lands in the window", async () => {
    show();
    fireEvent.click(await screen.findByText("#3"));
    const use = await screen.findByRole("button", { name: t("exactTimes.use") });
    fireEvent.click(use);
    await waitFor(() =>
      expect(sent("POST /api/v1/stations/{station_id}/exact-times/calculations/{id}/attempts/{n}/use")).toHaveLength(1),
    );
    expect(sent("POST /api/v1/stations/{station_id}/exact-times/calculations/{id}/attempts/{n}/use")[0]?.options).toMatchObject({
      params: { path: { station_id: 1, id: 40, n: 3 } },
    });
  });

  it("does not offer an attempt that fades a track out", async () => {
    show();
    fireEvent.click(await screen.findByText("#7"));
    expect(await screen.findByText(t("exactTimes.cannotUse"))).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("exactTimes.use") })).toBeNull();
    expect(screen.getByText(t("exactTimes.verdictFaded"))).toBeTruthy();
  });

  it("offers nothing to a viewer, nor on an outdated calculation", async () => {
    show({ role: "viewer" });
    fireEvent.click(await screen.findByText("#3"));
    await screen.findByRole("heading", { name: t("exactTimes.attemptTitle", { number: "3", rank: 2 }) });
    expect(screen.queryByRole("button", { name: t("exactTimes.use") })).toBeNull();
    expect(screen.queryByRole("button", { name: t("exactTimes.recalculate") })).toBeNull();
  });

  it("says when an attempt was chosen by hand, and goes back to automatic", async () => {
    show({
      choice: {
        calculation_id: 40,
        attempt: 3,
        exact_at_unix_ms: AT,
        user_id: 1,
        username: "ada",
        at_unix_ms: AT - 8 * MIN,
        ending: { offset_ms: 22_000, in_window: true, faded: false, filling_ms: 0, relaxed_steps: 0, age_ms: 0 },
      },
    });
    expect(await screen.findByText(t("exactTimes.byHandAttempt", { number: "3" }))).toBeTruthy();
    expect(screen.getByText(t("exactTimes.resultByHand"))).toBeTruthy();
    // The chosen attempt's ending, not the automatic best's.
    expect(screen.getByText(t("exactTimes.resultTitle", { number: "3", offset: "+22 s" }))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("exactTimes.backToAutomatic") }));
    await waitFor(() => expect(sent("DELETE /api/v1/stations/{station_id}/exact-times/choice")).toHaveLength(1));
  });

  it("explains an attempt of an outdated calculation", async () => {
    show({ calculation: { latest: false, can_choose: false } });
    fireEvent.click(await screen.findByText("#3"));
    expect(await screen.findByText(t("exactTimes.outdated"))).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("exactTimes.use") })).toBeNull();
  });
});
