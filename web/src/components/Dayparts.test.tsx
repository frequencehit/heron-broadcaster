// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Explanation } from "../api";
import { renderApp } from "../test/render";
import { CategoryDayparts, DaypartBadge, DaypartSettings, daypartNote } from "./Dayparts";

describe("DaypartSettings", () => {
  it("draws where a track played and the dayparts kept out", () => {
    const onChange = vi.fn();
    const { container } = renderApp(<DaypartSettings count={4} separation={1} disabled={false} onChange={onChange} />);
    const states = () => [...container.querySelectorAll(".daypart")].map((part) => part.className.split(" ")[1]);
    // Today, then tomorrow.
    expect(states()).toEqual(["played", "kept", "free", "free", "free", "free", "free", "free"]);
    expect(screen.getByText(/Played in A, it does not play in B; at the earliest in C\./)).toBeTruthy();
    // Played in D: tomorrow's A is kept out, across midnight.
    fireEvent.click(screen.getByTitle("Today · D: can play again"));
    expect(states()).toEqual(["free", "free", "free", "played", "kept", "free", "free", "free"]);
    expect(screen.getByText(/does not play in tomorrow's A; at the earliest in tomorrow's B/)).toBeTruthy();
    // Up to 6 with 4 dayparts.
    expect(screen.getByRole("group", { name: "Dayparts kept out after a play" }).querySelectorAll("button")).toHaveLength(7);
    fireEvent.click(screen.getByRole("button", { name: "3" }));
    expect(onChange).toHaveBeenLastCalledWith(4, 3);
  });

  it("keeps the separation within the division's maximum", () => {
    const onChange = vi.fn();
    renderApp(<DaypartSettings count={6} separation={10} disabled={false} onChange={onChange} />);
    expect(screen.getByText(/A 00:00–03:59 · B 04:00–07:59/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "4 dayparts of 6 h" }));
    expect(onChange).toHaveBeenLastCalledWith(4, 6);
  });
});

describe("CategoryDayparts", () => {
  it("offers the station's value or its own", () => {
    const onChange = vi.fn();
    renderApp(<CategoryDayparts value={undefined} station={1} count={4} onChange={onChange} />);
    const select = screen.getByRole("combobox");
    expect((select as HTMLSelectElement).value).toBe("");
    expect(screen.getByRole("option", { name: "Station's (1)" })).toBeTruthy();
    expect(screen.getAllByRole("option")).toHaveLength(8);
    fireEvent.change(select, { target: { value: "4" } });
    expect(onChange).toHaveBeenLastCalledWith(4);
    fireEvent.change(select, { target: { value: "" } });
    expect(onChange).toHaveBeenLastCalledWith(undefined);
  });
});

function explanation(changes: Partial<Explanation>): Explanation {
  return {
    at_unix_ms: 0,
    clock_id: 1,
    clock_name: "Hits hour",
    slot: 0,
    slot_category_id: 1,
    category_id: 1,
    category_name: "Hits",
    method: "random",
    enforce_rules: true,
    step: 0,
    step_description: "",
    windows: { album_min: 0, artist_min: 20, title_min: 30, track_min: 30, dayparts: 2 },
    pool: 10,
    restricted: 0,
    blocked: { track: 0, title: 0, artist: 0, album: 0 },
    candidates: 4,
    weight: 1,
    skipped_slots: [],
    daypart: { count: 4, daypart: 2, separation: 2, last_daypart: 0 },
    ...changes,
  };
}

describe("the rotation log", () => {
  it("shows a pick's daypart", () => {
    renderApp(<DaypartBadge explanation={explanation({})} />);
    expect(screen.getByTitle("Daypart C · 12:00–17:59").textContent).toBe("C");
  });

  it("says where the track last played, and what was relaxed", () => {
    expect(daypartNote(explanation({ last_played_unix_ms: 1_000 }))).toMatch(/^last in A, /);
    expect(daypartNote(explanation({ last_played_unix_ms: null, daypart: { count: 4, daypart: 2, separation: 2 } }))).toBeNull();
    const halved = explanation({
      step: 3,
      windows: { album_min: 0, artist_min: 20, title_min: 30, track_min: 30, dayparts: 1 },
      blocked: { track: 0, title: 0, artist: 0, album: 0, daypart: 3 },
    });
    expect(daypartNote(halved)).toBe("2 → 1: played in A, allowed in C; 3 tracks held back by their daypart");
    // Off, or logged before daypart separation: nothing to say.
    expect(daypartNote(explanation({ daypart: { count: 4, daypart: 2, separation: 0, last_daypart: 0 } }))).toBeNull();
    expect(daypartNote(explanation({ daypart: null }))).toBeNull();
  });
});
