// SPDX-License-Identifier: AGPL-3.0-or-later

// The master AGC chain of the station: one panel per block in the chain's
// order, each with its switch and a line saying what it does, the chain's
// meters, and the presets. Changes are heard as the controls move.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useId, useMemo, useRef, useState } from "react";
import {
  type AgcPreset,
  type AgcPresetSettings,
  type AgcSettings,
  type BandMode,
  type BandSettings,
  api,
  done,
  errorMessage,
  unwrap,
} from "../api";
import { ReductionMeter, StereoMeter } from "../components/Meter";
import {
  Card,
  ConfirmButton,
  ErrorView,
  Field,
  Icon,
  Loading,
  Modal,
  NumberInput,
  PageHead,
  Segmented,
  useToast,
} from "../components/ui";
import { type PlainKey, t } from "../i18n";
import {
  AGC_KEY,
  AGC_PRESETS_KEY,
  BLOCKS,
  type Block,
  RANGES,
  type Range,
  SLIDER_STEPS,
  bandRanges,
  blocksOf,
  formatDb,
  formatHz,
  fromPosition,
  keyStep,
  presetMatching,
  snap,
  toPosition,
  useAgcSettings,
  withBlocks,
  withCrossover,
} from "../lib/agc";
import { latestSender } from "../lib/eq";
import { useLive } from "../lib/live";
import { useAction, useCan } from "../lib/session";
import { here } from "../lib/station";

/** A change of ours, younger than this, is not overwritten by older news. */
const EDITING_MS = 1_500;

const BLOCK_NAMES: Record<Block, PlainKey> = {
  gated_agc: "agc.gatedAgc",
  stereo_expander: "agc.stereoExpander",
  bass_eq: "agc.bassEq",
  five_band: "agc.fiveBand",
  dual_band: "agc.dualBand",
  final_stage: "agc.finalStage",
};

/** What an arrow key does on a slider: up or down by `keyStep`. */
const ARROWS: Record<string, 1 | -1 | undefined> = { ArrowUp: 1, ArrowRight: 1, ArrowDown: -1, ArrowLeft: -1 };

const MODES: { value: BandMode; label: PlainKey }[] = [
  { value: "compressor", label: "agc.compressor" },
  { value: "expander", label: "agc.expander" },
  { value: "limiter", label: "agc.limiter" },
];

export function Agc() {
  const { data, error } = useAgcSettings();
  if (error) {
    return <ErrorView error={error} />;
  }
  if (!data) {
    return <Loading />;
  }
  return <AgcEditor saved={data} />;
}

