// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ALL_ON } from "../lib/modules";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent, user } from "../test/render";
import { DedicationOnAir, Requests } from "./Dedications";

const on = { ...ALL_ON, dedications: true };
const MODULES = { "GET /api/v1/stations/{id}/modules": { installation: on, station: on, on } };

const request = (id: number, from: string, more: Record<string, unknown> = {}) => ({
  id,
  track_id: 10 + id,
  artist: "Cassia Bloom",
  title: `Track ${id}`,
  from,
  to: "Julie",
  message: "Happy birthday!",
  status: "pending",
  reason: null,
  submitted_at_unix_ms: Date.now() - 120_000,
  decided_at_unix_ms: null,
  decided_by: null,
  planned_at_unix_ms: null,
  played_at_unix_ms: null,
  play_id: null,
  checks: [{ kind: "rules_kept", blocking: false }],
  ...more,
});

const LIST = {
  opens_at_unix_ms: Date.now() - 600_000,
  closes_at_unix_ms: Date.now() + 600_000,
  max_per_range: 30,
  counts: { pending: 2, accepted: 0, played: 1, refused: 0, received: 3 },
  dedications: [
    request(1, "Marc"),
    request(2, "Tom", { checks: [{ kind: "restriction", restriction_id: 4, blocking: true }] }),
    request(3, "Nadia", { status: "played", played_at_unix_ms: Date.now() - 60_000, decided_by: "ada" }),
  ],
  on_air: { from: "Nadia", to: "Papi Jean", message: "Happy 80th!" },
};

describe("dedications", () => {
  beforeEach(() => chooseStation(1));

  it("lists the requests with what the rules say, accepts and refuses", async () => {
    answer({
      ...MODULES,
      "GET /api/v1/stations/{station_id}/dedications": LIST,
      "POST /api/v1/stations/{station_id}/dedications/{id}/accept": request(1, "Marc", { status: "accepted" }),
      "POST /api/v1/stations/{station_id}/dedications/{id}/refuse": request(1, "Marc", { status: "refused" }),
    });
    renderApp(<Requests />);
    expect(await screen.findByText("Cassia Bloom – Track 1")).toBeTruthy();
    expect(screen.getByText("For Julie, from Marc")).toBeTruthy();
    expect(screen.getByText("Rules kept")).toBeTruthy();
    expect(screen.getByText("Held back by restriction 4")).toBeTruthy();
    expect(screen.getByText("3 / 30")).toBeTruthy();
    const accept = screen.getAllByText("Accept next");
    expect((accept[1] as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getAllByText("Best place")[0] as HTMLElement);
    await waitFor(() => expect(sent("POST /api/v1/stations/{station_id}/dedications/{id}/accept")).toHaveLength(1));
    expect(sent("POST /api/v1/stations/{station_id}/dedications/{id}/accept")[0]?.options).toMatchObject({
      body: { place: "best" },
    });
    fireEvent.change(screen.getAllByLabelText("Reason")[0] as HTMLElement, { target: { value: "just_played" } });
    fireEvent.click(screen.getAllByText("Refuse")[0] as HTMLElement);
    await waitFor(() => expect(sent("POST /api/v1/stations/{station_id}/dedications/{id}/refuse")).toHaveLength(1));
    expect(sent("POST /api/v1/stations/{station_id}/dedications/{id}/refuse")[0]?.options).toMatchObject({
      body: { reason: "just_played" },
    });
    // The accepted and played ones.
    fireEvent.click(screen.getByText("Accepted (1)"));
    expect(await screen.findByText("For Julie, from Nadia")).toBeTruthy();
    expect(screen.getByText("played")).toBeTruthy();
  });

  it("does not let viewers decide", async () => {
    answer({ ...MODULES, "GET /api/v1/stations/{station_id}/dedications": LIST });
    renderApp(<Requests />, user("viewer"));
    expect(await screen.findByText("Cassia Bloom – Track 1")).toBeTruthy();
    expect(screen.queryByText("Accept next")).toBeNull();
  });

  it("shows the dedication to read on air", async () => {
    answer({ ...MODULES, "GET /api/v1/stations/{station_id}/dedications": LIST });
    renderApp(<DedicationOnAir />);
    expect(await screen.findByText("For Papi Jean, from Nadia")).toBeTruthy();
    expect(screen.getByText("Happy 80th!")).toBeTruthy();
  });
});
