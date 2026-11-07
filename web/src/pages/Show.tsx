// SPDX-License-Identifier: AGPL-3.0-or-later

// A show's page (docs/web.md#shows): its hours, its notifications and its
// page and small widget on the website. Each show adds its own tabs (its
// requests, its match in progress) through `tabs`.

import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useEffect, useState } from "react";
import { api, unwrap } from "../api";
import { useStations } from "../components/Stations";
import { Banner, Card, ErrorView, Field, Icon, Loading, NumberInput, PageHead, Segmented, useToast } from "../components/ui";
import { t } from "../i18n";
import { dateTime, fullDate, shortTime } from "../lib/format";
import { useModule } from "../lib/modules";
import { DAY_NUMBERS, dayName, daysLabel } from "../lib/rotation";
import { Link } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import {
  COMMON_EVENTS,
  type HourRange,
  PLACEHOLDERS,
  type ShowKey,
  type ShowSettings,
  type ShowView,
  clashes,
  fillMessage,
  hourLabel,
  pageUrl,
  showLabel,
  unknownPlaceholders,
  validRange,
  weekGrid,
  widgetLine,
} from "../lib/shows";
import { here, useStation } from "../lib/station";

type Update = (change: (draft: ShowSettings) => ShowSettings) => void;

export function ShowPage({ module, tabs }: Readonly<{ module: ShowKey; tabs?: ReactNode }>) {
  const canEdit = useCan("operator");
  const station = useStation();
  const show = useQuery({
    queryKey: ["shows", module],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/shows/{module}", { params: { path: here({ module }) } })),
  });
  const all = useQuery({
    queryKey: ["shows"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/shows", { params: { path: here() } })),
  });
  const [draft, setDraft] = useState<ShowSettings | null>(null);
  useEffect(() => setDraft(show.data?.settings ?? null), [show.data]);
  const save = useAction(
    (body: ShowSettings) =>
      unwrap(api.PUT("/api/v1/stations/{station_id}/shows/{module}", { params: { path: here({ module }) }, body })),
    { success: t("categories.saved"), invalidate: [["shows"]] },
  );
  const title = t(showLabel(module));
  if (show.isPending) {
    return <Loading />;
  }
  if (show.isError) {
    return <ErrorView error={show.error} />;
  }
  if (!draft) {
    return <Loading />;
  }
  const dirty = JSON.stringify(draft) !== JSON.stringify(show.data.settings);
  const set: Update = (change) => setDraft((current) => (current ? change(current) : current));
  const others = (all.data ?? []).filter((other) => other.module !== module);
  return (
    <>
      <PageHead
        title={title}
        description={t("shows.help", { zone: show.data.timezone })}
        actions={
          canEdit && (
            <>
              {dirty && (
                <button type="button" className="btn" onClick={() => setDraft(show.data.settings)}>
                  {t("track.revert")}
                </button>
              )}
              <button
                type="button"
                className="btn primary"
                disabled={!dirty || save.isPending}
                onClick={() => save.mutate(draft)}
              >
                {t("common.save")}
              </button>
            </>
          )
        }
      />
      {tabs}
      <Hours show={show.data} draft={draft} set={set} others={others} canEdit={canEdit} />
      <Notifications show={show.data} draft={draft} set={set} canEdit={canEdit} />
      <Website module={module} title={title} show={show.data} draft={draft} set={set} canEdit={canEdit} station={station} />
    </>
  );
}

function Hours({
  show,
  draft,
  set,
  others,
  canEdit,
}: Readonly<{ show: ShowView; draft: ShowSettings; set: Update; others: ShowView[]; canEdit: boolean }>) {
  const found = clashes(
    draft.hours,
    others.map((other) => ({ module: other.module, hours: other.settings.hours })),
  );
  const mine = weekGrid(draft.hours);
  const theirs = weekGrid(others.flatMap((other) => other.settings.hours));
  const change = (index: number, range: Partial<HourRange>) =>
    set((d) => ({ ...d, hours: d.hours.map((r, i) => (i === index ? { ...r, ...range } : r)) }));
  const status = show.status;
  return (
    <Card
      title={t("shows.hours")}
      actions={
        <label className="check">
          <input
            type="checkbox"
            checked={draft.enabled}
            disabled={!canEdit}
            onChange={(e) => set((d) => ({ ...d, enabled: e.target.checked }))}
          />
          {t("shows.enabled")}
        </label>
      }
    >
      <p className="muted small">{t("shows.hoursHelp")}</p>
      <p className="small">
        {status.open
          ? t("shows.openUntil", { time: dateTime(status.closes_at_unix_ms) })
          : status.next_opening_unix_ms
            ? t("shows.closedUntil", { time: dateTime(status.next_opening_unix_ms) })
            : t("shows.neverOpen")}
      </p>
      <div className="stack">
        {draft.hours.map((range, index) => {
          const clash = found[index];
          return (
            <div className="row" key={index} style={{ flexWrap: "wrap", gap: 8 }}>
              <div className="segmented" role="group" aria-label={t("shows.days")}>
                {DAY_NUMBERS.map((day) => (
                  <button
                    key={day}
                    type="button"
                    aria-pressed={range.days.includes(day)}
                    disabled={!canEdit}
                    onClick={() =>
                      change(index, {
                        days: range.days.includes(day)
                          ? range.days.filter((d) => d !== day)
                          : [...range.days, day].sort((a, b) => a - b),
                      })
                    }
                  >
                    {dayName(day)}
                  </button>
                ))}
              </div>
              <label className="row small">
                {t("shows.from")}
                <select
                  className="input"
                  value={range.start_hour}
                  disabled={!canEdit}
                  onChange={(e) => change(index, { start_hour: Number(e.target.value) })}
                >
                  {Array.from({ length: 24 }, (_, hour) => (
                    <option key={hour} value={hour}>
                      {hourLabel(hour)}
                    </option>
                  ))}
                </select>
              </label>
              <label className="row small">
                {t("shows.to")}
                <select
                  className="input"
                  value={range.end_hour}
                  disabled={!canEdit}
                  onChange={(e) => change(index, { end_hour: Number(e.target.value) })}
                >
                  {Array.from({ length: 24 }, (_, hour) => hour + 1).map((hour) => (
                    <option key={hour} value={hour}>
                      {hourLabel(hour)}
                    </option>
                  ))}
                </select>
              </label>
              {canEdit && (
                <button
                  type="button"
                  className="btn ghost icon"
                  aria-label={t("shows.removeRange")}
                  onClick={() => set((d) => ({ ...d, hours: d.hours.filter((_, i) => i !== index) }))}
                >
                  <Icon name="close" />
                </button>
              )}
              {!validRange(range) && <span className="badge bad">{t("shows.invalidRange")}</span>}
              {clash && (
                <span className="badge bad">
                  {t("shows.clash", {
                    hours: `${hourLabel(clash.start)}–${hourLabel(clash.end)}`,
                    day: dayName(clash.day),
                    show: t(showLabel(clash.module)),
                  })}
                </span>
              )}
            </div>
          );
        })}
        {canEdit && (
          <div>
            <button
              type="button"
              className="btn small"
              onClick={() =>
                set((d) => ({ ...d, hours: [...d.hours, { days: [1, 2, 3, 4, 5], start_hour: 17, end_hour: 18 }] }))
              }
            >
              <Icon name="plus" /> {t("shows.addRange")}
            </button>
          </div>
        )}
      </div>
      <div className="table-wrap" style={{ marginTop: 12 }}>
        <table className="show-week" aria-label={t("shows.week")}>
          <thead>
            <tr>
              <th />
              {Array.from({ length: 24 }, (_, hour) => (
                <th key={hour} scope="col" className="muted small">
                  {hour % 3 === 0 ? hour : ""}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {DAY_NUMBERS.map((day) => (
              <tr key={day}>
                <th scope="row" className="muted small">
                  {dayName(day)}
                </th>
                {Array.from({ length: 24 }, (_, hour) => {
                  const own = mine[day - 1]?.[hour];
                  const other = theirs[day - 1]?.[hour];
                  const kind = own && other ? "clash" : own ? "own" : other ? "other" : "";
                  return <td key={hour} className={kind} title={`${dayName(day)} ${hourLabel(hour)}`} />;
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="muted small row" style={{ gap: 16 }}>
        <span className="row">
          <span className="show-week-key own" /> {t("shows.thisShow")}
        </span>
        <span className="row">
          <span className="show-week-key other" /> {t("shows.otherShows")}
        </span>
      </p>
    </Card>
  );
}

function Notifications({
  show,
  draft,
  set,
  canEdit,
}: Readonly<{ show: ShowView; draft: ShowSettings; set: Update; canEdit: boolean }>) {
  const webhooksOn = useModule("webhooks");
  if (!webhooksOn) {
    return null;
  }
  const notifications = draft.notifications;
  const setEvent = (name: string, change: { enabled?: boolean; message?: string }) =>
    set((d) => {
      const current = d.notifications.events[name] ?? { enabled: false, message: "" };
      return {
        ...d,
        notifications: { ...d.notifications, events: { ...d.notifications.events, [name]: { ...current, ...change } } },
      };
    });
  const example = {
    name: draft.website.title || t(showLabel(show.module)),
    link: draft.website.page_url ?? "https://…",
    end: "18:00",
    next: t("shows.exampleNext"),
  };
  const events = COMMON_EVENTS.filter((event) => notifications.events[event.name]);
  return (
    <Card title={t("shows.notifications")}>
      <p className="muted small">
        {t("shows.notificationsHelp")} <Link to="/webhooks">{t("nav.webhooks")}</Link>
      </p>
      <div className="grid-2">
        <div className="stack">
          <Field label={t("shows.sendTo")}>
            {show.webhooks.length === 0 ? (
              <span className="muted small">{t("shows.noWebhooks")}</span>
            ) : (
              <span className="row" style={{ flexWrap: "wrap" }}>
                {show.webhooks.map((webhook) => (
                  <label className="check" key={webhook.id}>
                    <input
                      type="checkbox"
                      disabled={!canEdit}
                      checked={notifications.webhook_ids.includes(webhook.id)}
                      onChange={(e) =>
                        set((d) => ({
                          ...d,
                          notifications: {
                            ...d.notifications,
                            webhook_ids: e.target.checked
                              ? [...d.notifications.webhook_ids, webhook.id]
                              : d.notifications.webhook_ids.filter((id) => id !== webhook.id),
                          },
                        }))
                      }
                    />
                    {webhook.name}
                  </label>
                ))}
              </span>
            )}
          </Field>
          {events.map((event) => {
            const setting = notifications.events[event.name] ?? { enabled: false, message: "" };
            const unknown = unknownPlaceholders(setting.message, PLACEHOLDERS);
            return (
              <div key={event.name} className="stack" style={{ gap: 4 }}>
                <div className="row">
                  <label className="check">
                    <input
                      type="checkbox"
                      disabled={!canEdit}
                      checked={setting.enabled}
                      onChange={(e) => setEvent(event.name, { enabled: e.target.checked })}
                    />
                    {t(event.label)}
                  </label>
                  {event.name === "show.upcoming" && (
                    <NumberInput
                      value={notifications.lead_minutes}
                      min={1}
                      max={1440}
                      step={1}
                      disabled={!canEdit}
                      suffix={t("shows.minutesBefore")}
                      label={t("shows.minutesBefore")}
                      onChange={(lead) =>
                        set((d) => ({ ...d, notifications: { ...d.notifications, lead_minutes: lead } }))
                      }
                    />
                  )}
                </div>
                <input
                  className="input"
                  aria-label={t(event.label)}
                  value={setting.message}
                  disabled={!canEdit}
                  maxLength={500}
                  onChange={(e) => setEvent(event.name, { message: e.target.value })}
                />
                {unknown.length > 0 && (
                  <span className="badge bad">{t("shows.unknownPlaceholders", { names: unknown.join(", ") })}</span>
                )}
              </div>
            );
          })}
          <Field label={t("shows.language")}>
            <select
              className="input"
              value={notifications.language}
              disabled={!canEdit}
              onChange={(e) =>
                set((d) => ({ ...d, notifications: { ...d.notifications, language: e.target.value } }))
              }
            >
              <option value="en">English</option>
              <option value="fr">Français</option>
              <option value="es">Español</option>
              <option value="de">Deutsch</option>
            </select>
          </Field>
          <p className="muted small">{t("shows.placeholders", { name: "{name}", link: "{link}", end: "{end}", next: "{next}" })}</p>
        </div>
        <div className="show-preview-messages" aria-label={t("shows.preview")}>
          <p className="muted small">{t("shows.preview")}</p>
          {events
            .filter((event) => notifications.events[event.name]?.enabled)
            .map((event) => (
              <div key={event.name} className="show-message">
                <span className="muted small">{t(event.label)}</span>
                <div>{fillMessage(notifications.events[event.name]?.message ?? "", example)}</div>
              </div>
            ))}
        </div>
      </div>
    </Card>
  );
}

function Website({
  module,
  title,
  show,
  draft,
  set,
  canEdit,
  station,
}: Readonly<{
  module: ShowKey;
  title: string;
  show: ShowView;
  draft: ShowSettings;
  set: Update;
  canEdit: boolean;
  station: number | null;
}>) {
  const toast = useToast();
  const stations = useStations();
  const code = stations.find((s) => s.id === station)?.code ?? "";
  const stationSettings = useQuery({
    queryKey: ["settings", "station"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/station", { params: { path: here() } })),
  });
  const gateway = stationSettings.data?.gateway_url ?? null;
  const [preview, setPreview] = useState<"open" | "closed">("open");
  const website = draft.website;
  const setWebsite = (change: Partial<ShowSettings["website"]>) =>
    set((d) => ({ ...d, website: { ...d.website, ...change } }));
  const copy = async (line: string) => {
    try {
      await navigator.clipboard.writeText(line);
      toast(t("shows.copied"));
    } catch {
      toast(t("shows.copyFailed"), true);
    }
  };
  const line = (page: boolean) => (gateway ? widgetLine(gateway, code, module, page) : "");
  return (
    <Card title={t("shows.website")}>
      {!gateway && <Banner kind="warn">{t("shows.noGateway")}</Banner>}
      <div className="grid-2">
        <div className="stack">
          <Field label={t("shows.page")} hint={t("shows.pageHelp")}>
            <code className="show-line">{line(true)}</code>
          </Field>
          {gateway && (
            <div className="row">
              <button type="button" className="btn small" onClick={() => void copy(line(true))}>
                {t("shows.copyLine")}
              </button>
              <a href={pageUrl(gateway, code, module)} target="_blank" rel="noreferrer" className="small">
                {t("shows.viewPage")}
              </a>
            </div>
          )}
          <label className="check">
            <input
              type="checkbox"
              checked={website.small_widget}
              disabled={!canEdit}
              onChange={(e) => setWebsite({ small_widget: e.target.checked })}
            />
            {t("shows.smallWidget")}
          </label>
          {website.small_widget && (
            <>
              <code className="show-line">{line(false)}</code>
              {gateway && (
                <div>
                  <button type="button" className="btn small" onClick={() => void copy(line(false))}>
                    {t("shows.copyLine")}
                  </button>
                </div>
              )}
            </>
          )}
          <Field label={t("shows.closed")}>
            <Segmented
              label={t("shows.closed")}
              value={website.closed}
              disabled={!canEdit}
              onChange={(closed) => setWebsite({ closed })}
              options={[
                { value: "message", label: t("shows.closedMessage") },
                { value: "nothing", label: t("shows.closedNothing") },
              ]}
            />
          </Field>
          {website.closed === "message" && (
            <Field label={t("shows.message")} hint={t("shows.messageHint", { day: "{day}", hour: "{hour}" })}>
              <input
                className="input"
                value={website.closed_message}
                maxLength={200}
                disabled={!canEdit}
                placeholder={t("shows.messagePlaceholder", { day: "{day}", hour: "{hour}" })}
                onChange={(e) => setWebsite({ closed_message: e.target.value })}
              />
            </Field>
          )}
          <div className="form-grid">
            <Field label={t("shows.color")}>
              <input
                className="input"
                type="color"
                value={website.color}
                disabled={!canEdit}
                onChange={(e) => setWebsite({ color: e.target.value })}
              />
            </Field>
            <Field label={t("shows.title")} hint={t("shows.titleHint")}>
              <input
                className="input"
                value={website.title}
                maxLength={60}
                placeholder={title}
                disabled={!canEdit}
                onChange={(e) => setWebsite({ title: e.target.value })}
              />
            </Field>
          </div>
          <Field label={t("shows.pageUrl")} hint={t("shows.pageUrlHint", { link: "{link}" })}>
            <input
              className="input"
              type="url"
              placeholder="https://…"
              value={website.page_url ?? ""}
              disabled={!canEdit}
              onChange={(e) => setWebsite({ page_url: e.target.value.trim() === "" ? null : e.target.value })}
            />
          </Field>
        </div>
        <div className="stack">
          <Segmented
            label={t("shows.preview")}
            value={preview}
            onChange={setPreview}
            options={[
              { value: "open", label: t("shows.previewOpen") },
              { value: "closed", label: t("shows.previewClosed") },
            ]}
          />
          <WidgetPreview
            open={preview === "open"}
            title={website.title || title}
            color={website.color}
            closed={website.closed}
            message={website.closed_message}
            hours={draft.hours}
            next={show.status.next_opening_unix_ms ?? null}
          />
          <p className="muted small">{t("shows.previewHelp")}</p>
        </div>
      </div>
    </Card>
  );
}

/** What the website shows, as the gateway's script draws it. */
export function WidgetPreview({
  open,
  title,
  color,
  closed,
  message,
  hours,
  next,
}: Readonly<{
  open: boolean;
  title: string;
  color: string;
  closed: "message" | "nothing";
  message: string;
  hours: HourRange[];
  next: number | null;
}>) {
  if (!open && closed === "nothing") {
    return <div className="show-widget empty muted small">{t("shows.previewNothing")}</div>;
  }
  const when = next ? dateTime(next) : "…";
  const values = next ? { day: fullDate(next), hour: shortTime(next) } : { day: "…", hour: "" };
  return (
    <div className="show-widget">
      <div className="show-widget-head" style={{ background: open ? color : "var(--muted)" }}>
        <strong>{title}</strong>
        <div className="small">{open ? t("shows.until", { time: "18:00" }) : t("shows.closedTitle")}</div>
      </div>
      <div className="show-widget-body">
        {open ? (
          <p className="muted">{t("shows.previewOpenBody")}</p>
        ) : (
          <>
            <Icon name="clock" size={28} />
            <p>{message ? fillMessage(message, values) : t("shows.backAt", { time: when })}</p>
            {hours.length > 0 && (
              <p className="muted small">
                {hours.map((r) => `${daysLabel(r.days)} ${hourLabel(r.start_hour)}–${hourLabel(r.end_hour)}`).join(" · ")}
              </p>
            )}
          </>
        )}
      </div>
    </div>
  );
}
