// SPDX-License-Identifier: AGPL-3.0-or-later

// One track: waveform with its cue and fade points, preview, per-track
// overrides, categories, programming details and labels, analysis, file
// and plays, and its tags edited.

import { PlaysByStation } from "../components/Stations";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import {
  type Fade,
  type PlayoutSettings,
  type TrackDetail,
  type TrackOverrides,
  api,
  done,
  unwrap,
} from "../api";
import { FadeEditor } from "../components/FadeEditor";
import { NewCategory } from "../components/NewCategory";
import { TaskState, useTask } from "../components/Task";
import { TryTransition } from "../components/TryTransition";
import {
  LabelChip,
  LabelsDialog,
  ProgrammingDialog,
  TAG_FIELDS,
  TagsDialog,
  type TagValues,
} from "../components/SelectionActions";
import { type Marker, type Region, WaveformView, useWaveform } from "../components/Waveform";
import {
  Banner,
  Card,
  ConfirmButton,
  Empty,
  ErrorView,
  Field,
  Icon,
  Loading,
  NumberInput,
  PageHead,
  Segmented,
  useToast,
} from "../components/ui";
import { number, rich, t } from "../i18n";
import { ago, bytes, dateTime, db, duration, parseDuration, trackName } from "../lib/format";
import { endingLabel } from "../lib/labels";
import { withNextStart } from "../lib/nextStart";
import { Link } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { DeleteQuestion, bitrate } from "./Library";
import { here } from "../lib/station";

/** Whether overrides change anything (every field unset). */
function isDefault(overrides: TrackOverrides): boolean {
  const unset = (value: unknown) => value === null || value === undefined;
  return (
    unset(overrides.cue_in_ms) &&
    unset(overrides.cue_out_ms) &&
    unset(overrides.gap_killer_enabled) &&
    Object.values(overrides.crossfade ?? {}).every(unset)
  );
}

/** The same overrides, with unset fields as `null`, so drafts compare. */
function normalize(overrides: TrackOverrides): TrackOverrides {
  const cf = overrides.crossfade ?? {};
  return {
    cue_in_ms: overrides.cue_in_ms ?? null,
    cue_out_ms: overrides.cue_out_ms ?? null,
    gap_killer_enabled: overrides.gap_killer_enabled ?? null,
    crossfade: {
      crossfade: cf.crossfade ?? null,
      fade_in: cf.fade_in ?? null,
      fade_out: cf.fade_out ?? null,
      fixed_ms: cf.fixed_ms ?? null,
      mode: cf.mode ?? null,
    },
  };
}

