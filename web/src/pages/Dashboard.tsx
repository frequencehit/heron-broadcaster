// SPDX-License-Identifier: AGPL-3.0-or-later

// On air: the decks, levels, transport, FX player, what comes next and the
// outputs.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { type DeckState, type OutputStatus, type QueueEntry, api, done, unwrap } from "../api";
import { StereoMeter, LineMeter } from "../components/Meter";
import { QueueList } from "../components/QueueList";
import { TrackSearch } from "../components/TrackSearch";
import { Card, ConfirmButton, Empty, Icon, Modal, PageHead } from "../components/ui";
import { CategoryChips, type CategoryRef, useTrackCategories } from "../components/TrackCategories";
import { type PlainKey, number, rich, t } from "../i18n";
import { ago, bytes, clockTime, duration, trackName } from "../lib/format";
import { sourceLabel } from "../lib/labels";
import { isShaping, useEqSettings } from "../lib/eq";
import { useListenersNow } from "../lib/listeners";
import { positionNow, useLive } from "../lib/live";
import { useStationCategories } from "./Clocks";
import { Link } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { introFromCueIn, jingleStart } from "../lib/intro";
import { here } from "../lib/station";

/** Re-renders `fps` times a second while mounted. */
export function useTicker(fps = 10): number {
  const [now, setNow] = useState(() => performance.now());
  useEffect(() => {
    let frame = 0;
    let last = 0;
    const step = (time: number) => {
      if (time - last >= 1000 / fps) {
        last = time;
        setNow(time);
      }
      frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [fps]);
  return now;
}

const DECK_NAMES: Record<string, string> = { a: "A", b: "B", fx: "FX" };
const DECK_COLORS: Record<string, string> = {
  a: "var(--deck-a)",
  b: "var(--deck-b)",
  fx: "var(--fx)",
};

export function Dashboard() {
  const player = useLive((live) => live.player);
  const levels = useLive((live) => live.levels);
  const canOperate = useCan("operator");
  const [fxOpen, setFxOpen] = useState(false);

  const skip = useAction(() => done(api.POST("/api/v1/stations/{station_id}/player/skip", { params: { path: here() } })), { success: t("dashboard.skipped") });
  const pause = useAction(() => done(api.POST("/api/v1/stations/{station_id}/player/pause", { params: { path: here() } })));
  const resume = useAction(() => done(api.POST("/api/v1/stations/{station_id}/player/resume", { params: { path: here() } })));
  const stopFx = useAction(() => done(api.DELETE("/api/v1/stations/{station_id}/player/fx", { params: { path: here() } })));
  const playFx = useAction(
    (trackId: number) => unwrap(api.POST("/api/v1/stations/{station_id}/player/fx", { params: { path: here() }, body: { track_id: trackId } })),
    { success: t("dashboard.fxPlaying"), onSuccess: () => setFxOpen(false) },
  );

  const decks = player?.decks.filter((deck) => deck.deck !== "fx") ?? [];
  const fx = player?.decks.find((deck) => deck.deck === "fx");
  const fxPlaying = Boolean(fx?.entry);
  const onAirDeck = decks.find((deck) => deck.on_air && deck.entry);
  const deckCategories = useTrackCategories([
    ...decks.map((deck) => deck.entry?.track.track_id),
    player?.cued?.track.track_id,
  ]);

  return (
    <>
      <PageHead
        title={t("nav.onAir")}
        actions={
          canOperate &&
          player && (
            <>
              {player.paused ? (
                <button
                  type="button"
                  className="btn primary big"
                  onClick={() => resume.mutate(undefined)}
                >
                  <Icon name="play" /> {t("dashboard.resume")}
                </button>
              ) : (
                <ConfirmButton
                  className="btn big"
                  confirm={t("dashboard.pause")}
                  question={t("dashboard.pauseQuestion")}
                  onConfirm={() => pause.mutate(undefined)}
                >
                  <Icon name="pause" /> {t("dashboard.pause")}
                </ConfirmButton>
              )}
              <button
                type="button"
                className="btn primary big"
                disabled={skip.isPending || !decks.some((deck) => deck.on_air)}
                onClick={() => skip.mutate(undefined)}
              >
                <Icon name="skip" /> {t("dashboard.skip")}
              </button>
            </>
          )
        }
      />

      {player?.paused && (
        <div className="banner warn" role="status">
          <Icon name="pause" /> {t("dashboard.paused")}
        </div>
      )}

      <div className="grid sidebar-right">
        <div className="stack">
          <div className="decks">
            {player === undefined && <Card><Empty>{t("queue.connecting")}</Empty></Card>}
            {decks.map((deck) => (
              <Deck
                key={deck.deck}
                deck={deck}
                paused={player?.paused ?? false}
                cued={!deck.entry && !deck.on_air ? (player?.cued ?? null) : null}
                pendingJingle={deck.on_air ? (player?.pending_jingle ?? null) : null}
                categories={deckCategories.get(
                  (deck.entry?.track ?? (!deck.on_air ? player?.cued?.track : undefined))?.track_id ?? -1,
                )}
              />
            ))}
          </div>

          <Card
            title={t("dashboard.upNext")}
            actions={<Link to="/queue" className="btn small">{t("dashboard.editQueue")}</Link>}
            flush
          >
            <QueueList limit={6} />
          </Card>
        </div>

        <div className="stack">
          <Card title={t("dashboard.levels")}>
            <div className="row" style={{ alignItems: "flex-start", gap: "1.5rem" }}>
              <StereoMeter levels={levels?.master ?? null} label={t("dashboard.master")} />
              <div className="stack" style={{ flex: 1, minWidth: "8rem" }}>
                {(["deck_a", "deck_b", "fx"] as const).map((key) => (
                  <div key={key} className="stack" style={{ gap: "0.2rem" }}>
                    <span className="small muted">
                      {key === "fx" ? t("dashboard.fxPlayer") : t("dashboard.deck", { deck: key.slice(-1).toUpperCase() })}
                    </span>
                    <LineMeter
                      levels={levels?.[key] ?? null}
                      label={key === "fx" ? t("dashboard.fxPlayer") : t("dashboard.deck", { deck: key.slice(-1).toUpperCase() })}
                    />
                  </div>
                ))}
              </div>
            </div>
          </Card>

          <Card
            title={t("dashboard.fxPlayer")}
            actions={
              canOperate && (
                <>
                  {fxPlaying && (
                    <button type="button" className="btn small" onClick={() => stopFx.mutate(undefined)}>
                      <Icon name="stop" size={14} /> {t("dashboard.stop")}
                    </button>
                  )}
                  <button type="button" className="btn small" onClick={() => setFxOpen(true)}>
                    <Icon name="bolt" size={14} /> {t("dashboard.playOver")}
                  </button>
                </>
              )
            }
          >
            {fx?.entry ? (
              <FxState deck={fx} />
            ) : player?.pending_jingle && onAirDeck ? (
              <FxCountdown jingle={player.pending_jingle} deck={onAirDeck} paused={player.paused} />
            ) : (
              <p className="muted small" style={{ margin: 0 }}>
                {t("dashboard.fxHelp")}
              </p>
            )}
          </Card>

          <OutputsCard outputs={player?.outputs ?? []} />
          <EngineCard />
        </div>
      </div>

      <Modal title={t("dashboard.fxTitle")} open={fxOpen} onClose={() => setFxOpen(false)}>
        <FxPicker play={(trackId) => playFx.mutate(trackId)} playing={playFx.isPending} />
      </Modal>
    </>
  );
}

const FX_TAB_KEY = "heron.fx.category";

function storedFxTab(): number | null {
  try {
    const stored = Number(localStorage.getItem(FX_TAB_KEY));
    return stored > 0 ? stored : null;
  } catch {
    // Blocked storage: the first category.
    return null;
  }
}

/**
 * Tracks to play over the title: the categories for the FX player (liners,
 * power intros, voice tracks) first, the last one used open, then the
 * whole library.
 */
function FxPicker({ play, playing }: { play: (trackId: number) => void; playing: boolean }) {
  // The FX categories of the station shown.
  const categories = useStationCategories();
  const [chosen, setChosen] = useState<number | "library" | null>(storedFxTab);
  const fxCategories = categories.data?.filter((category) => category.fx_player) ?? [];
  const tab =
    chosen === "library" || fxCategories.some((category) => category.id === chosen)
      ? chosen
      : (fxCategories[0]?.id ?? "library");
  const choose = (value: number | "library") => {
    setChosen(value);
    try {
      localStorage.setItem(FX_TAB_KEY, value === "library" ? "" : String(value));
    } catch {
      // Not remembered in this browser.
    }
  };
  const current = fxCategories.find((category) => category.id === tab);

  if (categories.isPending) {
    return null;
  }
  return (
    <div className="stack">
      {fxCategories.length > 0 ? (
        <div className="tabs" role="tablist" aria-label={t("dashboard.fxCategories")}>
          {fxCategories.map((category) => (
            <button
              key={category.id}
              type="button"
              role="tab"
              aria-selected={tab === category.id}
              onClick={() => choose(category.id)}
            >
              <span className="row" style={{ flexWrap: "nowrap" }}>
                <span className="swatch" style={{ background: category.color ?? undefined }} />
                {category.name}
              </span>
            </button>
          ))}
          <button type="button" role="tab" aria-selected={tab === "library"} onClick={() => choose("library")}>
            {t("dashboard.fxLibrary")}
          </button>
        </div>
      ) : (
        <p className="muted small" style={{ margin: 0 }}>
          {rich("dashboard.fxNoCategory", { link: <Link to="/categories">{t("dashboard.fxCategoriesLink")}</Link> })}
        </p>
      )}
      <TrackSearch
        categoryId={current?.id}
        limit={current ? 50 : 8}
        placeholder={current ? t("dashboard.fxSearchIn", { name: current.name }) : t("dashboard.fxSearch")}
        actions={(track) => (
          <button type="button" className="btn small primary" disabled={playing} onClick={() => play(track.id)}>
            <Icon name="play" size={14} /> {t("dashboard.play")}
          </button>
        )}
      />
    </div>
  );
}

function Deck({
  deck,
  paused,
  cued,
  pendingJingle,
  categories,
}: {
  deck: DeckState;
  paused: boolean;
  /** The next track, shown on the idle deck. */
  cued: QueueEntry | null;
  /** The jingle waiting to start over this deck's intro. */
  pendingJingle: QueueEntry | null;
  /** The track's categories. */
  categories: CategoryRef[] | undefined;
}) {
  const positions = useLive((live) => live.positions);
  const now = useTicker(deck.on_air ? 10 : 2);
  const live = positionNow(positions, deck.deck, paused, now);
  const position = live?.position_ms ?? deck.position_ms;
  const track = deck.entry?.track ?? cued?.track;
  const cueOut = track?.cue_out_ms ?? deck.length_ms;
  const remaining = Math.max(0, cueOut - position);
  const progress = cueOut > 0 ? Math.min(1, position / cueOut) : 0;
  const intro = track ? introFromCueIn(track) : null;
  const jingleAt = track && pendingJingle ? jingleStart(track, pendingJingle.track) : null;
  const at = (ms: number) => `${Math.min(100, (ms / Math.max(1, cueOut)) * 100)}%`;
  const eq = useEqSettings().data;
  const shaping = isShaping(deck.deck === "fx" ? eq?.fx : eq?.music);

  return (
    <section
      className={deck.on_air ? "card deck on-air" : "card deck"}
      style={{ ["--deck-color" as string]: DECK_COLORS[deck.deck] }}
      aria-label={t("dashboard.deck", { deck: DECK_NAMES[deck.deck] ?? deck.deck })}
    >
      <div className="card-body stack">
        <div className="row between" style={{ flexWrap: "nowrap" }}>
          <div className="row" style={{ flexWrap: "nowrap", minWidth: 0 }}>
            <span className="deck-label">{DECK_NAMES[deck.deck]}</span>
            {deck.on_air ? (
              <span className="on-air-pill">{t("app.onAirBadge")}</span>
            ) : track ? (
              <span className="badge accent">{t("dashboard.next")}</span>
            ) : (
              <span className="badge">{t("dashboard.empty")}</span>
            )}
          </div>
          <div className="row" style={{ flexWrap: "nowrap" }}>
            {shaping && (
              <Link to="/eq" className="badge accent badge-link" title={t("dashboard.eqOn")}>
                {t("dashboard.eq")}
              </Link>
            )}
            {track && <span className="badge">{sourceLabel(track.source)}</span>}
          </div>
        </div>
        {track ? (
          <>
            <div style={{ minWidth: 0 }}>
              <div className="deck-title ellipsis">
                {track.track_id ? (
                  <Link to={`/library/${track.track_id}`}>{track.title ?? trackName(track)}</Link>
                ) : (
                  (track.title ?? trackName(track))
                )}
              </div>
              <div className="muted ellipsis">
                {[track.artist, track.album].filter(Boolean).join(" · ") || track.path}
              </div>
              <CategoryChips categories={categories} />
            </div>
            {cued ? (
              <div className="deck-times">
                <span className="muted">{t("dashboard.startsAtCueIn")}</span>
                <span className="remaining">{duration(cueOut - (track.cue_in_ms ?? 0))}</span>
              </div>
            ) : (
              <>
                <div
                  className="progress"
                  role="progressbar"
                  aria-label={t("dashboard.position")}
                  aria-valuemin={0}
                  aria-valuemax={Math.round(cueOut / 1000)}
                  aria-valuenow={Math.round(position / 1000)}
                >
                  <div style={{ width: `${progress * 100}%` }} />
                  {jingleAt !== null && (
                    <span className="progress-mark jingle" style={{ left: at(jingleAt) }} title={t("dashboard.jingleStartsHere")} />
                  )}
                  {intro !== null && (
                    <span className="progress-mark intro" style={{ left: at(intro) }} title={t("dashboard.vocalsStartHere")} />
                  )}
                </div>
                <div className="deck-times">
                  <span className="muted">{duration(position)}</span>
                  {intro !== null && position < intro && (
                    <span className={intro - position <= 5_000 ? "intro-countdown soon" : "intro-countdown"}>
                      {t("dashboard.vocalsIn", { time: duration(intro - position, true) })}
                    </span>
                  )}
                  <span className="remaining">-{duration(remaining)}</span>
                </div>
              </>
            )}
          </>
        ) : (
          <p className="muted" style={{ margin: 0 }}>
            {t("dashboard.nothingLoaded")}
          </p>
        )}
      </div>
    </section>
  );
}

/** The jingle waiting for the intro of the track on air, and when it starts. */
function FxCountdown({ jingle, deck, paused }: { jingle: QueueEntry; deck: DeckState; paused: boolean }) {
  const positions = useLive((live) => live.positions);
  const now = useTicker(10);
  const song = deck.entry?.track;
  const start = song ? jingleStart(song, jingle.track) : null;
  const position = positionNow(positions, deck.deck, paused, now)?.position_ms ?? deck.position_ms;
  const left = start !== null ? Math.max(0, start - position) : null;
  return (
    <div className="row between">
      <span className="ellipsis">
        <span className="badge accent">{t("dashboard.fxNext")}</span> {trackName(jingle.track)}
      </span>
      {left !== null && (
        <span className={left <= 5_000 ? "intro-countdown soon" : "intro-countdown"}>
          {t("dashboard.fxIn", { time: duration(left, true) })}
        </span>
      )}
    </div>
  );
}

function FxState({ deck }: { deck: DeckState }) {
  const positions = useLive((live) => live.positions);
  const now = useTicker(5);
  const live = positionNow(positions, "fx", false, now);
  const track = deck.entry?.track;
  if (!track) {
    return null;
  }
  const remaining = Math.max(0, track.cue_out_ms - (live?.position_ms ?? deck.position_ms));
  return (
    <div className="row between">
      <span className="ellipsis">
        <span className="dot live" /> {trackName(track)}
      </span>
      <span className="num">-{duration(remaining)}</span>
    </div>
  );
}

const OUTPUT_STATES: Record<string, { label: PlainKey; dot: string }> = {
  connected: { label: "output.connected", dot: "good" },
  connecting: { label: "output.connecting", dot: "warn" },
  reconnecting: { label: "output.reconnecting", dot: "bad" },
  stopped: { label: "output.stopped", dot: "" },
};

export function OutputState({ status }: { status: OutputStatus | null | undefined }) {
  if (!status) {
    return <span className="badge">{t("output.notRunning")}</span>;
  }
  const known = OUTPUT_STATES[status.state];
  const state = known ? { label: t(known.label), dot: known.dot } : { label: status.state, dot: "" };
  return (
    <span className="row small" style={{ gap: "0.35rem", flexWrap: "nowrap" }}>
      <span className={`dot ${state.dot}`} /> {status.enabled ? state.label : t("output.disabled")}
    </span>
  );
}

/** Opens the stream as listeners hear it. */
export function ListenLink({ url }: { url: string }) {
  return (
    <a className="btn small" href={url} target="_blank" rel="noreferrer" title={url}>
      <Icon name="play" /> {t("output.listen")}
    </a>
  );
}

function OutputsCard({ outputs }: { outputs: OutputStatus[] }) {
  // The live state has no settings: the listen links come from the outputs.
  const configured = useQuery({ queryKey: ["outputs"], queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/outputs", { params: { path: here() } })) });
  const listenUrl = (id: number) => configured.data?.find((output) => output.id === id)?.listen_url;
  const listeners = useListenersNow();
  const listenersOf = (id: number) => listeners.data?.outputs.find((o) => o.output_id === id)?.listeners;
  const total = listeners.data?.total;
  return (
    <Card
      title={
        <h2>
          {t("nav.outputs")}
          {total !== undefined && total !== null && (
            <Link to="/statistics" className="muted small" style={{ fontWeight: 400, marginLeft: "0.5rem" }}>
              {t("dashboard.listeners", { count: total })}
            </Link>
          )}
        </h2>
      }
      actions={<Link to="/outputs" className="btn small">{t("dashboard.configure")}</Link>}
      flush
    >
      {outputs.length === 0 ? (
        <Empty>
          {rich("dashboard.noOutput", { link: <Link to="/outputs">{t("dashboard.addOutput")}</Link> })}
        </Empty>
      ) : (
        <ul className="list">
          {outputs.map((output) => (
            <li key={output.id} className="queue-item" style={{ alignItems: "flex-start" }}>
              <div className="meta">
                <div className="row between" style={{ flexWrap: "nowrap" }}>
                  <span className="ellipsis">{output.name}</span>
                  <span className="row" style={{ flexWrap: "nowrap" }}>
                    {listenUrl(output.id) && <ListenLink url={listenUrl(output.id)!} />}
                    <OutputState status={output} />
                  </span>
                </div>
                <div className="muted small">
                  {output.state === "connected" && output.connected_since_unix_ms
                    ? t("dashboard.outputSince", { time: clockTime(output.connected_since_unix_ms), sent: bytes(output.bytes_sent) })
                    : output.error
                      ? <span className="bad">{output.error}</span>
                      : null}
                  {listenersOf(output.id) !== undefined && listenersOf(output.id) !== null && (
                    <span> · {t("dashboard.listeners", { count: listenersOf(output.id) ?? 0 })}</span>
                  )}
                  {output.dropped_chunks > 0 && (
                    <span className="warn"> · {t("dashboard.dropped", { count: output.dropped_chunks })}</span>
                  )}
                </div>
              </div>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

function EngineCard() {
  const player = useLive((live) => live.player);
  const engine = useQuery({
    queryKey: ["engine"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/engine", { params: { path: here() } })),
    refetchInterval: 15_000,
  });
  const sync = engine.data?.config.state;
  return (
    <Card title={t("dashboard.engine")}>
      <dl className="facts small">
        <dt>{t("dashboard.runningSince")}</dt>
        <dd>{player ? ago(player.engine_started_at_unix_ms) : "–"}</dd>
        <dt>{t("dashboard.configuration")}</dt>
        <dd className={sync === "failed" ? "bad" : sync === "synced" ? "good" : "muted"}>
          {sync === "synced"
            ? t("dashboard.inSync")
            : sync === "pending"
              ? t("dashboard.waitingEngine")
              : sync === "failed"
                ? t("dashboard.refused")
                : t("dashboard.unknown")}
        </dd>
        <dt>{t("dashboard.underruns")}</dt>
        <dd className={player && player.underruns > 0 ? "warn num" : "num"}>
          {player ? number(player.underruns) : "–"}
        </dd>
      </dl>
    </Card>
  );
}