function AgcEditor({ saved }: Readonly<{ saved: AgcSettings }>) {
  const canEdit = useCan("operator");
  const client = useQueryClient();
  const toast = useToast();
  const [draft, setDraft] = useState(saved);
  const lastEdit = useRef(0);
  const sender = useMemo(
    () =>
      latestSender((agc: AgcSettings) =>
        unwrap(api.PUT("/api/v1/stations/{station_id}/settings/agc", { params: { path: here() }, body: agc })).then((result) =>
          client.setQueriesData({ queryKey: AGC_KEY }, result),
        ),
      ),
    [client],
  );
  useEffect(() => {
    sender.onError = (error) => toast(errorMessage(error), true);
  }, [sender, toast]);
  // Changes from elsewhere show, unless a control is moving here.
  useEffect(() => {
    if (!sender.active && Date.now() - lastEdit.current > EDITING_MS) {
      setDraft(saved);
    }
  }, [saved, sender]);

  const change = (next: AgcSettings) => {
    lastEdit.current = Date.now();
    setDraft(next);
    sender.push(next);
  };
  const changeBlock = <B extends Block>(block: B, value: AgcSettings[B]) => change({ ...draft, [block]: value });
  const disabled = !canEdit;
  const off = draft.bypass_all;

  return (
    <>
      <PageHead title={t("nav.agc")} description={t("agc.description")} />
      <div className="stack">
        <ChainCard settings={draft} disabled={disabled} onChange={change} />
        <div className={off ? "agc-blocks bypassed" : "agc-blocks"}>
          <GatedAgcCard value={draft.gated_agc} disabled={disabled} onChange={(v) => changeBlock("gated_agc", v)} />
          <BlockCard
            block="stereo_expander"
            hint={t("agc.stereoExpanderHint")}
            enabled={draft.stereo_expander.enabled}
            disabled={disabled}
            onToggle={(enabled) => changeBlock("stereo_expander", { ...draft.stereo_expander, enabled })}
          >
            <div className="agc-controls">
              <Control
                label={t("agc.level")}
                hint={t("agc.levelHint")}
                range={RANGES.percent}
                value={draft.stereo_expander.level}
                disabled={disabled}
                onChange={(level) => changeBlock("stereo_expander", { ...draft.stereo_expander, level })}
              />
              <Control
                label={t("agc.depth")}
                hint={t("agc.depthHint")}
                range={RANGES.percent}
                value={draft.stereo_expander.depth}
                disabled={disabled}
                onChange={(depth) => changeBlock("stereo_expander", { ...draft.stereo_expander, depth })}
              />
              <Control
                label={t("agc.threshold")}
                hint={t("agc.stereoThresholdHint")}
                range={RANGES.levelDb}
                value={draft.stereo_expander.threshold_db}
                disabled={disabled}
                onChange={(threshold_db) => changeBlock("stereo_expander", { ...draft.stereo_expander, threshold_db })}
              />
            </div>
          </BlockCard>
          <BlockCard
            block="bass_eq"
            hint={t("agc.bassEqHint")}
            enabled={draft.bass_eq.enabled}
            disabled={disabled}
            onToggle={(enabled) => changeBlock("bass_eq", { ...draft.bass_eq, enabled })}
          >
            <div className="agc-controls">
              <Field label={t("agc.shape")}>
                <Segmented
                  label={t("agc.shape")}
                  value={draft.bass_eq.shape}
                  disabled={disabled}
                  options={[
                    { value: "shelf", label: t("agc.shelf") },
                    { value: "peak", label: t("agc.peak") },
                  ]}
                  onChange={(shape) => changeBlock("bass_eq", { ...draft.bass_eq, shape })}
                />
              </Field>
              <Control
                label={t("agc.frequency")}
                range={RANGES.bassFrequencyHz}
                value={draft.bass_eq.frequency_hz}
                disabled={disabled}
                onChange={(frequency_hz) => changeBlock("bass_eq", { ...draft.bass_eq, frequency_hz })}
              />
              <Control
                label={t("agc.gain")}
                range={RANGES.gainDb}
                value={draft.bass_eq.gain_db}
                reset={0}
                disabled={disabled}
                onChange={(gain_db) => changeBlock("bass_eq", { ...draft.bass_eq, gain_db })}
              />
            </div>
          </BlockCard>
          <FiveBandCard value={draft.five_band} disabled={disabled} onChange={(v) => changeBlock("five_band", v)} />
          <DualBandCard value={draft.dual_band} disabled={disabled} onChange={(v) => changeBlock("dual_band", v)} />
          <FinalStageCard value={draft.final_stage} disabled={disabled} onChange={(v) => changeBlock("final_stage", v)} />
        </div>
        <Presets current={draft} canEdit={canEdit} onLoad={(blocks) => change(withBlocks(draft, blocks))} />
      </div>
    </>
  );
}

function usePresets() {
  return useQuery({
    queryKey: AGC_PRESETS_KEY,
    queryFn: () => unwrap(api.GET("/api/v1/agc-presets")),
  });
}

