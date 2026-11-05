// SPDX-License-Identifier: AGPL-3.0-or-later

// The play ratio (docs/rotation.md#play-ratio): its scale, a station's
// counter drawn as its 10 steps, weight and ratio explained, and the dialogs
// that disable tracks (a ratio of 0) or enable them again at a ratio.

import { useEffect, useState } from "react";
import { type Schemas, api, unwrap } from "../api";
import { t } from "../i18n";
import { cells, ratioLabel, ratioSummary } from "../lib/ratio";
import { useAction } from "../lib/session";
import { useWaiting } from "./QueueList";
import type { Selection } from "./SelectionActions";
import { Banner, Modal } from "./ui";

/** Queries a change of ratio or of enabled changes. */
const TRACKS: string[][] = [["tracks"], ["track"]];

/** The ratios to choose from, in tenths: 0 (disables) or 1 to 10. */
export function RatioScale({
  value,
  onChange,
  withZero,
  disabled,
}: Readonly<{
  value: number | null;
  onChange: (tenths: number) => void;
  withZero?: boolean;
  disabled?: boolean;
}>) {
  const steps = Array.from({ length: withZero ? 11 : 10 }, (_, index) => (withZero ? index : index + 1));
  return (
    <div className="segmented ratio-scale" role="group" aria-label={t("ratio.title")}>
      {steps.map((step) => (
        <button
          key={step}
          type="button"
          aria-pressed={step === value}
          disabled={disabled}
          onClick={() => onChange(step)}
        >
          {ratioLabel(step)}
        </button>
      ))}
    </div>
  );
}

/** A station's counter: its 10 steps, those that play and the next one. */
export function RatioCounter({ ratio, counter }: Readonly<{ ratio: number; counter: number }>) {
  return (
    <div className="ratio-counter">
      {cells(ratio, counter).map((cell) => {
        const word = cell.plays ? t("ratio.plays") : t("ratio.held");
        return (
          <span
            key={cell.step}
            className={`ratio-cell ${cell.plays ? "plays" : "waits"}${cell.next ? " next" : ""}`}
            title={`${cell.step}: ${cell.plays ? t("ratio.plays") : t("ratio.heldHelp")}`}
            aria-current={cell.next ? "step" : undefined}
          >
            {cell.step}
            <small>{word}</small>
          </span>
        );
      })}
    </div>
  );
}

/** Weight and ratio, side by side. */
export function WeightOrRatio() {
  return (
    <div className="ratio-explain" role="note">
      <strong className="small">{t("ratio.weightOrRatio")}</strong>
      <dl className="facts small">
        <dt>{t("track.weight")}</dt>
        <dd>{t("ratio.weightIs")}</dd>
        <dt>{t("ratio.short")}</dt>
        <dd>{t("ratio.ratioIs")}</dd>
      </dl>
      <p className="muted small">{t("ratio.together")}</p>
    </div>
  );
}

/** Asks before disabling tracks: a ratio of 0. `name` for one track. */
export function DisableDialog({
  open,
  onClose,
  onConfirm,
  name,
  count,
  trackIds,
  previous,
}: Readonly<{
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
  /** The track's name, for one track. */
  name?: string;
  count: number;
  /** The tracks known, to say which wait in the queue. */
  trackIds: number[];
  /** The ratio a single track had, proposed when it is enabled again. */
  previous?: number;
}>) {
  const waiting = useWaiting() ?? [];
  const queued = waiting.filter((entry) => entry.track.track_id && trackIds.includes(entry.track.track_id)).length;
  return (
    <Modal
      title={name ? t("ratio.disableOne", { name }) : t("ratio.disableMany", { count })}
      open={open}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("ui.cancel")}
          </button>
          <button
            type="button"
            className="btn danger solid"
            onClick={() => {
              onClose();
              onConfirm();
            }}
          >
            {t("track.disable")}
          </button>
        </>
      }
    >
      <div className="stack">
        <p style={{ margin: 0 }}>{t("ratio.disableQuestion", { count })}</p>
        <p className="muted small" style={{ margin: 0 }}>
          {previous !== undefined
            ? t("ratio.disableKeepsOne", { ratio: ratioLabel(previous) })
            : t("ratio.disableKeeps", { count })}
        </p>
        {queued > 0 && <Banner kind="warn">{t("ratio.disableQueued", { count: queued })}</Banner>}
      </div>
    </Modal>
  );
}

