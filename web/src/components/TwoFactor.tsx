// SPDX-License-Identifier: AGPL-3.0-or-later

// Two-factor authentication: the Account card, the dialogs that set up a
// factor, the recovery codes shown once, and the screen of a user who must
// set one up (docs/web.md#two-factor-authentication).

import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  type ClipboardEvent,
  type KeyboardEvent,
  type ReactNode,
  createContext,
  useContext,
  useRef,
  useState,
} from "react";
import { ApiError, type Role, type Schemas, api, done, errorMessage, unwrap } from "../api";
import { type PlainKey, rich, t } from "../i18n";
import { downloadText } from "../lib/backups";
import { ago, fullDate } from "../lib/format";
import { ME_KEY, useAction, useUser } from "../lib/session";
import { codesFile, digitsOf, groupSecret } from "../lib/twoFactor";
import { createPasskey, passkeysAvailable } from "../lib/webauthn";
import { Banner, Card, Field, Icon, Loading, Logo, Modal, useToast } from "./ui";

export const TWO_FACTOR_KEY = ["two-factor"] as const;

/** The message of a failed password check, in the user's language. */
function passwordError(error: unknown): string {
  return error instanceof ApiError && error.code === "invalid" && error.message === "the password is wrong"
    ? t("twoFactor.wrongPassword")
    : errorMessage(error);
}

// Code input ----------------------------------------------------------------

/** Six boxes for a 6-digit code; typing or pasting fills them. */
export function OtpInput({
  value,
  onChange,
  label,
  autoFocus,
}: Readonly<{ value: string; onChange: (code: string) => void; label: string; autoFocus?: boolean }>) {
  const boxes = useRef<(HTMLInputElement | null)[]>([]);
  const focus = (index: number) => boxes.current[Math.max(0, Math.min(5, index))]?.focus();
  const set = (index: number, typed: string) => {
    const digits = digitsOf(typed);
    if (digits.length > 1) {
      // Pasted, or filled by the browser.
      const next = digitsOf(value.slice(0, index) + digits);
      onChange(next);
      focus(next.length);
      return;
    }
    const chars = value.padEnd(6, " ").split("");
    chars[index] = digits || " ";
    onChange(digitsOf(chars.join("").trimEnd().replace(/ /g, "")));
    if (digits) {
      focus(index + 1);
    }
  };
  const keyDown = (index: number, event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Backspace" && !value[index]) {
      event.preventDefault();
      onChange(value.slice(0, Math.max(0, index - 1)));
      focus(index - 1);
    } else if (event.key === "ArrowLeft") {
      focus(index - 1);
    } else if (event.key === "ArrowRight") {
      focus(index + 1);
    }
  };
  const paste = (event: ClipboardEvent<HTMLInputElement>) => {
    event.preventDefault();
    const digits = digitsOf(event.clipboardData.getData("text"));
    onChange(digits);
    focus(digits.length);
  };
  return (
    <div className="otp" role="group" aria-label={label}>
      {Array.from({ length: 6 }, (_, index) => (
        <input
          key={index}
          ref={(element) => {
            boxes.current[index] = element;
          }}
          className="input"
          inputMode="numeric"
          autoComplete={index === 0 ? "one-time-code" : "off"}
          aria-label={t("twoFactor.digit", { n: String(index + 1) })}
          value={value[index] ?? ""}
          autoFocus={autoFocus && index === 0}
          onChange={(event) => set(index, event.target.value)}
          onKeyDown={(event) => keyDown(index, event)}
          onPaste={paste}
          onFocus={(event) => event.target.select()}
        />
      ))}
    </div>
  );
}

// Recovery codes, shown once --------------------------------------------------

const ShowCodes = createContext<(codes: string[]) => void>(() => {});

/** Shows recovery codes once, above whatever the page becomes meanwhile. */
export function RecoveryCodesHost({ username, children }: Readonly<{ username: string; children: ReactNode }>) {
  const [codes, setCodes] = useState<string[] | null>(null);
  const client = useQueryClient();
  return (
    <ShowCodes.Provider value={setCodes}>
      {children}
      {codes && (
        <RecoveryCodesDialog
          codes={codes}
          username={username}
          onDone={() => {
            setCodes(null);
            void client.invalidateQueries({ queryKey: ME_KEY });
          }}
        />
      )}
    </ShowCodes.Provider>
  );
}

