// SPDX-License-Identifier: AGPL-3.0-or-later

// The line at the bottom of every page: both decks, the station's
// listeners and, for operators and admins, the machine's load
// (docs/web.md#status-bar).

import { useEffect, useState } from "react";
import { t } from "../i18n";
import { useListenersNow } from "../lib/listeners";
import { positionNow, useLive } from "../lib/live";
import { type DeckSummary, listenersText, summarizeDeck, summarizeSystem } from "../lib/statusbar";

/** The time now, in the clock `positionNow` uses, moved on once a second. */
function useNow() {
  const [now, setNow] = useState(() => performance.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(performance.now()), 1_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

function Deck({ summary }: Readonly<{ summary: DeckSummary }>) {
  const state = t(`statusbar.${summary.status}`);
  const spoken = t("statusbar.deck", { deck: summary.letter, state });
  return (
    <span
      className={`statusbar-deck statusbar-deck-${summary.letter.toLowerCase()}${summary.onAir ? " statusbar-on-air" : ""}`}
    >
      {/* What the symbols and the cut title say, for screen readers. */}
      <span className="visually-hidden">{summary.text ? `${spoken}, ${summary.text}` : spoken}</span>
      <span className="statusbar-deck-letter" aria-hidden="true">
        {summary.letter}
      </span>
      <span className="statusbar-deck-symbol" aria-hidden="true">
        {summary.symbol}
      </span>
      {summary.text && (
        <span className="statusbar-deck-text" aria-hidden="true">
          {summary.text}
        </span>
      )}
    </span>
  );
}

export function StatusBar() {
  const now = useNow();
  const connected = useLive((live) => live.connected);
  const player = useLive((live) => live.player);
  const positions = useLive((live) => live.positions);
  const system = useLive((live) => live.system);
  const listeners = useListenersNow();

  const paused = player?.paused ?? false;
  const deckSummary = (letter: "A" | "B") => {
    const name = letter.toLowerCase();
    const deck = player?.decks.find((d) => d.deck === name);
    const position = positionNow(positions, name, paused, now);
    return summarizeDeck(letter, deck, paused, position?.position_ms ?? null);
  };
  const summary = system ? summarizeSystem(system) : null;

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
      <span className="statusbar-part">
        {t("statusbar.listeners")} {listenersText(listeners.data?.total)}
      </span>
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
