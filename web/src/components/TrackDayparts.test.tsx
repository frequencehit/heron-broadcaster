// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Schemas } from "../api";
import { answer, renderApp, sent } from "../test/render";
import { DaypartsCell, TrackDaypartsCard, TrackDaypartsDialog, hasOwnDayparts } from "./TrackDayparts";

describe("DaypartsCell", () => {
  it("shows the track's own values, the categories' otherwise", () => {
    expect(hasOwnDayparts({ dayparts: null, daypart_days: null })).toBe(false);
    expect(hasOwnDayparts({ dayparts: 0 })).toBe(true);
    const { container, unmount } = renderApp(<DaypartsCell track={{ dayparts: null, daypart_days: null }} />);
    expect(container.textContent).toBe("–");
    expect(screen.getByTitle("Follows its categories")).toBeTruthy();
    unmount();
    const own = renderApp(<DaypartsCell track={{ dayparts: 0, daypart_days: 2 }} />);
    expect(own.container.textContent).toBe("off · 2 d");
    own.unmount();
    const half = renderApp(<DaypartsCell track={{ dayparts: 3, daypart_days: null }} />);
    expect(half.container.textContent).toBe("3 · cat.");
  });
});

const CATEGORIES = [
  { id: 1, name: "Current", separation: {} },
  { id: 2, name: "Gold", separation: { dayparts: 4, daypart_days: 1 } },
];

function track(changes: Partial<Schemas["TrackDetail"]>): Schemas["TrackDetail"] {
  return {
    id: 7,
    dayparts: null,
    daypart_days: null,
    categories: [
      { category_id: 1, name: "Current", weight: 1 },
      { category_id: 2, name: "Gold", weight: 1 },
    ],
    ...changes,
  } as Schemas["TrackDetail"];
}

describe("TrackDaypartsCard", () => {
  it("shows what applies in each category, and saves the track's own values", async () => {
    answer({
      "GET /api/v1/categories": CATEGORIES,
      "POST /api/v1/tracks/programming": { tracks: 1, changed: 1 },
    });
    const { container } = renderApp(<TrackDaypartsCard track={track({})} canEdit />);
    const rows = () => [...container.querySelectorAll("tbody tr")].map((row) => row.textContent);
    await waitFor(() => expect(rows()).toEqual(["Currentoffoff", "Gold41 d"]));
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    // Its own 2 replaces both categories'; the days stay theirs.
    fireEvent.change(screen.getByRole("combobox", { name: "Dayparts kept out" }), { target: { value: "2" } });
    expect(rows()).toEqual(["Current2 (the track's)off", "Gold2 (the track's)1 d"]);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sent("POST /api/v1/tracks/programming")).toHaveLength(1));
    expect(sent("POST /api/v1/tracks/programming")[0]?.options).toEqual({
      body: { selection: { track_ids: [7] }, dayparts: 2 },
    });
  });

  it("goes back to the categories' value", async () => {
    answer({
      "GET /api/v1/categories": CATEGORIES,
      "POST /api/v1/tracks/programming": { tracks: 1, changed: 1 },
    });
    renderApp(<TrackDaypartsCard track={track({ daypart_days: 0 })} canEdit />);
    const days = screen.getByRole("combobox", { name: "Same daypart, the following days" });
    expect((days as HTMLSelectElement).value).toBe("0");
    fireEvent.change(days, { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sent("POST /api/v1/tracks/programming")).toHaveLength(1));
    expect(sent("POST /api/v1/tracks/programming")[0]?.options).toEqual({
      body: { selection: { track_ids: [7] }, daypart_days: null },
    });
  });

  it("says when the track is in no category, and is read-only without the role", async () => {
    answer({ "GET /api/v1/categories": CATEGORIES });
    renderApp(<TrackDaypartsCard track={track({ categories: [] })} canEdit={false} />);
    expect(screen.getByText(/In no category/)).toBeTruthy();
    expect((screen.getByRole("combobox", { name: "Dayparts kept out" }) as HTMLSelectElement).disabled).toBe(true);
  });
});

describe("TrackDaypartsDialog", () => {
  it("sets one value, and sends the other back to the categories'", async () => {
    answer({ "POST /api/v1/tracks/programming": { tracks: 12, changed: 12 } });
    const selection = { filters: { labels_any: "3" } };
    renderApp(<TrackDaypartsDialog open onClose={() => {}} selection={selection} tracks={12} />);
    expect(screen.getByText("Daypart separation of 12 tracks")).toBeTruthy();
    const apply = screen.getByRole("button", { name: "Apply" }) as HTMLButtonElement;
    expect(apply.disabled).toBe(true);
    const separation = screen.getByRole("group", { name: "Dayparts kept out" });
    const days = screen.getByRole("group", { name: "Same daypart, the following days" });
    fireEvent.click(within(separation).getByRole("button", { name: "Set" }));
    expect(apply.disabled).toBe(true);
    fireEvent.change(screen.getByRole("combobox", { name: "Dayparts kept out" }), { target: { value: "0" } });
    fireEvent.click(within(days).getByRole("button", { name: "The category's" }));
    fireEvent.click(apply);
    await waitFor(() => expect(sent("POST /api/v1/tracks/programming")).toHaveLength(1));
    expect(sent("POST /api/v1/tracks/programming")[0]?.options).toEqual({
      body: { selection, dayparts: 0, daypart_days: null },
    });
  });
});
