// SPDX-License-Identifier: AGPL-3.0-or-later

// Audience certification (docs/certification.md): the export addresses and
// how the gaps of a day read.

import type { Schemas } from "../api";
import { stationUrl } from "./station";

export type AccessLogView = Schemas["AccessLogView"];
export type AccessLogInput = Schemas["AccessLogInput"];
export type CertificationDay = Schemas["CertificationDay"];
export type CertificationSettings = Schemas["CertificationSettings"];
export type ExportFormat = "raw" | "combined" | "csv" | "json";

export const FORMATS: ExportFormat[] = ["raw", "combined", "csv", "json"];

/** The address of a day's export: a download, with the session's cookie. */
export function exportUrl(day: string, serverId: number, format: ExportFormat, mount: string): string {
  const search = new URLSearchParams({ day, server_id: String(serverId), format });
  const trimmed = mount.trim();
  if (trimmed !== "") {
    search.set("mount", trimmed);
  }
  return stationUrl(`certification/export?${search.toString()}`);
}

/** The access log as the form edits it: the password left empty keeps the stored one. */
export interface LogDraft {
  source: AccessLogInput["source"];
  path: string;
  url: string;
  username: string;
  password: string;
}

export function draftOf(log: AccessLogView | null | undefined): LogDraft {
  return {
    source: log?.source ?? "file",
    path: log?.path ?? "",
    url: log?.url ?? "",
    username: log?.username ?? "",
    password: "",
  };
}

export function inputOf(draft: LogDraft): AccessLogInput {
  const input: AccessLogInput =
    draft.source === "file"
      ? { source: "file", path: draft.path.trim() }
      : { source: "url", url: draft.url.trim(), username: draft.username.trim() || null };
  if (draft.source === "url" && draft.password !== "") {
    input.password = draft.password;
  }
  return input;
}
