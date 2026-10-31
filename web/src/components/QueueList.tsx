// SPDX-License-Identifier: AGPL-3.0-or-later

// The waiting tracks (the cued one first), reordered by dragging them or
// with the keyboard, from the live state.

import {
  DndContext,
  type DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useEffect, useMemo, useState } from "react";
import { type QueueEntry, api, done } from "../api";
import { t } from "../i18n";
import { clockTime, duration, trackName } from "../lib/format";
import { sourceLabel } from "../lib/labels";
import { positionNow, useLive } from "../lib/live";
import { Link } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { jingleStart } from "../lib/intro";
import { CategoryChips, type CategoryRef, useTrackCategories } from "./TrackCategories";
import { Empty, Icon } from "./ui";
import { here } from "../lib/station";

/**
 * The waiting tracks in play order: the jingle over the cued track's intro,
 * the cued one, then the queue.
 */
export function useWaiting(): QueueEntry[] | undefined {
  const player = useLive((live) => live.player);
  return useMemo(() => {
    if (!player) {
      return undefined;
    }
    return [player.cued_jingle, player.cued, ...player.queue].filter(
      (entry): entry is QueueEntry => !!entry,
    );
  }, [player]);
}

/**
 * When each waiting track should start (Unix ms), without crossfades. A
 * jingle over an intro takes no time of its own.
 */
function startTimes(
  entries: QueueEntry[],
  firstStart: number | null,
  overIntro: number | null,
): (number | null)[] {
  let at = firstStart;
  return entries.map((entry) => {
    const start = at;
    if (at !== null && entry.entry_id !== overIntro) {
      at += Math.max(0, entry.track.cue_out_ms - entry.track.cue_in_ms);
    }
    return start;
  });
}

export function QueueList({ limit }: { limit?: number }) {
  const waiting = useWaiting();
  const player = useLive((live) => live.player);
  const positions = useLive((live) => live.positions);
  const canEdit = useCan("operator");
  const categoriesOf = useTrackCategories((waiting ?? []).map((entry) => entry.track.track_id));
  const [order, setOrder] = useState<QueueEntry[] | undefined>(waiting);

  // The live state wins whenever it changes.
  useEffect(() => {
    setOrder(waiting);
  }, [waiting]);

  const move = useAction(
    ({ entry, position }: { entry: number; position: number }) =>
      done(
        api.PUT("/api/v1/stations/{station_id}/queue/{entry_id}/position", {
          params: { path: here({ entry_id: entry }) },
          body: { position },
        }),
      ),
  );
  const remove = useAction((entry: number) =>
    done(api.DELETE("/api/v1/stations/{station_id}/queue/{entry_id}", { params: { path: here({ entry_id: entry }) } })),
  );

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  if (player === undefined) {
    return <Empty>{t("queue.connecting")}</Empty>;
  }
  if (player === null) {
    return <Empty>{t("queue.engineUnreachable")}</Empty>;
  }
  const entries = order ?? [];
  if (entries.length === 0) {
    return <Empty>{t("queue.empty")}</Empty>;
  }

  // The on-air deck's remaining time gives the first start.
  const onAir = player.decks.find((deck) => deck.on_air && deck.deck !== "fx");
  const position = onAir ? positionNow(positions, onAir.deck, player.paused) : null;
  const firstStart =
    onAir?.entry && position
      ? Date.now() + Math.max(0, onAir.entry.track.cue_out_ms - position.position_ms)
      : null;
  const starts = startTimes(entries, firstStart, player.cued_jingle?.entry_id ?? null);
  const shown = limit ? entries.slice(0, limit) : entries;

  const onDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;
    if (!over || active.id === over.id) {
      return;
    }
    const from = entries.findIndex((entry) => entry.entry_id === active.id);
    const to = entries.findIndex((entry) => entry.entry_id === over.id);
    if (from < 0 || to < 0) {
      return;
    }
    setOrder(arrayMove(entries, from, to));
    move.mutate({ entry: Number(active.id), position: to }, { onError: () => setOrder(waiting) });
  };

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
      <SortableContext
        items={shown.map((entry) => entry.entry_id)}
        strategy={verticalListSortingStrategy}
      >
        <ol className="list" aria-label={t("nav.queue")}>
          {shown.map((entry, index) => (
            <Item
              key={entry.entry_id}
              entry={entry}
              index={index}
              start={starts[index] ?? null}
              cued={player.cued?.entry_id === entry.entry_id}
              overIntro={player.cued_jingle?.entry_id === entry.entry_id}
              categories={entry.track.track_id != null ? categoriesOf.get(entry.track.track_id) : undefined}
              jingleAt={
                player.cued_jingle?.entry_id === entry.entry_id && player.cued
                  ? jingleStart(player.cued.track, entry.track)
                  : null
              }
              canEdit={canEdit}
              onNext={() => {
                setOrder(arrayMove(entries, index, 0));
                move.mutate(
                  { entry: entry.entry_id, position: 0 },
                  { onError: () => setOrder(waiting) },
                );
              }}
              onRemove={() => remove.mutate(entry.entry_id)}
            />
          ))}
        </ol>
      </SortableContext>
      {limit && entries.length > limit && (
        <div className="pager">
          <span className="muted small">{t("queue.more", { count: entries.length - limit })}</span>
          <Link to="/queue">{t("queue.whole")}</Link>
        </div>
      )}
    </DndContext>
  );
}