/** Asks for the ratio tracks are enabled again at. */
export function EnableDialog({
  open,
  onClose,
  onConfirm,
  name,
  count,
  proposed,
  pending,
}: Readonly<{
  open: boolean;
  onClose: () => void;
  onConfirm: (ratio: number, resetCounters: boolean) => void;
  name?: string;
  count: number;
  /** The ratio they had (the same for all), or none. */
  proposed: number | null;
  pending?: boolean;
}>) {
  const [ratio, setRatio] = useState(proposed ?? 10);
  const [reset, setReset] = useState(true);
  useEffect(() => {
    if (open) {
      setRatio(proposed ?? 10);
      setReset(true);
    }
  }, [open, proposed]);
  return (
    <Modal
      title={name ? t("ratio.enableOne", { name }) : t("ratio.enableMany", { count })}
      open={open}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("ui.cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={pending}
            onClick={() => onConfirm(ratio, reset)}
          >
            {t("ratio.enableAt", { ratio: ratioLabel(ratio) })}
          </button>
        </>
      }
    >
      <div className="stack">
        <p style={{ margin: 0 }}>{t("ratio.enableQuestion", { count })}</p>
        <div className="field">
          <span>{t("ratio.title")}</span>
          <RatioScale value={ratio} onChange={setRatio} />
          <span className="hint">
            {proposed !== null && proposed === ratio
              ? t("ratio.proposed", { ratio: ratioLabel(proposed) })
              : ""}{" "}
            {ratioSummary(ratio)}
          </span>
        </div>
        <label className="check">
          <input type="checkbox" checked={reset} onChange={(event) => setReset(event.target.checked)} />
          {t("ratio.resetOnEnable")}
        </label>
      </div>
    </Modal>
  );
}

/** Sets the ratio of a selection; 0 disables the tracks, after a confirmation. */
export function RatioDialog({
  open,
  onClose,
  selection,
  tracks,
  known,
}: Readonly<{
  open: boolean;
  onClose: () => void;
  selection: Selection;
  /** How many tracks are selected. */
  tracks: number;
  /** The selected tracks seen, for their ratios today. */
  known: Schemas["TrackSummary"][];
}>) {
  const [ratio, setRatio] = useState<number | null>(null);
  const [confirm, setConfirm] = useState(false);
  useEffect(() => {
    if (open) {
      setRatio(null);
    }
  }, [open]);
  const apply = useAction(
    (play_ratio: number) => unwrap(api.POST("/api/v1/tracks/programming", { body: { selection, play_ratio } })),
    {
      success: (result) =>
        ratio === 0 ? t("library.enabledChanged", { count: result.changed }) : t("ratio.set", { count: result.tracks }),
      invalidate: TRACKS,
      onSuccess: onClose,
    },
  );
  const today = [...new Set(known.map((track) => (track.enabled ? track.play_ratio : 0)))].sort((a, b) => a - b);
  return (
    <>
      <Modal
        title={t("ratio.ofTracks", { count: tracks })}
        open={open && !confirm}
        onClose={onClose}
        footer={
          <>
            <button type="button" className="btn" onClick={onClose}>
              {t("ui.cancel")}
            </button>
            <button
              type="button"
              className={ratio === 0 ? "btn danger solid" : "btn primary"}
              disabled={ratio === null || apply.isPending}
              onClick={() => (ratio === 0 ? setConfirm(true) : ratio !== null && apply.mutate(ratio))}
            >
              {ratio === null ? t("ratio.choose") : t("ratio.setTo", { ratio: ratioLabel(ratio) })}
            </button>
          </>
        }
      >
        <div className="stack">
          <RatioScale value={ratio} onChange={setRatio} withZero />
          <span className="hint small muted">
            {today.length > 0 && today.length <= 5 && t("ratio.today", { ratios: today.map(ratioLabel).join(", ") })}{" "}
            {ratio !== null && ratioSummary(ratio)} {t("ratio.zeroDisables")}
          </span>
          <WeightOrRatio />
        </div>
      </Modal>
      <DisableDialog
        open={open && confirm}
        onClose={() => setConfirm(false)}
        onConfirm={() => apply.mutate(0)}
        count={tracks}
        trackIds={known.map((track) => track.id)}
      />
    </>
  );
}
