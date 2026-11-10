// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { act, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { startLive, stopLive } from "../lib/live";
import { chooseStation } from "../lib/station";
import { ALL_ON } from "../lib/modules";
import { DEFAULT_STATUS_BAR, type StatusBarSettings } from "../lib/statusbar";
import { answer, renderApp, sent, user } from "../test/render";
import { StatusBar } from "./StatusBar";

/** A WebSocket the test drives. */
class FakeSocket {
  static last: FakeSocket;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  constructor() {
    FakeSocket.last = this;
  }
  close() {}
  receive(message: unknown) {
    act(() => this.onmessage?.({ data: JSON.stringify(message) }));
  }
}

const track = (artist: string, title: string) => ({ entry_id: 1, track: { artist, title, path: "x.mp3" } });
const state = (decks: unknown[], paused = false) => ({
  type: "state",
  player: { decks, paused, queue: [], outputs: [], underruns: 0 },
});
const SYSTEM = { type: "system", cpu_percent: 12.4, mem_percent: 96, load_avg: [0.4, 0.5, 0.6], cores: 4 };

function open() {
  startLive();
  act(() => FakeSocket.last.onopen?.());
  return FakeSocket.last;
}

describe("StatusBar", () => {
  beforeEach(() => {
    vi.stubGlobal("WebSocket", FakeSocket);
    chooseStation(1);
    answer({
      "GET /api/v1/stations/{station_id}/listeners/now": {
        outputs: [],
        total: 42,
        record: { listeners: 128, day: "2026-10-25" },
      },
      "GET /api/v1/stations/{station_id}/shows": [],
    });
  });
  afterEach(() => {
    stopLive();
    vi.unstubAllGlobals();
  });

  it("shows both decks and the station's listeners", async () => {
    renderApp(<StatusBar />);
    const socket = open();
    socket.receive(
      state([
        { deck: "a", on_air: true, position_ms: 151_000, length_ms: 240_000, entry: track("Jamiroquai", "Space Cowboy") },
        { deck: "b", on_air: false, position_ms: 0, length_ms: 0, entry: null },
      ]),
    );
    expect(screen.getByText("Jamiroquai – Space Cowboy")).toBeTruthy();
    expect(screen.getByText("2:31")).toBeTruthy();
    expect(screen.getByText("[ON AIR]")).toBeTruthy();
    expect(screen.queryByText("[NEXT]")).toBeNull();
    // Deck A fills as its track plays; empty deck B has no fill.
    const lines = document.querySelectorAll<HTMLElement>(".statusbar-deck-progress");
    expect([...lines].map((line) => line.style.width)).toEqual(["62.92%"]);
    // The decks open the On air page, the listeners the Statistics page.
    const links = [...document.querySelectorAll<HTMLAnchorElement>(".statusbar a")];
    expect(links.map((link) => link.getAttribute("href"))).toEqual(["/", "/", "/statistics"]);
    // What a screen reader gets, in place of the symbols.
    expect(screen.getByText("Deck B: empty")).toBeTruthy();
    expect(screen.getByText("Deck A: playing, ON AIR, Jamiroquai – Space Cowboy 2:31")).toBeTruthy();
    await waitFor(() => expect(screen.getByText("listeners 42")).toBeTruthy());
    // The record, its day on hovering.
    expect(screen.getByText("record 128").getAttribute("title")).toBe("Record reached on Oct 25, 2026");
  });

  it("marks the deck loaded next", () => {
    renderApp(<StatusBar />);
    open().receive(
      state([
        { deck: "a", on_air: true, position_ms: 151_000, length_ms: 240_000, entry: track("Jamiroquai", "Space Cowboy") },
        { deck: "b", on_air: false, position_ms: 0, length_ms: 200_000, entry: track("Daft Punk", "Da Funk") },
      ]),
    );
    expect(screen.getByText("[NEXT]")).toBeTruthy();
    expect(screen.getByText("Daft Punk – Da Funk")).toBeTruthy();
    expect(screen.getByText("0:00")).toBeTruthy();
    expect(screen.getByText("Deck B: playing, NEXT, Daft Punk – Da Funk 0:00")).toBeTruthy();
  });

  it("colours the intro and counts down to the vocals on the deck on air, urgent in its last 5 seconds", () => {
    renderApp(<StatusBar />);
    const socket = open();
    const withIntro = (artist: string, title: string) => ({
      entry_id: 2,
      track: { artist, title, path: "x.mp3", cue_in_ms: 1_000, cue_out_ms: 241_000, intro_ms: 21_000 },
    });
    const decks = (position: number) => [
      { deck: "a", on_air: true, position_ms: position, length_ms: 240_000, entry: withIntro("Daft Punk", "One More Time") },
      { deck: "b", on_air: false, position_ms: 0, length_ms: 240_000, entry: withIntro("Cerrone", "Supernature") },
    ];
    socket.receive(state(decks(12_500)));
    // The intro is 20 s after the cue-in: its zone on both decks, played on A.
    const zones = [...document.querySelectorAll<HTMLElement>(".statusbar-deck-intro")];
    expect(zones.map((zone) => zone.style.width)).toEqual(["8.33%", "8.33%"]);
    const played = [...document.querySelectorAll<HTMLElement>(".statusbar-deck-intro-progress")];
    expect(played.map((zone) => zone.style.width)).toEqual(["5.21%"]);
    // The countdown takes the place of the elapsed time on A only.
    const countdown = screen.getByText("intro −0:08");
    expect(countdown.className).toContain("statusbar-countdown-intro");
    expect(screen.queryByText("0:12")).toBeNull();
    expect(screen.getByText("0:00").className).toContain("statusbar-deck-elapsed");
    expect(screen.getByText("Deck A: playing, ON AIR, Daft Punk – One More Time 0:12, intro, 8 seconds left")).toBeTruthy();
    // The last 5 seconds are urgent: they blink (or stay outlined with reduced motion, in the stylesheet).
    socket.receive(state(decks(16_000)));
    expect(screen.getByText("intro −0:04").className).toContain("statusbar-countdown-blink");
    // Once the vocals start, the elapsed time is back.
    socket.receive(state(decks(20_000)));
    expect(screen.getByText("0:20").className).toContain("statusbar-deck-elapsed");
  });

  it("counts down the last 10 seconds before the next track starts", () => {
    renderApp(<StatusBar />);
    const socket = open();
    const onAir = (position: number, nextStart: number | null) => ({
      deck: "a",
      on_air: true,
      position_ms: position,
      length_ms: 240_000,
      entry: { ...track("Jamiroquai", "Space Cowboy"), next_start_ms: nextStart },
    });
    socket.receive(state([onAir(220_000, 230_500)]));
    expect(screen.getByText("3:40").className).toContain("statusbar-deck-elapsed");
    // The engine's transition, not the end of the track.
    socket.receive(state([onAir(221_000, 230_500)]));
    expect(screen.getByText("−0:10").className).toContain("statusbar-countdown-end");
    expect(screen.getByText("Deck A: playing, ON AIR, Jamiroquai – Space Cowboy 3:41, next track in 10 seconds")).toBeTruthy();
    // Without it, the end of the track.
    socket.receive(state([onAir(231_000, null)]));
    expect(screen.getByText("−0:09")).toBeTruthy();
  });

  it("shows a gap, not zero, when no server could be read", async () => {
    answer({
      "GET /api/v1/stations/{station_id}/listeners/now": { outputs: [], total: null },
      "GET /api/v1/stations/{station_id}/shows": [],
    });
    renderApp(<StatusBar />);
    open().receive(state([]));
    await waitFor(() => expect(screen.getByText("listeners –")).toBeTruthy());
  });

  it("shows the machine's load to those who receive it, in the alert colour when high", () => {
    renderApp(<StatusBar />);
    const socket = open();
    socket.receive(state([]));
    expect(screen.queryByText(/^cpu/)).toBeNull();
    socket.receive(SYSTEM);
    expect(screen.getByText("cpu 12%").className).toContain("statusbar-ok");
    expect(screen.getByText("mem 96%").className).toContain("statusbar-bad");
    expect(screen.getByText(/^load 0[.,]4 0[.,]5 0[.,]6$/)).toBeTruthy();
  });

  it("says when the engine cannot be reached", () => {
    renderApp(<StatusBar />);
    open().receive({ type: "state", player: null });
    expect(screen.getByText("engine ?")).toBeTruthy();
  });

  it("greys out and says offline while the live connection is down", () => {
    renderApp(<StatusBar />);
    const socket = open();
    socket.receive(state([]));
    expect(screen.queryByText("offline")).toBeNull();
    act(() => socket.onclose?.());
    expect(screen.getByText("offline")).toBeTruthy();
    expect(screen.getByRole("status").className).toContain("statusbar-offline");
  });

  const as = (settings: Partial<StatusBarSettings>) => ({ ...user(), status_bar: { ...DEFAULT_STATUS_BAR, ...settings } });
  const both = [
    { deck: "a", on_air: true, position_ms: 83_000, length_ms: 240_000, entry: track("Jamiroquai", "Space Cowboy") },
    { deck: "b", on_air: false, position_ms: 0, length_ms: 200_000, entry: track("Daft Punk", "Da Funk") },
  ];

  it("follows the user's settings: the deck on air only, the time chosen, the parts left out", () => {
    renderApp(<StatusBar />, as({ on_air_only: true, time: "elapsed_remaining_total", listeners: false, system: false }));
    const socket = open();
    socket.receive(state(both));
    socket.receive(SYSTEM);
    expect(screen.getByText("Jamiroquai – Space Cowboy")).toBeTruthy();
    expect(screen.queryByText("Daft Punk – Da Funk")).toBeNull();
    expect(screen.getByText("1:23 / −2:37 / 4:00")).toBeTruthy();
    expect(screen.queryByText(/^listeners/)).toBeNull();
    expect(screen.queryByText(/^cpu/)).toBeNull();
  });

  it("takes the colours chosen for the track and the intro", () => {
    renderApp(<StatusBar />, as({ track_color: "#112233", intro_color: "#445566" }));
    open().receive(state(both));
    const deck = document.querySelector<HTMLElement>(".statusbar-deck-a")!;
    expect(deck.style.getPropertyValue("--deck")).toBe("#112233");
    expect(deck.style.getPropertyValue("--statusbar-intro")).toBe("#445566");
  });

  it("goes, and gives the page its room back, when the user or the module turns it off; the preview stays", async () => {
    const { unmount } = renderApp(<StatusBar />, as({ show: false }));
    open().receive(state(both));
    expect(screen.queryByRole("status")).toBeNull();
    expect(document.documentElement.dataset.statusbar).toBe("off");
    unmount();
    stopLive();
    renderApp(<StatusBar settings={{ ...DEFAULT_STATUS_BAR, show: false }} preview />);
    open().receive(state(both));
    expect(screen.getByRole("status").className).toContain("statusbar-preview");
    expect(document.documentElement.dataset.statusbar).toBeUndefined();
  });

  it("goes for every user when the station turns its module off", async () => {
    answer({
      "GET /api/v1/stations/{station_id}/listeners/now": { outputs: [], total: 1 },
      "GET /api/v1/stations/{station_id}/shows": [],
      "GET /api/v1/stations/{id}/modules": { on: { ...ALL_ON, status_bar: false }, own: { ...ALL_ON, status_bar: false } },
    });
    renderApp(<StatusBar />);
    open().receive(state(both));
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
    expect(document.documentElement.dataset.statusbar).toBe("off");
  });

  it("shows the last track of a type, the queue, the outputs and the next fixed time", async () => {
    const now = Date.now();
    answer({
      "GET /api/v1/stations/{station_id}/listeners/now": { outputs: [], total: 1 },
      "GET /api/v1/stations/{station_id}/history": [{ id: 9, artist: "Justice", title: "D.A.N.C.E.", deck: "a", source: "rotation", started_at_unix_ms: 0 }],
      "GET /api/v1/stations/{station_id}/shows": [
        {
          module: "match",
          settings: { enabled: true, website: { title: "" } },
          status: { open: false, next_opening_unix_ms: now + 12 * 60_000 - 1_000 },
        },
      ],
    });
    renderApp(<StatusBar />, as({ history: true, history_type_id: 2, queue: true, outputs: true, listeners: false }));
    open().receive({
      type: "state",
      player: {
        decks: both,
        paused: false,
        underruns: 0,
        queue: [
          { entry_id: 5, next_start_ms: 175_000, track: { path: "a.mp3", cue_in_ms: 0, cue_out_ms: 180_000 } },
          { entry_id: 6, track: { path: "b.mp3", cue_in_ms: 0, cue_out_ms: 125_000 } },
        ],
        outputs: [
          { id: 1, name: "MP3", enabled: true, state: "connected", bytes_sent: 0, dropped_chunks: 0 },
          { id: 2, name: "AAC", enabled: true, state: "reconnecting", bytes_sent: 0, dropped_chunks: 0 },
        ],
      },
    });
    await waitFor(() => expect(screen.getByText("Justice – D.A.N.C.E.")).toBeTruthy());
    expect(sent("GET /api/v1/stations/{station_id}/history")[0]?.options).toMatchObject({
      params: { query: { finished: true, limit: 1, category_type_id: 2 } },
    });
    expect(screen.getByText("queue 5:00").getAttribute("href")).toBe("/queue");
    const outputs = screen.getByText("outputs 1/2");
    expect([outputs.className.includes("statusbar-bad"), outputs.getAttribute("title")]).toEqual([true, "Not connected: AAC"]);
    const next = await screen.findByText("Match in 12 min");
    expect(next.getAttribute("href")).toBe("/shows/match");
  });
});