function Item({
  entry,
  index,
  start,
  cued,
  overIntro,
  categories,
  jingleAt,
  canEdit,
  onNext,
  onRemove,
}: {
  entry: QueueEntry;
  index: number;
  start: number | null;
  cued: boolean;
  overIntro: boolean;
  categories?: CategoryRef[] | undefined;
  /** Where the jingle starts in the track it plays over. */
  jingleAt?: number | null;
  canEdit: boolean;
  onNext: () => void;
  onRemove: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: entry.entry_id,
    disabled: !canEdit,
  });
  const track = entry.track;
  const name = trackName(track);
  return (
    <li
      ref={setNodeRef}
      className={isDragging ? "queue-item dragging" : "queue-item"}
      style={{ transform: CSS.Transform.toString(transform), transition }}
    >
      {canEdit && (
        <button
          type="button"
          className="btn ghost small icon handle"
          aria-label={t("queue.move", { name })}
          {...attributes}
          {...listeners}
        >
          <Icon name="grip" />
        </button>
      )}
      <span className="index">{index + 1}</span>
      <div className="meta">
        <div className="ellipsis">
          {track.track_id ? <Link to={`/library/${track.track_id}`}>{name}</Link> : name}
        </div>
        <div className="row small muted" style={{ gap: "0.4rem" }}>
          {cued && <span className="badge accent">{t("queue.cued")}</span>}
          {overIntro && (
            <span className="badge accent" title={t("queue.overIntroHint")}>
              {jingleAt !== null && jingleAt !== undefined
                ? t("queue.overIntroAt", { time: duration(jingleAt, true) })
                : t("queue.overIntro")}
            </span>
          )}
          <span className="badge">
            {sourceLabel(track.source)}
          </span>
          <span className="num">{duration(track.cue_out_ms - track.cue_in_ms)}</span>
          <CategoryChips categories={categories} />
        </div>
      </div>
      <span className="start" title={t("queue.expectedStart")}>
        {start !== null ? `≈ ${clockTime(start)}` : ""}
      </span>
      {canEdit && (
        <div className="row" style={{ flexWrap: "nowrap", gap: "0.25rem" }}>
          {index > 0 && (
            <button
              type="button"
              className="btn ghost small icon"
              title={t("queue.playNext")}
              aria-label={t("queue.playNamedNext", { name })}
              onClick={onNext}
            >
              <Icon name="next" />
            </button>
          )}
          <button
            type="button"
            className="btn ghost small icon danger"
            title={t("queue.remove")}
            aria-label={t("queue.removeNamed", { name })}
            onClick={onRemove}
          >
            <Icon name="trash" />
          </button>
        </div>
      )}
    </li>
  );
}