export function Track({ id }: { id: number }) {
  const track = useQuery({
    queryKey: ["track", id],
    queryFn: () => unwrap(api.GET("/api/v1/tracks/{id}", { params: { path: { id } } })),
  });
  const playout = useQuery({
    queryKey: ["settings", "playout"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/playout", { params: { path: here() } })),
  });
  if (track.isPending) {
    return <Loading />;
  }
  if (track.isError) {
    return <ErrorView error={track.error} />;
  }
  return <TrackView track={track.data} settings={playout.data} />;
}

/** Where the track plays from and to, with `overrides` applied. */
export function effectiveCues(track: TrackDetail, overrides: TrackOverrides) {
  const length = track.duration_ms ?? track.analysis?.cue_out_ms ?? 0;
  let cueIn = track.analysis?.cue_in_ms ?? 0;
  let cueOut = track.analysis?.cue_out_ms ?? length;
  if (overrides.gap_killer_enabled === false) {
    cueIn = 0;
    cueOut = length;
  }
  cueIn = overrides.cue_in_ms ?? cueIn;
  cueOut = Math.min(overrides.cue_out_ms ?? cueOut, length || Number.POSITIVE_INFINITY);
  return { length, cueIn: Math.min(cueIn, cueOut), cueOut };
}

function TrackView({ track, settings }: { track: TrackDetail; settings: PlayoutSettings | undefined }) {
  const canEdit = useCan("operator");
  const waveform = useWaveform(track.id);
  const saved = normalize(track.overrides);
  const [draft, setDraft] = useState<TrackOverrides>(saved);
  // Where the vocals start, kept apart from the overrides (`track_markers`).
  const savedIntro = track.markers?.intro_ms ?? null;
  const [intro, setIntro] = useState<number | null>(savedIntro);
  // The hook (most recognizable part), likewise.
  const savedHook: Hook = { in: track.markers?.hook_in_ms ?? null, out: track.markers?.hook_out_ms ?? null };
  const [hook, setHook] = useState<Hook>(savedHook);
  // Looping the hook being edited, until stopped.
  const [looping, setLooping] = useState(false);
  const loop = useRef<Hook | null>(null);
  loop.current = looping ? hook : null;
  const [zoom, setZoom] = useState<"whole" | "intro" | "outro">("whole");
  const audio = useRef<HTMLAudioElement>(null);
  const [playing, setPlaying] = useState(false);
  const [playhead, setPlayhead] = useState<number | null>(null);
  const [editingTags, setEditingTags] = useState(false);
  const fileTags = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/tracks/tag-overrides/clear", { body: { selection: { track_ids: [track.id] } } }),
      ),
    { success: t("track.fileTagsDone"), invalidate: [["tracks"], ["track"]] },
  );

  useEffect(() => {
    setDraft(normalize(track.overrides));
  }, [track.overrides]);
  useEffect(() => {
    setIntro(savedIntro);
  }, [savedIntro]);
  useEffect(() => {
    setHook({ in: savedHook.in, out: savedHook.out });
  }, [savedHook.in, savedHook.out]);

  // Follow the audio while it plays.
  useEffect(() => {
    if (!playing) {
      return;
    }
    let frame = 0;
    const step = () => {
      const element = audio.current;
      if (element) {
        // Looping the hook: back to its start at its end.
        const range = loop.current;
        if (range?.in != null && range.out != null && element.currentTime * 1000 >= range.out) {
          element.currentTime = range.in / 1000;
        }
        setPlayhead(element.currentTime * 1000);
      }
      frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [playing]);

  // The overrides and the intro point, each when it changed.
  const saveOverrides = useAction(
    async ({
      overrides,
      introMs,
      hookPoints,
    }: {
      overrides: TrackOverrides | null;
      introMs: number | null | undefined;
      hookPoints: Hook | undefined;
    }) => {
      const path = { params: { path: { id: track.id } } };
      if (overrides) {
        await unwrap(api.PUT("/api/v1/tracks/{id}/overrides", { ...path, body: overrides }));
      }
      if (introMs !== undefined) {
        await unwrap(api.PUT("/api/v1/tracks/{id}/intro", { ...path, body: { intro_ms: introMs } }));
      }
      if (hookPoints) {
        await unwrap(
          api.PUT("/api/v1/tracks/{id}/hook", {
            ...path,
            body: { hook_in_ms: hookPoints.in, hook_out_ms: hookPoints.out },
          }),
        );
      }
    },
    { success: t("track.saved"), invalidate: [["track", track.id], ["tracks"]] },
  );
  const clearOverrides = useAction(
    () => done(api.DELETE("/api/v1/tracks/{id}/overrides", { params: { path: { id: track.id } } })),
    { success: t("track.cleared"), invalidate: [["track", track.id]] },
  );
  const setEnabled = useAction(
    (enabled: boolean) =>
      unwrap(api.PATCH("/api/v1/tracks/{id}", { params: { path: { id: track.id } }, body: { enabled } })),
    { invalidate: [["track", track.id], ["tracks"]] },
  );
  const enqueue = useAction(
    (next: boolean) =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/queue", { params: { path: here() },
          body: next ? { track_ids: [track.id], position: 0 } : { track_ids: [track.id] },
        }),
      ),
    { success: t("queuePage.queued") },
  );
  const client = useQueryClient();
  const toast = useToast();
  const [deleteTask, setDeleteTask] = useState<number | null>(null);
  const deleting = useTask(deleteTask);
  const remove = useAction(
    () => unwrap(api.POST("/api/v1/tracks/delete", { body: { track_ids: [track.id] } })),
    { onSuccess: (task) => setDeleteTask(task.id) },
  );
  useEffect(() => {
    const task = deleting.data;
    if (task?.status === "done") {
      const skipped = task.report?.skipped[0];
      toast(skipped ? t("track.notDeleted", { reason: skipped.reason }) : t("track.deleted"), Boolean(skipped));
      void client.invalidateQueries({ queryKey: ["track", track.id] });
      void client.invalidateQueries({ queryKey: ["tracks"] });
      void client.invalidateQueries({ queryKey: ["trash"] });
    } else if (task?.status === "failed") {
      toast(task.error ?? t("track.deleteFailed"), true);
    }
  }, [deleting.data, client, toast, track.id]);
  const deleted = track.deleted_at_unix_ms !== null && track.deleted_at_unix_ms !== undefined;

  const { length, cueIn, cueOut } = effectiveCues(track, draft);
  const crossfade = settings?.crossfade;
  const overrides = draft.crossfade;
  const mode = overrides.mode ?? crossfade?.mode ?? "auto";
  const fixedMs = overrides.fixed_ms ?? crossfade?.fixed_ms ?? 2000;
  const shortTrack = crossfade ? cueOut - cueIn <= crossfade.min_track_ms : false;
  const crossfades = overrides.crossfade !== false && !shortTrack;
  const autoPoint = track.analysis?.fade_out_start_ms ?? null;
  // A short track keeps a point it sets itself, without fades.
  const ownPoint =
    overrides.crossfade !== false && overrides.mode === "fixed" && overrides.fixed_ms != null
      ? cueOut - overrides.fixed_ms
      : null;
  const mixPoint = shortTrack
    ? ownPoint !== null && ownPoint >= cueIn && ownPoint < cueOut
      ? ownPoint
      : null
    : !crossfades
      ? null
      : mode === "fixed"
        ? Math.max(cueIn, cueOut - fixedMs)
        : autoPoint !== null && autoPoint >= cueIn && autoPoint < cueOut
          ? autoPoint
          : null;
  const fadeOut: Fade | undefined = overrides.fade_out ?? crossfade?.fade_out;
  const fadeIn: Fade | undefined = overrides.fade_in ?? crossfade?.fade_in;

  const overridesDirty = JSON.stringify(draft) !== JSON.stringify(saved);
  const introDirty = intro !== savedIntro;
  const hookDirty = hook.in !== savedHook.in || hook.out !== savedHook.out;
  // Both points, in before out, or none.
  const hookValid = (hook.in === null) === (hook.out === null) && (hook.in === null || hook.in < (hook.out ?? 0));
  const hookSet = hook.in !== null && hook.out !== null && hook.in < hook.out;
  const dirty = overridesDirty || introDirty || hookDirty;
  const hasOverrides = !isDefault(track.overrides);

  const markers: Marker[] = [
    {
      id: "cue_in",
      label: t("track.cueIn"),
      ms: cueIn,
      color: "var(--good)",
      ...(canEdit && {
        onDrag: (ms: number) => setDraft((d) => ({ ...d, cue_in_ms: Math.min(ms, cueOut - 100) })),
      }),
    },
    {
      id: "cue_out",
      label: t("track.cueOut"),
      ms: cueOut,
      color: "var(--bad)",
      end: true,
      ...(canEdit && {
        onDrag: (ms: number) => setDraft((d) => ({ ...d, cue_out_ms: Math.max(ms, cueIn + 100) })),
      }),
    },
  ];
  // Always shown: on the cue-out when the next track follows back to back.
  {
    markers.push({
      id: "mix",
      label: mixPoint !== null ? t("transition.nextStarts") : t("transition.nextStartsBackToBack"),
      ms: mixPoint ?? cueOut,
      color: "var(--accent)",
      end: true,
      row: 1,
      // Dragging it sets a fixed crossfade for this track.
      ...(canEdit && {
        onDrag: (ms: number) => setDraft((d) => withNextStart(d, ms, cueOut)),
      }),
    });
  }
  const regions: Region[] = [];
  if (mixPoint !== null && fadeOut?.enabled && !shortTrack) {
    regions.push({
      startMs: mixPoint,
      endMs: Math.min(cueOut, mixPoint + fadeOut.time_ms),
      color: getVar("--bad"),
      label: t("track.fadeOut"),
    });
  }
  if (fadeIn?.enabled && crossfades) {
    regions.push({
      startMs: cueIn,
      endMs: Math.min(cueOut, cueIn + fadeIn.time_ms),
      color: getVar("--good"),
      label: t("track.fadeIn"),
    });
  }

  // Points imported from another software (the intro, above, can be set
  // here too): shown, not played by Heron.
  const imported = track.markers;
  if (intro !== null) {
    markers.push({
      id: "intro",
      label: t("track.introEnds"),
      ms: intro,
      color: "var(--fx)",
      row: 2,
      ...(canEdit && { onDrag: (ms: number) => setIntro(Math.max(1, Math.min(ms, length))) }),
    });
  }
  if (imported?.intro2_ms != null) {
    markers.push({ id: "intro2", label: t("track.intro2Ends"), ms: imported.intro2_ms, color: "var(--fx)", row: 2 });
  }
  if (hookSet) {
    regions.push({ startMs: hook.in ?? 0, endMs: hook.out ?? 0, color: getVar("--deck-b"), label: t("track.hook") });
  }
  if (hook.in !== null) {
    markers.push({
      id: "hook_in",
      label: t("track.hookIn"),
      ms: hook.in,
      color: "var(--deck-b)",
      row: 3,
      ...(canEdit && {
        onDrag: (ms: number) => setHook((h) => ({ ...h, in: Math.max(0, Math.min(ms, (h.out ?? length) - 100)) })),
      }),
    });
  }
  if (hook.out !== null) {
    markers.push({
      id: "hook_out",
      label: t("track.hookOut"),
      ms: hook.out,
      color: "var(--deck-b)",
      end: true,
      // A row of its own: a short hook would put both flags on each other.
      row: 4,
      ...(canEdit && {
        onDrag: (ms: number) => setHook((h) => ({ ...h, out: Math.min(length, Math.max(ms, (h.in ?? 0) + 100)) })),
      }),
    });
  }

  const view: [number, number] | undefined =
    zoom === "intro"
      ? [Math.max(0, cueIn - 5_000), Math.min(length, cueIn + 20_000)]
      : zoom === "outro"
        ? [Math.max(0, (mixPoint ?? cueOut) - 15_000), Math.min(length, cueOut + 5_000)]
        : undefined;

  const playFrom = (ms: number) => {
    const element = audio.current;
    if (!element) {
      return;
    }
    element.currentTime = ms / 1000;
    setPlayhead(ms);
    void element.play();
  };

  return (
    <>
      <PageHead
        title={track.title ?? trackName(track)}
        description={
          <>
            {[track.artist, track.album].filter(Boolean).join(" · ") || t("track.noTags")} ·{" "}
            <code>{track.path}</code>
          </>
        }
        actions={
          canEdit &&
          !deleted && (
            <>
              {!track.missing && !track.silent && (
                <>
                  <button type="button" className="btn" onClick={() => enqueue.mutate(true)}>
                    <Icon name="next" /> {t("queue.playNext")}
                  </button>
                  <button type="button" className="btn" onClick={() => enqueue.mutate(false)}>
                    <Icon name="plus" /> {t("track.queue")}
                  </button>
                </>
              )}
              {!track.missing && (
                <button type="button" className="btn" onClick={() => setEditingTags(true)}>
                  <Icon name="edit" /> {t("track.editTags")}
                </button>
              )}
              <button
                type="button"
                className="btn"
                onClick={() => setEnabled.mutate(!track.enabled)}
                title={track.enabled ? t("track.disableHelp") : undefined}
              >
                {track.enabled ? t("track.disable") : t("track.enable")}
              </button>
              <ConfirmButton
                className="btn danger"
                confirm={t("library.delete")}
                disabled={deleteTask !== null && deleting.data?.status !== "failed"}
                question={<DeleteQuestion tracks={[track]} />}
                onConfirm={() => remove.mutate(undefined)}
              >
                <Icon name="trash" /> {t("library.delete")}
              </ConfirmButton>
            </>
          )
        }
      />
      {deleteTask !== null && deleting.data?.status !== "done" && deleting.data?.status !== "failed" && (
        <Banner kind="warn">
          {t("library.deleting")} <TaskState task={deleting.data} />
        </Banner>
      )}
      {deleted && (
        <Banner kind="warn">
          {rich("track.inTrash", {
            date: dateTime(track.deleted_at_unix_ms),
            link: <Link to="/trash">{t("track.restoreFromTrash")}</Link>,
          })}
        </Banner>
      )}
      {track.missing && !deleted && (
        <Banner kind="bad">
          {t("track.missingSince", { date: dateTime(track.file.missing_since_unix_ms) })}
        </Banner>
      )}
      {track.silent && <Banner kind="bad">{t("track.silent")}</Banner>}
      {!track.enabled && <Banner kind="warn">{t("track.disabled")}</Banner>}
      {track.tag_overrides.length > 0 && (
        <Banner kind="warn">
          {t("track.tagOverrides", {
            fields: track.tag_overrides
              .map((field) => {
                const known = TAG_FIELDS.find((tag) => tag.field === field);
                return known ? t(known.label) : field;
              })
              .join(", "),
          })}{" "}
          {canEdit && (
            <button
              type="button"
              className="btn small"
              disabled={fileTags.isPending}
              onClick={() => fileTags.mutate(undefined)}
            >
              {t("track.fileTags")}
            </button>
          )}
        </Banner>
      )}
      {track.identification?.status === "needs_review" && (
        <Banner kind="warn">
          {rich("track.needsReview", {
            reason: track.identification.reason,
            link: <Link to="/review">{t("track.reviewIt")}</Link>,
          })}
        </Banner>
      )}

      <Card
        title={t("track.waveform")}
        actions={
          <>
            <Segmented
              label={t("track.zoom")}
              value={zoom}
              onChange={setZoom}
              options={[
                { value: "whole", label: t("track.whole") },
                { value: "intro", label: t("track.intro") },
                { value: "outro", label: t("track.outro") },
              ]}
            />
            {!track.missing && (
              <>
                <button
                  type="button"
                  className="btn small"
                  onClick={() => (playing ? audio.current?.pause() : playFrom(playhead ?? cueIn))}
                >
                  <Icon name={playing ? "pause" : "play"} size={14} /> {playing ? t("dashboard.pause") : t("track.listen")}
                </button>
                <span
                  className="num small playhead-time"
                  title={t("track.playheadHelp", { fromCueIn: duration((playhead ?? cueIn) - cueIn, true) })}
                >
                  <span className="visually-hidden">{t("track.playhead")} </span>
                  {duration(playhead ?? 0, true)} / {duration(length)}
                </span>
                <button type="button" className="btn small" onClick={() => playFrom(cueIn)}>
                  {t("track.fromCueIn")}
                </button>
                {(hookSet || looping) && (
                  <button
                    type="button"
                    className={looping ? "btn small primary" : "btn small"}
                    aria-pressed={looping}
                    onClick={() => {
                      if (looping) {
                        audio.current?.pause();
                      } else {
                        playFrom(hook.in ?? 0);
                        setLooping(true);
                      }
                    }}
                  >
                    <Icon name={looping ? "stop" : "play"} size={14} /> {looping ? t("track.stopHook") : t("track.loopHook")}
                  </button>
                )}
                <button
                  type="button"
                  className="btn small"
                  onClick={() => playFrom(Math.max(0, (mixPoint ?? cueOut) - 5_000))}
                >
                  {t("track.ending")}
                </button>
              </>
            )}
          </>
        }
      >
        <div className="stack">
          {waveform.isError ? (
            <ErrorView error={waveform.error} />
          ) : (
            <WaveformView
              data={waveform.data ?? null}
              durationMs={length}
              markers={markers}
              regions={regions}
              playheadMs={playhead}
              onSeek={playFrom}
              {...(view && { view })}
            />
          )}
          <div className="legend muted">
            <span>
              <span className="swatch" style={{ background: "var(--good)" }} /> {t("track.cueIn")}{" "}
              <strong className="num">{duration(cueIn, true)}</strong>
            </span>
            <span>
              <span className="swatch" style={{ background: "var(--bad)" }} /> {t("track.cueOut")}{" "}
              <strong className="num">{duration(cueOut, true)}</strong>
            </span>
            <span>
              <span className="swatch" style={{ background: "var(--accent)" }} /> {t("track.nextTrackStarts")}{" "}
              <strong className="num">
                {mixPoint !== null ? duration(mixPoint, true) : t("track.backToBackAt", { at: duration(cueOut, true) })}
              </strong>
            </span>
            <span>{t("track.plays", { duration: duration(cueOut - cueIn) })}</span>
            {intro !== null && (
              <span>
                <span className="swatch" style={{ background: "var(--fx)" }} /> {t("track.introEnds")}{" "}
                <strong className="num">{duration(intro, true)}</strong>
                {imported?.intro2_ms != null && (
                  <>
                    {" · "}
                    {t("track.intro2Ends")} <strong className="num">{duration(imported.intro2_ms, true)}</strong>
                  </>
                )}
              </span>
            )}
            {hookSet && (
              <span>
                <span className="swatch" style={{ background: "var(--deck-b)" }} /> {t("track.hook")}{" "}
                <strong className="num">
                  {duration(hook.in, true)} – {duration(hook.out, true)}
                </strong>
              </span>
            )}
            {(imported?.next_soft_ms != null || imported?.next_hard_ms != null) && (
              <span title={t("track.importedNextHelp")}>
                {t("track.importedNext", {
                  soft: imported.next_soft_ms != null ? duration(imported.next_soft_ms, true) : "–",
                  hard: imported.next_hard_ms != null ? duration(imported.next_hard_ms, true) : "–",
                })}
              </span>
            )}
            {canEdit && <span>{t("track.dragHelp")}</span>}
          </div>
          <audio
            ref={audio}
            preload="none"
            src={`/api/v1/tracks/${track.id}/audio`}
            onPlay={() => setPlaying(true)}
            onPause={() => {
              setPlaying(false);
              setLooping(false);
              // Where it stopped, to the frame.
              if (audio.current) {
                setPlayhead(audio.current.currentTime * 1000);
              }
            }}
            onEnded={() => setPlaying(false)}
          />
        </div>
      </Card>

      {track.analysis && <TryTransition trackId={track.id} trackName={trackName(track)} overrides={draft} />}

      <div className="grid two">
        <OverridesCard
          track={track}
          settings={settings}
          draft={draft}
          setDraft={setDraft}
          canEdit={canEdit}
          dirty={dirty}
          hasOverrides={hasOverrides}
          shortTrack={shortTrack}
          cueOut={cueOut}
          mixPoint={mixPoint}
          intro={intro}
          setIntro={setIntro}
          hook={hook}
          setHook={setHook}
          hookValid={hookValid}
          playhead={playhead}
          onSave={() =>
            saveOverrides.mutate({
              overrides: overridesDirty ? draft : null,
              introMs: introDirty ? intro : undefined,
              hookPoints: hookDirty ? hook : undefined,
            })
          }
          onRevert={() => {
            setDraft(saved);
            setIntro(savedIntro);
            setHook(savedHook);
          }}
          onClear={() => clearOverrides.mutate(undefined)}
          saving={saveOverrides.isPending}
        />
        <div className="stack">
          <ProgrammingCard track={track} canEdit={canEdit} />
          <CategoriesCard track={track} canEdit={canEdit} />
          <DetailsCard track={track} />
          <PlaysCard track={track} />
        </div>
      </div>
      <TagsDialog
        open={editingTags}
        onClose={() => setEditingTags(false)}
        selection={{ track_ids: [track.id] }}
        tracks={1}
        current={currentTags(track)}
        genres={track.musicbrainz?.genres ?? []}
      />
    </>
  );
}

