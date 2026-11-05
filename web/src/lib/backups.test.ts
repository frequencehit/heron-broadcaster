// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { backupTime, backupWarnings, isEncryptedFile, looksLikeKey, looksLikeRecipient, shortKey } from "./backups";

// The prefix, then 58 characters of Bech32: 74 in all.
const KEY = `AGE-SECRET-KEY-1${"QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L".repeat(2).slice(0, 58)}`;
const RECIPIENT = "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";

describe("keys", () => {
  it("are told from public keys", () => {
    expect(KEY).toHaveLength(74);
    expect(looksLikeKey(KEY)).toBe(true);
    expect(looksLikeKey(` ${KEY.toLowerCase()}\n`)).toBe(true);
    expect(looksLikeKey(`# Heron backup key\n# public key: ${RECIPIENT}\n${KEY}\n`)).toBe(true);
    expect(looksLikeKey(RECIPIENT)).toBe(false);
    expect(looksLikeKey("AGE-SECRET-KEY-1TOOSHORT")).toBe(false);
    expect(looksLikeRecipient(RECIPIENT)).toBe(true);
    expect(looksLikeRecipient(` ${RECIPIENT}\n`)).toBe(true);
    expect(looksLikeRecipient(KEY)).toBe(false);
  });

  it("are shortened in sentences", () => {
    expect(shortKey(RECIPIENT)).toBe("age1ql3z7h…mcac8p");
    expect(shortKey("age1short")).toBe("age1short");
  });
});

describe("backup names", () => {
  it("carry the time the backup was taken", () => {
    expect(backupTime("2026-11-03T03-30-00Z-daily")).toBe(Date.UTC(2026, 10, 3, 3, 30, 0));
    expect(backupTime("2026-11-03T03-30-00Z-before_restore")).toBe(Date.UTC(2026, 10, 3, 3, 30, 0));
    expect(backupTime("not a backup")).toBeNull();
  });
});

describe("files", () => {
  it("are encrypted when they start as age files do", async () => {
    expect(await isEncryptedFile(new Blob(["age-encryption.org/v1\n-> X25519 abc\n"]))).toBe(true);
    expect(await isEncryptedFile(new Blob(['{"format": 1}']))).toBe(false);
    expect(await isEncryptedFile(new Blob([""]))).toBe(false);
  });
});

describe("banners", () => {
  const state = { encryption: "on", check_due: false, damaged: 0, test_due: false } as const;

  it("say nothing when all is well", () => {
    expect(backupWarnings(state)).toEqual([]);
  });

  it("say what needs attention", () => {
    expect(backupWarnings({ ...state, encryption: "off" })).toEqual(["notEncrypted"]);
    expect(backupWarnings({ ...state, encryption: "pending", damaged: 2, test_due: true })).toEqual([
      "damaged",
      "testDue",
    ]);
  });
});
