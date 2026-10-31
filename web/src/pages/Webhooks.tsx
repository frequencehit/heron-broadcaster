// SPDX-License-Identifier: AGPL-3.0-or-later

// Webhooks on track changes and their delivery log (admins).

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { type DeliveryView, type WebhookEvent, type WebhookView, api, done, unwrap } from "../api";
import { Banner, Card, ConfirmButton, Empty, ErrorView, Field, Icon, Loading, Modal, PageHead } from "../components/ui";
import { rich, t } from "../i18n";
import { ago, clockTime, dateTime } from "../lib/format";
import { useAction, useCan } from "../lib/session";
import { DEFAULT_TEMPLATE, EVENTS, PLACEHOLDERS, eventLabel, toggleEvent, webhookBody } from "../lib/webhooks";
import { here } from "../lib/station";

function StatusBadge({
  delivery,
}: Readonly<{ delivery: Pick<DeliveryView, "status"> & { next_attempt_at_unix_ms?: number | null } }>) {
  switch (delivery.status) {
    case "delivered":
      return <span className="badge good">{t("webhooks.status.delivered")}</span>;
    case "failed":
      return <span className="badge bad">{t("webhooks.status.failed")}</span>;
    default:
      return (
        <span className="badge warn" title={delivery.next_attempt_at_unix_ms ? dateTime(delivery.next_attempt_at_unix_ms) : undefined}>
          {t("webhooks.status.pending")}
        </span>
      );
  }
}

