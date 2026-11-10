// SPDX-License-Identifier: AGPL-3.0-or-later

// Your account: theme, language, width, the whole queue on On air, the status bar, password, second factors and API tokens.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useState } from "react";
import { type Theme, type Width, api, done, unwrap } from "../api";
import { StatusBarSettingsCard } from "../components/StatusBarSettings";
import { TwoFactorCard } from "../components/TwoFactor";
import { Banner, Card, ConfirmButton, Empty, Field, Icon, NumberInput, PageHead, Segmented } from "../components/ui";
import { LANGUAGES, type LanguageSetting, resolve, rich, setLanguage, t, toSetting } from "../i18n";
import { ago, dateTime } from "../lib/format";
import { ME_KEY, useAction, useUser } from "../lib/session";
import { applyTheme, applyWidth } from "../lib/theme";

export function Account() {
  const user = useUser();
  return (
    <>
      <PageHead
        title={t("account.title")}
        description={t("account.signedInAs", { user: user.username, role: t(`role.${user.role}`) })}
      />
      <div className="grid two">
        <DisplayCard
          theme={user.theme}
          language={toSetting(user.language)}
          width={user.width}
          wholeQueue={user.on_air_whole_queue}
        />
        <PasswordCard />
      </div>
      <StatusBarSettingsCard />
      <TwoFactorCard />
      <TokensCard />
    </>
  );
}

function DisplayCard({
  theme,
  language,
  width,
  wholeQueue,
}: {
  theme: Theme;
  language: LanguageSetting;
  width: Width;
  wholeQueue: boolean;
}) {
  const client = useQueryClient();
  const save = useAction(
    (preferences: { theme?: Theme; language?: LanguageSetting; width?: Width; on_air_whole_queue?: boolean }) =>
      unwrap(api.PUT("/api/v1/auth/preferences", { body: preferences })),
    { onSuccess: (user) => client.setQueryData(ME_KEY, user) },
  );
  const system = LANGUAGES.find((entry) => entry.code === resolve("system"))?.name;
  return (
    <Card title={t("account.display")}>
      <div className="stack">
        <Field label={t("account.theme")}>
          <Segmented
            label={t("account.theme")}
            value={theme}
            onChange={(next) => {
              applyTheme(next);
              save.mutate({ theme: next });
            }}
            options={[
              { value: "system", label: <><Icon name="monitor" size={14} /> {t("account.system")}</> },
              { value: "light", label: <><Icon name="sun" size={14} /> {t("account.light")}</> },
              { value: "dark", label: <><Icon name="moon" size={14} /> {t("account.dark")}</> },
            ]}
          />
        </Field>
        <Field label={t("account.language")}>
          <select
            className="input"
            value={language}
            onChange={(event) => {
              const next = toSetting(event.target.value);
              void setLanguage(next);
              save.mutate({ language: next });
            }}
          >
            <option value="system">{t("account.systemLanguage", { language: system ?? "" })}</option>
            {LANGUAGES.map((entry) => (
              <option key={entry.code} value={entry.code} lang={entry.code}>
                {entry.name}
              </option>
            ))}
          </select>
        </Field>
        <Field label={t("account.width")}>
          <Segmented
            label={t("account.width")}
            value={width}
            onChange={(next) => {
              applyWidth(next);
              save.mutate({ width: next });
            }}
            options={[
              { value: "normal", label: t("account.widthNormal") },
              { value: "wide", label: t("account.widthWide") },
              { value: "full", label: t("account.widthFull") },
            ]}
          />
        </Field>
        <label className="check">
          <input
            type="checkbox"
            checked={wholeQueue}
            onChange={(event) => save.mutate({ on_air_whole_queue: event.target.checked })}
          />
          {t("account.onAirWholeQueue")}
        </label>
        <p className="muted small" style={{ margin: 0 }}>
          {t("account.displayHelp")}
        </p>
      </div>
    </Card>
  );
}