/** Bypass all, the chain's blocks in order, its meters, loading a preset. */
function ChainCard({
  settings,
  disabled,
  onChange,
}: Readonly<{ settings: AgcSettings; disabled: boolean; onChange: (settings: AgcSettings) => void }>) {
  const presets = usePresets();
  const levels = useLive((live) => live.levels);
  const meters = levels?.agc ?? null;
  const loaded = presetMatching(presets.data, settings);
  const off = settings.bypass_all;
  return (
    <Card
      title={
        <div>
          <h2>{t("agc.chain")}</h2>
          <p className="muted small" style={{ margin: 0 }}>
            {off ? t("agc.bypassedHint") : t("agc.chainHint")}
          </p>
        </div>
      }
      actions={
        <>
          <select
            className="input"
            aria-label={t("agc.loadPreset")}
            value={loaded ? String(loaded.id) : ""}
            disabled={disabled || !presets.data}
            onChange={(event) => {
              const preset = presets.data?.find((p) => String(p.id) === event.target.value);
              if (preset) {
                onChange(withBlocks(settings, preset.settings));
              }
            }}
          >
            <option value="">{loaded ? loaded.name : t("agc.loadPreset")}</option>
            {presets.data?.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {preset.name}
              </option>
            ))}
          </select>
          <label className="check">
            <input
              type="checkbox"
              checked={off}
              disabled={disabled}
              onChange={(event) => onChange({ ...settings, bypass_all: event.target.checked })}
            />
            {t("agc.bypassAll")}
          </label>
        </>
      }
    >
      <div className="agc-chain">
        <ol className={off ? "agc-flow bypassed" : "agc-flow"} aria-label={t("agc.order")}>
          {BLOCKS.map((block) => (
            <li key={block} className={settings[block].enabled ? "on" : ""}>
              <a href={`#agc-${block}`}>{t(BLOCK_NAMES[block])}</a>
            </li>
          ))}
        </ol>
        <div className="agc-meters">
          <StereoMeter levels={meters?.input ?? null} label={t("agc.input")} />
          <StereoMeter levels={meters ? (levels?.master ?? null) : null} label={t("agc.output")} />
          <div className="agc-readouts">
            <div>
              <span className="muted small">{t("agc.agcGain")}</span>
              <span className="num">
                {meters?.agc_gain_db === null || meters?.agc_gain_db === undefined
                  ? "–"
                  : `${formatDb(meters.agc_gain_db)} dB`}
              </span>
            </div>
            <div>
              <span className="muted small">{t("agc.finalStage")}</span>
              <ReductionMeter db={meters?.final_stage_db} label={t("agc.finalStage")} />
            </div>
            {!meters && <p className="muted small">{t("agc.noMeters")}</p>}
          </div>
        </div>
      </div>
    </Card>
  );
}

function BlockCard({
  block,
  hint,
  enabled,
  disabled,
  onToggle,
  extra,
  children,
}: Readonly<{
  block: Block;
  hint: string;
  enabled: boolean;
  disabled: boolean;
  onToggle: (enabled: boolean) => void;
  extra?: ReactNode;
  children: ReactNode;
}>) {
  return (
    <div id={`agc-${block}`} className={enabled ? "agc-block" : "agc-block off"}>
      <Card
        title={
          <div>
            <h2>{t(BLOCK_NAMES[block])}</h2>
            <p className="muted small" style={{ margin: 0 }}>
              {hint}
            </p>
          </div>
        }
        actions={
          <>
            {extra}
            <label className="check">
              <input
                type="checkbox"
                checked={enabled}
                disabled={disabled}
                onChange={(event) => onToggle(event.target.checked)}
              />
              {t("agc.on")}
            </label>
          </>
        }
      >
        {children}
      </Card>
    </div>
  );
}

