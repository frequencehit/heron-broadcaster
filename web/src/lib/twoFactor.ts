// SPDX-License-Identifier: AGPL-3.0-or-later

// Two-factor authentication, what the pages share (see
// docs/web.md#two-factor-authentication).

import type { Schemas, User } from "../api";

export type TwoFactorStatus = Schemas["TwoFactorStatus"];

/** Whether a user has a second factor (the app or a passkey). */
export function hasSecondFactor(status: TwoFactorStatus): boolean {
  return status.totp || status.passkeys > 0;
}

/** Whether the signed-in user must set up a second factor before anything else. */
export function mustEnrol(user: User): boolean {
  return user.two_factor.required && !hasSecondFactor(user.two_factor);
}

/** A user's state, for the Users page: on, to set up (required, none) or off. */
export function twoFactorState(status: TwoFactorStatus): "on" | "toSetUp" | "off" {
  if (hasSecondFactor(status)) {
    return "on";
  }
  return status.required ? "toSetUp" : "off";
}

/** Only digits, at most `length` of them: what a code input keeps of what is typed or pasted. */
export function digitsOf(text: string, length = 6): string {
  return text.replace(/\D/g, "").slice(0, length);
}

/** The recovery codes as a text file. */
export function codesFile(header: string, codes: string[]): string {
  return `${header}\n\n${codes.join("\n")}\n`;
}

/** The secret in groups of four, easier to type. */
export function groupSecret(secret: string): string {
  return secret.replace(/(.{4})(?=.)/g, "$1 ");
}