/** What the scanner read from the file, to start a tag edit from. */
function currentTags(track: TrackDetail): TagValues {
  return {
    title: track.title,
    artist: track.artist,
    album: track.album,
    album_artist: track.album_artist,
    year: track.file.year_tag,
    genre: track.genre,
    track: track.file.track_number,
    disc: track.file.disc_number,
  };
}

function ProgrammingCard({ track, canEdit }: { track: TrackDetail; canEdit: boolean }) {
  const [dialog, setDialog] = useState<"labels" | "programming" | null>(null);
  const measured = track.analysis?.bpm;
  const mb = track.musicbrainz;
  const bpm = (value: number | null | undefined) =>
    value === null || value === undefined ? "–" : number(value);
  return (
    <Card
      title={t("track.programming")}
      actions={
        canEdit && (
          <>
            <button type="button" className="btn small" onClick={() => setDialog("labels")}>
              {t("library.column.labels")}
            </button>
            <button type="button" className="btn small" onClick={() => setDialog("programming")}>
              {t("track.programmingEdit")}
            </button>
          </>
        )
      }
    >
      <dl className="facts small">
        <dt>{t("library.column.labels")}</dt>
        <dd>
          {track.labels.length === 0 ? (
            <span className="muted">{t("track.noLabel")}</span>
          ) : (
            <span className="chips">
              {track.labels.map((label) => (
                <Link key={label.id} to={`/library?labels_any=${label.id}`}>
                  <LabelChip label={label} />
                </Link>
              ))}
            </span>
          )}
        </dd>
        <dt>{t("library.column.era")}</dt>
        <dd>
          {track.era ?? "–"}
          {track.year && (
            <span className="muted">
              {" "}
              {mb?.first_release_year
                ? // A year, not an amount: no thousands separator ("2,018").
                  t("track.firstReleased", { year: String(track.year) })
                : t("track.yearTag", { year: String(track.year) })}
            </span>
          )}
        </dd>
        <dt>{t("library.column.bpm")}</dt>
        <dd>
          {bpm(track.bpm)}
          {track.bpm_by_hand !== null && track.bpm_by_hand !== undefined ? (
            <span className="muted">
              {" "}
              {measured ? t("track.bpmByHandMeasured", { bpm: number(measured) }) : t("track.bpmByHand")}
            </span>
          ) : (
            measured && <span className="muted"> {t("track.bpmMeasured")}</span>
          )}
        </dd>
        <dt>{t("library.column.mood")}</dt>
        <dd>{track.mood ?? "–"}</dd>
        <dt>{t("library.column.soundCode")}</dt>
        <dd>{track.sound_code ?? "–"}</dd>
        {mb ? (
          <>
            <dt>{t("library.sort.album")}</dt>
            <dd>{mb.album ?? "–"}</dd>
            <dt>{t("selection.recordLabel")}</dt>
            <dd>
              {mb.record_label ?? "–"}
              {mb.catalog_number && <span className="muted"> · {mb.catalog_number}</span>}
            </dd>
            <dt>{t("selection.isrc")}</dt>
            <dd>
              <code>{mb.isrc ?? "–"}</code>
            </dd>
            {mb.genres.length > 0 && (
              <>
                <dt>{t("track.genres")}</dt>
                <dd title={t("track.genresHelp")}>{mb.genres.join(", ")}</dd>
              </>
            )}
            <dt>MusicBrainz</dt>
            <dd className="muted">{t("track.lookedUp", { ago: ago(mb.fetched_at_unix_ms) })}</dd>
          </>
        ) : (
          <>
            <dt>MusicBrainz</dt>
            <dd className="muted">
              {track.file.musicbrainz_release_id ? t("track.notLookedUp") : t("track.notIdentified")}
            </dd>
          </>
        )}
      </dl>
      <LabelsDialog
        open={dialog === "labels"}
        onClose={() => setDialog(null)}
        selection={{ track_ids: [track.id] }}
        tracks={1}
      />
      <ProgrammingDialog
        open={dialog === "programming"}
        onClose={() => setDialog(null)}
        selection={{ track_ids: [track.id] }}
        tracks={1}
      />
    </Card>
  );
}

function getVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || "#888";
}

/** A time typed as `m:ss.t`, or empty for the default. */
function CueInput({
  value,
  placeholder,
  onChange,
  disabled,
}: {
  value: number | null;
  /** Shown when empty; nothing when missing. */
  placeholder?: number;
  onChange: (ms: number | null) => void;
  disabled: boolean;
}) {
  const [text, setText] = useState(value === null ? "" : duration(value, true));
  useEffect(() => {
    setText(value === null ? "" : duration(value, true));
  }, [value]);
  const valid = text.trim() === "" || parseDuration(text) !== null;
  return (
    <input
      className={valid ? "input num" : "input num invalid"}
      value={text}
      placeholder={placeholder === undefined ? undefined : duration(placeholder, true)}
      disabled={disabled}
      aria-invalid={!valid}
      onChange={(event) => setText(event.target.value)}
      onBlur={() => {
        if (text.trim() === "") {
          onChange(null);
        } else {
          const ms = parseDuration(text);
          if (ms !== null) {
            onChange(ms);
          }
        }
      }}
    />
  );
}

/** A hook's start and end, on the file's timeline; both null for none. */
type Hook = { in: number | null; out: number | null };

type Tri = "default" | "on" | "off";
const tri = (value: boolean | null): Tri => (value === null ? "default" : value ? "on" : "off");
const fromTri = (value: Tri): boolean | null => (value === "default" ? null : value === "on");