/** A slider with a number to type the value; double-click for `reset`. */
function Control({
  label,
  hint,
  range,
  value,
  disabled,
  reset,
  onChange,
}: Readonly<{
  label: string;
  hint?: string;
  range: Range;
  value: number;
  disabled: boolean;
  reset?: number;
  onChange: (value: number) => void;
}>) {
  const id = useId();
  return (
    <div className="agc-control">
      <label htmlFor={id}>{label}</label>
      <NumberInput
        value={value}
        min={range.min}
        max={range.max}
        step={range.step}
        disabled={disabled}
        suffix={range.unit}
        label={label}
        onChange={(next) => onChange(snap(range, next))}
      />
      <input
        id={id}
        type="range"
        min={0}
        max={SLIDER_STEPS}
        step={1}
        value={toPosition(range, value)}
        disabled={disabled}
        aria-valuetext={`${value} ${range.unit}`}
        title={reset === undefined ? hint : t("agc.reset", { value: String(reset) })}
        onChange={(event) => onChange(fromPosition(range, Number(event.target.value)))}
        onDoubleClick={() => reset !== undefined && onChange(reset)}
        onKeyDown={(event) => {
          const direction = ARROWS[event.key];
          if (direction) {
            event.preventDefault();
            onChange(keyStep(range, value, direction));
          }
        }}
      />
      {hint && <span className="hint">{hint}</span>}
    </div>
  );
}

