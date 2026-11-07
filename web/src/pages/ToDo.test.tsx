// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { TrackSummary } from "../api";
import { type PlainKey, t } from "../i18n";
import { chooseStation } from "../lib/station";
import { answer, renderApp, sent } from "../test/render";
import { ToDo } from "./ToDo";

const SONG = {
  id: 7,
  path: "Artist/Song.flac",
  title: "Song",
  artist: "Artist",
  album: "Album",
} as TrackSummary;

type TracksQuery = { params: { query: { filter: string; sort: string; desc: boolean; active_songs: boolean } } };

function show(coverArt = true) {
  chooseStation(1);
  answer({
    "GET /api/v1/stations/{id}/modules": { on: { cover_art: coverArt, review: true } },
    "GET /api/v1/library/todo": { to_identify: 2, no_intro: 3, no_hook: 0, no_cover: coverArt ? 1 : null },
    "GET /api/v1/library/review": { items: [], total: 0 },
    "GET /api/v1/covers/candidates": [],
    "GET /api/v1/tracks": { items: [SONG], total: 1 },
  });
  renderApp(<ToDo />);
}

const tab = (label: PlainKey) => screen.getByRole("button", { name: new RegExp(`^${t(label)}`) });

afterEach(() => window.history.replaceState(null, "", "/"));

describe("To do", () => {
  it("shows each tab with its count, to identify first", async () => {
    show();
    await waitFor(() => expect(within(tab("todo.tab.intros")).getByText("3")).toBeTruthy());
    expect(within(tab("todo.tab.identify")).getByText("2")).toBeTruthy();
    expect(within(tab("todo.tab.covers")).getByText("1")).toBeTruthy();
    // Nothing missing: no count.
    expect(tab("todo.tab.hooks").querySelector(".count")).toBeNull();
    expect(tab("todo.tab.identify").getAttribute("aria-selected")).toBe("true");
    expect(sent("GET /api/v1/tracks")).toHaveLength(0);
  });

  it("lists the songs missing an intro, linked to their page, sorted by title on request", async () => {
    show();
    fireEvent.click(tab("todo.tab.intros"));
    const link = await screen.findByRole("link", { name: "Song" });
    expect(link.getAttribute("href")).toBe("/library/7");
    const query = (sent("GET /api/v1/tracks")[0]?.options as TracksQuery).params.query;
    expect(query).toMatchObject({ active_songs: true, filter: "no_intro", sort: "artist", desc: false });

    fireEvent.change(screen.getByRole("combobox"), { target: { value: "title" } });
    await waitFor(() =>
      expect(sent("GET /api/v1/tracks").some((call) => (call.options as TracksQuery).params.query.sort === "title")).toBe(
        true,
      ),
    );
  });

  it("links a song without a cover to its details", async () => {
    show();
    fireEvent.click(tab("todo.tab.covers"));
    const link = await screen.findByRole("link", { name: "Song" });
    expect(link.getAttribute("href")).toBe("/library/7?tab=details");
    expect((sent("GET /api/v1/tracks")[0]?.options as TracksQuery).params.query.filter).toBe("no_cover");
  });

  it("has no covers tab without cover art", async () => {
    show(false);
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: new RegExp(`^${t("todo.tab.covers")}`) })).toBeNull(),
    );
    expect(within(tab("todo.tab.intros")).getByText("3")).toBeTruthy();
  });
});