function OverridesCard({
  track,
  settings,
  draft,
  setDraft,
  canEdit,
  dirty,
  hasOverrides,
  shortTrack,
  cueOut,
  mixPoint,
  intro,
  setIntro,
  hook,
  setHook,
  hookValid,
  playhead,
  onSave,
  onRevert,
  onClear,
  saving,
}: {
  track: TrackDetail;
  settings: PlayoutSettings | undefined;
  draft: TrackOverrides;
  setDraft: (update: (draft: TrackOverrides) => TrackOverrides) => void;
  /** Where the vocals start, on the file's timeline. */
  intro: number | null;
  setIntro: (ms: number | null) => void;
  hook: Hook;
  setHook: (update: (hook: Hook) => Hook) => void;
  /** Both points in order, or none: it can be saved. */
  hookValid: boolean;
  /** Where the preview is, to set the intro there. */
  playhead: number | null;
  canEdit: boolean;
  dirty: boolean;
  hasOverrides: boolean;
  shortTrack: boolean;
  /** The cue-out, with the draft applied. */
  cueOut: number;
  /** Where the next track starts, with the draft applied; null: back to back. */
  mixPoint: number | null;
  onSave: () => void;
  onRevert: () => void;
  onClear: () => void;
  saving: boolean;
}) {
  const disabled = !canEdit;
  const analysis = track.analysis;
  const station = settings?.crossfade;
  const cf = draft.crossfade;
  const setCrossfade = (changes: Partial<TrackOverrides["crossfade"]>) =>
    setDraft((d) => ({ ...d, crossfade: { ...d.crossfade, ...changes } }));
  return (
    <Card
      title={t("track.playout")}
      actions={
        canEdit && (
          <>
            {hasOverrides && !dirty && (
              <button type="button" className="btn small" onClick={onClear}>
                <Icon name="undo" size={14} /> {t("track.useDefaults")}
              </button>
            )}
            {dirty && (
              <button type="button" className="btn small" onClick={onRevert}>
                {t("track.revert")}
              </button>
            )}
            <button type="button" className="btn small primary" disabled={!dirty || saving || !hookValid} onClick={onSave}>
              {t("common.save")}
            </button>
          </>
        )
      }
    >
      <div className="stack">
        <p className="muted small" style={{ margin: 0 }}>
          {rich("track.emptyFields", { link: <Link to="/playout">{t("track.crossfadeSettings")}</Link> })}
        </p>
        <div className="form-grid">
          <Field label={t("track.cueIn")} hint={t("track.analysisValue", { value: duration(analysis?.cue_in_ms ?? 0, true) })}>
            <CueInput
              value={draft.cue_in_ms}
              placeholder={analysis?.cue_in_ms ?? 0}
              disabled={disabled}
              onChange={(ms) => setDraft((d) => ({ ...d, cue_in_ms: ms }))}
            />
          </Field>
          <Field
            label={t("track.cueOut")}
            hint={t("track.analysisValue", { value: duration(analysis?.cue_out_ms ?? track.duration_ms ?? 0, true) })}
          >
            <CueInput
              value={draft.cue_out_ms}
              placeholder={analysis?.cue_out_ms ?? track.duration_ms ?? 0}
              disabled={disabled}
              onChange={(ms) => setDraft((d) => ({ ...d, cue_out_ms: ms }))}
            />
          </Field>
          <Field
            label={t("transition.nextStarts")}
            hint={
              shortTrack && mixPoint === null
                ? t("track.nextStartsShort")
                : mixPoint === null
                  ? t("track.nextStartsNow", { value: t("track.backToBackAt", { at: duration(cueOut, true) }) })
                  : t("track.nextStartsNow", { value: duration(mixPoint, true) })
            }
          >
            <CueInput
              value={
                draft.crossfade?.mode === "fixed" && draft.crossfade.fixed_ms != null && draft.crossfade.crossfade !== false
                  ? Math.max(0, cueOut - draft.crossfade.fixed_ms)
                  : null
              }
              placeholder={mixPoint ?? cueOut}
              disabled={disabled}
              onChange={(ms) =>
                setDraft((d) =>
                  ms === null
                    ? { ...d, crossfade: { ...d.crossfade, mode: null, fixed_ms: null } }
                    : withNextStart(d, ms, cueOut),
                )
              }
            />
          </Field>
          <Field label={t("track.gapKiller")} hint={t("track.gapKillerHelp")}>
            <select
              className="input"
              value={tri(draft.gap_killer_enabled)}
              disabled={disabled}
              onChange={(event) =>
                setDraft((d) => ({ ...d, gap_killer_enabled: fromTri(event.target.value as Tri) }))
              }
            >
              <option value="default">{t("track.stationDefaultOn")}</option>
              <option value="on">{t("track.on")}</option>
              <option value="off">{t("track.off")}</option>
            </select>
          </Field>
          <Field label={t("track.introField")} hint={t("track.introHelp")}>
            <div className="row" style={{ flexWrap: "nowrap" }}>
              <CueInput value={intro} disabled={disabled} onChange={(ms) => setIntro(ms === 0 ? null : ms)} />
              {canEdit && (
                <button
                  type="button"
                  className="btn small"
                  disabled={playhead === null}
                  title={t("track.introAtPlayheadHelp")}
                  onClick={() => playhead !== null && setIntro(Math.max(1, Math.round(playhead / 100) * 100))}
                >
                  {t("track.introAtPlayhead")}
                </button>
              )}
            </div>
          </Field>
        </div>

        <fieldset>
          <legend>{t("track.hook")}</legend>
          <div className="stack">
            <p className="muted small" style={{ margin: 0 }}>
              {t("track.hookHelp")}
            </p>
            <div className="form-grid">
              {(["in", "out"] as const).map((end) => (
                <Field key={end} label={end === "in" ? t("track.hookIn") : t("track.hookOut")}>
                  <div className="row" style={{ flexWrap: "nowrap" }}>
                    <CueInput
                      value={hook[end]}
                      disabled={disabled}
                      onChange={(ms) => setHook((h) => ({ ...h, [end]: ms }))}
                    />
                    {canEdit && (
                      <button
                        type="button"
                        className="btn small"
                        disabled={playhead === null}
                        title={t("track.atPlayheadHelp")}
                        onClick={() =>
                          playhead !== null && setHook((h) => ({ ...h, [end]: Math.round(playhead / 100) * 100 }))
                        }
                      >
                        {t("track.introAtPlayhead")}
                      </button>
                    )}
                  </div>
                </Field>
              ))}
            </div>
            {!hookValid && (
              <p className="small" style={{ margin: 0, color: "var(--bad)" }}>
                {t("track.hookInvalid")}
              </p>
            )}
          </div>
        </fieldset>

        <fieldset>
          <legend>{t("track.intoNext")}</legend>
          <div className="stack">
            {shortTrack && (
              <p className="muted small" style={{ margin: 0 }}>
                {t("track.shortTrack", { duration: duration(station?.min_track_ms ?? 0) })}
              </p>
            )}
            <div className="form-grid">
              <Field label={t("track.crossfade")}>
                <select
                  className="input"
                  value={cf.crossfade === false ? "off" : "default"}
                  disabled={disabled}
                  onChange={(event) =>
                    setCrossfade({ crossfade: event.target.value === "off" ? false : null })
                  }
                >
                  <option value="default">{t("track.crossfade")}</option>
                  <option value="off">{t("track.backToBack")}</option>
                </select>
              </Field>
              <Field label={t("track.nextTrackStarts")}>
                <select
                  className="input"
                  value={cf.mode ?? "default"}
                  disabled={disabled || cf.crossfade === false}
                  onChange={(event) =>
                    setCrossfade({
                      mode: event.target.value === "default" ? null : (event.target.value as "auto" | "fixed"),
                    })
                  }
                >
                  <option value="default">
                    {station?.mode === "fixed" ? t("track.stationDefaultFixed") : t("track.stationDefaultAuto")}
                  </option>
                  <option value="auto">{t("track.modeAuto")}</option>
                  <option value="fixed">{t("track.modeFixed")}</option>
                </select>
              </Field>
              {(cf.mode ?? station?.mode) === "fixed" && (
                <Field label={t("track.beforeCueOut")}>
                  <NumberInput
                    value={(cf.fixed_ms ?? station?.fixed_ms ?? 2000) / 1000}
                    min={0}
                    max={30}
                    step={0.1}
                    suffix="s"
                    disabled={disabled || cf.crossfade === false}
                    onChange={(seconds) => setCrossfade({ fixed_ms: Math.round(seconds * 1000) })}
                  />
                </Field>
              )}
            </div>
            <OverrideFade
              label={t("track.fadeOutOfTrack")}
              kind="out"
              value={cf.fade_out}
              station={station?.fade_out}
              disabled={disabled || cf.crossfade === false}
              onChange={(fade) => setCrossfade({ fade_out: fade })}
            />
          </div>
        </fieldset>
        <fieldset>
          <legend>{t("track.outOfPrevious")}</legend>
          <OverrideFade
            label={t("track.fadeInOfTrack")}
            kind="in"
            value={cf.fade_in}
            station={station?.fade_in}
            disabled={disabled}
            onChange={(fade) => setCrossfade({ fade_in: fade })}
          />
        </fieldset>
      </div>
    </Card>
  );
}

