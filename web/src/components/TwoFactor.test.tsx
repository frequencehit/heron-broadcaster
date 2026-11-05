// @vitest-environment jsdom
// SPDX-License-Identifier: AGPL-3.0-or-later

import { fireEvent, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { t } from "../i18n";
import { Login } from "../pages/Login";
import { Failure, answer, renderApp, sent, user } from "../test/render";
import { Enrolment, OtpInput, RecoveryCodesDialog, TwoFactorCard } from "./TwoFactor";

const CODES = ["7KQ2-M9XD", "P4WN-3HTZ", "Z8RC-6LBE"];

function Code() {
  const [code, setCode] = useState("");
  return (
    <>
      <OtpInput value={code} onChange={setCode} label="Code" />
      <output>{code}</output>
    </>
  );
}

function digits() {
  return screen.getAllByRole("textbox").filter((box) => box.getAttribute("inputmode") === "numeric");
}

describe("OtpInput", () => {
  it("fills a box per digit, and all of them from a paste", () => {
    renderApp(<Code />);
    const boxes = digits();
    expect(boxes).toHaveLength(6);
    fireEvent.change(boxes[0] as HTMLElement, { target: { value: "4" } });
    fireEvent.change(boxes[1] as HTMLElement, { target: { value: "8" } });
    expect(document.querySelector("output")?.textContent).toBe("48");
    fireEvent.paste(boxes[0] as HTMLElement, { clipboardData: { getData: () => "123 456" } });
    expect(document.querySelector("output")?.textContent).toBe("123456");
    fireEvent.keyDown(boxes[5] as HTMLElement, { key: "Backspace" });
    expect(document.querySelector("output")?.textContent).toBe("123456");
  });
});

describe("RecoveryCodesDialog", () => {
  it("closes only once the codes are put away", () => {
    const onDone = vi.fn();
    renderApp(<RecoveryCodesDialog codes={CODES} username="ada" onDone={onDone} />);
    expect(screen.getAllByRole("listitem").map((item) => item.textContent)).toEqual(CODES);
    const finish = screen.getByRole("button", { name: t("twoFactor.done") });
    expect((finish as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("checkbox", { name: t("twoFactor.savedThem") }));
    fireEvent.click(finish);
    expect(onDone).toHaveBeenCalled();
  });
});

describe("TwoFactorCard", () => {
  const view = {
    totp: { created_at_unix_ms: 1_793_000_000_000, last_used_at_unix_ms: null },
    passkeys: [{ id: 3, name: "Blue key", created_at_unix_ms: 1_793_000_000_000, last_used_at_unix_ms: null }],
    recovery_codes_left: 8,
    required_by: null,
  };

  it("lists the app, the passkeys and the codes left", async () => {
    answer({ "GET /api/v1/auth/two-factor": view });
    renderApp(<TwoFactorCard />);
    expect(await screen.findByText("Blue key")).toBeTruthy();
    expect(screen.getByText(t("twoFactor.codesLeft", { count: 8 }))).toBeTruthy();
    expect(screen.getAllByRole("button", { name: t("twoFactor.remove") })).toHaveLength(2);
    expect(screen.getByText("heron-api user reset-2fa ada")).toBeTruthy();
  });

  it("keeps the last factor when the role requires one", async () => {
    answer({ "GET /api/v1/auth/two-factor": { ...view, passkeys: [], required_by: "operator" } });
    renderApp(<TwoFactorCard />);
    expect(await screen.findByRole("button", { name: t("twoFactor.replace") })).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("twoFactor.remove") })).toBeNull();
    expect(screen.getByText(new RegExp(t("twoFactor.requiredNote")))).toBeTruthy();
  });

  it("asks for the password, then gives the new codes once", async () => {
    answer({
      "GET /api/v1/auth/two-factor": view,
      "POST /api/v1/auth/two-factor/recovery-codes": (options: { body: { password: string } }) =>
        options.body.password === "correct horse" ? { recovery_codes: CODES } : new Failure(422, "invalid", "the password is wrong"),
    });
    renderApp(<TwoFactorCard />);
    fireEvent.click(await screen.findByRole("button", { name: t("twoFactor.newCodes") }));
    const password = screen.getByLabelText(new RegExp(`^${t("twoFactor.passwordAgain")}`));
    fireEvent.change(password, { target: { value: "wrong" } });
    fireEvent.click(screen.getByRole("button", { name: t("twoFactor.generate") }));
    expect(await screen.findByText(t("twoFactor.wrongPassword"))).toBeTruthy();
    fireEvent.change(password, { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: t("twoFactor.generate") }));
    await waitFor(() => expect(sent("POST /api/v1/auth/two-factor/recovery-codes")).toHaveLength(2));
  });
});

describe("Enrolment", () => {
  it("says which role requires it, and offers both factors", async () => {
    answer({
      "GET /api/v1/auth/two-factor": { totp: null, passkeys: [], recovery_codes_left: 0, required_by: "operator" },
    });
    const onSignOut = vi.fn();
    renderApp(<Enrolment onSignOut={onSignOut} />, user("operator"));
    expect(await screen.findByText(t("twoFactor.requiredForOperators"))).toBeTruthy();
    expect(screen.getByRole("button", { name: new RegExp(t("twoFactor.setUpApp")) })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: t("twoFactor.signOut") }));
    expect(onSignOut).toHaveBeenCalled();
  });
});

describe("Login", () => {
  const signIn = () => {
    fireEvent.change(screen.getByLabelText(t("login.username")), { target: { value: "ada" } });
    fireEvent.change(screen.getByLabelText(t("login.password")), { target: { value: "correct horse" } });
    fireEvent.click(screen.getByRole("button", { name: t("login.title") }));
  };

  it("asks for the app's code after the password, and sends it once typed", async () => {
    answer({
      "POST /api/v1/auth/login": { second_factor: ["totp", "recovery"] },
      "POST /api/v1/auth/login/totp": (options: { body: { code: string } }) =>
        options.body.code === "123456" ? user() : new Failure(422, "invalid", "the code is wrong"),
    });
    renderApp(<Login />);
    signIn();
    expect(await screen.findByText(t("twoFactor.hello", { user: "ada" }))).toBeTruthy();
    fireEvent.paste(digits()[0] as HTMLElement, { clipboardData: { getData: () => "654321" } });
    expect(await screen.findByText(t("twoFactor.wrongCode"))).toBeTruthy();
    fireEvent.paste(digits()[0] as HTMLElement, { clipboardData: { getData: () => "123456" } });
    await waitFor(() => expect(sent("POST /api/v1/auth/login/totp")).toHaveLength(2));
  });

  it("takes a recovery code instead, and starts again when the sign-in expired", async () => {
    answer({
      "POST /api/v1/auth/login": { second_factor: ["totp", "recovery"] },
      "POST /api/v1/auth/login/recovery": new Failure(401, "unauthorized", "sign in first"),
    });
    renderApp(<Login />);
    signIn();
    fireEvent.click(await screen.findByRole("button", { name: t("twoFactor.useRecovery") }));
    fireEvent.change(screen.getByLabelText(t("twoFactor.recoveryCodes")), { target: { value: "7kq2-m9xd" } });
    fireEvent.click(screen.getByRole("button", { name: t("twoFactor.verify") }));
    expect(await screen.findByText(t("twoFactor.expired"))).toBeTruthy();
    expect(sent("POST /api/v1/auth/login/recovery")[0]?.options).toEqual({ body: { code: "7kq2-m9xd" } });
    expect(screen.getByLabelText(t("login.username"))).toBeTruthy();
  });
});
