// SPDX-License-Identifier: AGPL-3.0-or-later

// The About page's text: how long a service has been up, and the diagnostics
// copied for a bug report.

import type { About, ConfigItem } from "../api";
import { locale, t } from "../i18n";
import { type Build, buildLabel } from "./version";

/** `3 d 4 h`, `4 h 12 min` or `35 min`, in the current language. */
export function uptime(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) {
    return "–";
  }
  const unit = (value: number, name: "day" | "hour" | "minute") =>
    new Intl.NumberFormat(locale(), { style: "unit", unit: name, unitDisplay: "narrow" }).format(value);
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  if (days > 0) {
    return `${unit(days, "day")} ${unit(hours, "hour")}`;
  }
  if (hours > 0) {
    return `${unit(hours, "hour")} ${unit(minutes, "minute")}`;
  }
  return unit(minutes, "minute");
}

/** A configuration value as text. */
export function configText(item: ConfigItem): string {
  switch (item.kind) {
    case "flag":
      return item.value ? t("about.on") : t("about.off");
    case "secret":
      return item.value ? t("about.secretSet") : t("about.secretUnset");
    default:
      return item.value || "–";
  }
}

/** A service and its instance: `heron-engine (main)`. */
export function serviceName(service: string, instance?: string | null): string {
  return instance && instance !== service ? `${service} (${instance})` : service;
}

type Runtime = NonNullable<About["runtime"]>;

/** Whether the server's clock is synchronized, in words. */
export function clockText(clock: Runtime["clock"]): string {
  const synced = clock.synchronized;
  if (synced === null || synced === undefined) {
    return t("about.clockUnknown");
  }
  if (synced) {
    return t("about.clockSynced");
  }
  return clock.warning ?? t("about.clockNotSynced");
}

function serviceState(service: About["services"][number]): string {
  if (service.running) {
    return t("about.running");
  }
  return service.version ? t("about.stopped") : t("about.neverSeen");
}

function serviceLines(about: About): string[] {
  const lines = [`# ${t("about.services")}`];
  for (const service of about.services) {
    const build = service.version ? buildLabel({ version: service.version, revision: service.revision }) : "–";
    const built = service.build_date ? `, ${t("about.built")} ${service.build_date}` : "";
    lines.push(`${serviceName(service.service, service.instance)}: ${build}${built}, ${serviceState(service)}`);
  }
  return lines;
}

function runtimeLines(runtime: Runtime): string[] {
  const lines = ["", `# ${t("about.runtime")}`];
  for (const host of runtime.hosts) {
    const system = [host.os, host.kernel, host.arch, host.container ? t("about.container") : null]
      .filter(Boolean)
      .join(", ");
    lines.push(`${serviceName(host.service, host.instance)}: ${uptime(host.uptime_seconds)}, ${system}`);
  }
  lines.push(
    `${t("about.timezone")}: ${runtime.timezone ?? "–"}`,
    `${t("about.clock")}: ${clockText(runtime.clock)}`,
    `${t("about.cores")}: ${runtime.cpu_cores}`,
    `${t("about.memory")}: ${runtime.memory_mib} MiB`,
  );
  return lines;
}

function dependencyLines(dependencies: NonNullable<About["dependencies"]>): string[] {
  return [
    "",
    `# ${t("about.dependencies")}`,
    `${t("about.database")}: ${dependencies.database.version ?? "–"}`,
    `${t("about.ffmpeg")}: ${dependencies.ffmpeg ?? t("about.builtIn")}`,
  ];
}

function configurationLines(config: NonNullable<About["configuration"]>[number]): string[] {
  return [
    "",
    `# ${t("about.configuration")}: ${serviceName(config.service, config.instance)}`,
    ...config.items.map((item) => `${item.name}=${configText(item)}`),
  ];
}

/**
 * Everything the signed-in user sees on the page, as plain text to paste
 * into a bug report. Nothing more: what the API did not send is not here.
 */
export function diagnostics(about: About, web: Build, api: Build): string {
  const lines = [`${t("about.web")}: ${buildLabel(web)}`, `${t("about.api")}: ${buildLabel(api)}`, ""];
  lines.push(...serviceLines(about));
  if (about.runtime) {
    lines.push(...runtimeLines(about.runtime));
  }
  if (about.dependencies) {
    lines.push(...dependencyLines(about.dependencies));
  }
  for (const config of about.configuration ?? []) {
    lines.push(...configurationLines(config));
  }
  return `${lines.join("\n")}\n`;
}