function OverrideFade({
  label,
  kind,
  value,
  station,
  disabled,
  onChange,
}: {
  label: string;
  kind: "in" | "out";
  value: Fade | null;
  station: Fade | undefined;
  disabled: boolean;
  onChange: (fade: Fade | null) => void;
}) {
  return (
    <div className="stack">
      <label className="check">
        <input
          type="checkbox"
          checked={value !== null}
          disabled={disabled || !station}
          onChange={(event) => onChange(event.target.checked && station ? { ...station } : null)}
        />
        {value === null ? t("track.fadeStation", { fade: label }) : t("track.fadeOwn", { fade: label })}
      </label>
      {value !== null && <FadeEditor value={value} onChange={onChange} kind={kind} disabled={disabled} />}
    </div>
  );
}

function CategoriesCard({ track, canEdit }: { track: TrackDetail; canEdit: boolean }) {
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const save = useAction(
    (assigned: { category_id: number; weight: number }[]) =>
      unwrap(api.PUT("/api/v1/tracks/{id}/categories", { params: { path: { id: track.id } }, body: assigned })),
    { invalidate: [["track", track.id], ["categories"]] },
  );
  const current = track.categories.map((c) => ({ category_id: c.category_id, weight: c.weight }));
  const available = (categories.data ?? []).filter(
    (c) => !track.categories.some((assigned) => assigned.category_id === c.id),
  );
  return (
    <Card title={t("nav.categories")} actions={<Link to="/categories" className="btn small">{t("track.manage")}</Link>}>
      <div className="stack">
        {track.categories.length === 0 && (
          <p className="muted" style={{ margin: 0 }}>
            {t("track.noCategory")}
          </p>
        )}
        <ul className="list">
          {track.categories.map((category) => (
            <li key={category.category_id} className="row between" style={{ padding: "0.35rem 0" }}>
              <span className="row">
                <span className="swatch" style={{ background: category.color ?? undefined }} />
                {category.name}
              </span>
              <span className="row">
                <label className="row small muted" title={t("track.weightHelp")}>
                  {t("track.weight")}
                  <span style={{ width: "5rem" }}>
                    <NumberInput
                      value={category.weight}
                      min={0.01}
                      step={0.1}
                      disabled={!canEdit}
                      onChange={(weight) =>
                        save.mutate(
                          current.map((c) =>
                            c.category_id === category.category_id ? { ...c, weight } : c,
                          ),
                        )
                      }
                    />
                  </span>
                </label>
                {canEdit && (
                  <button
                    type="button"
                    className="btn ghost small icon danger"
                    aria-label={t("track.removeFrom", { category: category.name })}
                    onClick={() =>
                      save.mutate(current.filter((c) => c.category_id !== category.category_id))
                    }
                  >
                    <Icon name="close" />
                  </button>
                )}
              </span>
            </li>
          ))}
        </ul>
        {canEdit && available.length > 0 && (
          <label>
            <span className="visually-hidden">{t("track.addCategory")}</span>
            <select
              className="input"
              value=""
              onChange={(event) =>
                save.mutate([...current, { category_id: Number(event.target.value), weight: 1 }])
              }
            >
              <option value="">{t("track.addCategoryOption")}</option>
              {available.map((category) => (
                <option key={category.id} value={category.id}>
                  {category.name}
                </option>
              ))}
            </select>
          </label>
        )}
        {canEdit && (
          <NewCategory
            onCreated={(created) => save.mutate([...current, { category_id: created.id, weight: 1 }])}
          />
        )}
      </div>
    </Card>
  );
}

