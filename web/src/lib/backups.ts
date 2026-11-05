// SPDX-License-Identifier: AGPL-3.0-or-later

// Backups: encryption keys and what the banners say (docs/backups.md#encryption).

import type { BackupState } from "../api";

/** How every age file starts. */
const AGE_MAGIC = "age-encryption.org/v1\n";

/** Whether a file is an encrypted backup. */
export async function isEncryptedFile(file: Blob): Promise<boolean> {
  return (await file.slice(0, AGE_MAGIC.length).text()) === AGE_MAGIC;
}

/** Whether `text` holds a secret key, alone or in its file. */
export function looksLikeKey(text: string): boolean {
  return text
    .split("\n")
    .map((line) => line.trim())
    .some((line) => /^AGE-SECRET-KEY-1[0-9A-Z]{58}$/i.test(line));
}

/** Whether `text` is a public key. */
export function looksLikeRecipient(text: string): boolean {
  return /^age1[0-9a-z]{58}$/.test(text.trim());
}

/** When a backup was taken, from its name (`2026-11-03T03-30-00Z-daily`). */
export function backupTime(name: string): number | null {
  const match = /^(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})-(\d{2})Z-/.exec(name);
  return match ? Date.parse(`${match[1]}T${match[2]}:${match[3]}:${match[4]}Z`) : null;
}

/** A public key, shortened for a sentence. */
export function shortKey(recipient: string): string {
  return recipient.length > 20 ? `${recipient.slice(0, 10)}…${recipient.slice(-6)}` : recipient;
}

export type BackupWarning = "notEncrypted" | "damaged" | "testDue";

/** The banners an admin sees about backups. */
export function backupWarnings(state: BackupState): BackupWarning[] {
  const warnings: BackupWarning[] = [];
  if (state.encryption === "off") {
    warnings.push("notEncrypted");
  }
  if (state.damaged > 0) {
    warnings.push("damaged");
  }
  if (state.test_due) {
    warnings.push("testDue");
  }
  return warnings;
}

/** Saves `text` as a file the browser downloads. */
export function downloadText(name: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}
