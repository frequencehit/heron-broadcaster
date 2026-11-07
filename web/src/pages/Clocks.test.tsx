// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { t } from "../i18n";
import { chooseStation } from "../lib/station";
import { Failure, answer, renderApp, sent } from "../test/render";
import { ClockEditor } from "./Clocks";

const CLOCK = {
  id: 3,
  name: "Morning",
  looping: false,
  scheduled_hours: 0,
  slots: [{ category_id: 1, method: "least_recently_played", enforce_rules: true, separation: {}, linked_to_next: false }],
};

function api(put: unknown = (options: { body: unknown }) => ({ ...CLOCK, ...(options.body as object) })) {
  answer({
    "GET /api/v1/stations/{station_id}/clocks/{id}": CLOCK,
    "GET /api/v1/categories": [{ id: 1, name: "90s", color: "#2e7d32", type_id: null, station_ids: [1] }],
    "GET /api/v1/stations/{station_id}/settings/rotation": {},
    "GET /api/v1/category-types": [],
    "POST /api/v1/stations/{station_id}/clocks/preview": { slots: [], short: [] },
    "PUT /api/v1/stations/{station_id}/clocks/{id}": put,
  });
}

describe("ClockEditor", () => {
  beforeEach(() => {
    chooseStation(1);
  });

  it("saves a clock by itself a moment after a change, with no save button", async () => {
    api();
    renderApp(<ClockEditor id={3} />);
    const name = await screen.findByDisplayValue("Morning");
    expect(screen.queryByRole("button", { name: t("common.save") })).toBeNull();
    fireEvent.change(name, { target: { value: "Morning show" } });
    expect(screen.getByText(t("clocks.saving"))).toBeTruthy();
    await waitFor(() => expect(sent("PUT /api/v1/stations/{station_id}/clocks/{id}")).toHaveLength(1), {
      timeout: 3000,
    });
    const body = (sent("PUT /api/v1/stations/{station_id}/clocks/{id}")[0]?.options as { body: { name: string } })
      .body;
    expect(body.name).toBe("Morning show");
  });

  it("says why a change was not saved", async () => {
    api(new Failure(422, "invalid", "exact times out of order"));
    renderApp(<ClockEditor id={3} />);
    fireEvent.change(await screen.findByDisplayValue("Morning"), { target: { value: "Late" } });
    expect(
      await screen.findByText(t("clocks.notSaved", { error: "exact times out of order" }), undefined, {
        timeout: 3000,
      }),
    ).toBeTruthy();
  });

  it("creates a new clock with its button", () => {
    api();
    renderApp(<ClockEditor id={null} />);
    expect(screen.getByRole("button", { name: t("clocks.create") })).toBeTruthy();
  });
});
