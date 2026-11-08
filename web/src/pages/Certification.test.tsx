// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { t } from "../i18n";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent } from "../test/render";
import { DeleteDay } from "./Certification";

describe("DeleteDay", () => {
  beforeEach(() => {
    chooseStation(1);
  });

  it("deletes a server's day after asking", async () => {
    answer({ "DELETE /api/v1/stations/{station_id}/certification/days/{day}": { lines: 5, gaps: 0 } });
    renderApp(<DeleteDay day="2026-11-07" serverId={3} server="Main" />);
    fireEvent.click(screen.getByRole("button", { name: t("certification.deleteDay") }));
    const question = await screen.findByText(t("certification.deleteDayQuestion", { day: "2026-11-07", server: "Main" }));
    const dialog = question.closest("dialog") as HTMLElement;
    fireEvent.click(within(dialog).getByRole("button", { name: t("certification.deleteDay") }));
    await waitFor(() =>
      expect(sent("DELETE /api/v1/stations/{station_id}/certification/days/{day}")).toHaveLength(1),
    );
    const options = sent("DELETE /api/v1/stations/{station_id}/certification/days/{day}")[0]?.options as {
      params: { path: { day: string }; query: { server_id: number } };
    };
    expect(options.params.path.day).toBe("2026-11-07");
    expect(options.params.query.server_id).toBe(3);
  });
});
