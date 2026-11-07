// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { t } from "../i18n";
import { storedWidth } from "../lib/theme";
import { answer, renderApp, sent, user } from "../test/render";
import { Account } from "./Account";

afterEach(() => {
  delete document.documentElement.dataset.width;
  window.localStorage.clear();
});

describe("Account › Display", () => {
  it("picks a width of normal, 80 % or 100 %, applies it and saves it", async () => {
    answer({
      "GET /api/v1/tokens": [],
      "GET /api/v1/auth/two-factor": { passkeys: [], recovery_codes_left: 0 },
      "PUT /api/v1/auth/preferences": (options: { body: { width: string } }) => ({
        ...user(),
        width: options.body.width,
      }),
    });
    renderApp(<Account />);
    const width = screen.getByRole("group", { name: t("account.width") });
    // By its text: inside the field's <label>, the first button is named by the label too.
    const normal = within(width).getByText(t("account.widthNormal"));
    expect(normal.getAttribute("aria-pressed")).toBe("true");
    expect(document.documentElement.dataset.width).toBeUndefined();

    fireEvent.click(within(width).getByRole("button", { name: t("account.widthWide") }));
    expect(document.documentElement.dataset.width).toBe("wide");
    expect(storedWidth()).toBe("wide");
    await waitFor(() => expect(sent("PUT /api/v1/auth/preferences")).toHaveLength(1));
    expect(sent("PUT /api/v1/auth/preferences")[0]?.options).toEqual({ body: { width: "wide" } });

    fireEvent.click(within(width).getByRole("button", { name: t("account.widthFull") }));
    expect(document.documentElement.dataset.width).toBe("full");
    await waitFor(() => expect(sent("PUT /api/v1/auth/preferences")).toHaveLength(2));

    fireEvent.click(normal);
    expect(document.documentElement.dataset.width).toBeUndefined();
    expect(storedWidth()).toBe("normal");
  });

  it("shows the user's width", () => {
    answer({
      "GET /api/v1/tokens": [],
      "GET /api/v1/auth/two-factor": { passkeys: [], recovery_codes_left: 0 },
    });
    renderApp(<Account />, { ...user(), width: "full" });
    const width = screen.getByRole("group", { name: t("account.width") });
    expect(within(width).getByRole("button", { name: t("account.widthFull") }).getAttribute("aria-pressed")).toBe(
      "true",
    );
  });

  it("shows the whole queue on On air when asked, and saves it", async () => {
    answer({
      "GET /api/v1/tokens": [],
      "GET /api/v1/auth/two-factor": { passkeys: [], recovery_codes_left: 0 },
      "PUT /api/v1/auth/preferences": (options: { body: { on_air_whole_queue: boolean } }) => ({
        ...user(),
        on_air_whole_queue: options.body.on_air_whole_queue,
      }),
    });
    renderApp(<Account />);
    const whole = screen.getByRole("checkbox", { name: t("account.onAirWholeQueue") });
    expect((whole as HTMLInputElement).checked).toBe(false);
    fireEvent.click(whole);
    await waitFor(() => expect(sent("PUT /api/v1/auth/preferences")).toHaveLength(1));
    expect(sent("PUT /api/v1/auth/preferences")[0]?.options).toEqual({ body: { on_air_whole_queue: true } });
  });

  it("shows the user's choice of the whole queue", () => {
    answer({
      "GET /api/v1/tokens": [],
      "GET /api/v1/auth/two-factor": { passkeys: [], recovery_codes_left: 0 },
    });
    renderApp(<Account />, { ...user(), on_air_whole_queue: true });
    const whole = screen.getByRole("checkbox", { name: t("account.onAirWholeQueue") });
    expect((whole as HTMLInputElement).checked).toBe(true);
  });
});
