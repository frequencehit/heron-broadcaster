// SPDX-License-Identifier: AGPL-3.0-or-later

// A track's page: its transition with another track, before or after it,
// with the track's points as they are on the page, saved or not
// (docs/web.md › Track). A preview only: the station hears nothing.

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";
import { type TrackOverrides, type TrackSummary, api, errorMessage, unwrap } from "../api";
import { t } from "../i18n";
import { db, trackName } from "../lib/format";
import { useTransitionPlayer } from "../lib/transitionPlayer";
import { TrackSearch, useDebounced } from "./TrackSearch";
import { TransitionGraph } from "./TransitionGraph";
import { useWaveform } from "./Waveform";
import { Card, Empty, ErrorView, Icon, Loading, Modal, Segmented, useToast } from "./ui";
import { here } from "../lib/station";

type Other = { id: number; name: string };
/** Where the other track plays: after this one, or before it. */
type Side = "after" | "before";

export function TryTransition({
  trackId,
  trackName: name,
  overrides,
}: {
  trackId: number;
  trackName: string;
  overrides: TrackOverrides;
}) {
  const [side, setSide] = useState<Side>("after");
  const [other, setOther] = useState<Other | null>(null);
  const [picking, setPicking] = useState(false);

  // Until one is picked, a playable track other than this one.
  const fallback = useQuery({
    queryKey: ["tracks", "transition-partner"],
    queryFn: () =>
      unwrap(api.GET("/api/v1/tracks", { params: { query: { filter: "playable", sort: "added", limit: 2 } } })),
    enabled: other === null,
  });
  useEffect(() => {
    const first = fallback.data?.items.find((item) => item.id !== trackId);
    if (other === null && first) {
      setOther({ id: first.id, name: trackName(first) });
    }
  }, [fallback.data, other, trackId]);

  const debounced = useDebounced(overrides, 250);
  const fromId = side === "after" ? trackId : other?.id;
  const toId = side === "after" ? other?.id : trackId;
  const preview = useQuery({
    queryKey: ["transition", "track", trackId, other?.id, side, debounced],
    enabled: other !== null,
    queryFn: () =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/playout/transition", { params: { path: here() },
          body: {
            from_track_id: fromId ?? 0,
            to_track_id: toId ?? 0,
            ...(side === "after" ? { from_overrides: debounced } : { to_overrides: debounced }),
          },
        }),
      ),
    placeholderData: keepPreviousData,
  });
  const outgoingWave = useWaveform(fromId);
  const incomingWave = useWaveform(toId);

  const toast = useToast();
  const failed = useCallback((error: unknown) => toast(errorMessage(error), true), [toast]);
  const listening = useTransitionPlayer(failed);
  const { stop } = listening;
  // Another track, side or point makes another transition: stop this one.
  useEffect(() => stop(), [stop, preview.data]);
  const active = listening.loading || listening.position !== null;

  const choose = (track: TrackSummary) => {
    setOther({ id: track.id, name: trackName(track) });
    setPicking(false);
  };
  const otherButton = (
    <button type="button" className="btn small" onClick={() => setPicking(true)}>
      <span className="ellipsis" style={{ maxWidth: "18rem" }}>
        {other?.name ?? t("tryTransition.pick")}
      </span>
    </button>
  );
  const thisTrack = (
    <strong className="ellipsis" style={{ maxWidth: "18rem" }}>
      {name}
    </strong>
  );

  return (
    <Card title={t("tryTransition.title")} actions={<span className="muted small">{t("tryTransition.help")}</span>}>
      <div className="stack">
        <div className="row">
          <Segmented<Side>
            label={t("tryTransition.side")}
            value={side}
            onChange={setSide}
            options={[
              { value: "after", label: t("tryTransition.after") },
              { value: "before", label: t("tryTransition.before") },
            ]}
          />
          <span className="row" style={{ gap: "0.4rem" }}>
            <span className="swatch" style={{ background: "var(--deck-a)" }} />
            {side === "after" ? thisTrack : otherButton}
            <span className="muted">{t("playout.into")}</span>
            <span className="swatch" style={{ background: "var(--deck-b)" }} />
            {side === "after" ? otherButton : thisTrack}
          </span>
          <button
            type="button"
            className="btn small"
            disabled={!preview.data || fromId === undefined || toId === undefined}
            onClick={() => {
              if (active) {
                listening.stop();
              } else if (preview.data && fromId !== undefined && toId !== undefined) {
                listening.play(fromId, toId, preview.data);
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
        {other === null ? (
          <Empty>{t("tryTransition.none")}</Empty>
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
        title={side === "after" ? t("tryTransition.pickAfter") : t("tryTransition.pickBefore")}
        open={picking}
        onClose={() => setPicking(false)}
      >
        <TrackSearch
          actions={(track) => (
            <button
              type="button"
              className="btn small primary"
              disabled={track.id === trackId}
              onClick={() => choose(track)}
            >
              {t("restrictions.pick")}
            </button>
          )}
        />
      </Modal>
    </Card>
  );
}
