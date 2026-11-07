// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { PlayerState, QueueEntry } from "../api";
import { t } from "../i18n";
import type { LiveState } from "../lib/live";
import { chooseStation } from "../lib/station";
import { answer, renderApp, user } from "../test/render";
import { Dashboard } from "./Dashboard";

function entry(id: number): QueueEntry {
  return {
    entry_id: id,
    track: {
      title: `Song ${id}`,
      artist: "Artist",
      path: `song-${id}.flac`,
      cue_in_ms: 0,
      cue_out_ms: 180_000,
      duration_ms: 180_000,
      continues: false,
      no_metadata: false,
      over_intro: false,
      source: "rotation",
    },
  };
}

// Ten tracks waiting: more than the six On air shows by default.
const player: PlayerState = {
  decks: [],
  outputs: [],
  queue: Array.from({ length: 10 }, (_, index) => entry(index + 1)),
  paused: false,
  engine_started_at_unix_ms: 0,
  rendered_ms: 0,
  underruns: 0,
};

// The live state the page reads, instead of the WebSocket's.
const live = vi.hoisted(() => ({ state: undefined as unknown }));
live.state = {
  connected: true,
  player,
  positions: null,
  levels: null,
  events: [],
  eq: null,
  agc: null,
  system: null,
} satisfies LiveState;

vi.mock("../lib/live", async (original) => ({
  ...(await original<typeof import("../lib/live")>()),
  useLive: <T,>(select: (state: LiveState) => T) => select(live.state as LiveState),
}));

// The other cards' requests (outputs, listeners, engine) go unanswered.
function render(wholeQueue: boolean) {
  chooseStation(1);
  answer({});
  renderApp(<Dashboard />, { ...user("operator"), on_air_whole_queue: wholeQueue });
  return screen.getByRole("list", { name: t("nav.queue") });
}

describe("On air › Up next", () => {
  it("shows the first tracks and a button to the Queue page by default", () => {
    const list = render(false);
    expect(within(list).getAllByRole("listitem")).toHaveLength(6);
    expect(screen.getByRole("link", { name: t("dashboard.editQueue") })).toBeTruthy();
    expect(screen.getByRole("link", { name: t("queue.whole") })).toBeTruthy();
  });

  it("shows the whole queue, editable, without the button, when the user asks", () => {
    const list = render(true);
    expect(within(list).getAllByRole("listitem")).toHaveLength(10);
    expect(screen.queryByRole("link", { name: t("dashboard.editQueue") })).toBeNull();
    expect(screen.queryByRole("link", { name: t("queue.whole") })).toBeNull();
    const name = "Artist – Song 10";
    expect(within(list).getByRole("button", { name: t("queue.move", { name }) })).toBeTruthy();
    expect(within(list).getByRole("button", { name: t("queue.playNamedNext", { name }) })).toBeTruthy();
    expect(within(list).getByRole("button", { name: t("queue.removeNamed", { name }) })).toBeTruthy();
    // Adding tracks stays on the Queue page.
    expect(screen.queryByText(t("queuePage.add"))).toBeNull();
  });
});