function GatedAgcCard({
  value,
  disabled,
  onChange,
}: Readonly<{
  value: AgcSettings["gated_agc"];
  disabled: boolean;
  onChange: (value: AgcSettings["gated_agc"]) => void;
}>) {
  const playout = useQuery({
    queryKey: ["settings", "playout"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/playout", { params: { path: here() } })),
  });
  const gain = useLive((live) => live.levels?.agc?.agc_gain_db);
  const target = playout.data?.loudness.target_lufs;
  return (
    <BlockCard
      block="gated_agc"
      hint={t("agc.gatedAgcHint")}
      enabled={value.enabled}
      disabled={disabled}
      onToggle={(enabled) => onChange({ ...value, enabled })}
      extra={
        gain !== null && gain !== undefined && <span className="badge num">{`${formatDb(gain)} dB`}</span>
      }
    >
      <div className="agc-controls">
        <Control
          label={t("agc.gate")}
          hint={t("agc.gateHint")}
          range={RANGES.gateLufs}
          value={value.gate_lufs}
          disabled={disabled}
          onChange={(gate_lufs) => onChange({ ...value, gate_lufs })}
        />
        <Control
          label={t("agc.maxGain")}
          hint={t("agc.maxGainHint")}
          range={RANGES.maxGainDb}
          value={value.max_gain_db}
          disabled={disabled}
          onChange={(max_gain_db) => onChange({ ...value, max_gain_db })}
        />
      </div>
      {target !== undefined && (
        <p className="muted small agc-note">{t("agc.target", { target: String(target) })}</p>
      )}
    </BlockCard>
  );
}

function FiveBandCard({
  value,
  disabled,
  onChange,
}: Readonly<{
  value: AgcSettings["five_band"];
  disabled: boolean;
  onChange: (value: AgcSettings["five_band"]) => void;
}>) {
  const reductions = useLive((live) => live.levels?.agc?.five_band_db);
  return (
    <BlockCard
      block="five_band"
      hint={t("agc.fiveBandHint")}
      enabled={value.enabled}
      disabled={disabled}
      onToggle={(enabled) => onChange({ ...value, enabled })}
    >
      <div className="agc-controls">
        <Control
          label={t("agc.overdrive")}
          hint={t("agc.overdriveHint")}
          range={RANGES.overdriveDb}
          value={value.overdrive_db}
          reset={0}
          disabled={disabled}
          onChange={(overdrive_db) => onChange({ ...value, overdrive_db })}
        />
        {value.crossovers_hz.map((frequency, index) => {
          // The bands it splits name a crossover: there are always four.
          const bands = `${index + 1}–${index + 2}`;
          return (
            <Control
              key={bands}
              label={t("agc.crossover", { bands })}
              range={RANGES.crossoverHz}
              value={frequency}
              disabled={disabled}
              onChange={(next) =>
                onChange({
                  ...value,
                  crossovers_hz: withCrossover(value.crossovers_hz, index, next, RANGES.crossoverHz) as [
                    number,
                    number,
                    number,
                    number,
                  ],
                })
              }
            />
          );
        })}
      </div>
      <Bands
        bands={value.bands}
        crossovers={value.crossovers_hz}
        links={value.links}
        reductions={reductions}
        disabled={disabled}
        onBands={(bands) => onChange({ ...value, bands: bands as AgcSettings["five_band"]["bands"] })}
        onLinks={(links) => onChange({ ...value, links: links as AgcSettings["five_band"]["links"] })}
      />
    </BlockCard>
  );
}

function DualBandCard({
  value,
  disabled,
  onChange,
}: Readonly<{
  value: AgcSettings["dual_band"];
  disabled: boolean;
  onChange: (value: AgcSettings["dual_band"]) => void;
}>) {
  const reductions = useLive((live) => live.levels?.agc?.dual_band_db);
  return (
    <BlockCard
      block="dual_band"
      hint={t("agc.dualBandHint")}
      enabled={value.enabled}
      disabled={disabled}
      onToggle={(enabled) => onChange({ ...value, enabled })}
    >
      <div className="agc-controls">
        <Control
          label={t("agc.crossover", { bands: "1–2" })}
          range={RANGES.dualCrossoverHz}
          value={value.crossover_hz}
          disabled={disabled}
          onChange={(crossover_hz) => onChange({ ...value, crossover_hz })}
        />
      </div>
      <Bands
        bands={value.bands}
        crossovers={[value.crossover_hz]}
        links={[value.link]}
        reductions={reductions}
        disabled={disabled}
        onBands={(bands) => onChange({ ...value, bands: bands as AgcSettings["dual_band"]["bands"] })}
        onLinks={(links) => onChange({ ...value, link: links[0] ?? false })}
      />
    </BlockCard>
  );
}

/** What a band covers: below the first crossover, above the last, or between two. */
function bandCovers(from: number | null, to: number | null): string {
  if (from === null) {
    return t("agc.bandBelow", { frequency: formatHz(to ?? 0) });
  }
  if (to === null) {
    return t("agc.bandAbove", { frequency: formatHz(from) });
  }
  return `${formatHz(from)} – ${formatHz(to)}`;
}

/** The bands of a processor side by side, with the links between them. */
function Bands({
  bands,
  crossovers,
  links,
  reductions,
  disabled,
  onBands,
  onLinks,
}: Readonly<{
  bands: BandSettings[];
  crossovers: number[];
  links: boolean[];
  reductions: number[] | undefined;
  disabled: boolean;
  onBands: (bands: BandSettings[]) => void;
  onLinks: (links: boolean[]) => void;
}>) {
  const ranges = bandRanges(crossovers);
  const setBand = (index: number, band: BandSettings) => onBands(bands.map((b, i) => (i === index ? band : b)));
  return (
    <>
      <div className="agc-bands" style={{ ["--bands" as string]: bands.length }}>
        {bands.map((band, index) => {
          const [from, to] = ranges[index] ?? [null, null];
          const name = t("agc.band", { number: String(index + 1) });
          const covers = bandCovers(from, to);
          const set = (changes: Partial<BandSettings>) => setBand(index, { ...band, ...changes });
          return (
            // A processor's bands are fixed: its number names a band.
            <section className="agc-band" key={name} aria-label={name}>
              <header>
                <strong>{name}</strong>
                <span className="muted small">{covers}</span>
              </header>
              <ReductionMeter db={reductions?.[index]} label={name} />
              <select
                className="input"
                aria-label={t("agc.mode")}
                value={band.mode}
                disabled={disabled}
                onChange={(event) => set({ mode: event.target.value as BandMode })}
              >
                {MODES.map((mode) => (
                  <option key={mode.value} value={mode.value}>
                    {t(mode.label)}
                  </option>
                ))}
              </select>
              <Control
                label={t("agc.threshold")}
                range={RANGES.levelDb}
                value={band.threshold_db}
                disabled={disabled}
                onChange={(threshold_db) => set({ threshold_db })}
              />
              {band.mode !== "limiter" && (
                <Control
                  label={t("agc.ratio")}
                  range={RANGES.ratio}
                  value={band.ratio}
                  disabled={disabled}
                  onChange={(ratio) => set({ ratio })}
                />
              )}
              <Control
                label={t("agc.attack")}
                range={RANGES.attackMs}
                value={band.attack_ms}
                disabled={disabled}
                onChange={(attack_ms) => set({ attack_ms })}
              />
              <Control
                label={t("agc.release")}
                range={RANGES.releaseMs}
                value={band.release_ms}
                disabled={disabled}
                onChange={(release_ms) => set({ release_ms })}
              />
              <Control
                label={t("agc.hold")}
                range={RANGES.holdMs}
                value={band.hold_ms}
                reset={0}
                disabled={disabled}
                onChange={(hold_ms) => set({ hold_ms })}
              />
              <Control
                label={t("agc.bandGain")}
                range={RANGES.gainDb}
                value={band.gain_db}
                reset={0}
                disabled={disabled}
                onChange={(gain_db) => set({ gain_db })}
              />
              {index < links.length && (
                <label className="check agc-link" title={t("agc.linkHint")}>
                  <input
                    type="checkbox"
                    checked={links[index] ?? false}
                    disabled={disabled}
                    onChange={(event) => onLinks(links.map((l, i) => (i === index ? event.target.checked : l)))}
                  />
                  {t("agc.link", { bands: `${index + 1}–${index + 2}` })}
                </label>
              )}
            </section>
          );
        })}
      </div>
      <p className="muted small agc-note">{t("agc.modesHint")}</p>
    </>
  );
}

function FinalStageCard({
  value,
  disabled,
  onChange,
}: Readonly<{
  value: AgcSettings["final_stage"];
  disabled: boolean;
  onChange: (value: AgcSettings["final_stage"]) => void;
}>) {
  const reduction = useLive((live) => live.levels?.agc?.final_stage_db);
  return (
    <BlockCard
      block="final_stage"
      hint={t("agc.finalStageHint")}
      enabled={value.enabled}
      disabled={disabled}
      onToggle={(enabled) => onChange({ ...value, enabled })}
    >
      <div className="agc-controls">
        <Field label={t("agc.mode")} hint={value.mode === "limiter" ? t("agc.limiterHint") : t("agc.clipperHint")}>
          <Segmented
            label={t("agc.mode")}
            value={value.mode}
            disabled={disabled}
            options={[
              { value: "limiter", label: t("agc.limiter") },
              { value: "clipper", label: t("agc.clipper") },
            ]}
            onChange={(mode) => onChange({ ...value, mode })}
          />
        </Field>
        <Control
          label={t("agc.ceiling")}
          hint={t("agc.ceilingHint")}
          range={RANGES.ceilingDb}
          value={value.ceiling_db}
          disabled={disabled}
          onChange={(ceiling_db) => onChange({ ...value, ceiling_db })}
        />
        {value.mode === "limiter" && (
          <Control
            label={t("agc.release")}
            range={RANGES.finalReleaseMs}
            value={value.release_ms}
            disabled={disabled}
            onChange={(release_ms) => onChange({ ...value, release_ms })}
          />
        )}
        <div className="agc-control">
          <span>{t("agc.reduction")}</span>
          <ReductionMeter db={reduction} label={t("agc.finalStage")} />
        </div>
      </div>
    </BlockCard>
  );
}

function Presets({
  current,
  canEdit,
  onLoad,
}: Readonly<{ current: AgcSettings; canEdit: boolean; onLoad: (blocks: AgcPresetSettings) => void }>) {
  const presets = usePresets();
  const loaded = presetMatching(presets.data, current);
  const [naming, setNaming] = useState<{ preset: AgcPreset | null; settings: AgcPresetSettings } | null>(null);
  const save = useAction(
    ({ preset, name, settings }: { preset: AgcPreset | null; name: string; settings: AgcPresetSettings }) => {
      const body = { name, settings };
      return preset
        ? unwrap(api.PUT("/api/v1/agc-presets/{id}", { params: { path: { id: preset.id } }, body }))
        : unwrap(api.POST("/api/v1/agc-presets", { body }));
    },
    { success: t("agc.presetSaved"), invalidate: [AGC_PRESETS_KEY], onSuccess: () => setNaming(null) },
  );
  const overwrite = useAction(
    (preset: AgcPreset) =>
      unwrap(
        api.PUT("/api/v1/agc-presets/{id}", {
          params: { path: { id: preset.id } },
          body: { name: preset.name, settings: blocksOf(current) },
        }),
      ),
    { success: t("agc.presetSaved"), invalidate: [AGC_PRESETS_KEY] },
  );
  const remove = useAction(
    (preset: AgcPreset) => done(api.DELETE("/api/v1/agc-presets/{id}", { params: { path: { id: preset.id } } })),
    { success: t("agc.presetDeleted"), invalidate: [AGC_PRESETS_KEY] },
  );

  return (
    <Card
      title={t("agc.presets")}
      actions={
        canEdit && (
          <button
            type="button"
            className="btn small"
            onClick={() => setNaming({ preset: null, settings: blocksOf(current) })}
          >
            <Icon name="plus" /> {t("agc.saveChain")}
          </button>
        )
      }
      flush
    >
      {presets.isError && <ErrorView error={presets.error} />}
      {presets.isPending && <Loading />}
      <ul className="agc-presets">
        {presets.data?.map((preset) => (
          <li key={preset.id}>
            <span className="ellipsis">
              {preset.name}
              {loaded?.id === preset.id && <span className="badge accent">{t("agc.loaded")}</span>}
            </span>
            <span className="muted small ellipsis">
              {BLOCKS.filter((block) => preset.settings[block].enabled)
                .map((block) => t(BLOCK_NAMES[block]))
                .join(" · ")}
            </span>
            {canEdit && (
              <span className="row">
                <button
                  type="button"
                  className="btn small ghost"
                  disabled={loaded?.id === preset.id}
                  onClick={() => onLoad(preset.settings)}
                >
                  {t("agc.load")}
                </button>
                <button
                  type="button"
                  className="btn small ghost"
                  title={t("agc.overwrite")}
                  disabled={overwrite.isPending || loaded?.id === preset.id}
                  onClick={() => overwrite.mutate(preset)}
                >
                  {t("agc.overwriteShort")}
                </button>
                <button
                  type="button"
                  className="btn small ghost"
                  onClick={() => setNaming({ preset, settings: preset.settings })}
                >
                  {t("agc.rename")}
                </button>
                <ConfirmButton
                  className="btn small ghost danger icon"
                  title={t("agc.deletePreset")}
                  question={t("agc.deleteQuestion", { name: preset.name })}
                  confirm={t("agc.deletePreset")}
                  onConfirm={() => remove.mutate(preset)}
                >
                  <Icon name="trash" />
                </ConfirmButton>
              </span>
            )}
          </li>
        ))}
      </ul>
      {naming && (
        <NameModal
          initial={naming.preset?.name ?? ""}
          title={naming.preset ? t("agc.rename") : t("agc.newPreset")}
          busy={save.isPending}
          onClose={() => setNaming(null)}
          onSave={(name) => save.mutate({ preset: naming.preset, name, settings: naming.settings })}
        />
      )}
    </Card>
  );
}

function NameModal({
  initial,
  title,
  busy,
  onClose,
  onSave,
}: Readonly<{
  initial: string;
  title: string;
  busy: boolean;
  onClose: () => void;
  onSave: (name: string) => void;
}>) {
  const [name, setName] = useState(initial);
  const valid = name.trim().length > 0;
  return (
    <Modal
      title={title}
      open
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("ui.cancel")}
          </button>
          <button type="submit" form="agc-preset-name" className="btn primary" disabled={!valid || busy}>
            {t("common.save")}
          </button>
        </>
      }
    >
      <form
        id="agc-preset-name"
        onSubmit={(event) => {
          event.preventDefault();
          if (valid) {
            onSave(name.trim());
          }
        }}
      >
        <Field label={t("agc.presetName")}>
          <input className="input" value={name} maxLength={100} onChange={(event) => setName(event.target.value)} />
        </Field>
      </form>
    </Modal>
  );
}
