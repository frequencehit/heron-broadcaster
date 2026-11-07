// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { afterEach, describe, expect, it, vi } from "vitest";
import type { QueueEntry } from "../api";
import { startTimes } from "./QueueList";

function entry(entry_id: number, length_ms: number, start_at_unix_ms: number | null = null): QueueEntry {
  return { entry_id, track: { cue_in_ms: 0, cue_out_ms: length_ms, start_at_unix_ms } } as unknown as QueueEntry;
}

describe("startTimes", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts an exact time at its time when what is before it runs over, and the rest follows", () => {
    vi.useFakeTimers({ now: 0 });
    const starts = startTimes([entry(1, 60_000), entry(2, 37_000, 50_000), entry(3, 10_000)], 0, null);
    expect(starts).toEqual([0, 50_000, 87_000]);
  });

  it("starts an exact time earlier when what is before it ends earlier", () => {
    vi.useFakeTimers({ now: 0 });
    expect(startTimes([entry(1, 30_000), entry(2, 37_000, 50_000)], 0, null)).toEqual([0, 30_000]);
  });

  it("starts each track where the one before hands over to it", () => {
    vi.useFakeTimers({ now: 0 });
    const crossfaded = { ...entry(1, 60_000), next_start_ms: 54_000 } as QueueEntry;
    expect(startTimes([crossfaded, entry(2, 30_000), entry(3, 10_000)], 0, null)).toEqual([0, 54_000, 84_000]);
  });

  it("starts an exact time at its time when the first start is unknown", () => {
    vi.useFakeTimers({ now: 0 });
    expect(startTimes([entry(1, 30_000), entry(2, 37_000, 50_000), entry(3, 1_000)], null, null)).toEqual([
      null,
      50_000,
      87_000,
    ]);
  });
});
