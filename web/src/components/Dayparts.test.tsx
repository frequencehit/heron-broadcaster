// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { Explanation } from "../api";
import { renderApp } from "../test/render";
import { CategoryDaypartDays, CategoryDayparts, DaypartBadge, DaypartSettings, daypartNote } from "./Dayparts";

describe("DaypartSettings", () => {
  it("draws where a track played and the dayparts kept out, in an example", () => {
    const onChange = vi.fn();
    const { container } = renderApp(<DaypartSettings count={4} disabled={false} onChange={onChange} />);
    const states = () => [...container.querySelectorAll(".daypart")].map((part) => part.className.split(" ")[1]);
    // The separation is the categories', and so is the same daypart on the
    // following days.
    expect(screen.getByRole("link", { name: "Categories" }).getAttribute("href")).toMatch(/\/categories$/);
    expect(screen.getByText(/played in B on Monday, not in B on Tuesday\./)).toBeTruthy();
    // Today, then tomorrow, with a separation of 1.
    expect(states()).toEqual(["played", "kept", "free", "free", "free", "free", "free", "free"]);
    expect(screen.getByText(/Played in A, it does not play in B; at the earliest in C\./)).toBeTruthy();
    // Played in D: tomorrow's A is kept out, across midnight.
    fireEvent.click(screen.getByTitle("Today · D: can play again"));
    expect(states()).toEqual(["free", "free", "free", "played", "kept", "free", "free", "free"]);
    expect(screen.getByText(/does not play in tomorrow's A; at the earliest in tomorrow's B/)).toBeTruthy();
    // Up to 6 with 4 dayparts; the example's separation is not saved.
    const example = screen.getByRole("group", { name: "Dayparts kept out, in the example" });
    expect(example.querySelectorAll("button")).toHaveLength(7);
    fireEvent.click(screen.getByRole("button", { name: "3" }));
    expect(states().slice(3, 7)).toEqual(["played", "kept", "kept", "kept"]);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("changes the division only", () => {
    const onChange = vi.fn();
    renderApp(<DaypartSettings count={6} disabled={false} onChange={onChange} />);
    expect(screen.getByText(/A 00:00–03:59 · B 04:00–07:59/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "4 dayparts of 6 h" }));
    expect(onChange).toHaveBeenLastCalledWith(4);
  });
});

describe("CategoryDayparts", () => {
  it("is a plain value, 0 for off", () => {
    const onChange = vi.fn();
    renderApp(<CategoryDayparts value={0} count={4} onChange={onChange} />);
    const select = screen.getByRole("combobox");
    expect((select as HTMLSelectElement).value).toBe("0");
    expect(screen.getByRole("option", { name: "0: off" })).toBeTruthy();
    expect(screen.getAllByRole("option")).toHaveLength(7);
    expect(screen.getByText("Off: only the windows in minutes count.")).toBeTruthy();
    fireEvent.change(select, { target: { value: "4" } });
    expect(onChange).toHaveBeenLastCalledWith(4);
  });

  it("keeps a value set for another station's division", () => {
    renderApp(<CategoryDayparts value={9} count={4} onChange={vi.fn()} />);
    expect(screen.getAllByRole("option")).toHaveLength(10);
  });
});

describe("CategoryDaypartDays", () => {
  it("is 0 (off) to 7 days, with an example", () => {
    const onChange = vi.fn();
    renderApp(<CategoryDaypartDays value={0} onChange={onChange} />);
    expect(screen.getByText("Same daypart, the following days")).toBeTruthy();
    const select = screen.getByRole("combobox");
    expect((select as HTMLSelectElement).value).toBe("0");
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([
      "0: off",
      "1 day",
      "2 days",
      "3 days",
      "4 days",
      "5 days",
      "6 days",
      "7 days",
    ]);
    expect(screen.getByText(/Played in B on Monday, not in B on Tuesday \(1 day\)/)).toBeTruthy();
    fireEvent.change(select, { target: { value: "2" } });
    expect(onChange).toHaveBeenLastCalledWith(2);
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
    windows: { album_min: 0, artist_min: 20, title_min: 30, track_min: 30, dayparts: 2, daypart_days: 0 },
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
      windows: { album_min: 0, artist_min: 20, title_min: 30, track_min: 30, dayparts: 1, daypart_days: 0 },
      blocked: { track: 0, title: 0, artist: 0, album: 0, daypart: 3 },
    });
    expect(daypartNote(halved)).toBe("2 → 1: played in A, allowed in C; 3 tracks held back by their daypart");
    // Off, or logged before daypart separation: nothing to say.
    expect(daypartNote(explanation({ daypart: { count: 4, daypart: 2, separation: 0, last_daypart: 0 } }))).toBeNull();
    expect(daypartNote(explanation({ daypart: null }))).toBeNull();
  });

  it("says what the same daypart on the following days held back, with or without dayparts", () => {
    const halved = explanation({
      step: 3,
      windows: { album_min: 0, artist_min: 20, title_min: 30, track_min: 30, dayparts: 0, daypart_days: 1 },
      blocked: { track: 0, title: 0, artist: 0, album: 0, daypart: 0, daypart_days: 4 },
      daypart: { count: 4, daypart: 1, separation: 0, days: 2, last_daypart: 1 },
    });
    expect(daypartNote(halved)).toBe("same daypart 2 → 1 d; 4 tracks held back by the same daypart on the days before");
    const dropped = explanation({
      step: 4,
      windows: { album_min: 0, artist_min: 20, title_min: 30, track_min: 30, dayparts: 0, daypart_days: 0 },
      blocked: { track: 0, title: 0, artist: 0, album: 0, daypart: 2, daypart_days: 1 },
      daypart: { count: 4, daypart: 1, separation: 2, days: 1, last_daypart: 0 },
    });
    expect(daypartNote(dropped)).toBe(
      "2 tracks held back by their daypart; 1 track held back by the same daypart on the days before",
    );
  });

  it("says when the track's own values applied, even off", () => {
    const own = explanation({
      last_played_unix_ms: null,
      daypart: { count: 4, daypart: 1, separation: 0, separation_from: "track", days: 0, last_daypart: 0 },
    });
    expect(daypartNote(own)).toBe("the track's own: dayparts 0");
    const both = explanation({
      last_played_unix_ms: null,
      daypart: { count: 4, daypart: 1, separation: 2, separation_from: "track", days: 1, days_from: "track" },
    });
    expect(daypartNote(both)).toBe("the track's own: dayparts 2, same daypart 1 d");
  });
});
