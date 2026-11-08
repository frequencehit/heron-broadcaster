// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  type CertificationDelivery,
  DEFAULT_FILE_NAME,
  deliveryDraftOf,
  deliveryInputOf,
  draftOf,
  inputOf,
  previewFileName,
  safeName,
} from "./certification";

const DELIVERY: CertificationDelivery = {
  id: 4,
  name: "Certifier",
  enabled: true,
  server_id: 3,
  server_name: "Main",
  mount: "/live",
  protocol: "ftps",
  host: "ftp.example.org",
  port: 990,
  username: "station",
  password_set: true,
  remote_dir: "/incoming",
  file_name: "{station}_{yyyy}{mm}{dd}.{ext}",
  format: "csv",
  compression: "gzip",
  send_at: "05:30",
  days_back: 2,
  last: null,
};

describe("delivery drafts", () => {
  it("starts a new delivery with the defaults", () => {
    const draft = deliveryDraftOf(null, 3);
    expect(draft).toMatchObject({ serverId: 3, port: "21", fileName: DEFAULT_FILE_NAME, sendAt: "04:00", daysBack: 1 });
    expect(deliveryInputOf({ ...draft, name: " Certifier ", host: "ftp.example.org", username: "u", password: "p" })).toEqual({
      name: "Certifier",
      enabled: true,
      server_id: 3,
      mount: null,
      protocol: "ftps",
      host: "ftp.example.org",
      port: 21,
      username: "u",
      password: "p",
      remote_dir: "",
      file_name: DEFAULT_FILE_NAME,
      format: "raw",
      compression: "none",
      send_at: "04:00",
      days_back: 1,
    });
  });

  it("keeps the password left empty", () => {
    const draft = deliveryDraftOf(DELIVERY, null);
    expect(draft.password).toBe("");
    const input = deliveryInputOf(draft);
    expect("password" in input).toBe(false);
    expect(input).toMatchObject({ server_id: 3, mount: "/live", port: 990, days_back: 2, compression: "gzip" });
  });
});

describe("file names", () => {
  const names = { station: "main", server: "Main server", mount: "", day: "2026-11-07", format: "raw" as const, compression: "none" as const };

  it("follows its pattern, as heron-api does", () => {
    expect(previewFileName(DEFAULT_FILE_NAME, names)).toBe("main-Main-server-2026-11-07.log");
    expect(previewFileName("{yyyy}{mm}{dd}_{mount}.{ext}", { ...names, mount: "/live", format: "csv", compression: "gzip" })).toBe(
      "20261107_live.csv.gz",
    );
    expect(previewFileName("x.{ext}", { ...names, format: "json" })).toBe("x.json");
    expect(previewFileName("{mount}", names)).toBe("all");
  });

  it("refuses what heron-api refuses", () => {
    for (const bad of ["{day}.log", "a/{date}", "{date", " ", ".."]) {
      expect(previewFileName(bad, names)).toBeNull();
    }
  });

  it("writes names with letters, digits, - and _", () => {
    expect(safeName("Main server/1")).toBe("Main-server-1");
  });
});

describe("access log drafts", () => {
  it("sends the password only when typed", () => {
    expect(inputOf(draftOf(null))).toEqual({ source: "file", path: "" });
    expect(inputOf({ ...draftOf(null), source: "url", url: " https://x/log ", password: "p" })).toEqual({
      source: "url",
      url: "https://x/log",
      username: null,
      password: "p",
    });
  });
});
