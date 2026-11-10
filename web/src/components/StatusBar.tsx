// SPDX-License-Identifier: AGPL-3.0-or-later

// The line at the bottom of every page (docs/web.md#status-bar): the decks
// (with the intro and the countdowns of the deck on air), the last track of
// a category type, the listeners, the next fixed time, the queue, the
// outputs, the machine's load and the station's clock, as each user chooses
// in Account › Status bar.

import { useQuery } from "@tanstack/react-query";
import { type CSSProperties, useEffect, useState } from "react";
import { api, unwrap } from "../api";
import { t } from "../i18n";
import { clockTime, duration, shortTime, trackName } from "../lib/format";
import { useListenersNow } from "../lib/listeners";
import { positionNow, useLive } from "../lib/live";
import { useModule } from "../lib/modules";
import { Link } from "../lib/router";
import { useUser } from "../lib/session";
import { here, useStation } from "../lib/station";
import {
  type Countdown,
  type DeckSummary,
  type StatusBarSettings,
  countdownText,
  listenersText,
  nextFixedTime,
  outputsOf,
  queueLength,
  recordOf,
  summarizeDeck,
  summarizeSystem,
  whenOf,
} from "../lib/statusbar";

/** The time now, in the clock `positionNow` uses, moved on once a second. */
function useNow() {
  const [now, setNow] = useState(() => performance.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(performance.now()), 1_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

/** The countdown's text and class: `intro −0:07`, urgent in its last seconds; `−0:09` before the next track. */
function countdownView(countdown: Countdown) {
  const time = countdownText(countdown);
  const urgency = countdown.blink ? "statusbar-countdown-blink" : "statusbar-countdown-alert";
  if (countdown.kind === "intro") {
    return {
      text: t("statusbar.intro", { time }),
      spoken: t("statusbar.introLeft", { count: countdown.seconds }),
      className: countdown.urgent ? urgency : "statusbar-countdown-intro",
    };
  }
  return {
    text: time,
    spoken: t("statusbar.nextIn", { count: countdown.seconds }),
    className: countdown.blink ? urgency : "statusbar-countdown-end",
  };
}

function Deck({ summary, colors }: Readonly<{ summary: DeckSummary; colors: CSSProperties }>) {
  const state = t(`statusbar.${summary.status}`);
  const badge = summary.badge && t(`statusbar.${summary.badge}`);
  const countdown = summary.countdown && countdownView(summary.countdown);
  const spoken = [
    t("statusbar.deck", { deck: summary.letter, state }),
    badge,
    summary.text && `${summary.text} ${summary.time}`,
    countdown?.spoken,
  ]
    .filter(Boolean)
    .join(", ");
  const percent = (fraction: number) => `${(fraction * 100).toFixed(2)}%`;
  return (
    <Link
      to="/"
      className={`statusbar-deck statusbar-deck-${summary.letter.toLowerCase()}${summary.onAir ? " statusbar-on-air" : ""}`}
      style={colors}
    >
      {/* What the symbols and the cut title say, for screen readers. */}
      <span className="visually-hidden">{spoken}</span>
      {/* Where the intro ends, how far the track has played and how much of the intro, behind the text. */}
      {summary.intro !== null && (
        <span className="statusbar-deck-intro" aria-hidden="true" style={{ width: percent(summary.intro) }} />
      )}
      {summary.progress !== null && (
        <span className="statusbar-deck-progress" aria-hidden="true" style={{ width: percent(summary.progress) }} />
      )}
      {summary.progress !== null && summary.intro !== null && summary.progress > 0 && (
        <span
          className="statusbar-deck-intro-progress"
          aria-hidden="true"
          style={{ width: percent(Math.min(summary.progress, summary.intro)) }}
        />
      )}
      <span className="statusbar-deck-letter" aria-hidden="true">
        {summary.letter}
      </span>
      <span className="statusbar-deck-symbol" aria-hidden="true">
        {summary.symbol}
      </span>
      {badge && (
        <span className={`statusbar-badge statusbar-badge-${summary.badge}`} aria-hidden="true">
          [{badge}]
        </span>
      )}
      {summary.text && (
        <span className="statusbar-deck-text" aria-hidden="true">
          {summary.text}
        </span>
      )}
      {countdown ? (
        <span className={`statusbar-deck-time ${countdown.className}`} aria-hidden="true">
          {countdown.text}
        </span>
      ) : (
        summary.time && (
          <span className="statusbar-deck-time statusbar-deck-elapsed" aria-hidden="true">
            {summary.time}
          </span>
        )
      )}
    </Link>
  );
}

/** The last track played to its end, of the category type chosen; it changes when the track on air does. */
function History({ typeId, onAir }: Readonly<{ typeId: number | null | undefined; onAir: number | null }>) {
  const station = useStation();
  const last = useQuery({
    queryKey: ["statusbar", "history", station, typeId ?? null, onAir],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/history", {
          params: {
            path: here(),
            query: { finished: true, limit: 1, ...(typeId == null ? {} : { category_type_id: typeId }) },
          },
        }),
      ),
    staleTime: 30_000,
  });
  const play = last.data?.[0];
  const text = (play && [play.artist, play.title].filter(Boolean).join(" – ")) || "–";
  return (
    <Link to="/history" className="statusbar-part statusbar-link statusbar-history" title={text}>
      <span>{t("statusbar.previous")}</span>
      <span className="statusbar-history-text">{text}</span>
    </Link>
  );
}