function PasswordCard() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const change = useAction(
    () =>
      done(api.PUT("/api/v1/auth/password", { body: { current_password: current, new_password: next } })),
    {
      success: t("account.passwordChanged"),
      onSuccess: () => {
        setCurrent("");
        setNext("");
      },
    },
  );
  const submit = (event: FormEvent) => {
    event.preventDefault();
    change.mutate(undefined);
  };
  return (
    <Card title={t("login.password")}>
      <form className="stack" onSubmit={submit}>
        <Field label={t("account.currentPassword")}>
          <input
            className="input"
            type="password"
            autoComplete="current-password"
            value={current}
            onChange={(e) => setCurrent(e.target.value)}
            required
          />
        </Field>
        <Field label={t("account.newPassword")} hint={t("account.passwordHint")}>
          <input
            className="input"
            type="password"
            autoComplete="new-password"
            minLength={8}
            value={next}
            onChange={(e) => setNext(e.target.value)}
            required
          />
        </Field>
        <div className="row end">
          <button type="submit" className="btn primary" disabled={change.isPending}>
            {t("account.change")}
          </button>
        </div>
      </form>
    </Card>
  );
}

function TokensCard() {
  const tokens = useQuery({ queryKey: ["tokens"], queryFn: () => unwrap(api.GET("/api/v1/tokens")) });
  const [name, setName] = useState("");
  const [days, setDays] = useState(0);
  const [created, setCreated] = useState<string | null>(null);
  const create = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/tokens", {
          body: days > 0 ? { name, expires_in_days: days } : { name },
        }),
      ),
    {
      invalidate: [["tokens"]],
      onSuccess: (token) => {
        setCreated(token.token);
        setName("");
      },
    },
  );
  const revoke = useAction(
    (id: number) => done(api.DELETE("/api/v1/tokens/{id}", { params: { path: { id } } })),
    { success: t("account.tokenRevoked"), invalidate: [["tokens"]] },
  );
  return (
    <Card title={t("account.tokens")}>
      <div className="stack">
        <p className="muted small" style={{ margin: 0 }}>
          {rich("account.tokensHelp", { header: <code>Authorization: Bearer heron_…</code> })}
        </p>
        {created && (
          <Banner kind="warn">
            {rich("account.copyToken", { token: <code style={{ overflowWrap: "anywhere" }}>{created}</code> })}
          </Banner>
        )}
        <form
          className="row"
          onSubmit={(event) => {
            event.preventDefault();
            create.mutate(undefined);
          }}
        >
          <label style={{ flex: "1 1 12rem" }}>
            <span className="visually-hidden">{t("account.tokenName")}</span>
            <input
              className="input"
              placeholder={t("account.tokenPurpose")}
              value={name}
              onChange={(e) => setName(e.target.value)}
              required
            />
          </label>
          <label className="row small muted" style={{ flexWrap: "nowrap" }}>
            {rich("account.expiresAfter", {
              days: (
                <span style={{ width: "5rem" }}>
                  <NumberInput value={days} min={0} max={3650} step={1} onChange={(d) => setDays(Math.round(d))} />
                </span>
              ),
            })}
          </label>
          <button type="submit" className="btn primary" disabled={create.isPending}>
            {t("account.create")}
          </button>
        </form>
        {tokens.data?.length === 0 && <Empty>{t("account.noToken")}</Empty>}
        <ul className="list">
          {tokens.data?.map((token) => (
            <li key={token.id} className="row between" style={{ padding: "0.5rem 0" }}>
              <span>
                <strong>{token.name}</strong>
                <span className="muted small">
                  {" "}
                  · {t("account.tokenCreated", { date: dateTime(token.created_at_unix_ms) })} ·{" "}
                  {t("account.tokenUsed", { ago: ago(token.last_used_at_unix_ms) })}
                  {token.expires_at_unix_ms &&
                    ` · ${t("account.tokenExpires", { date: dateTime(token.expires_at_unix_ms) })}`}
                </span>
              </span>
              <ConfirmButton
                className="btn small danger"
                confirm={t("account.revoke")}
                question={<p>{t("account.revokeQuestion", { name: token.name })}</p>}
                onConfirm={() => revoke.mutate(token.id)}
              >
                {t("account.revoke")}
              </ConfirmButton>
            </li>
          ))}
        </ul>
      </div>
    </Card>
  );
}
