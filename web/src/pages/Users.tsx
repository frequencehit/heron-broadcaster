// SPDX-License-Identifier: AGPL-3.0-or-later

// Users and their roles (admins).

import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { type Role, type User, api, done, unwrap } from "../api";
import { Banner, Card, ConfirmButton, ErrorView, Field, Icon, Loading, Modal, PageHead } from "../components/ui";
import { type PlainKey, t } from "../i18n";
import { ago } from "../lib/format";
import { useAction, useCan, useUser } from "../lib/session";
import { useStations } from "../components/Stations";
import { twoFactorState } from "../lib/twoFactor";
import type { StationRoleName } from "../lib/station";

const STATION_ROLES: { value: StationRoleName; label: PlainKey }[] = [
  { value: "none", label: "users.role.none" },
  { value: "viewer", label: "users.role.viewer" },
  { value: "operator", label: "users.role.operator" },
  { value: "admin", label: "users.role.admin" },
];

const ROLES: { value: Role; label: PlainKey; hint: PlainKey }[] = [
  { value: "viewer", label: "users.role.viewer", hint: "users.role.viewerHint" },
  { value: "operator", label: "users.role.operator", hint: "users.role.operatorHint" },
  { value: "admin", label: "users.role.admin", hint: "users.role.adminHint" },
];

