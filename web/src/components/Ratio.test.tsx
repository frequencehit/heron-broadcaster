// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { TrackSummary } from "../api";
import { t } from "../i18n";
import { answer, renderApp, sent } from "../test/render";
import { EnableDialog, RatioCounter, RatioDialog } from "./Ratio";

describe("RatioCounter", () => {
  it("draws the 10 steps, those that play and the next one", () => {
    const { container } = renderApp(<RatioCounter ratio={2} counter={8} />);
    const cells = [...container.querySelectorAll(".ratio-cell")];
    expect(cells.map((cell) => cell.textContent)).toEqual([
      "10plays",
      "9plays",
      "8held back",
      "7held back",
      "6held back",
      "5held back",
      "4held back",
      "3held back",
      "2held back",
      "1held back",
    ]);
    expect(container.querySelector(".ratio-cell.next")?.textContent).toBe("8held back");
  });
});

describe("EnableDialog", () => {
  it("proposes the ratio the track had, and resets its counters", () => {
    const onConfirm = vi.fn();
    renderApp(
      <EnableDialog open onClose={() => {}} onConfirm={onConfirm} name="Neon Orchard" count={1} proposed={2} />,
    );
    expect(screen.getByText(/Proposed: 0\.2/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("ratio.enableAt", { ratio: "0.2" }) }));
    expect(onConfirm).toHaveBeenLastCalledWith(2, true);
    // Another ratio, the counters kept.
    fireEvent.click(screen.getByRole("button", { name: "0.5" }));
    fireEvent.click(screen.getByRole("checkbox"));
    fireEvent.click(screen.getByRole("button", { name: t("ratio.enableAt", { ratio: "0.5" }) }));
    expect(onConfirm).toHaveBeenLastCalledWith(5, false);
  });

  it("proposes 1 when the tracks had different ratios", () => {
    renderApp(<EnableDialog open onClose={() => {}} onConfirm={() => {}} count={3} proposed={null} />);
    expect(screen.getByRole("button", { name: t("ratio.enableAt", { ratio: "1" }) })).toBeTruthy();
    // 0 is not offered: it would disable them.
    expect(screen.queryByRole("button", { name: "0" })).toBeNull();
  });
});

function track(id: number, play_ratio: number, enabled = true): TrackSummary {
  return { id, play_ratio, enabled } as TrackSummary;
}

describe("RatioDialog", () => {
  it("sets the ratio of a selection", async () => {
    answer({ "POST /api/v1/tracks/programming": { tracks: 2, changed: 2 } });
    const onClose = vi.fn();
    renderApp(
      <RatioDialog
        open
        onClose={onClose}
        selection={{ track_ids: [1, 2] }}
        tracks={2}
        known={[track(1, 2), track(2, 5)]}
      />,
    );
    expect(screen.getByText(/Today: 0\.2, 0\.5\./)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "0.3" }));
    fireEvent.click(screen.getByRole("button", { name: t("ratio.setTo", { ratio: "0.3" }) }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(sent("POST /api/v1/tracks/programming")[0]?.options).toEqual({
      body: { selection: { track_ids: [1, 2] }, play_ratio: 3 },
    });
  });

  it("asks before 0 disables them", async () => {
    answer({ "POST /api/v1/tracks/programming": { tracks: 2, changed: 2 } });
    renderApp(
      <RatioDialog open onClose={() => {}} selection={{ track_ids: [1, 2] }} tracks={2} known={[track(1, 2)]} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "0" }));
    fireEvent.click(screen.getByRole("button", { name: t("ratio.setTo", { ratio: "0" }) }));
    expect(sent("POST /api/v1/tracks/programming")).toHaveLength(0);
    expect(await screen.findByText(t("ratio.disableQuestion", { count: 2 }))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("track.disable") }));
    await waitFor(() => expect(sent("POST /api/v1/tracks/programming")).toHaveLength(1));
    expect(sent("POST /api/v1/tracks/programming")[0]?.options).toEqual({
      body: { selection: { track_ids: [1, 2] }, play_ratio: 0 },
    });
  });
});
