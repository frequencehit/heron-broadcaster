// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ALL_ON } from "../lib/modules";
import { chooseStation } from "../lib/station";
import { answer, renderApp } from "../test/render";
import { LikedCell, LikesWeekCard, NowLikes, TrackLikesCard } from "./Likes";

function modules(likes: boolean) {
  const on = { ...ALL_ON, likes };
  return { "GET /api/v1/stations/{id}/modules": { installation: on, station: on, on } };
}

describe("likes", () => {
  beforeEach(() => chooseStation(1));

  it("shows the share with its votes, or the votes alone below 10", () => {
    const { rerender } = renderApp(<LikedCell likes={483} dislikes={128} />);
    expect(screen.getByText("79%")).toBeTruthy();
    expect(screen.getByText("611 votes")).toBeTruthy();
    rerender(<LikedCell likes={4} dislikes={3} />);
    expect(screen.getByText("7 votes, not ranked yet")).toBeTruthy();
  });

  it("gives a track's votes per period and per play", async () => {
    answer({
      ...modules(true),
      "GET /api/v1/stations/{station_id}/likes/tracks/{id}": {
        week: { likes: 121, dislikes: 27 },
        days_30: { likes: 483, dislikes: 128 },
        total: { likes: 1447, dislikes: 457 },
        min_votes: 10,
        plays: [{ play_id: 9, started_at_unix_ms: Date.UTC(2026, 10, 5, 16, 58), likes: 21, dislikes: 4 }],
      },
    });
    renderApp(<TrackLikesCard trackId={12} />);
    expect(await screen.findByText("82%")).toBeTruthy();
    expect(screen.getByText("148 votes")).toBeTruthy();
    expect(screen.getByText("1,904 votes")).toBeTruthy();
    expect(screen.getByTitle("21 likes, 4 dislikes")).toBeTruthy();
  });

  it("ranks the week's tracks", async () => {
    answer({
      ...modules(true),
      "GET /api/v1/stations/{station_id}/likes/week": {
        from_unix_ms: Date.UTC(2026, 10, 1, 23),
        min_votes: 10,
        most_liked: [{ track_id: 1, artist: "Cassia Bloom", title: "Neon Orchard", likes: 12, dislikes: 0, share: 1, plays: 2 }],
        least_liked: [],
      },
    });
    renderApp(<LikesWeekCard />);
    expect(await screen.findByText("Cassia Bloom – Neon Orchard")).toBeTruthy();
    expect(screen.getByText("No track has enough votes yet.")).toBeTruthy();
  });

  it("gives the votes on the track playing", async () => {
    answer({
      ...modules(true),
      "GET /api/v1/stations/{station_id}/likes/current": { play_id: 3, track_id: 1, likes: 5, dislikes: 1 },
    });
    renderApp(<NowLikes />);
    expect(await screen.findByText("83%")).toBeTruthy();
    expect(screen.getByText("5")).toBeTruthy();
  });
});
