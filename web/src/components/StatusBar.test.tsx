// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { act, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { startLive, stopLive } from "../lib/live";
import { chooseStation } from "../lib/station";
import { answer, renderApp } from "../test/render";
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
    expect(screen.getByText("Jamiroquai – Space Cowboy 2:31")).toBeTruthy();
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
    expect(screen.getByText("Daft Punk – Da Funk 0:00")).toBeTruthy();
    expect(screen.getByText("Deck B: playing, NEXT, Daft Punk – Da Funk 0:00")).toBeTruthy();
  });

  it("shows a gap, not zero, when no server could be read", async () => {
    answer({ "GET /api/v1/stations/{station_id}/listeners/now": { outputs: [], total: null } });
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
});
