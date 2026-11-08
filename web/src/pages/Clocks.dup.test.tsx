// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { t } from "../i18n";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent } from "../test/render";
import { Clocks } from "./Clocks";

const SLOT = {
  category_id: 1,
  method: "least_recently_played",
  enforce_rules: true,
  separation: {},
  linked_to_next: false,
};

describe("duplicating a clock", () => {
  beforeEach(() => {
    chooseStation(1);
  });

  it("copies its slots and looping under a new name", async () => {
    answer({
      "GET /api/v1/stations/{station_id}/clocks": [
        { id: 3, name: "Morning", looping: true, scheduled_hours: 12, slots: [SLOT, SLOT] },
      ],
      "GET /api/v1/categories": [{ id: 1, name: "90s", color: "#2e7d32", type_id: 1, station_ids: [1] }],
      "POST /api/v1/stations/{station_id}/clocks": {
        id: 9,
        name: "Morning (copy)",
        looping: true,
        scheduled_hours: 0,
        slots: [SLOT, SLOT],
      },
    });
    renderApp(<Clocks />);
    fireEvent.click(await screen.findByRole("button", { name: t("clocks.duplicateNamed", { name: "Morning" }) }));
    await waitFor(() => expect(sent("POST /api/v1/stations/{station_id}/clocks")).toHaveLength(1));
    const body = (
      sent("POST /api/v1/stations/{station_id}/clocks")[0]?.options as {
        body: { name: string; looping: boolean; slots: unknown[] };
      }
    ).body;
    expect(body.name).toBe(t("clocks.copyName", { name: "Morning" }));
    expect(body.looping).toBe(true);
    expect(body.slots).toHaveLength(2);
  });
});
