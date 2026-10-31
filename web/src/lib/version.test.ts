// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { buildLabel, differentBuilds } from "./version";

describe("differentBuilds", () => {
  it("compares the commits when both are known", () => {
    expect(differentBuilds({ version: "0.1.0", revision: "abc1234" }, { version: "0.1.0", revision: "abc1234" })).toBe(false);
    expect(differentBuilds({ version: "0.1.0", revision: "abc1234" }, { version: "0.1.0", revision: "def5678" })).toBe(true);
  });

  it("compares the versions otherwise", () => {
    expect(differentBuilds({ version: "0.1.0", revision: "" }, { version: "0.1.0", revision: "abc1234" })).toBe(false);
    expect(differentBuilds({ version: "0.1.0" }, { version: "0.2.0", revision: null })).toBe(true);
  });
});

describe("buildLabel", () => {
  it("adds the commit when known", () => {
    expect(buildLabel({ version: "0.1.0", revision: "abc1234" })).toBe("0.1.0+abc1234");
    expect(buildLabel({ version: "0.1.0", revision: "" })).toBe("0.1.0");
  });
});
