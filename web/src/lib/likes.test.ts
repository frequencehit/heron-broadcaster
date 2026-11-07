// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { MIN_VOTES, isRanked, likeShare, likedFilter, shareLabel } from "./likes";

describe("likes", () => {
  it("is the share of likes among the votes", () => {
    expect(likeShare(3, 1)).toBe(0.75);
    expect(likeShare(0, 4)).toBe(0);
    expect(likeShare(0, 0)).toBeNull();
    expect(shareLabel(0.79)).toMatch(/^79\s?%$/);
  });

  it("ranks a track from 10 votes", () => {
    expect(MIN_VOTES).toBe(10);
    expect(isRanked(6, 3)).toBe(false);
    expect(isRanked(6, 4)).toBe(true);
  });

  it("filters the track list on the share and its period", () => {
    expect(likedFilter("low", null)).toEqual({ liked_max: 39, votes_min: 10 });
    expect(likedFilter("high", "7")).toEqual({ liked_min: 70, votes_min: 10, likes_days: 7 });
    expect(likedFilter("constructor", "12")).toEqual({});
    expect(likedFilter(null, "365")).toEqual({ likes_days: 365 });
  });
});
