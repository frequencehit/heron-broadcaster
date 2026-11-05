// SPDX-License-Identifier: AGPL-3.0-or-later

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { liveState, startLive, stopLive } from "./live";

/** A WebSocket the test drives. */
class FakeSocket {
  static all: FakeSocket[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  closed = false;
  constructor(readonly url: string) {
    FakeSocket.all.push(this);
  }
  close() {
    this.closed = true;
  }
  /** What the server sends. */
  receive(message: unknown) {
    this.onmessage?.({ data: JSON.stringify(message) });
  }
}

const player = (title: string) => ({ type: "state", player: { decks: [], queue: [], title } });

describe("the live socket across stations", () => {
  beforeEach(() => {
    FakeSocket.all = [];
    vi.useFakeTimers();
    vi.stubGlobal("WebSocket", FakeSocket);
    vi.stubGlobal("window", { location: { protocol: "http:", host: "heron" } });
  });
  afterEach(() => {
    stopLive();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("ignores the socket of the station left", () => {
    startLive();
    const first = FakeSocket.all[0];
    first?.onopen?.();
    first?.receive(player("first"));

    // Another station: the old socket is closed, the state emptied.
    stopLive();
    expect(first?.closed).toBe(true);
    expect(liveState().player).toBeUndefined();
    startLive();
    const second = FakeSocket.all[1];
    second?.onopen?.();
    second?.receive(player("second"));

    // What the old socket still delivers is not this station's.
    first?.receive(player("first"));
    first?.onclose?.();
    expect(liveState().connected).toBe(true);
    expect(liveState().player).toMatchObject({ title: "second" });

    // Its close opens no other socket, and the new one stays the one used.
    vi.advanceTimersByTime(30_000);
    expect(FakeSocket.all).toHaveLength(2);
    stopLive();
    expect(second?.closed).toBe(true);
  });

  it("connects again when its own socket drops", () => {
    startLive();
    const socket = FakeSocket.all[0];
    socket?.onopen?.();
    socket?.onclose?.();
    expect(liveState().connected).toBe(false);
    vi.advanceTimersByTime(1_000);
    expect(FakeSocket.all).toHaveLength(2);
  });

  it("keeps the machine's load while connected, and forgets it after", () => {
    startLive();
    const socket = FakeSocket.all[0];
    socket?.onopen?.();
    // Viewers never get one.
    expect(liveState().system).toBeNull();
    socket?.receive({ type: "system", cpu_percent: 12, mem_percent: 41, load_avg: [0.4, 0.5, 0.6], cores: 4 });
    expect(liveState().system).toEqual({ cpu_percent: 12, mem_percent: 41, load_avg: [0.4, 0.5, 0.6], cores: 4 });
    socket?.onclose?.();
    expect(liveState().system).toBeNull();
  });
});
