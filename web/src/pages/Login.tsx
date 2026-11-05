// SPDX-License-Identifier: AGPL-3.0-or-later

import { useQueryClient } from "@tanstack/react-query";
import { type FormEvent, type ReactNode, useEffect, useState } from "react";
import { ApiError, type Schemas, type User, api, errorMessage, unwrap } from "../api";
import { OtpInput } from "../components/TwoFactor";
import { Banner, Field, Logo } from "../components/ui";
import { t } from "../i18n";
import { ME_KEY } from "../lib/session";
import { getPasskey, passkeysAvailable } from "../lib/webauthn";

type SecondFactor = Schemas["SecondFactor"];

export function Login() {
  const client = useQueryClient();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // After the password, when the user has a second factor.
  const [factors, setFactors] = useState<SecondFactor[] | null>(null);

  useEffect(() => {
    document.title = t("app.title", { page: t("login.title") });
  }, []);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const answer = await unwrap(
        api.POST("/api/v1/auth/login", { body: { username: username.trim(), password } }),
      );
      if ("second_factor" in answer) {
        setFactors(answer.second_factor);
        setPassword("");
        setBusy(false);
      } else {
        client.setQueryData(ME_KEY, answer);
      }
    } catch (failure) {
      // A wrong username or password answers 401, like a missing session.
      setError(
        failure instanceof ApiError && failure.code === "unauthorized"
          ? t("login.wrong")
          : errorMessage(failure),
      );
      setBusy(false);
    }
  };

  const brand = (
    <div className="brand" style={{ padding: 0, fontSize: "1.3rem" }}>
      <Logo size={30} /> Heron Broadcaster
    </div>
  );

  if (factors) {
    return (
      <SecondStep
        brand={brand}
        username={username.trim()}
        factors={factors}
        onSignedIn={(user) => client.setQueryData(ME_KEY, user)}
        onRestart={(message) => {
          setFactors(null);
          setError(message);
        }}
      />
    );
  }

  return (
    <main className="login">
      <form className="card" onSubmit={submit}>
        <div className="card-body stack">
          {brand}
          <p className="muted" style={{ margin: 0 }}>
            {t("login.tagline")}
          </p>
          {error && <Banner kind="bad">{error}</Banner>}
          <Field label={t("login.username")}>
            <input
              className="input"
              autoComplete="username"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              required
              autoFocus
            />
          </Field>
          <Field label={t("login.password")}>
            <input
              className="input"
              type="password"
              autoComplete="current-password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              required
            />
          </Field>
          <button type="submit" className="btn primary big" disabled={busy}>
            {busy ? t("login.signingIn") : t("login.title")}
          </button>
          <p className="muted small" style={{ margin: 0 }}>
            {t("login.firstAdmin")}{" "}
            <code>heron-api user add admin --role admin</code>
          </p>
        </div>
      </form>
    </main>
  );
}

/** The second step: a code of the app, a passkey or a recovery code. */
function SecondStep({
  brand,
  username,
  factors,
  onSignedIn,
  onRestart,
}: Readonly<{
  brand: ReactNode;
  username: string;
  factors: SecondFactor[];
  onSignedIn: (user: User) => void;
  onRestart: (message: string | null) => void;
}>) {
  const hasApp = factors.includes("totp");
  const hasPasskey = factors.includes("webauthn") && passkeysAvailable();
  const hasCodes = factors.includes("recovery");
  const [recovery, setRecovery] = useState(!hasApp && !hasPasskey);
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const failed = (failure: unknown, wrong: string) => {
    if (failure instanceof ApiError && failure.code === "unauthorized") {
      onRestart(t("twoFactor.expired"));
      return;
    }
    setError(failure instanceof ApiError && failure.code === "invalid" ? wrong : errorMessage(failure));
    setCode("");
    setBusy(false);
  };

  const verify = async (typed: string) => {
    setBusy(true);
    setError(null);
    try {
      const path = recovery ? "/api/v1/auth/login/recovery" : "/api/v1/auth/login/totp";
      onSignedIn(await unwrap(api.POST(path, { body: { code: typed } })));
    } catch (failure) {
      failed(failure, t("twoFactor.wrongCode"));
    }
  };

  const passkey = async () => {
    setBusy(true);
    setError(null);
    try {
      const options = await unwrap(api.POST("/api/v1/auth/login/webauthn/start"));
      let credential: Record<string, unknown> | null = null;
      try {
        credential = await getPasskey(options.publicKey as Record<string, unknown>);
      } catch {
        credential = null;
      }
      if (!credential) {
        setError(t("twoFactor.passkeyNotUsed"));
        setBusy(false);
        return;
      }
      onSignedIn(await unwrap(api.POST("/api/v1/auth/login/webauthn/finish", { body: { credential } })));
    } catch (failure) {
      failed(failure, t("twoFactor.passkeyRefused"));
    }
  };

  const changeCode = (next: string) => {
    setCode(next);
    // Six digits are the whole code: no need to press Verify.
    if (!recovery && next.length === 6 && !busy) {
      void verify(next);
    }
  };

  return (
    <main className="login">
      <form
        className="card"
        onSubmit={(event) => {
          event.preventDefault();
          void verify(code);
        }}
      >
        <div className="card-body stack">
          {brand}
          <p className="muted" style={{ margin: 0 }}>
            {t("twoFactor.hello", { user: username })}
          </p>
          {error && <Banner kind="bad">{error}</Banner>}
          {recovery ? (
            <Field label={t("twoFactor.recoveryCodes")}>
              <input
                className="input mono"
                autoComplete="off"
                autoCapitalize="characters"
                spellCheck={false}
                placeholder="XXXX-XXXX"
                value={code}
                onChange={(event) => setCode(event.target.value)}
                required
                autoFocus
              />
            </Field>
          ) : (
            hasApp && (
              <div className="field">
                <span>{t("twoFactor.appCodeLabel")}</span>
                <OtpInput value={code} onChange={changeCode} label={t("twoFactor.appCode")} autoFocus />
              </div>
            )
          )}
          {(recovery || hasApp) && (
            <button type="submit" className="btn primary big" disabled={busy || (!recovery && code.length < 6)}>
              {t("twoFactor.verify")}
            </button>
          )}
          {!recovery && hasPasskey && (
            <>
              {hasApp && <div className="or-rule muted small">{t("twoFactor.or")}</div>}
              <button type="button" className="btn big" disabled={busy} onClick={() => void passkey()}>
                {t("twoFactor.usePasskey")}
              </button>
            </>
          )}
          <p className="small" style={{ margin: 0, display: "flex", justifyContent: "space-between", gap: "1rem" }}>
            {recovery
              ? (hasApp || hasPasskey) && (
                  <button
                    type="button"
                    className="link-button"
                    onClick={() => {
                      setRecovery(false);
                      setCode("");
                      setError(null);
                    }}
                  >
                    {t("twoFactor.useOther")}
                  </button>
                )
              : hasCodes && (
                  <button
                    type="button"
                    className="link-button"
                    onClick={() => {
                      setRecovery(true);
                      setCode("");
                      setError(null);
                    }}
                  >
                    {t("twoFactor.useRecovery")}
                  </button>
                )}
            <button type="button" className="link-button" onClick={() => onRestart(null)}>
              {t("twoFactor.changeUser")}
            </button>
          </p>
        </div>
      </form>
    </main>
  );
}
