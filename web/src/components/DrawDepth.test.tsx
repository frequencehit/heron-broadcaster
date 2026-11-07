// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderApp } from "../test/render";
import { DrawDepthEditor, applyDraw, describeDrawLog } from "./DrawDepth";

describe("DrawDepthEditor", () => {
  it("shows what is inherited and sets the start or the maximum", () => {
    const onChange = vi.fn();
    const { rerender } = renderApp(
      <DrawDepthEditor value={{ track_min: 60 }} inherited={{ start_pct: 20, max_pct: 50 }} onChange={onChange} />,
    );
    expect(screen.getByText("20 % (inherited)")).toBeTruthy();
    expect(screen.getByText("50 % (inherited)")).toBeTruthy();
    expect(screen.getByText(/Draws among the 20 % .* up to 50 %/)).toBeTruthy();
    // Overriding the maximum starts from what is inherited, other overrides kept.
    fireEvent.click(screen.getByRole("checkbox", { name: "Widens up to" }));
    expect(onChange).toHaveBeenLastCalledWith({ track_min: 60, draw_max_pct: 50 });
    rerender(
      <DrawDepthEditor
        value={{ track_min: 60, draw_max_pct: 50 }}
        inherited={{ start_pct: 20, max_pct: 50 }}
        onChange={onChange}
      />,
    );
    fireEvent.change(screen.getByRole("spinbutton", { name: "Widens up to" }), { target: { value: "70" } });
    expect(onChange).toHaveBeenLastCalledWith({ track_min: 60, draw_max_pct: 70 });
    // Back to the inherited value.
    fireEvent.click(screen.getByRole("checkbox", { name: "Widens up to" }));
    expect(onChange).toHaveBeenLastCalledWith({ track_min: 60 });
  });
});

describe("draw depth", () => {
  it("applies a category's or a slot's over the station's", () => {
    const station = { start_pct: 20, max_pct: 50 };
    expect(applyDraw(station, undefined)).toEqual(station);
    expect(applyDraw(station, { draw_max_pct: 80 })).toEqual({ start_pct: 20, max_pct: 80 });
    // A start above the maximum starts at the maximum.
    expect(applyDraw(station, { draw_start_pct: 70 })).toEqual({ start_pct: 50, max_pct: 50 });
  });

  it("describes a pick's draw", () => {
    const blocked = { track: 0, title: 0, artist: 4, album: 0, daypart: 0, daypart_days: 0, across: 1 };
    expect(describeDrawLog({ pct: 30, start_pct: 20, max_pct: 50, tracks: 30, blocked })).toBe(
      "30 %, 30 tracks, 4 held back by the artist, 1 held back by another station",
    );
  });
});