function DetailsCard({ track }: { track: TrackDetail }) {
  const analysis = track.analysis;
  return (
    <Card title={t("track.details")}>
      <dl className="facts small">
        <dt>{t("track.loudness")}</dt>
        <dd>{analysis ? db(analysis.integrated_lufs, "LUFS") : t("track.notAnalyzed")}</dd>
        <dt>{t("track.truePeak")}</dt>
        <dd>{analysis ? db(analysis.true_peak_db, "dBTP") : "–"}</dd>
        <dt>{t("track.normalization")}</dt>
        <dd>{db(track.playout.gain_db)}</dd>
        {analysis?.has_internal_silence && (
          <>
            <dt>{t("track.silence")}</dt>
            <dd className="warn">{t("track.internalSilence")}</dd>
          </>
        )}
        <dt>{t("track.analyzed")}</dt>
        <dd>{analysis ? dateTime(analysis.analyzed_at_unix_ms) : "–"}</dd>
        <dt>{t("library.length")}</dt>
        <dd className="num">{duration(track.duration_ms, true)}</dd>
        <dt>{t("track.format")}</dt>
        <dd>
          {bitrate(track)}
          {track.file.sample_rate && ` · ${t("track.kilohertz", { rate: track.file.sample_rate / 1000 })}`}
          {track.file.channels &&
            ` · ${
              track.file.channels === 1
                ? t("track.mono")
                : track.file.channels === 2
                  ? t("track.stereo")
                  : t("track.channels", { count: track.file.channels })
            }`}
        </dd>
        <dt>{t("track.size")}</dt>
        <dd>{bytes(track.file.size)}</dd>
        <dt>{t("track.added")}</dt>
        <dd>{dateTime(track.added_at_unix_ms)}</dd>
        <dt>{t("track.identification")}</dt>
        <dd>
          {track.identification?.status === "identified" && track.identification.release_mbid ? (
            <a
              href={`https://musicbrainz.org/release/${track.identification.release_mbid}`}
              target="_blank"
              rel="noreferrer"
            >
              {t("track.musicbrainzRelease")}
            </a>
          ) : track.identification?.status === "needs_review" ? (
            <span className="warn">{t("library.filter.needsReview")}</span>
          ) : track.identification?.status === "kept" ? (
            t("track.reviewedOwnTags")
          ) : (
            t("track.ownTags")
          )}
        </dd>
      </dl>
    </Card>
  );
}