export function RecoveryCodesDialog({
  codes,
  username,
  onDone,
}: Readonly<{ codes: string[]; username: string; onDone: () => void }>) {
  const toast = useToast();
  const [saved, setSaved] = useState(false);
  const text = codesFile(t("twoFactor.fileHeader", { user: username }), codes);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(codes.join("\n"));
      toast(t("twoFactor.codesCopied"));
    } catch {
      toast(t("twoFactor.copyFailed"), true);
    }
  };
  const print = () => {
    const page = window.open("", "_blank", "width=480,height=640");
    if (!page) {
      return;
    }
    page.document.title = t("twoFactor.printTitle");
    const pre = page.document.createElement("pre");
    pre.style.font = "14pt ui-monospace, monospace";
    pre.textContent = text;
    page.document.body.append(pre);
    page.print();
    page.close();
  };
  // Closing is "Done": only once the codes are put away.
  const close = () => {
    if (saved) {
      onDone();
    }
  };
  return (
    <Modal
      title={t("twoFactor.codesTitle")}
      open
      onClose={close}
      footer={
        <button type="button" className="btn primary" disabled={!saved} onClick={onDone}>
          {t("twoFactor.done")}
        </button>
      }
    >
      <div className="stack">
        <p style={{ margin: 0 }}>
          <span className="badge good">{t("twoFactor.on")}</span> {t("twoFactor.isOn")}
        </p>
        <p style={{ margin: 0 }}>{rich("twoFactor.keepCodes", { shownOnce: <b>{t("twoFactor.shownOnce")}</b> })}</p>
        <ul className="recovery-codes" aria-label={t("twoFactor.recoveryCodes")}>
          {codes.map((code) => (
            <li key={code}>{code}</li>
          ))}
        </ul>
        <div className="row">
          <button type="button" className="btn small" onClick={() => void copy()}>
            {t("twoFactor.copy")}
          </button>
          <button
            type="button"
            className="btn small"
            onClick={() => downloadText(`heron-recovery-codes-${username}.txt`, text)}
          >
            {t("twoFactor.download")}
          </button>
          <button type="button" className="btn small" onClick={print}>
            {t("twoFactor.print")}
          </button>
        </div>
        <label className="check">
          <input type="checkbox" checked={saved} onChange={(event) => setSaved(event.target.checked)} />
          {t("twoFactor.savedThem")}
        </label>
      </div>
    </Modal>
  );
}

// Dialogs ---------------------------------------------------------------------

/** Asks for the password before changing a factor. */
function PasswordDialog({
  title,
  help,
  confirm,
  danger,
  onClose,
  action,
}: Readonly<{
  title: string;
  help: ReactNode;
  confirm: string;
  danger?: boolean;
  onClose: () => void;
  action: (password: string) => Promise<void>;
}>) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await action(password);
    } catch (failure) {
      setError(passwordError(failure));
      setBusy(false);
    }
  };
  return (
    <Modal
      title={title}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="submit"
            form="two-factor-password"
            className={danger ? "btn danger solid" : "btn primary"}
            disabled={busy}
          >
            {confirm}
          </button>
        </>
      }
    >
      <form
        id="two-factor-password"
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <div className="muted small">{help}</div>
        {error && <Banner kind="bad">{error}</Banner>}
        <Field label={t("twoFactor.passwordAgain")} hint={t("twoFactor.signsOutElsewhere")}>
          <input
            className="input"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            required
            autoFocus
          />
        </Field>
      </form>
    </Modal>
  );
}

type TotpSetup = Schemas["TotpSetup"];

