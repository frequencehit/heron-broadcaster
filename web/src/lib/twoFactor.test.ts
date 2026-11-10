// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import type { User } from "../api";
import { DEFAULT_STATUS_BAR } from "./statusbar";
import { codesFile, digitsOf, groupSecret, mustEnrol, twoFactorState } from "./twoFactor";
import { creationOptions, credentialJson, fromBase64url, requestOptions, toBase64url } from "./webauthn";

describe("two-factor state", () => {
  it("is on with the app or a passkey, to set up when required", () => {
    expect(twoFactorState({ totp: true, passkeys: 0, required: false })).toBe("on");
    expect(twoFactorState({ totp: false, passkeys: 2, required: true })).toBe("on");
    expect(twoFactorState({ totp: false, passkeys: 0, required: true })).toBe("toSetUp");
    expect(twoFactorState({ totp: false, passkeys: 0, required: false })).toBe("off");
  });

  it("asks a user to set it up only when their role requires it and they have none", () => {
    const someone: User = {
      id: 1,
      username: "ada",
      role: "operator",
      enabled: true,
      theme: "system",
      language: "en",
      width: "normal",
      on_air_whole_queue: false,
      status_bar: DEFAULT_STATUS_BAR,
      created_at_unix_ms: 0,
      station_roles: [],
      two_factor: { totp: false, passkeys: 0, required: false },
    };
    expect(mustEnrol(someone)).toBe(false);
    expect(mustEnrol({ ...someone, two_factor: { totp: false, passkeys: 0, required: true } })).toBe(true);
    expect(mustEnrol({ ...someone, two_factor: { totp: false, passkeys: 1, required: true } })).toBe(false);
  });
});

describe("codes", () => {
  it("keeps the digits of what is typed or pasted", () => {
    expect(digitsOf("123 456")).toBe("123456");
    expect(digitsOf("12a3")).toBe("123");
    expect(digitsOf("12345678")).toBe("123456");
  });

  it("writes the recovery codes file and groups the secret", () => {
    expect(codesFile("Codes", ["AAAA-2222", "BBBB-3333"])).toBe("Codes\n\nAAAA-2222\nBBBB-3333\n");
    expect(groupSecret("JBSWY3DPEHPK3PXP")).toBe("JBSW Y3DP EHPK 3PXP");
    expect(groupSecret("JBSWY3")).toBe("JBSW Y3");
  });
});

describe("passkeys", () => {
  it("converts base64url both ways", () => {
    const bytes = new Uint8Array([0, 1, 250, 251, 252, 253, 254, 255]);
    const text = toBase64url(bytes);
    expect(text).toBe("AAH6-_z9_v8");
    expect(new Uint8Array(fromBase64url(text))).toEqual(bytes);
    expect(toBase64url(new Uint8Array([102, 111]).buffer)).toBe("Zm8");
  });

  it("turns the API's options into the browser's", () => {
    const created = creationOptions({
      challenge: "AAE",
      rp: { id: "localhost", name: "Heron" },
      user: { id: "AAI", name: "ada", displayName: "ada" },
      excludeCredentials: [{ id: "AAM", type: "public-key" }],
    });
    expect(new Uint8Array(created.challenge as ArrayBuffer)).toEqual(new Uint8Array([0, 1]));
    expect(new Uint8Array(created.user.id as ArrayBuffer)).toEqual(new Uint8Array([0, 2]));
    expect(new Uint8Array(created.excludeCredentials?.[0]?.id as ArrayBuffer)).toEqual(new Uint8Array([0, 3]));
    expect(created.rp.id).toBe("localhost");
    const requested = requestOptions({ challenge: "AAQ", allowCredentials: [{ id: "AAU", type: "public-key" }] });
    expect(new Uint8Array(requested.allowCredentials?.[0]?.id as ArrayBuffer)).toEqual(new Uint8Array([0, 5]));
  });

  it("sends the browser's answer as JSON", () => {
    const buffer = (bytes: number[]) => new Uint8Array(bytes).buffer;
    const assertion = {
      id: "AAY",
      rawId: buffer([0, 6]),
      type: "public-key",
      response: {
        clientDataJSON: buffer([1]),
        authenticatorData: buffer([2]),
        signature: buffer([3]),
        userHandle: null,
      },
      getClientExtensionResults: () => ({}),
    } as unknown as PublicKeyCredential;
    expect(credentialJson(assertion)).toEqual({
      id: "AAY",
      rawId: "AAY",
      type: "public-key",
      response: { clientDataJSON: "AQ", authenticatorData: "Ag", signature: "Aw" },
      extensions: {},
    });
    const attestation = {
      id: "AAY",
      rawId: buffer([0, 6]),
      type: "public-key",
      response: { clientDataJSON: buffer([1]), attestationObject: buffer([4]), getTransports: () => ["usb"] },
    } as unknown as PublicKeyCredential;
    expect(credentialJson(attestation).response).toEqual({
      clientDataJSON: "AQ",
      attestationObject: "BA",
      transports: ["usb"],
    });
  });
});