function PlaysCard({ track }: { track: TrackDetail }) {
  const trackId = track.id;
  const plays = useQuery({
    queryKey: ["history", "track", trackId],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/history", { params: { path: here(), query: { track_id: trackId, limit: 10 } } })),
  });
  return (
    <Card
      title={t("track.recentPlays")}
      flush
      actions={
        <span className="muted small">
          {track.last_played_at_unix_ms
            ? t("track.playCountLast", { count: track.play_count, ago: ago(track.last_played_at_unix_ms) })
            : t("track.playCount", { count: track.play_count })}
        </span>
      }
    >
      <div style={{ padding: "0.5rem 1rem 0" }}>
        <PlaysByStation plays={track.plays} />
      </div>
      {plays.isError && <ErrorView error={plays.error} />}
      {plays.data?.length === 0 && <Empty>{t("track.neverPlayed")}</Empty>}
      <ul className="list">
        {plays.data?.map((play) => (
          <li key={play.id} className="queue-item">
            <span className="meta">{dateTime(play.started_at_unix_ms)}</span>
            <span className="muted small">
              {play.ended_at_unix_ms ? endingLabel(play.end_reason) : t("track.playing")}
              {play.played_ms ? ` · ${duration(play.played_ms)}` : ""} · {ago(play.started_at_unix_ms)}
            </span>
          </li>
        ))}
      </ul>
    </Card>
  );
}