/** The next show to open (or open now), top or track at an exact time. */
function NextFixed({ now }: Readonly<{ now: number }>) {
  const station = useStation();
  const tops = useModule("top30");
  const player = useLive((live) => live.player);
  const shows = useQuery({
    queryKey: ["statusbar", "shows", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/shows", { params: { path: here() } })),
    staleTime: 60_000,
    refetchInterval: 60_000,
  });
  const topList = useQuery({
    queryKey: ["statusbar", "tops", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/tops", { params: { path: here() } })),
    enabled: tops,
    staleTime: 60_000,
    refetchInterval: 60_000,
  });
  const next = nextFixedTime(
    shows.data ?? [],
    tops ? (topList.data ?? []) : [],
    player?.queue ?? [],
    now,
    (show) => show.settings.website.title.trim() || t(`nav.${show.module === "match" ? "match" : "dedications"}`),
    (entry) => trackName(entry.track),
  );
  if (next === null) {
    return null;
  }
  const time = shortTime(next.at);
  let text: string;
  if (next.open) {
    text = t("statusbar.until", { name: next.name, time });
  } else {
    const when = whenOf(next.at, now);
    text =
      when.kind === "in"
        ? t("statusbar.inMinutes", { name: next.name, count: when.minutes })
        : when.kind === "at"
          ? t("statusbar.at", { name: next.name, time })
          : t("statusbar.onDay", { name: next.name, day: when.day, time });
  }
  return (
    <Link to={next.link} className="statusbar-part statusbar-link statusbar-next">
      {text}
    </Link>
  );
}

/**
 * The status bar. `settings` and `preview`: the one in Account › Status
 * bar, which shows settings not saved yet and stays in its place.
 */
export function StatusBar({ settings: draft, preview = false }: Readonly<{ settings?: StatusBarSettings; preview?: boolean }>) {
  const user = useUser();
  const settings = draft ?? user.status_bar;
  const module = useModule("status_bar");
  const shown = preview || (module && settings.show);
  const wallNow = useNow();
  const connected = useLive((live) => live.connected);
  const player = useLive((live) => live.player);
  const positions = useLive((live) => live.positions);
  const system = useLive((live) => live.system);
  const listeners = useListenersNow();
  const statistics = useModule("listener_statistics");

  // The page takes its room back when there is no bar.
  useEffect(() => {
    if (preview) {
      return;
    }
    document.documentElement.dataset.statusbar = shown ? "on" : "off";
    return () => {
      delete document.documentElement.dataset.statusbar;
    };
  }, [preview, shown]);
  if (!shown) {
    return null;
  }

  const now = Date.now();
  const paused = player?.paused ?? false;
  const onAir = player?.decks.find((deck) => deck.on_air && deck.entry);
  const deckSummary = (letter: "A" | "B") => {
    const name = letter.toLowerCase();
    const deck = player?.decks.find((d) => d.deck === name);
    const position = positionNow(positions, name, paused, wallNow);
    return summarizeDeck(letter, deck, paused, position?.position_ms ?? null, settings);
  };
  const letters: ("A" | "B")[] =
    settings.on_air_only && onAir ? [onAir.deck === "b" ? "B" : "A"] : ["A", "B"];
  const colors = {
    ...(settings.track_color ? { "--deck": settings.track_color } : {}),
    ...(settings.intro_color ? { "--statusbar-intro": settings.intro_color } : {}),
  } as CSSProperties;
  const summary = system ? summarizeSystem(system) : null;
  const record = recordOf(listeners.data?.record);
  const outputs = player ? outputsOf(player.outputs) : null;

  return (
    <div
      className={`statusbar${connected ? "" : " statusbar-offline"}${preview ? " statusbar-preview" : ""}`}
      role="status"
      aria-live="off"
      aria-label={t("statusbar.label")}
    >
      {settings.decks &&
        (player === null ? (
          <span className="statusbar-deck">{t("statusbar.engine")}</span>
        ) : (
          letters.map((letter) => <Deck key={letter} summary={deckSummary(letter)} colors={colors} />)
        ))}
      {settings.history && <History typeId={settings.history_type_id} onAir={onAir?.entry?.entry_id ?? null} />}
      {settings.listeners && statistics && (
        <Link to="/statistics" className="statusbar-part statusbar-link">
          <span>
            {t("statusbar.listeners")} {listenersText(listeners.data?.total)}
          </span>
          {record && (
            <span className="statusbar-record" title={t("statusbar.recordOn", { day: record.day })}>
              {t("statusbar.record")} {record.listeners}
            </span>
          )}
        </Link>
      )}
      {settings.next_fixed && <NextFixed now={now} />}
      {settings.queue && player && (
        <Link to="/queue" className="statusbar-part statusbar-link statusbar-queue">
          {t("statusbar.queue", { time: duration(queueLength(player.queue)) })}
        </Link>
      )}
      {settings.outputs && outputs && outputs.total > 0 && (
        <Link
          to="/outputs"
          className={`statusbar-part statusbar-link ${outputs.down.length ? "statusbar-bad" : "statusbar-good"}`}
          title={outputs.down.length ? t("statusbar.outputsDown", { names: outputs.down.join(", ") }) : undefined}
        >
          {t("statusbar.outputs", { connected: outputs.connected, total: outputs.total })}
        </Link>
      )}
      {settings.system && summary && (
        <span className="statusbar-part statusbar-system">
          <span className={`statusbar-${summary.cpuLevel}`}>cpu {summary.cpu}</span>
          <span className={`statusbar-${summary.memLevel}`}>mem {summary.mem}</span>
          <span className={`statusbar-${summary.loadLevel}`}>load {summary.load}</span>
        </span>
      )}
      {settings.station_time && <span className="statusbar-part statusbar-clock">{clockTime(now)}</span>}
      {!connected && <span className="statusbar-part">{t("statusbar.offline")}</span>}
    </div>
  );
}
