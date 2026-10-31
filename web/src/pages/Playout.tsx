// SPDX-License-Identifier: AGPL-3.0-or-later

// Crossfade and loudness settings, with a live preview of a transition.

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";
import { type PlayoutSettings, type TrackSummary, api, errorMessage, unwrap } from "../api";
import { FadeEditor } from "../components/FadeEditor";
import { TrackSearch, useDebounced } from "../components/TrackSearch";
import { TransitionGraph } from "../components/TransitionGraph";
import { useWaveform } from "../components/Waveform";
import {
  Card,
  Empty,
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
import { t } from "../i18n";
import { db, trackName } from "../lib/format";
import { useLive } from "../lib/live";
import { useAction, useCan } from "../lib/session";
import { useTransitionPlayer } from "../lib/transitionPlayer";
import { here } from "../lib/station";

export function Playout() {
  const settings = useQuery({
    queryKey: ["settings", "playout"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/playout", { params: { path: here() } })),
  });
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return <PlayoutForm saved={settings.data} />;
}

interface Pick {
  id: number;
  name: string;
}

function PlayoutForm({ saved }: { saved: PlayoutSettings }) {
  const isAdmin = useCan("admin");
  const [draft, setDraft] = useState(saved);
  useEffect(() => setDraft(saved), [saved]);
  const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
  const save = useAction(
    (settings: PlayoutSettings) => unwrap(api.PUT("/api/v1/stations/{station_id}/settings/playout", { params: { path: here() }, body: settings })),
    { success: t("playout.saved"), invalidate: [["settings", "playout"]] },
  );
  const crossfade = draft.crossfade;
  const loudness = draft.loudness;
  const setCrossfade = (changes: Partial<PlayoutSettings["crossfade"]>) =>
    setDraft((d) => ({ ...d, crossfade: { ...d.crossfade, ...changes } }));
  const setLoudness = (changes: Partial<PlayoutSettings["loudness"]>) =>
    setDraft((d) => ({ ...d, loudness: { ...d.loudness, ...changes } }));
  const watchdog = draft.silence_watchdog;
  const setWatchdog = (changes: Partial<PlayoutSettings["silence_watchdog"]>) =>
    setDraft((d) => ({ ...d, silence_watchdog: { ...d.silence_watchdog, ...changes } }));
  const disabled = !isAdmin;

  return (
    <>
      <PageHead
        title={t("nav.playout")}
        description={t("playout.description")}
        actions={
          isAdmin && (
            <>
              {dirty && (
                <button type="button" className="btn" onClick={() => setDraft(saved)}>
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
      <Preview settings={draft} />
      <div className="grid two">
        <Card title={t("track.crossfade")}>
          <div className="stack">
            <Field label={t("playout.nextStarts")}>
              <Segmented
                label={t("playout.nextStarts")}
                value={crossfade.mode}
                disabled={disabled}
                onChange={(mode) => setCrossfade({ mode })}
                options={[
                  { value: "auto", label: t("playout.auto") },
                  { value: "fixed", label: t("playout.fixed") },
                ]}
              />
            </Field>
            <p className="muted small" style={{ margin: 0 }}>
              {crossfade.mode === "auto"
                ? t("playout.autoHelp")
                : t("playout.fixedHelp")}
            </p>
            <div className="form-grid">
              {crossfade.mode === "fixed" && (
                <Field label={t("track.beforeCueOut")}>
                  <NumberInput
                    value={crossfade.fixed_ms / 1000}
                    min={0}
                    max={30}
                    step={0.1}
                    suffix="s"
                    disabled={disabled}
                    onChange={(s) => setCrossfade({ fixed_ms: Math.round(s * 1000) })}
                  />
                </Field>
              )}
              <Field label={t("playout.minTrack")} hint={t("playout.minTrackHint")}>
                <NumberInput
                  value={crossfade.min_track_ms / 1000}
                  min={0}
                  max={600}
                  step={1}
                  suffix="s"
                  disabled={disabled}
                  onChange={(s) => setCrossfade({ min_track_ms: Math.round(s * 1000) })}
                />
              </Field>
              <Field label={t("playout.skipFade")}>
                <NumberInput
                  value={crossfade.skip_fade_ms / 1000}
                  min={0}
                  max={10}
                  step={0.1}
                  suffix="s"
                  disabled={disabled}
                  onChange={(s) => setCrossfade({ skip_fade_ms: Math.round(s * 1000) })}
                />
              </Field>
            </div>
            <fieldset>
              <legend>{t("track.fadeOut")}</legend>
              <FadeEditor
                kind="out"
                value={crossfade.fade_out}
                disabled={disabled}
                onChange={(fade_out) => setCrossfade({ fade_out })}
              />
            </fieldset>
            <fieldset>
              <legend>{t("track.fadeIn")}</legend>
              <FadeEditor
                kind="in"
                value={crossfade.fade_in}
                disabled={disabled}
                onChange={(fade_in) => setCrossfade({ fade_in })}
              />
            </fieldset>
          </div>
        </Card>
        <Card title={t("track.loudness")}>
          <div className="stack">
            <label className="check">
              <input
                type="checkbox"
                checked={loudness.enabled}
                disabled={disabled}
                onChange={(event) => setLoudness({ enabled: event.target.checked })}
              />
              {t("playout.normalize")}
            </label>
            <div className="form-grid">
              <Field label={t("playout.target")} hint={t("playout.targetHint")}>
                <NumberInput
                  value={loudness.target_lufs}
                  min={-40}
                  max={0}
                  step={0.5}
                  suffix="LUFS"
                  disabled={disabled || !loudness.enabled}
                  onChange={(target_lufs) => setLoudness({ target_lufs })}
                />
              </Field>
              <Field label={t("playout.maxGain")} hint={t("playout.maxGainHint")}>
                <NumberInput
                  value={loudness.max_gain_db}
                  min={0}
                  max={30}
                  step={0.5}
                  suffix="dB"
                  disabled={disabled || !loudness.enabled}
                  onChange={(max_gain_db) => setLoudness({ max_gain_db })}
                />
              </Field>
              <Field label={t("playout.ceiling")} hint={t("playout.ceilingHint")}>
                <NumberInput
                  value={loudness.peak_ceiling_dbtp}
                  min={-12}
                  max={0}
                  step={0.1}
                  suffix="dBTP"
                  disabled={disabled || !loudness.enabled}
                  onChange={(peak_ceiling_dbtp) => setLoudness({ peak_ceiling_dbtp })}
                />
              </Field>
            </div>
            <p className="muted small" style={{ margin: 0 }}>
              {t("playout.noLimiter")}
            </p>
          </div>
        </Card>
        <Card title="Silence watchdog">
          <div className="stack">
            <label className="check">
              <input
                type="checkbox"
                checked={watchdog.enabled}
                disabled={disabled}
                onChange={(event) => setWatchdog({ enabled: event.target.checked })}
              />
              Skip tracks that stay silent
            </label>
            <div className="form-grid">
              <Field label="After" hint="Silence is below -50 dBFS">
                <NumberInput
                  value={watchdog.timeout_ms / 1000}
                  min={16}
                  max={300}
                  step={1}
                  suffix="s"
                  disabled={disabled || !watchdog.enabled}
                  onChange={(s) => setWatchdog({ timeout_ms: Math.round(s * 1000) })}
                />
              </Field>
            </div>
            <p className="muted small" style={{ margin: 0 }}>
              The engine skips to the next track, then to the fallback directory if the
              next track is silent too. Keep it above 15 s: the analysis keeps silences up
              to that long inside tracks.
            </p>
          </div>
        </Card>
      </div>
    </>
  );
}

/** The transition between two tracks with the settings being edited. */
function Preview({ settings }: { settings: PlayoutSettings }) {
  const player = useLive((live) => live.player);
  const [outgoing, setOutgoing] = useState<Pick | null>(null);
  const [incoming, setIncoming] = useState<Pick | null>(null);
  const [picking, setPicking] = useState<"outgoing" | "incoming" | null>(null);

  // By default, what is on air and what comes next; else two tracks.
  const fallback = useQuery({
    queryKey: ["tracks", "preview-pair"],
    queryFn: () =>
      unwrap(api.GET("/api/v1/tracks", { params: { query: { filter: "playable", sort: "added", limit: 2 } } })),
    enabled: outgoing === null || incoming === null,
  });
  useEffect(() => {
    if (outgoing && incoming) {
      return;
    }
    const onAir = player?.decks.find((deck) => deck.on_air && deck.deck !== "fx")?.entry?.track;
    const next = player?.cued?.track ?? player?.queue[0]?.track;
    const pick = (track: { track_id?: number | null } & Parameters<typeof trackName>[0]) =>
      track.track_id ? { id: track.track_id, name: trackName(track) } : null;
    const live = onAir && next ? [pick(onAir), pick(next)] : [null, null];
    const [a, b] = fallback.data?.items ?? [];
    setOutgoing((current) => current ?? live[0] ?? (a ? { id: a.id, name: trackName(a) } : null));
    setIncoming((current) => current ?? live[1] ?? (b ? { id: b.id, name: trackName(b) } : null));
  }, [player, fallback.data, outgoing, incoming]);

  const debounced = useDebounced(settings, 250);
  const preview = useQuery({
    queryKey: ["transition", outgoing?.id, incoming?.id, debounced],
    enabled: outgoing !== null && incoming !== null,
    queryFn: () =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/playout/transition", { params: { path: here() },
          body: {
            from_track_id: outgoing?.id ?? 0,
            to_track_id: incoming?.id ?? 0,
            settings: debounced,
          },
        }),
      ),
    placeholderData: keepPreviousData,
  });
  const outgoingWave = useWaveform(outgoing?.id);
  const incomingWave = useWaveform(incoming?.id);

  const toast = useToast();
  const failed = useCallback((error: unknown) => toast(errorMessage(error), true), [toast]);
  const listening = useTransitionPlayer(failed);
  const { stop } = listening;
  // A change of tracks or settings makes another transition: stop this one.
  useEffect(() => stop(), [stop, preview.data]);
  const active = listening.loading || listening.position !== null;

  const choose = (track: TrackSummary) => {
    const pick = { id: track.id, name: trackName(track) };
    if (picking === "outgoing") {
      setOutgoing(pick);
    } else {
      setIncoming(pick);
    }
    setPicking(null);
  };

  return (
    <Card
      title={t("playout.preview")}
      actions={
        <span className="muted small">
          {t("playout.previewHelp")}
        </span>
      }
    >
      <div className="stack">
        <div className="row">
          <button type="button" className="btn small" onClick={() => setPicking("outgoing")}>
            <span className="swatch" style={{ background: "var(--deck-a)" }} />
            <span className="ellipsis" style={{ maxWidth: "18rem" }}>
              {outgoing?.name ?? t("playout.pickOutgoing")}
            </span>
          </button>
          <span className="muted">{t("playout.into")}</span>
          <button type="button" className="btn small" onClick={() => setPicking("incoming")}>
            <span className="swatch" style={{ background: "var(--deck-b)" }} />
            <span className="ellipsis" style={{ maxWidth: "18rem" }}>
              {incoming?.name ?? t("playout.pickIncoming")}
            </span>
          </button>
          <button
            type="button"
            className="btn small"
            disabled={!preview.data || !outgoing || !incoming}
            onClick={() => {
              if (active) {
                listening.stop();
              } else if (preview.data && outgoing && incoming) {
                listening.play(outgoing.id, incoming.id, preview.data);
              }
            }}
          >
            <Icon name={active ? "stop" : "play"} size={14} />{" "}
            {listening.loading
              ? t("playout.loadingTransition")
              : active
                ? t("playout.stopTransition")
                : t("playout.listen")}
          </button>
          {preview.data && (
            <span className="muted small">
              {t("playout.gains", { outgoing: db(preview.data.outgoing.gain_db), incoming: db(preview.data.incoming.gain_db) })}
            </span>
          )}
        </div>
        {preview.isError && <ErrorView error={preview.error} />}
        {!outgoing || !incoming ? (
          <Empty>{t("playout.pickTwo")}</Empty>
        ) : preview.data ? (
          <TransitionGraph
            preview={preview.data}
            outgoingWave={outgoingWave.data ?? null}
            incomingWave={incomingWave.data ?? null}
            playhead={listening.position}
          />
        ) : (
          <Loading />
        )}
      </div>
      <Modal
        title={picking === "outgoing" ? t("playout.outgoingTrack") : t("playout.incomingTrack")}
        open={picking !== null}
        onClose={() => setPicking(null)}
      >
        <TrackSearch
          actions={(track) => (
            <button type="button" className="btn small primary" onClick={() => choose(track)}>
              {t("restrictions.pick")}
            </button>
          )}
        />
      </Modal>
    </Card>
  );
}
