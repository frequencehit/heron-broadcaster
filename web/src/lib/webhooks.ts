// SPDX-License-Identifier: AGPL-3.0-or-later

// Webhook events, placeholders and the body sent to save a webhook.

import type { WebhookEvent } from "../api";
import { type PlainKey, t } from "../i18n";

export const EVENTS: { value: WebhookEvent; label: PlainKey }[] = [
  { value: "track.started", label: "webhooks.event.started" },
  { value: "track.ended", label: "webhooks.event.ended" },
  { value: "track.skipped", label: "webhooks.event.skipped" },
  { value: "queue.changed", label: "webhooks.event.queueChanged" },
  { value: "certification.failed", label: "webhooks.event.certificationFailed" },
];

/** What a body template may hold, as `{{name}}`. */
export const PLACEHOLDERS = [
  "event",
  "artist",
  "title",
  "album",
  "duration_ms",
  "played_ms",
  "reason",
  "deck",
  "source",
  "track_id",
  "at_unix_ms",
  "delivery_id",
  "upcoming_json",
  "next.artist",
  "next.title",
  "next.album",
  "next.planned_start_unix_ms",
  "dedication_from",
  "dedication_to",
  "dedication_message",
  "message",
];

export const DEFAULT_TEMPLATE = '{"text": "Now playing: {{artist}} – {{title}}"}';

/** `Track started`, `Test`; an unknown event as it is. */
export function eventLabel(event: string): string {
  const known = EVENTS.find((e) => e.value === event);
  if (known) {
    return t(known.label);
  }
  return event === "ping" ? t("webhooks.event.ping") : event;
}

/** The events with `event` checked or not, in the order of `EVENTS`. */
export function toggleEvent(events: readonly WebhookEvent[], event: WebhookEvent, checked: boolean): WebhookEvent[] {
  return EVENTS.map((e) => e.value).filter((v) => (v === event ? checked : events.includes(v)));
}

export type WebhookForm = {
  name: string;
  url: string;
  events: WebhookEvent[];
  includeFx: boolean;
  enabled: boolean;
  /** A body template instead of the default JSON body. */
  custom: boolean;
  contentType: string;
  template: string;
};

/** What the form saves: the default JSON body unless a template is chosen. */
export function webhookBody(form: Readonly<WebhookForm>) {
  return {
    name: form.name,
    url: form.url,
    events: form.events,
    include_fx: form.includeFx,
    enabled: form.enabled,
    content_type: form.custom ? form.contentType : "application/json",
    body_template: form.custom ? form.template : null,
  };
}