export function Webhooks() {
  const isAdmin = useCan("admin");
  const webhooks = useQuery({
    queryKey: ["webhooks"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/webhooks", { params: { path: here() } })),
    enabled: isAdmin,
    refetchInterval: 10_000,
  });
  const [editing, setEditing] = useState<WebhookView | "new" | null>(null);
  const [secret, setSecret] = useState<{ name: string; secret: string } | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const test = useAction(
    (id: number) => unwrap(api.POST("/api/v1/stations/{station_id}/webhooks/{id}/test", { params: { path: here({ id }) } })),
    { success: t("webhooks.pingSent"), invalidate: [["webhooks"], ["webhook-deliveries"]] },
  );
  const rotate = useAction(
    (webhook: WebhookView) =>
      unwrap(api.POST("/api/v1/stations/{station_id}/webhooks/{id}/secret", { params: { path: here({ id: webhook.id }) } })),
    { onSuccess: (result, webhook) => setSecret({ name: webhook.name, secret: result.secret }) },
  );
  const remove = useAction(
    (id: number) => done(api.DELETE("/api/v1/stations/{station_id}/webhooks/{id}", { params: { path: here({ id }) } })),
    { success: t("webhooks.deleted"), invalidate: [["webhooks"]] },
  );

  if (!isAdmin) {
    return <Banner kind="warn">{t("webhooks.adminsOnly")}</Banner>;
  }
  const shown = webhooks.data?.find((webhook) => webhook.id === selected) ?? null;
  return (
    <>
      <PageHead
        title={t("nav.webhooks")}
        description={t("webhooks.description")}
        actions={
          <button type="button" className="btn primary" onClick={() => setEditing("new")}>
            <Icon name="plus" /> {t("webhooks.new")}
          </button>
        }
      />
      <Card flush>
        {webhooks.isPending && <Loading />}
        {webhooks.isError && <ErrorView error={webhooks.error} />}
        {webhooks.data?.length === 0 && <Empty>{t("webhooks.none")}</Empty>}
        {webhooks.data && webhooks.data.length > 0 && (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("webhooks.name")}</th>
                  <th className="hide-narrow">{t("webhooks.events")}</th>
                  <th>{t("webhooks.lastDelivery")}</th>
                  <th>
                    <span className="visually-hidden">{t("library.actions")}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {webhooks.data.map((webhook) => (
                  <tr key={webhook.id}>
                    <td>
                      <div>
                        {webhook.name} {!webhook.enabled && <span className="badge">{t("output.disabled")}</span>}
                      </div>
                      <div className="mono small muted" style={{ overflowWrap: "anywhere" }}>
                        {webhook.url}
                      </div>
                    </td>
                    <td className="hide-narrow">
                      {webhook.events.map((event) => (
                        <span key={event} className="badge" style={{ marginRight: "0.25rem" }}>
                          {eventLabel(event)}
                        </span>
                      ))}
                      {webhook.include_fx && <span className="badge accent">{t("webhooks.fx")}</span>}
                    </td>
                    <td>
                      {webhook.last_delivery ? (
                        <>
                          <StatusBadge delivery={webhook.last_delivery} />{" "}
                          <span className="muted small">{ago(webhook.last_delivery.created_at_unix_ms)}</span>
                        </>
                      ) : (
                        <span className="muted">{t("format.never")}</span>
                      )}
                    </td>
                    <td className="actions">
                      <button
                        type="button"
                        className="btn small"
                        aria-pressed={webhook.id === selected}
                        onClick={() => setSelected(webhook.id === selected ? null : webhook.id)}
                      >
                        {t("webhooks.deliveries")}
                      </button>
                      <button type="button" className="btn small" onClick={() => test.mutate(webhook.id)}>
                        {t("webhooks.test")}
                      </button>
                      <button
                        type="button"
                        className="btn ghost small icon"
                        title={t("webhooks.edit", { name: webhook.name })}
                        aria-label={t("webhooks.edit", { name: webhook.name })}
                        onClick={() => setEditing(webhook)}
                      >
                        <Icon name="edit" />
                      </button>
                      <ConfirmButton
                        className="btn ghost small icon danger"
                        title={t("categories.delete", { name: webhook.name })}
                        confirm={t("categories.deleteButton")}
                        question={<p>{t("webhooks.deleteQuestion", { name: webhook.name })}</p>}
                        onConfirm={() => {
                          if (selected === webhook.id) {
                            setSelected(null);
                          }
                          remove.mutate(webhook.id);
                        }}
                      >
                        <Icon name="trash" />
                      </ConfirmButton>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
      {shown && <Deliveries webhook={shown} onRotate={() => rotate.mutate(shown)} />}
      <SigningHelp />
      {editing && (
        <WebhookDialog
          webhook={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onCreated={(name, created) => {
            setEditing(null);
            setSecret({ name, secret: created });
          }}
        />
      )}
      {secret && <SecretDialog {...secret} onClose={() => setSecret(null)} />}
    </>
  );
}

function Deliveries({ webhook, onRotate }: Readonly<{ webhook: WebhookView; onRotate: () => void }>) {
  const deliveries = useQuery({
    queryKey: ["webhook-deliveries", webhook.id],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/webhooks/{id}/deliveries", {
          params: { path: here({ id: webhook.id }), query: { limit: 100 } },
        }),
      ),
    refetchInterval: 5_000,
  });
  const retry = useAction(
    (id: number) => unwrap(api.POST("/api/v1/stations/{station_id}/webhook-deliveries/{id}/retry", { params: { path: here({ id }) } })),
    { success: t("webhooks.retrying"), invalidate: [["webhook-deliveries"], ["webhooks"]] },
  );
  return (
    <Card
      title={t("webhooks.deliveriesOf", { name: webhook.name })}
      flush
      actions={
        <ConfirmButton
          className="btn small"
          title={t("webhooks.newSecret")}
          confirm={t("webhooks.newSecretButton")}
          question={<p>{t("webhooks.newSecretQuestion", { name: webhook.name })}</p>}
          onConfirm={onRotate}
        >
          {t("webhooks.newSecret")}
        </ConfirmButton>
      }
    >
      {deliveries.isPending && <Loading />}
      {deliveries.isError && <ErrorView error={deliveries.error} />}
      {deliveries.data?.length === 0 && <Empty>{t("webhooks.noDeliveries")}</Empty>}
      {deliveries.data && deliveries.data.length > 0 && (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>{t("webhooks.time")}</th>
                <th>{t("webhooks.event")}</th>
                <th>{t("webhooks.status")}</th>
                <th className="hide-narrow">{t("webhooks.attempts")}</th>
                <th>{t("webhooks.answer")}</th>
                <th>
                  <span className="visually-hidden">{t("library.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {deliveries.data.map((delivery) => (
                <tr key={delivery.id}>
                  <td style={{ whiteSpace: "nowrap" }}>{dateTime(delivery.created_at_unix_ms)}</td>
                  <td style={{ whiteSpace: "nowrap" }}>{eventLabel(delivery.event)}</td>
                  <td style={{ whiteSpace: "nowrap" }}>
                    <StatusBadge delivery={delivery} />
                    {delivery.status === "pending" && delivery.next_attempt_at_unix_ms && delivery.attempts > 0 && (
                      <div className="muted small">
                        {t("webhooks.nextAttempt", { time: clockTime(delivery.next_attempt_at_unix_ms) })}
                      </div>
                    )}
                  </td>
                  <td className="hide-narrow">{delivery.attempts}</td>
                  <td>
                    <details>
                      <summary>
                        {delivery.last_status_code ?? (delivery.last_error ? t("webhooks.noAnswer") : "—")}
                      </summary>
                      <div className="stack small" style={{ marginTop: "0.5rem" }}>
                        {delivery.last_error && <div style={{ color: "var(--bad)" }}>{delivery.last_error}</div>}
                        <div>
                          <div className="muted">{t("webhooks.sent")}</div>
                          <pre className="mono" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere", margin: 0 }}>
                            {delivery.body}
                          </pre>
                        </div>
                        {delivery.last_response && (
                          <div>
                            <div className="muted">{t("webhooks.response")}</div>
                            <pre className="mono" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere", margin: 0 }}>
                              {delivery.last_response}
                            </pre>
                          </div>
                        )}
                      </div>
                    </details>
                  </td>
                  <td className="actions">
                    {delivery.status !== "pending" && (
                      <button type="button" className="btn small" onClick={() => retry.mutate(delivery.id)}>
                        <Icon name="undo" /> {t("webhooks.retry")}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}

function SigningHelp() {
  return (
    <Card title={t("webhooks.signing")}>
      <p className="muted small" style={{ marginTop: 0 }}>
        {rich("webhooks.signingHelp", {
          header: <code>X-Heron-Signature</code>,
          value: <code>sha256=HMAC-SHA256(secret, timestamp + "." + body)</code>,
          timestamp: <code>X-Heron-Timestamp</code>,
        })}
      </p>
      <p className="muted small" style={{ marginBottom: 0 }}>
        {t("webhooks.retryHelp")}
      </p>
    </Card>
  );
}

function SecretDialog({ name, secret, onClose }: Readonly<{ name: string; secret: string; onClose: () => void }>) {
  return (
    <Modal
      title={t("webhooks.secretOf", { name })}
      open
      onClose={onClose}
      footer={
        <button type="button" className="btn primary" onClick={onClose}>
          {t("webhooks.copied")}
        </button>
      }
    >
      <div className="stack">
        <Banner kind="warn">
          {rich("webhooks.copySecret", { secret: <code style={{ overflowWrap: "anywhere" }}>{secret}</code> })}
        </Banner>
        <p className="muted small" style={{ margin: 0 }}>
          {t("webhooks.secretUse")}
        </p>
      </div>
    </Modal>
  );
}

function WebhookDialog({
  webhook,
  onClose,
  onCreated,
}: Readonly<{
  webhook: WebhookView | null;
  onClose: () => void;
  onCreated: (name: string, secret: string) => void;
}>) {
  const [name, setName] = useState(webhook?.name ?? "");
  const [url, setUrl] = useState(webhook?.url ?? "");
  const [events, setEvents] = useState<WebhookEvent[]>(
    webhook?.events ?? ["track.started", "track.ended", "track.skipped"],
  );
  const [includeFx, setIncludeFx] = useState(webhook?.include_fx ?? false);
  const [enabled, setEnabled] = useState(webhook?.enabled ?? true);
  const [custom, setCustom] = useState(Boolean(webhook?.body_template));
  const [contentType, setContentType] = useState(webhook?.content_type ?? "application/json");
  const [template, setTemplate] = useState(webhook?.body_template ?? DEFAULT_TEMPLATE);
  const body = webhookBody({ name, url, events, includeFx, enabled, custom, contentType, template });
  const save = useAction(
    () =>
      webhook
        ? unwrap(api.PUT("/api/v1/stations/{station_id}/webhooks/{id}", { params: { path: here({ id: webhook.id }) }, body })).then(() => null)
        : unwrap(api.POST("/api/v1/stations/{station_id}/webhooks", { params: { path: here() }, body })).then((created) => created.secret),
    {
      success: t("webhooks.saved"),
      invalidate: [["webhooks"]],
      onSuccess: (secret) => (secret ? onCreated(name, secret) : onClose()),
    },
  );
  return (
    <Modal
      title={webhook ? t("webhooks.edit", { name: webhook.name }) : t("webhooks.new")}
      open
      wide
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="submit"
            form="webhook-form"
            className="btn primary"
            disabled={save.isPending || events.length === 0}
          >
            {webhook ? t("webhooks.save") : t("account.create")}
          </button>
        </>
      }
    >
      <form
        id="webhook-form"
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          save.mutate(undefined);
        }}
      >
        <Field label={t("webhooks.name")}>
          <input className="input" value={name} onChange={(e) => setName(e.target.value)} required autoComplete="off" />
        </Field>
        <Field label={t("webhooks.url")} hint={t("webhooks.urlHint")}>
          <input
            className="input mono"
            type="url"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://example.org/heron"
            required
          />
        </Field>
        <fieldset>
          <legend>{t("webhooks.events")}</legend>
          <div className="row">
            {EVENTS.map((option) => (
              <label key={option.value} className="check">
                <input
                  type="checkbox"
                  checked={events.includes(option.value)}
                  onChange={(e) => setEvents(toggleEvent(events, option.value, e.target.checked))}
                />{" "}
                {t(option.label)}
              </label>
            ))}
          </div>
          <p className="muted small" style={{ marginBottom: 0 }}>
            {t("webhooks.eventsHint")}
          </p>
        </fieldset>
        <div className="row">
          <label className="check">
            <input type="checkbox" checked={includeFx} onChange={(e) => setIncludeFx(e.target.checked)} />{" "}
            {t("webhooks.includeFx")}
          </label>
          <label className="check">
            <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} /> {t("outputs.enabled")}
          </label>
        </div>
        <fieldset>
          <legend>{t("webhooks.body")}</legend>
          <label className="check">
            <input type="radio" name="body" checked={!custom} onChange={() => setCustom(false)} /> {t("webhooks.defaultBody")}
          </label>
          <label className="check">
            <input type="radio" name="body" checked={custom} onChange={() => setCustom(true)} /> {t("webhooks.customBody")}
          </label>
          {custom && (
            <div className="stack" style={{ marginTop: "0.75rem" }}>
              <Field label={t("webhooks.contentType")}>
                <input className="input mono" value={contentType} onChange={(e) => setContentType(e.target.value)} required />
              </Field>
              <Field
                label={t("webhooks.template")}
                hint={rich("webhooks.templateHint", {
                  placeholders: <code>{PLACEHOLDERS.map((p) => `{{${p}}}`).join(" ")}</code>,
                })}
              >
                <textarea
                  className="input mono"
                  rows={6}
                  value={template}
                  onChange={(e) => setTemplate(e.target.value)}
                  required
                />
              </Field>
            </div>
          )}
        </fieldset>
      </form>
    </Modal>
  );
}
