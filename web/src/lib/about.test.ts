// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import type { About, ConfigItem } from "../api";
import { t } from "../i18n";
import { clockText, configText, diagnostics, serviceName, uptime } from "./about";

describe("uptime", () => {
  it("shows the two largest units", () => {
    expect(uptime(35 * 60)).toMatch(/35/);
    expect(uptime(4 * 3_600 + 12 * 60)).toMatch(/4.*12/);
    expect(uptime(3 * 86_400 + 4 * 3_600)).toMatch(/3.*4/);
  });

  it("shows a dash when it is not known", () => {
    expect(uptime(null)).toBe("–");
    expect(uptime(undefined)).toBe("–");
    expect(uptime(-1)).toBe("–");
    expect(uptime(Number.NaN)).toBe("–");
  });
});

describe("serviceName", () => {
  it("adds the instance unless it repeats the service", () => {
    expect(serviceName("heron-engine", "main")).toBe("heron-engine (main)");
    expect(serviceName("heron-analyzer", "heron-analyzer")).toBe("heron-analyzer");
    expect(serviceName("heron-api")).toBe("heron-api");
    expect(serviceName("heron-api", null)).toBe("heron-api");
  });
});

describe("configText", () => {
  it("says on or off for a flag, set or not set for a secret, and the value for a text", () => {
    const item = (kind: ConfigItem["kind"], value: ConfigItem["value"]) => ({ name: "X", kind, value }) as ConfigItem;
    expect(configText(item("flag", true))).toBe(t("about.on"));
    expect(configText(item("flag", false))).toBe(t("about.off"));
    expect(configText(item("secret", true))).toBe(t("about.secretSet"));
    expect(configText(item("secret", false))).toBe(t("about.secretUnset"));
    expect(configText(item("text", "/music"))).toBe("/music");
    expect(configText(item("text", ""))).toBe("–");
  });
});

describe("clockText", () => {
  const clock = (synchronized: boolean | null, warning: string | null = null) => ({
    synchronized,
    offset_ms: null,
    estimated_error_ms: null,
    warning,
  });

  it("tells whether the clock is synchronized, with the server's warning when it is not", () => {
    expect(clockText(clock(null))).toBe(t("about.clockUnknown"));
    expect(clockText(clock(true))).toBe(t("about.clockSynced"));
    expect(clockText(clock(false, "may be off"))).toBe("may be off");
    expect(clockText(clock(false))).toBe(t("about.clockNotSynced"));
  });
});

describe("diagnostics", () => {
  it("lists only the parts the answer holds", () => {
    const about = {
      services: [
        { service: "heron-api", instance: null, version: "0.2.0", revision: "abc1234", build_date: "2026-11-05", running: true },
        { service: "heron-librarian", instance: "heron-librarian", version: "0.2.0", revision: null, build_date: null, running: false },
        { service: "heron-analyzer", instance: "heron-analyzer", version: null, revision: null, build_date: null, running: false },
      ],
    } as unknown as About;
    const text = diagnostics(about, { version: "0.2.0", revision: "abc1234" }, { version: "0.2.0", revision: "abc1234" });
    expect(text).toContain(`heron-api: 0.2.0+abc1234, ${t("about.built")} 2026-11-05, ${t("about.running")}`);
    expect(text).toContain(`heron-librarian: 0.2.0, ${t("about.stopped")}`);
    expect(text).toContain(`heron-analyzer: –, ${t("about.neverSeen")}`);
    expect(text).not.toContain(t("about.runtime"));
    expect(text).not.toContain(t("about.dependencies"));
    expect(text.endsWith("\n")).toBe(true);
  });
});
