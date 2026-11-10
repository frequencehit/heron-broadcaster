// SPDX-License-Identifier: AGPL-3.0-or-later

// The line at the bottom of every page: both decks (with the intro and
// the countdowns of the deck on air), the station's listeners and, for
// operators and admins, the machine's load (docs/web.md#status-bar).

import { useModule } from "../lib/modules";
import { useEffect, useState } from "react";
import { t } from "../i18n";
import { useListenersNow } from "../lib/listeners";
import { positionNow, useLive } from "../lib/live";
import { Link } from "../lib/router";
import {
  type Countdown,
  type DeckSummary,
  countdownText,
  listenersText,
  recordOf,
  summarizeDeck,
  summarizeSystem,
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

/** The countdown's text and class: `intro −0:07`, blinking in its last seconds; `−0:09` before the next track. */
function countdownView(countdown: Countdown) {
  const time = countdownText(countdown);
  if (countdown.kind === "intro") {
    return {
      text: t("statusbar.intro", { time }),
      spoken: t("statusbar.introLeft", { count: countdown.seconds }),
      className: countdown.urgent ? "statusbar-countdown-urgent" : "statusbar-countdown-intro",
    };
  }
  return {
    text: time,
    spoken: t("statusbar.nextIn", { count: countdown.seconds }),
    className: "statusbar-countdown-end",
  };
}

function Deck({ summary }: Readonly<{ summary: DeckSummary }>) {
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

export function StatusBar() {
  const now = useNow();
  const connected = useLive((live) => live.connected);
  const player = useLive((live) => live.player);
  const positions = useLive((live) => live.positions);
  const system = useLive((live) => live.system);
  const listeners = useListenersNow();
  const statistics = useModule("listener_statistics");

  const paused = player?.paused ?? false;
  const deckSummary = (letter: "A" | "B") => {
    const name = letter.toLowerCase();
    const deck = player?.decks.find((d) => d.deck === name);
    const position = positionNow(positions, name, paused, now);
    return summarizeDeck(letter, deck, paused, position?.position_ms ?? null);
  };
  const summary = system ? summarizeSystem(system) : null;
  const record = recordOf(listeners.data?.record);

  return (
    <div className={`statusbar${connected ? "" : " statusbar-offline"}`} role="status" aria-live="off" aria-label={t("statusbar.label")}>
      {player === null ? (
        <span className="statusbar-deck">{t("statusbar.engine")}</span>
      ) : (
        <>
          <Deck summary={deckSummary("A")} />
          <Deck summary={deckSummary("B")} />
        </>
      )}
      {statistics && (
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
      {summary && (
        <span className="statusbar-part statusbar-system">
          <span className={`statusbar-${summary.cpuLevel}`}>cpu {summary.cpu}</span>
          <span className={`statusbar-${summary.memLevel}`}>mem {summary.mem}</span>
          <span className={`statusbar-${summary.loadLevel}`}>load {summary.load}</span>
        </span>
      )}
      {!connected && <span className="statusbar-part">{t("statusbar.offline")}</span>}
    </div>
  );
}
