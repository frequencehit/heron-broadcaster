// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { overridesFrom, parseRelayMounts, relayMountsText, serverAddress } from "./servers";

describe("stream servers", () => {
  it("reads and writes a relay's mounts", () => {
    expect(parseRelayMounts("  \n")).toBeNull();
    const mounts = parseRelayMounts("/live\n  /aac   /relay-aac \n/hd /hd");
    expect(mounts).toEqual([{ mount: "/live" }, { mount: "/aac", relay_mount: "/relay-aac" }, { mount: "/hd" }]);
    expect(relayMountsText(mounts)).toBe("/live\n/aac /relay-aac\n/hd");
    expect(relayMountsText(null)).toBe("");
  });

  it("writes addresses", () => {
    expect(serverAddress({ host: "icecast", port: 8000, tls: false })).toBe("icecast:8000");
    expect(serverAddress({ host: "::1", port: 8443, tls: true })).toBe("https://[::1]:8443");
  });

  it("overrides only what is typed or chosen", () => {
    expect(overridesFrom({ name: " HD ", description: "", genre: "", url: "" }, null)).toEqual({ name: "HD" });
    expect(overridesFrom({ name: "", description: "", genre: "", url: "" }, false)).toEqual({ public: false });
  });
});