/** Sets up (or replaces) the authenticator app: password, QR code, a code. */
export function TotpDialog({ onClose }: Readonly<{ onClose: () => void }>) {
  const user = useUser();
  const client = useQueryClient();
  const toast = useToast();
  const showCodes = useContext(ShowCodes);
  const [setup, setSetup] = useState<TotpSetup | null>(null);
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  if (!setup) {
    return (
      <PasswordDialog
        title={t("twoFactor.totpTitle")}
        help={t("twoFactor.intro")}
        confirm={t("twoFactor.continue")}
        onClose={onClose}
        action={async (password) => {
          setSetup(await unwrap(api.POST("/api/v1/auth/two-factor/totp", { body: { password } })));
        }}
      />
    );
  }
  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      const added = await unwrap(api.POST("/api/v1/auth/two-factor/totp/confirm", { body: { code } }));
      await client.invalidateQueries({ queryKey: TWO_FACTOR_KEY });
      toast(t("twoFactor.appReady"));
      onClose();
      if (added.recovery_codes) {
        showCodes(added.recovery_codes);
      } else {
        await client.invalidateQueries({ queryKey: ME_KEY });
      }
    } catch (failure) {
      setError(
        failure instanceof ApiError && failure.code === "invalid" ? t("twoFactor.wrongCode") : errorMessage(failure),
      );
      setCode("");
      setBusy(false);
    }
  };
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(setup.secret);
      toast(t("twoFactor.keyCopied"));
    } catch {
      toast(t("twoFactor.copyFailed"), true);
    }
  };
  return (
    <Modal
      title={t("twoFactor.totpTitle")}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="submit" form="totp-form" className="btn primary" disabled={busy || code.length < 6}>
            {t("twoFactor.verifyAndTurnOn")}
          </button>
        </>
      }
    >
      <form
        id="totp-form"
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          void confirm();
        }}
      >
        <p style={{ margin: 0 }}>{rich("twoFactor.totpScan", { step: <b>1.</b> })}</p>
        <div className="row" style={{ gap: "1.25rem", alignItems: "flex-start" }}>
          <img
            className="qr"
            alt={t("twoFactor.qrLabel")}
            src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(setup.qr_svg)}`}
          />
          <div className="stack" style={{ gap: "0.4rem", flex: "1 1 10rem" }}>
            <span className="muted small">{t("twoFactor.noCamera")}</span>
            <span className="totp-secret">{groupSecret(setup.secret)}</span>
            <button type="button" className="btn small" style={{ alignSelf: "flex-start" }} onClick={() => void copy()}>
              {t("twoFactor.copyKey")}
            </button>
            <span className="muted small">
              {rich("twoFactor.account", { account: <b>Heron ({user.username})</b> })}
            </span>
          </div>
        </div>
        {error && <Banner kind="bad">{error}</Banner>}
        <div className="field">
          <span>{rich("twoFactor.typeCode", { step: <b>2.</b> })}</span>
          <OtpInput value={code} onChange={setCode} label={t("twoFactor.appCode")} autoFocus />
          <span className="hint">{t("twoFactor.codeChanges")}</span>
        </div>
      </form>
    </Modal>
  );
}

/** Adds a passkey: a name and the password, then the browser asks for it. */
export function PasskeyDialog({ onClose }: Readonly<{ onClose: () => void }>) {
  const client = useQueryClient();
  const toast = useToast();
  const showCodes = useContext(ShowCodes);
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const options = await unwrap(
        api.POST("/api/v1/auth/two-factor/passkeys", { body: { password, name: name.trim() } }),
      );
      let credential: Record<string, unknown> | null = null;
      try {
        credential = await createPasskey(options.publicKey as Record<string, unknown>);
      } catch {
        credential = null;
      }
      if (!credential) {
        setError(t("twoFactor.passkeyCancelled"));
        setBusy(false);
        return;
      }
      const added = await unwrap(api.POST("/api/v1/auth/two-factor/passkeys/finish", { body: { credential } }));
      await client.invalidateQueries({ queryKey: TWO_FACTOR_KEY });
      toast(t("twoFactor.passkeyReady"));
      onClose();
      if (added.recovery_codes) {
        showCodes(added.recovery_codes);
      } else {
        await client.invalidateQueries({ queryKey: ME_KEY });
      }
    } catch (failure) {
      setError(passwordError(failure));
      setBusy(false);
    }
  };
  return (
    <Modal
      title={t("twoFactor.addPasskey")}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="submit" form="passkey-form" className="btn primary" disabled={busy}>
            {t("twoFactor.add")}
          </button>
        </>
      }
    >
      <form
        id="passkey-form"
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <p className="muted small" style={{ margin: 0 }}>
          {t("twoFactor.passkeyHelp")}
        </p>
        {error && <Banner kind="bad">{error}</Banner>}
        <Field label={t("twoFactor.passkeyName")}>
          <input
            className="input"
            value={name}
            placeholder={t("twoFactor.passkeyNamePlaceholder")}
            maxLength={255}
            onChange={(event) => setName(event.target.value)}
            required
            autoFocus
          />
        </Field>
        <Field label={t("twoFactor.passwordAgain")} hint={t("twoFactor.signsOutElsewhere")}>
          <input
            className="input"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            required
          />
        </Field>
      </form>
    </Modal>
  );
}

function RenameDialog({
  passkey,
  onClose,
}: Readonly<{ passkey: Schemas["PasskeyView"]; onClose: () => void }>) {
  const [name, setName] = useState(passkey.name);
  const rename = useAction(
    () =>
      done(
        api.PATCH("/api/v1/auth/two-factor/passkeys/{id}", {
          params: { path: { id: passkey.id } },
          body: { name: name.trim() },
        }),
      ),
    { success: t("twoFactor.renamed"), invalidate: [[...TWO_FACTOR_KEY]], onSuccess: onClose },
  );
  return (
    <Modal
      title={t("twoFactor.renameTitle", { name: passkey.name })}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="submit" form="rename-passkey" className="btn primary" disabled={rename.isPending}>
            {t("common.save")}
          </button>
        </>
      }
    >
      <form
        id="rename-passkey"
        onSubmit={(event) => {
          event.preventDefault();
          rename.mutate(undefined);
        }}
      >
        <Field label={t("twoFactor.passkeyName")}>
          <input
            className="input"
            value={name}
            maxLength={255}
            onChange={(event) => setName(event.target.value)}
            required
            autoFocus
          />
        </Field>
      </form>
    </Modal>
  );
}

// Account card ----------------------------------------------------------------

type Dialog =
  | { kind: "totp" }
  | { kind: "removeTotp" }
  | { kind: "passkey" }
  | { kind: "rename"; passkey: Schemas["PasskeyView"] }
  | { kind: "removePasskey"; passkey: Schemas["PasskeyView"] }
  | { kind: "codes" };

/** Account › Two-factor authentication. */
export function TwoFactorCard() {
  const user = useUser();
  const client = useQueryClient();
  const toast = useToast();
  const showCodes = useContext(ShowCodes);
  const view = useQuery({ queryKey: TWO_FACTOR_KEY, queryFn: () => unwrap(api.GET("/api/v1/auth/two-factor")) });
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const close = () => setDialog(null);
  const refresh = async () => {
    await client.invalidateQueries({ queryKey: TWO_FACTOR_KEY });
    await client.invalidateQueries({ queryKey: ME_KEY });
  };
  const data = view.data;
  const on = Boolean(data && (data.totp || data.passkeys.length > 0));
  const factorCount = data ? (data.totp ? 1 : 0) + data.passkeys.length : 0;
  // The last factor stays when the role requires one.
  const canRemove = !data?.required_by || factorCount > 1;
  const canPasskey = passkeysAvailable();
  return (
    <Card
      title={
        <h2>
          {t("twoFactor.title")}{" "}
          {data && <span className={on ? "badge good" : "badge"}>{on ? t("twoFactor.on") : t("twoFactor.off")}</span>}
        </h2>
      }
    >
      {view.isPending && <Loading />}
      {view.isError && <Banner kind="bad">{errorMessage(view.error)}</Banner>}
      {data && (
        <div className="stack">
          <p className="muted small" style={{ margin: 0 }}>
            {t("twoFactor.intro")} {data.required_by && t("twoFactor.requiredNote")}
          </p>
          <div className="grid two">
            <div className="stack">
              <h3 className="factor-title">
                <Icon name="phone" size={16} /> {t("twoFactor.app")}
              </h3>
              {data.totp ? (
                <ul className="list">
                  <li className="row between" style={{ padding: "0.5rem 0" }}>
                    <span>
                      {t("twoFactor.appCode")}{" "}
                      <span className="muted small">
                        · {t("twoFactor.appSetUpOn", { date: fullDate(data.totp.created_at_unix_ms) })},{" "}
                        {data.totp.last_used_at_unix_ms
                          ? t("twoFactor.used", { ago: ago(data.totp.last_used_at_unix_ms) })
                          : t("twoFactor.neverUsed")}
                      </span>
                    </span>
                    <span className="row">
                      <button type="button" className="btn small" onClick={() => setDialog({ kind: "totp" })}>
                        {t("twoFactor.replace")}
                      </button>
                      {canRemove && (
                        <button
                          type="button"
                          className="btn small danger"
                          onClick={() => setDialog({ kind: "removeTotp" })}
                        >
                          {t("twoFactor.remove")}
                        </button>
                      )}
                    </span>
                  </li>
                </ul>
              ) : (
                <div className="row between">
                  <span className="muted">{t("twoFactor.noApp")}</span>
                  <button type="button" className="btn small primary" onClick={() => setDialog({ kind: "totp" })}>
                    {t("twoFactor.setUp")}
                  </button>
                </div>
              )}
              <h3 className="factor-title">
                <Icon name="key" size={16} /> {t("twoFactor.passkeys")}
              </h3>
              {data.passkeys.length === 0 ? (
                <span className="muted">{t("twoFactor.noPasskey")}</span>
              ) : (
                <ul className="list">
                  {data.passkeys.map((passkey) => (
                    <li key={passkey.id} className="row between" style={{ padding: "0.5rem 0" }}>
                      <span>
                        {passkey.name}{" "}
                        <span className="muted small">
                          · {t("twoFactor.passkeyAdded", { date: fullDate(passkey.created_at_unix_ms) })},{" "}
                          {passkey.last_used_at_unix_ms
                            ? t("twoFactor.used", { ago: ago(passkey.last_used_at_unix_ms) })
                            : t("twoFactor.neverUsed")}
                        </span>
                      </span>
                      <span className="row">
                        <button
                          type="button"
                          className="btn small"
                          onClick={() => setDialog({ kind: "rename", passkey })}
                        >
                          {t("twoFactor.rename")}
                        </button>
                        {canRemove && (
                          <button
                            type="button"
                            className="btn small danger"
                            onClick={() => setDialog({ kind: "removePasskey", passkey })}
                          >
                            {t("twoFactor.remove")}
                          </button>
                        )}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <div className="row">
                <button
                  type="button"
                  className="btn"
                  disabled={!canPasskey}
                  onClick={() => setDialog({ kind: "passkey" })}
                >
                  <Icon name="key" size={16} /> {t("twoFactor.addPasskey")}
                </button>
              </div>
              {!canPasskey && <span className="hint">{t("twoFactor.passkeysNeedHttps")}</span>}
            </div>
            <div className="stack">
              <h3 className="factor-title">
                <Icon name="shield" size={16} /> {t("twoFactor.recoveryCodes")}
              </h3>
              {on && <p style={{ margin: 0 }}>{t("twoFactor.codesLeft", { count: data.recovery_codes_left })}</p>}
              <p className="muted small" style={{ margin: 0 }}>
                {t("twoFactor.codesHelp")}
              </p>
              {on && (
                <div className="row">
                  <button type="button" className="btn" onClick={() => setDialog({ kind: "codes" })}>
                    {t("twoFactor.newCodes")}
                  </button>
                </div>
              )}
              <Banner kind="warn">
                {rich("twoFactor.lostEverything", { command: <code>heron-api user reset-2fa {user.username}</code> })}
              </Banner>
            </div>
          </div>
        </div>
      )}
      {dialog?.kind === "totp" && <TotpDialog onClose={close} />}
      {dialog?.kind === "passkey" && <PasskeyDialog onClose={close} />}
      {dialog?.kind === "rename" && <RenameDialog passkey={dialog.passkey} onClose={close} />}
      {dialog?.kind === "removeTotp" && (
        <PasswordDialog
          title={t("twoFactor.removeApp")}
          help={t("twoFactor.removeAppHelp")}
          confirm={t("twoFactor.remove")}
          danger
          onClose={close}
          action={async (password) => {
            await done(api.DELETE("/api/v1/auth/two-factor/totp", { body: { password } }));
            close();
            toast(t("twoFactor.appRemoved"));
            await refresh();
          }}
        />
      )}
      {dialog?.kind === "removePasskey" && (
        <PasswordDialog
          title={t("twoFactor.removePasskey", { name: dialog.passkey.name })}
          help={t("twoFactor.removePasskeyHelp")}
          confirm={t("twoFactor.remove")}
          danger
          onClose={close}
          action={async (password) => {
            await done(
              api.DELETE("/api/v1/auth/two-factor/passkeys/{id}", {
                params: { path: { id: dialog.passkey.id } },
                body: { password },
              }),
            );
            close();
            toast(t("twoFactor.passkeyRemoved"));
            await refresh();
          }}
        />
      )}
      {dialog?.kind === "codes" && (
        <PasswordDialog
          title={t("twoFactor.newCodes")}
          help={t("twoFactor.newCodesHelp")}
          confirm={t("twoFactor.generate")}
          onClose={close}
          action={async (password) => {
            const fresh = await unwrap(api.POST("/api/v1/auth/two-factor/recovery-codes", { body: { password } }));
            close();
            await client.invalidateQueries({ queryKey: TWO_FACTOR_KEY });
            showCodes(fresh.recovery_codes);
          }}
        />
      )}
    </Card>
  );
}

// Required, not set up ----------------------------------------------------------

const REQUIRED_FOR: Record<Role, PlainKey> = {
  admin: "twoFactor.requiredForAdmins",
  operator: "twoFactor.requiredForOperators",
  viewer: "twoFactor.requiredForViewers",
};

/** What a user whose role requires a second factor sees until they set one up. */
export function Enrolment({ onSignOut }: Readonly<{ onSignOut: () => void }>) {
  const user = useUser();
  const view = useQuery({ queryKey: TWO_FACTOR_KEY, queryFn: () => unwrap(api.GET("/api/v1/auth/two-factor")) });
  const [dialog, setDialog] = useState<"totp" | "passkey" | null>(null);
  const role = view.data?.required_by;
  return (
    <main className="login">
      <div className="card">
        <div className="card-body stack">
          <div className="brand" style={{ padding: 0, fontSize: "1.3rem" }}>
            <Logo size={30} /> Heron Broadcaster
          </div>
          {role && <Banner kind="warn">{t(REQUIRED_FOR[role])}</Banner>}
          <p style={{ margin: 0 }}>{rich("twoFactor.enrolIntro", { user: <b>{user.username}</b> })}</p>
          <button type="button" className="btn primary big" onClick={() => setDialog("totp")}>
            <Icon name="phone" size={16} /> {t("twoFactor.setUpApp")}
          </button>
          <button
            type="button"
            className="btn big"
            disabled={!passkeysAvailable()}
            onClick={() => setDialog("passkey")}
          >
            <Icon name="key" size={16} /> {t("twoFactor.addPasskey")}
          </button>
          {!passkeysAvailable() && <span className="hint">{t("twoFactor.passkeysNeedHttps")}</span>}
          <p className="muted small" style={{ margin: 0 }}>
            {rich("twoFactor.enrolCodes", {
              signOut: (
                <button type="button" className="link-button" onClick={onSignOut}>
                  {t("twoFactor.signOut")}
                </button>
              ),
            })}
          </p>
        </div>
      </div>
      {dialog === "totp" && <TotpDialog onClose={() => setDialog(null)} />}
      {dialog === "passkey" && <PasskeyDialog onClose={() => setDialog(null)} />}
    </main>
  );
}
