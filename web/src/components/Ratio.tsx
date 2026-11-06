// SPDX-License-Identifier: AGPL-3.0-or-later

// The play ratio (docs/rotation.md#play-ratio): its scale, a station's
// counter drawn as its 10 steps, weight and ratio explained, and the dialogs
// that disable tracks (a ratio of 0) or enable them again at a ratio.

import { useEffect, useState } from "react";
import { type Schemas, api, unwrap } from "../api";
import { t } from "../i18n";
import { cells, effectiveRatio, ratioLabel, ratioSummary } from "../lib/ratio";
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
  station,
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
  previous?: number | undefined;
  /** Disabled on this station only (its name). */
  station?: string | undefined;
}>) {
  const waiting = useWaiting() ?? [];
  const queued = waiting.filter((entry) => entry.track.track_id && trackIds.includes(entry.track.track_id)).length;
  let title = name ? t("ratio.disableOne", { name }) : t("ratio.disableMany", { count });
  if (station) {
    title = name ? t("ratio.disableOneOn", { name, station }) : t("ratio.disableManyOn", { count, station });
  }
  return (
    <Modal
      title={title}
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
            {station ? t("ratio.disableHere") : t("track.disable")}
          </button>
        </>
      }
    >
      <div className="stack">
        <p style={{ margin: 0 }}>
          {station ? t("ratio.disableQuestionOn", { count, station }) : t("ratio.disableQuestion", { count })}
        </p>
        <p className="muted small" style={{ margin: 0 }}>
          {previous !== undefined
            ? t(station ? "ratio.disableKeepsOneOn" : "ratio.disableKeepsOne", { ratio: ratioLabel(previous) })
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
  station,
  fallback,
}: Readonly<{
  open: boolean;
  onClose: () => void;
  onConfirm: (ratio: number, resetCounters: boolean) => void;
  name?: string;
  count: number;
  /** The ratio they had (the same for all), or none. */
  proposed: number | null;
  pending?: boolean;
  /** Enabled on this station only (its name). */
  station?: string;
  /** On a station: the track's default ratio, proposed when it had none
   * of its own there. */
  fallback?: number;
}>) {
  const start = proposed ?? fallback ?? 10;
  const [ratio, setRatio] = useState(start);
  const [reset, setReset] = useState(true);
  useEffect(() => {
    if (open) {
      setRatio(start);
      setReset(true);
    }
  }, [open, start]);
  let title = name ? t("ratio.enableOne", { name }) : t("ratio.enableMany", { count });
  if (station) {
    title = name ? t("ratio.enableOneOn", { name, station }) : t("ratio.enableManyOn", { count, station });
  }
  let proposedHint = "";
  if (proposed !== null && proposed === ratio) {
    proposedHint = t(station ? "ratio.proposedOn" : "ratio.proposed", { ratio: ratioLabel(proposed) });
  } else if (proposed === null && fallback !== undefined && fallback === ratio) {
    proposedHint = t("ratio.proposedDefault", { ratio: ratioLabel(fallback) });
  }
  return (
    <Modal
      title={title}
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
            {proposedHint} {ratioSummary(ratio)}
          </span>
        </div>
        <label className="check">
          <input type="checkbox" checked={reset} onChange={(event) => setReset(event.target.checked)} />
          {station ? t("ratio.resetOnEnableOn", { station }) : t("ratio.resetOnEnable")}
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
  station,
}: Readonly<{
  open: boolean;
  onClose: () => void;
  selection: Selection;
  /** How many tracks are selected. */
  tracks: number;
  /** The selected tracks seen, for their ratios today. */
  known: Schemas["TrackSummary"][];
  /** Sets their ratio on this station, not their default. */
  station?: { id: number; name: string } | undefined;
}>) {
  const [ratio, setRatio] = useState<number | null>(null);
  const [confirm, setConfirm] = useState(false);
  useEffect(() => {
    if (open) {
      setRatio(null);
    }
  }, [open]);
  const apply = useAction(
    (play_ratio: number) =>
      unwrap(
        api.POST("/api/v1/tracks/programming", {
          body: station ? { selection, play_ratio, station_id: station.id } : { selection, play_ratio },
        }),
      ),
    {
      success: (result) =>
        ratio === 0 ? t("library.enabledChanged", { count: result.changed }) : t("ratio.set", { count: result.tracks }),
      invalidate: TRACKS,
      onSuccess: onClose,
    },
  );
  const today = [...new Set(known.map((track) => effectiveRatio(track)))].sort((a, b) => a - b);
  return (
    <>
      <Modal
        title={
          station ? t("ratio.ofTracksOn", { count: tracks, station: station.name }) : t("ratio.ofTracks", { count: tracks })
        }
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
            {ratio !== null && ratioSummary(ratio)} {station ? t("ratio.zeroDisablesOn") : t("ratio.zeroDisables")}
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
        station={station?.name}
      />
    </>
  );
}

