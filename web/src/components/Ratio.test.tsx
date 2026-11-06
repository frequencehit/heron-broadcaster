// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { TrackSummary } from "../api";
import { t } from "../i18n";
import { answer, renderApp, sent } from "../test/render";
import { DisableDialog, EnableDialog, RatioCounter, RatioDialog, RatioImportDialog } from "./Ratio";

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

describe("on a station", () => {
  it("sets the ratio of a selection on the station shown", async () => {
    answer({ "POST /api/v1/tracks/programming": { tracks: 1, changed: 1 } });
    const onClose = vi.fn();
    renderApp(
      <RatioDialog
        open
        onClose={onClose}
        selection={{ track_ids: [1] }}
        tracks={1}
        known={[{ ...track(1, 10), station_ratio: { ratio: 3 } } as TrackSummary]}
        station={{ id: 2, name: "Hits 80" }}
      />,
    );
    expect(screen.getByText(t("ratio.ofTracksOn", { count: 1, station: "Hits 80" }))).toBeTruthy();
    expect(screen.getByText(/Today: 0\.3\./)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "0.5" }));
    fireEvent.click(screen.getByRole("button", { name: t("ratio.setTo", { ratio: "0.5" }) }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(sent("POST /api/v1/tracks/programming")[0]?.options).toEqual({
      body: { selection: { track_ids: [1] }, play_ratio: 5, station_id: 2 },
    });
  });

  it("disables on that station only", () => {
    const onConfirm = vi.fn();
    renderApp(
      <DisableDialog
        open
        onClose={() => {}}
        onConfirm={onConfirm}
        name="Neon Orchard"
        count={1}
        trackIds={[]}
        previous={3}
        station="Hits 80"
      />,
    );
    expect(screen.getByText(t("ratio.disableOneOn", { name: "Neon Orchard", station: "Hits 80" }))).toBeTruthy();
    expect(screen.getByText(t("ratio.disableQuestionOn", { count: 1, station: "Hits 80" }))).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("ratio.disableHere") }));
    expect(onConfirm).toHaveBeenCalled();
  });

  it("proposes the default when the station had no ratio of its own", () => {
    const onConfirm = vi.fn();
    renderApp(
      <EnableDialog
        open
        onClose={() => {}}
        onConfirm={onConfirm}
        name="Neon Orchard"
        count={1}
        proposed={null}
        fallback={4}
        station="Hits 80"
      />,
    );
    expect(screen.getByText(/the track's default ratio/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("ratio.enableAt", { ratio: "0.4" }) }));
    expect(onConfirm).toHaveBeenLastCalledWith(4, true);
  });
});

describe("RatioImportDialog", () => {
  it("checks the file, reports, then sets the ratios", async () => {
    const report = {
      dry_run: true,
      matched_by: "path",
      columns: ["fm"],
      unknown_columns: ["nope"],
      rows: 3,
      matched: 2,
      values: 2,
      changed: 0,
      unmatched: [{ line: 4, name: "Gone.wav", reason: "not_found" }],
      unmatched_total: 1,
      invalid: [{ line: 3, column: "fm", value: "0.25" }],
      invalid_total: 1,
    };
    answer({
      "POST /api/v1/tracks/ratios/import": (options: unknown) => {
        const dryRun = (options as { params: { query: { dry_run: boolean } } }).params.query.dry_run;
        return { ...report, dry_run: dryRun, changed: dryRun ? 0 : 2 };
      },
    });
    const onClose = vi.fn();
    const { container } = renderApp(<RatioImportDialog open onClose={onClose} />);
    const input = container.ownerDocument.querySelector<HTMLInputElement>("input[type=file]");
    const file = new File(["path;fm;nope\nA.wav;0,2;x\n"], "ratios.csv", { type: "text/csv" });
    fireEvent.change(input as HTMLInputElement, { target: { files: [file] } });
    expect(await screen.findByText(t("ratioImport.unmatched", { count: 1 }))).toBeTruthy();
    expect(screen.getByText(t("ratioImport.invalid", { count: 1 }))).toBeTruthy();
    expect(screen.getByText(t("ratioImport.unknownColumns", { columns: "nope" }))).toBeTruthy();
    expect(sent("POST /api/v1/tracks/ratios/import")[0]?.options).toMatchObject({
      params: { query: { dry_run: true } },
      body: "path;fm;nope\nA.wav;0,2;x\n",
      headers: { "Content-Type": "text/csv" },
    });
    fireEvent.click(screen.getByRole("button", { name: t("ratioImport.apply", { count: 2 }) }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(sent("POST /api/v1/tracks/ratios/import")[1]?.options).toMatchObject({
      params: { query: { dry_run: false } },
    });
  });
});
