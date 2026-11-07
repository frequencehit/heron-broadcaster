// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { type Check, type Dedication, acceptable, checkBadge, reasonLabel } from "./dedications";

const check = (kind: Check["kind"], more: Partial<Check> = {}): Check => ({
  kind,
  blocking: kind === "restriction" || kind === "unplayable",
  ...more,
});

describe("dedications", () => {
  it("says what the rules say", () => {
    const time = () => "17:52";
    expect(checkBadge(check("rules_kept"), time)).toEqual({ text: "Rules kept", tone: "good" });
    expect(checkBadge(check("artist_played", { minutes_ago: 12, separation_min: 20 }), time)).toEqual({
      text: "Artist played 12 min ago (separation: 20 min)",
      tone: "warn",
    });
    expect(checkBadge(check("in_queue", { at_unix_ms: 1 }), time).text).toBe("Already in the queue, at 17:52");
    expect(checkBadge(check("restriction", { restriction_id: 4 }), time)).toEqual({
      text: "Held back by restriction 4",
      tone: "bad",
    });
  });

  it("accepts only a waiting request nothing blocks", () => {
    const request = { status: "pending", checks: [check("track_played")] } as unknown as Dedication;
    expect(acceptable(request)).toBe(true);
    expect(acceptable({ ...request, checks: [check("restriction")] })).toBe(false);
    expect(acceptable({ ...request, status: "accepted" })).toBe(false);
  });

  it("names the reasons", () => {
    expect(reasonLabel("closed")).toBe("dedications.reason.closed");
    expect(reasonLabel(null)).toBe("dedications.reason.none");
  });
});