/** Sends a CSV file of ratios: a dry run, or for real. */
function importRatios(content: string, dryRun: boolean) {
  return unwrap(
    api.POST("/api/v1/tracks/ratios/import", {
      params: { query: { dry_run: dryRun } },
      body: content,
      bodySerializer: () => content,
      headers: { "Content-Type": "text/csv" },
    }),
  );
}

/**
 * Imports play ratios from a CSV file: one column per station code (and
 * `default`), the track named by `path` or `filename`. A dry run first,
 * with the rows matching no track and the values that are not ratios.
 */
export function RatioImportDialog({ open, onClose }: Readonly<{ open: boolean; onClose: () => void }>) {
  const [content, setContent] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [report, setReport] = useState<Schemas["RatioImport"] | null>(null);
  useEffect(() => {
    if (open) {
      setContent(null);
      setName("");
      setReport(null);
    }
  }, [open]);
  const check = useAction((text: string) => importRatios(text, true), { onSuccess: setReport });
  const apply = useAction(() => importRatios(content ?? "", false), {
    success: (result) => t("ratioImport.done", { count: result.changed }),
    invalidate: TRACKS,
    onSuccess: onClose,
  });
  const choose = async (file: File | undefined) => {
    if (!file) {
      return;
    }
    const text = await file.text();
    setName(file.name);
    setContent(text);
    setReport(null);
    check.mutate(text);
  };
  return (
    <Modal
      title={t("ratioImport.title")}
      open={open}
      onClose={onClose}
      wide
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("ui.cancel")}
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={!report || report.values === 0 || apply.isPending}
            onClick={() => apply.mutate(undefined)}
          >
            {t("ratioImport.apply", { count: report?.values ?? 0 })}
          </button>
        </>
      }
    >
      <div className="stack">
        <p className="small" style={{ margin: 0 }}>
          {t("ratioImport.help")}
        </p>
        <pre className="small muted" style={{ margin: 0 }}>
          {t("ratioImport.example")}
        </pre>
        <label className="field">
          <span>{t("ratioImport.file")}</span>
          <input
            type="file"
            accept=".csv,text/csv"
            onChange={(event) => void choose(event.target.files?.[0])}
          />
          {name && <span className="hint">{name}</span>}
        </label>
        {check.isPending && <span className="muted small">{t("ratioImport.checking")}</span>}
        {report && <RatioImportReport report={report} />}
      </div>
    </Modal>
  );
}

function RatioImportReport({ report }: Readonly<{ report: Schemas["RatioImport"] }>) {
  return (
    <div className="stack">
      <p className="small" style={{ margin: 0 }}>
        {t("ratioImport.summary", {
          rows: report.rows,
          matched: report.matched,
          values: report.values,
          by: report.matched_by,
        })}{" "}
        {report.columns.length > 0
          ? t("ratioImport.columns", { columns: report.columns.join(", ") })
          : t("ratioImport.noColumn")}
      </p>
      {report.unknown_columns.length > 0 && (
        <Banner kind="warn">{t("ratioImport.unknownColumns", { columns: report.unknown_columns.join(", ") })}</Banner>
      )}
      {report.unmatched_total > 0 && (
        <div className="small">
          <strong>{t("ratioImport.unmatched", { count: report.unmatched_total })}</strong>
          <ul className="held">
            {report.unmatched.map((row) => (
              <li key={row.line}>
                {t("ratioImport.line", { line: row.line })} · {row.name || "–"} · {t(`ratioImport.reason.${row.reason}`)}
              </li>
            ))}
          </ul>
        </div>
      )}
      {report.invalid_total > 0 && (
        <div className="small">
          <strong>{t("ratioImport.invalid", { count: report.invalid_total })}</strong>
          <ul className="held">
            {report.invalid.map((value) => (
              <li key={`${value.line} ${value.column}`}>
                {t("ratioImport.line", { line: value.line })} · {value.column}: “{value.value}”
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