export function Users() {
  const isAdmin = useCan("admin");
  const me = useUser();
  const users = useQuery({
    queryKey: ["users"],
    queryFn: () => unwrap(api.GET("/api/v1/users")),
    enabled: isAdmin,
  });
  const stations = useStations();
  const several = stations.length > 1;
  const [creating, setCreating] = useState(false);
  const [resetting, setResetting] = useState<User | null>(null);
  const update = useAction(
    ({
      id,
      ...body
    }: {
      id: number;
      role?: Role;
      enabled?: boolean;
      station_roles?: { station_id: number; role: StationRoleName }[];
    }) =>
      unwrap(api.PATCH("/api/v1/users/{id}", { params: { path: { id } }, body })),
    { success: t("categories.saved"), invalidate: [["users"]] },
  );
  const remove = useAction(
    (id: number) => done(api.DELETE("/api/v1/users/{id}", { params: { path: { id } } })),
    { success: t("users.deleted"), invalidate: [["users"]] },
  );

  if (!isAdmin) {
    return <Banner kind="warn">{t("users.adminsOnly")}</Banner>;
  }
  return (
    <>
      <PageHead
        title={t("nav.users")}
        actions={
          <button type="button" className="btn primary" onClick={() => setCreating(true)}>
            <Icon name="plus" /> {t("users.new")}
          </button>
        }
      />
      <TwoFactorRequirement />
      <Card title={t("nav.users")} flush>
        {users.isPending && <Loading />}
        {users.isError && <ErrorView error={users.error} />}
        {users.data && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("users.user")}</th>
                  <th>{several ? t("users.sharedRole") : t("users.role")}</th>
                  {several &&
                    stations.map((station) => (
                      <th key={station.id} title={station.name}>
                        {station.code}
                      </th>
                    ))}
                  <th>{t("twoFactor.title")}</th>
                  <th className="hide-narrow">{t("users.lastSignIn")}</th>
                  <th>
                    <span className="visually-hidden">{t("library.actions")}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {users.data.map((user) => (
                  <tr key={user.id}>
                    <td>
                      {user.username} {user.id === me.id && <span className="badge accent">{t("users.you")}</span>}{" "}
                      {!user.enabled && <span className="badge">{t("output.disabled")}</span>}
                    </td>
                    <td>
                      <select
                        className="input"
                        value={user.role}
                        aria-label={t("users.roleOf", { name: user.username })}
                        onChange={(e) => update.mutate({ id: user.id, role: e.target.value as Role })}
                      >
                        {ROLES.map((role) => (
                          <option key={role.value} value={role.value}>
                            {t(role.label)}
                          </option>
                        ))}
                      </select>
                    </td>
                    {several &&
                      stations.map((station) => {
                        const current =
                          user.role === "admin"
                            ? "admin"
                            : (user.station_roles.find((r) => r.station_id === station.id)?.role ?? "none");
                        return (
                          <td key={station.id}>
                            <select
                              className="input"
                              value={current}
                              disabled={user.role === "admin"}
                              title={user.role === "admin" ? t("users.adminEverywhere") : undefined}
                              aria-label={t("users.roleOn", { name: user.username, station: station.code })}
                              onChange={(e) =>
                                update.mutate({
                                  id: user.id,
                                  station_roles: [
                                    ...user.station_roles.filter((r) => r.station_id !== station.id),
                                    { station_id: station.id, role: e.target.value as StationRoleName },
                                  ],
                                })
                              }
                            >
                              {STATION_ROLES.map((role) => (
                                <option key={role.value} value={role.value}>
                                  {t(role.label)}
                                </option>
                              ))}
                            </select>
                          </td>
                        );
                      })}
                    <td>
                      <TwoFactorState user={user} />
                    </td>
                    <td className="hide-narrow muted">{ago(user.last_login_at_unix_ms)}</td>
                    <td className="actions">
                      <button type="button" className="btn small" onClick={() => setResetting(user)}>
                        {t("login.password")}
                      </button>
                      {user.id !== me.id && (
                        <>
                          <button
                            type="button"
                            className="btn small"
                            onClick={() => update.mutate({ id: user.id, enabled: !user.enabled })}
                          >
                            {user.enabled ? t("track.disable") : t("track.enable")}
                          </button>
                          <ConfirmButton
                            className="btn ghost small icon danger"
                            title={t("categories.delete", { name: user.username })}
                            confirm={t("categories.deleteButton")}
                            question={<p>{t("users.deleteQuestion", { name: user.username })}</p>}
                            onConfirm={() => remove.mutate(user.id)}
                          >
                            <Icon name="trash" />
                          </ConfirmButton>
                        </>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
      {several && <p className="muted small">{t("users.stationRolesHint")}</p>}
      <dl className="facts small">
        {ROLES.map((role) => (
          <div key={role.value} style={{ display: "contents" }}>
            <dt>{t(role.label)}</dt>
            <dd>{t(role.hint)}</dd>
          </div>
        ))}
      </dl>
      {creating && <NewUserDialog onClose={() => setCreating(false)} />}
      {resetting && <PasswordDialog user={resetting} onClose={() => setResetting(null)} />}
    </>
  );
}

/** A user's second factors: on (the app, how many passkeys), to set up, or off. */
function TwoFactorState({ user }: { user: User }) {
  const status = user.two_factor;
  const state = twoFactorState(status);
  let detail = "—";
  if (status.totp) {
    detail = status.passkeys > 0 ? t("twoFactor.appAndKeys", { count: status.passkeys }) : t("twoFactor.appOnly");
  } else if (status.passkeys > 0) {
    detail = t("twoFactor.keys", { count: status.passkeys });
  }
  const badge = { on: "badge good", toSetUp: "badge warn", off: "badge" }[state];
  return (
    <>
      <span className={badge}>{t(`twoFactor.${state}`)}</span> <span className="muted small">{detail}</span>
    </>
  );
}

/** Which roles must have a second factor. */
function TwoFactorRequirement() {
  const settings = useQuery({
    queryKey: ["settings", "two-factor"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/two-factor")),
  });
  const [draft, setDraft] = useState({ admin: false, operator: false, viewer: false });
  useEffect(() => {
    if (settings.data) {
      setDraft(settings.data);
    }
  }, [settings.data]);
  const save = useAction(() => unwrap(api.PUT("/api/v1/settings/two-factor", { body: draft })), {
    success: t("twoFactor.requirementSaved"),
    invalidate: [["settings", "two-factor"], ["users"]],
  });
  const boxes: { role: Role; label: PlainKey }[] = [
    { role: "admin", label: "twoFactor.requireAdmins" },
    { role: "operator", label: "twoFactor.requireOperators" },
    { role: "viewer", label: "twoFactor.requireViewers" },
  ];
  return (
    <Card
      title={t("twoFactor.title")}
      actions={
        <button
          type="button"
          className="btn small primary"
          disabled={!settings.data || save.isPending}
          onClick={() => save.mutate(undefined)}
        >
          {t("common.save")}
        </button>
      }
    >
      <div className="stack">
        <p className="muted small" style={{ margin: 0 }}>
          {t("twoFactor.usersHelp")}
        </p>
        {settings.isError && <ErrorView error={settings.error} />}
        <div className="row" style={{ gap: "1.5rem" }}>
          {boxes.map(({ role, label }) => (
            <label key={role} className="check">
              <input
                type="checkbox"
                checked={draft[role]}
                disabled={!settings.data}
                onChange={(event) => setDraft({ ...draft, [role]: event.target.checked })}
              />
              {t(label)}
            </label>
          ))}
        </div>
        <span className="hint">{t("twoFactor.passkeysNeedHttps")}</span>
      </div>
    </Card>
  );
}

function NewUserDialog({ onClose }: { onClose: () => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>("operator");
  const create = useAction(
    () => unwrap(api.POST("/api/v1/users", { body: { username, password, role } })),
    { success: t("users.created"), invalidate: [["users"]], onSuccess: onClose },
  );
  return (
    <Modal
      title={t("users.new")}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="submit" form="user-form" className="btn primary" disabled={create.isPending}>
            {t("account.create")}
          </button>
        </>
      }
    >
      <form
        id="user-form"
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          create.mutate(undefined);
        }}
      >
        <Field label={t("login.username")}>
          <input className="input" value={username} onChange={(e) => setUsername(e.target.value)} required autoComplete="off" />
        </Field>
        <Field label={t("login.password")} hint={t("account.passwordHint")}>
          <input
            className="input"
            type="password"
            value={password}
            minLength={8}
            onChange={(e) => setPassword(e.target.value)}
            required
            autoComplete="new-password"
          />
        </Field>
        <Field label={t("users.role")} hint={t(ROLES.find((r) => r.value === role)?.hint ?? "users.role.operatorHint")}>
          <select className="input" value={role} onChange={(e) => setRole(e.target.value as Role)}>
            {ROLES.map((r) => (
              <option key={r.value} value={r.value}>
                {t(r.label)}
              </option>
            ))}
          </select>
        </Field>
      </form>
    </Modal>
  );
}

function PasswordDialog({ user, onClose }: { user: User; onClose: () => void }) {
  const [password, setPassword] = useState("");
  const reset = useAction(
    () => unwrap(api.PATCH("/api/v1/users/{id}", { params: { path: { id: user.id } }, body: { password } })),
    { success: t("users.passwordSet", { name: user.username }), onSuccess: onClose },
  );
  return (
    <Modal
      title={t("users.passwordOf", { name: user.username })}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="submit" form="password-form" className="btn primary" disabled={reset.isPending}>
            {t("users.set")}
          </button>
        </>
      }
    >
      <form
        id="password-form"
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          reset.mutate(undefined);
        }}
      >
        <p className="muted small" style={{ margin: 0 }}>
          {t("users.signedOut")}
        </p>
        <Field label={t("account.newPassword")} hint={t("account.passwordHint")}>
          <input
            className="input"
            type="password"
            value={password}
            minLength={8}
            onChange={(e) => setPassword(e.target.value)}
            required
            autoComplete="new-password"
          />
        </Field>
      </form>
    </Modal>
  );
}
