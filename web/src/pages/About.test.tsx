// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { About as AboutData, ApiStatus } from "../api";
import { t } from "../i18n";
import { Failure, answer, renderApp, user } from "../test/render";
import { About } from "./About";

const WEB = { version: __APP_VERSION__, revision: __APP_REVISION__ };

function status(changes: Partial<ApiStatus> = {}): ApiStatus {
  return {
    service: "heron-api",
    version: WEB.version,
    revision: WEB.revision || null,
    database: "ok",
    engine: "ok",
    clock: { synchronized: true, offset_ms: 0, estimated_error_ms: 0, warning: null },
    server_time_unix_ms: 0,
    backups: null,
    ...changes,
  } as ApiStatus;
}

const SERVICES: AboutData["services"] = [
  { service: "heron-api", instance: null, version: "0.2.0", revision: "abc1234", build_date: null, running: true },
  { service: "heron-engine", instance: "main", version: "0.2.0", revision: "abc1234", build_date: null, running: true },
  { service: "heron-analyzer", instance: "box", version: "0.2.0", revision: null, build_date: null, running: false },
  { service: "heron-librarian", instance: null, version: null, revision: null, build_date: null, running: false },
];

const VIEWER: AboutData = { services: SERVICES, runtime: null, dependencies: null, configuration: null };

const ADMIN: AboutData = {
  services: SERVICES,
  runtime: {
    hosts: [
      {
        service: "heron-api",
        instance: null,
        uptime_seconds: 90_000,
        os: "Debian GNU/Linux 13",
        kernel: "6.12.0",
        arch: "aarch64",
        container: true,
      },
    ],
    timezone: "Europe/Paris",
    clock: { synchronized: false, offset_ms: null, estimated_error_ms: null, warning: "the server clock may be off" },
    cpu_cores: 4,
    memory_mib: 3_900,
  },
  dependencies: { database: { version: "12.3.1-MariaDB", migrations: 41 }, ffmpeg: null },
  configuration: [
    {
      service: "heron-api",
      instance: null,
      items: [
        { name: "HERON_LIBRARY_WATCH", kind: "flag", value: true },
        { name: "HERON_LIBRARY_DIR", kind: "text", value: "/music" },
        { name: "HERON_DATABASE_URL", kind: "secret", value: true },
        { name: "HERON_SENTRY_DSN", kind: "secret", value: false },
      ],
    },
  ],
};

function page(about: AboutData, api: ApiStatus = status(), role: "viewer" | "admin" = "admin") {
  answer({ "GET /api/v1/about": about, "GET /api/v1/status": api });
  renderApp(<About />, user(role));
}

describe("About", () => {
  it("shows every service and whether it runs", async () => {
    page(VIEWER, status(), "viewer");
    const engine = (await screen.findByText("heron-engine (main)")).closest("tr") as HTMLElement;
    expect(within(engine).getByText(t("about.running"))).toBeTruthy();
    const analyzer = screen.getByText("heron-analyzer (box)").closest("tr") as HTMLElement;
    expect(within(analyzer).getByText(t("about.stopped"))).toBeTruthy();
    const librarian = screen.getByText("heron-librarian").closest("tr") as HTMLElement;
    expect(within(librarian).getByText(t("about.neverSeen"))).toBeTruthy();
  });

  it("gives a viewer the build only", async () => {
    page(VIEWER, status(), "viewer");
    await screen.findByText("heron-api");
    expect(screen.queryByText(t("about.runtime"))).toBeNull();
    expect(screen.queryByText(t("about.dependencies"))).toBeNull();
    expect(screen.getByText(t("about.adminsOnly"))).toBeTruthy();
  });

  it("gives an admin the machine, the dependencies and the configuration", async () => {
    page(ADMIN);
    expect(await screen.findByText("Debian GNU/Linux 13, 6.12.0, aarch64")).toBeTruthy();
    expect(screen.getByText("Europe/Paris")).toBeTruthy();
    expect(screen.getByText("the server clock may be off")).toBeTruthy();
    expect(screen.getByText(/12\.3\.1-MariaDB/)).toBeTruthy();
    expect(screen.getByText(t("about.builtIn"))).toBeTruthy();
    expect(screen.getByText("/music")).toBeTruthy();
    const url = screen.getByText("HERON_DATABASE_URL").closest("tr") as HTMLElement;
    expect(within(url).getByText(t("about.secretSet"))).toBeTruthy();
    const dsn = screen.getByText("HERON_SENTRY_DSN").closest("tr") as HTMLElement;
    expect(within(dsn).getByText(t("about.secretUnset"))).toBeTruthy();
  });

  it("warns when the web interface and the API are from different builds", async () => {
    page(VIEWER, status({ version: "9.9.9", revision: "ffffff0" }), "viewer");
    expect(await screen.findByRole("status")).toBeTruthy();
    expect(screen.getByText(/9\.9\.9\+ffffff0/)).toBeTruthy();
  });

  it("copies what the page shows, and no secret", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    page(ADMIN);
    await screen.findByText("Europe/Paris");
    const button = screen.getByRole("button", { name: t("about.copy") });
    await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(button);
    await waitFor(() => expect(writeText).toHaveBeenCalled());
    const text = writeText.mock.calls[0]?.[0] as string;
    expect(text).toContain("heron-engine (main): 0.2.0+abc1234");
    expect(text).toContain("HERON_LIBRARY_WATCH=" + t("about.on"));
    expect(text).toContain("HERON_DATABASE_URL=" + t("about.secretSet"));
    expect(text).toContain("12.3.1-MariaDB");
    expect(text).toContain(t("about.clock") + ": the server clock may be off");
    expect(await screen.findByText(t("about.copied"))).toBeTruthy();
  });

  it("says why the page cannot be read", async () => {
    answer({
      "GET /api/v1/about": new Failure(500, "conflict", "no luck"),
      "GET /api/v1/status": status(),
    });
    renderApp(<About />);
    expect(await screen.findByText("no luck")).toBeTruthy();
  });
});
