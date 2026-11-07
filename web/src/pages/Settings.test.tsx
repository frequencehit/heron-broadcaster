// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { LibraryTask } from "../api";
import { t } from "../i18n";
import { answer, renderApp, sent } from "../test/render";
import { KeptTagsCard } from "./Settings";

function task(changes: Partial<LibraryTask> = {}): LibraryTask {
  return {
    id: 7,
    request: { kind: "write_tag_overrides" },
    status: "pending",
    error: null,
    plan: null,
    report: null,
    relations: null,
    details: null,
    purge: null,
    covers: null,
    requested_by: "ada",
    created_at_unix_ms: 0,
    started_at_unix_ms: null,
    finished_at_unix_ms: null,
    ...changes,
  };
}

describe("Writing the tags kept in Heron into the files", () => {
  it("is not offered while no track has tags kept in Heron", async () => {
    answer({ "GET /api/v1/library/tag-overrides": { tracks: 0 } });
    renderApp(<KeptTagsCard />);
    await waitFor(() => expect(sent("GET /api/v1/library/tag-overrides")).toHaveLength(1));
    expect(screen.queryByText(t("settings.keptTags"))).toBeNull();
  });

  it("says how many tracks, asks first, then reports the batch", async () => {
    answer({
      "GET /api/v1/library/tag-overrides": { tracks: 3 },
      "POST /api/v1/library/tag-overrides/write": task(),
      "GET /api/v1/library/tasks/{id}": task({
        status: "done",
        finished_at_unix_ms: Date.now(),
        report: { batch_id: 12, done: 2, unchanged: 1, skipped: [] },
      }),
    });
    renderApp(<KeptTagsCard />);
    const button = await screen.findByRole("button", { name: t("settings.writeKeptTags", { count: 3 }) });
    expect(screen.getByRole("link", { name: t("nav.review") }).getAttribute("href")).toBe("/to-organize?tab=batches");

    fireEvent.click(button);
    expect(screen.getByText(t("settings.writeKeptTagsQuestion", { count: 3 }))).toBeTruthy();
    expect(sent("POST /api/v1/library/tag-overrides/write")).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: t("settings.writeKeptTagsConfirm") }));

    await waitFor(() => expect(sent("POST /api/v1/library/tag-overrides/write")).toHaveLength(1));
    expect(
      await screen.findByText(
        t("selection.written", { count: 2 }) + t("selection.alreadyRight", { count: 1 }),
      ),
    ).toBeTruthy();
    expect(sent("GET /api/v1/library/tasks/{id}")[0]?.options).toEqual({ params: { path: { id: 7 } } });
  });
});
