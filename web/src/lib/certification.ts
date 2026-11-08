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

export type CertificationDelivery = Schemas["CertificationDelivery"];
export type CertificationDeliveryInput = Schemas["CertificationDeliveryInput"];
export type DeliveryRun = Schemas["DeliveryRun"];
export type DeliveryCompression = Schemas["DeliveryCompression"];

export const DEFAULT_FILE_NAME = "{station}-{server}-{date}.{ext}";
export const FILE_NAME_TOKENS = ["{station}", "{server}", "{mount}", "{date}", "{yyyy}", "{mm}", "{dd}", "{ext}"];

/** A delivery as the form edits it: the password left empty keeps the stored one. */
export interface DeliveryDraft {
  name: string;
  enabled: boolean;
  serverId: number | null;
  mount: string;
  host: string;
  port: string;
  username: string;
  password: string;
  remoteDir: string;
  fileName: string;
  format: ExportFormat;
  compression: DeliveryCompression;
  sendAt: string;
  daysBack: number;
}

export function deliveryDraftOf(delivery: CertificationDelivery | null, serverId: number | null): DeliveryDraft {
  return {
    name: delivery?.name ?? "",
    enabled: delivery?.enabled ?? true,
    serverId: delivery?.server_id ?? serverId,
    mount: delivery?.mount ?? "",
    host: delivery?.host ?? "",
    port: delivery ? String(delivery.port) : "21",
    username: delivery?.username ?? "",
    password: "",
    remoteDir: delivery?.remote_dir ?? "",
    fileName: delivery?.file_name ?? DEFAULT_FILE_NAME,
    format: delivery?.format ?? "raw",
    compression: delivery?.compression ?? "none",
    sendAt: delivery?.send_at ?? "04:00",
    daysBack: delivery?.days_back ?? 1,
  };
}

export function deliveryInputOf(draft: DeliveryDraft): CertificationDeliveryInput {
  const port = Number.parseInt(draft.port, 10);
  const input: CertificationDeliveryInput = {
    name: draft.name.trim(),
    enabled: draft.enabled,
    server_id: draft.serverId ?? 0,
    mount: draft.mount.trim() || null,
    protocol: "ftps",
    host: draft.host.trim(),
    port: Number.isFinite(port) ? port : null,
    username: draft.username.trim(),
    remote_dir: draft.remoteDir.trim(),
    file_name: draft.fileName.trim() || DEFAULT_FILE_NAME,
    format: draft.format,
    compression: draft.compression,
    send_at: draft.sendAt,
    days_back: draft.daysBack,
  };
  if (draft.password !== "") {
    input.password = draft.password;
  }
  return input;
}

/** A name for a file, as heron-api writes it: letters, digits, `-` and `_`. */
export function safeName(name: string): string {
  return name.replace(/[^A-Za-z0-9_-]/g, "-").replace(/^-+|-+$/g, "");
}

/** The file name a pattern gives, as heron-api writes it; `null` for a pattern it refuses. */
export function previewFileName(
  pattern: string,
  names: { station: string; server: string; mount: string; day: string; format: ExportFormat; compression: DeliveryCompression },
): string | null {
  const ext = (names.format === "csv" ? "csv" : names.format === "json" ? "json" : "log") + (names.compression === "gzip" ? ".gz" : "");
  const [yyyy = "", mm = "", dd = ""] = names.day.split("-");
  const values: Record<string, string> = {
    station: safeName(names.station),
    server: safeName(names.server),
    mount: safeName(names.mount) || "all",
    date: names.day,
    yyyy,
    mm,
    dd,
    ext,
  };
  let unknown = false;
  const out = pattern
    .replace(/\{([^{}]*)\}/g, (_, token: string) => {
      const value = values[token];
      if (value === undefined) {
        unknown = true;
        return "";
      }
      return value;
    })
    .trim();
  if (unknown || out.includes("{") || out === "" || out === "." || out === ".." || /[/\\]/.test(out)) {
    return null;
  }
  return out;
}
