// SPDX-License-Identifier: AGPL-3.0-or-later

// Cover art: thumbnails with a neutral placeholder, a track's cover with
// where it comes from and how to change it, and the Cover Art Archive's
// candidates (docs/web.md#cover-art).

import { useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { ApiError, type Schemas, type TrackDetail, api, done, unwrap } from "../api";
import { type PlainKey, t } from "../i18n";
import { useAction, useCan } from "../lib/session";
import { TaskState, useTask } from "./Task";
import { Card, ConfirmButton, Icon, Segmented, useToast } from "./ui";

export type CoverScope = Schemas["CoverScope"];
export type CoverCandidate = Schemas["CoverCandidate"];

/** The address of a cover at a size: `cover_url` is the 600 px one. */
export function coverSrc(url: string | null | undefined, small: boolean): string | null {
  if (!url) {
    return null;
  }
  return small ? `${url}${url.includes("?") ? "&" : "?"}size=96` : url;
}

/** An image, or the neutral placeholder when there is none or it fails. */
export function CoverImage({
  src,
  px,
  alt = "",
  className,
}: Readonly<{ src: string | null; px: number; alt?: string; className?: string }>) {
  const [failed, setFailed] = useState<string | null>(null);
  const style = { width: px, height: px };
  const classes = ["cover-art", className].filter(Boolean).join(" ");
  if (!src || failed === src) {
    return (
      <span className={`${classes} placeholder`} style={style} role={alt ? "img" : undefined} aria-label={alt || undefined}>
        <Icon name="image" size={Math.max(12, Math.round(px / 2.6))} />
      </span>
    );
  }
  return (
    <img
      className={classes}
      style={style}
      src={src}
      alt={alt}
      loading="lazy"
      decoding="async"
      onError={() => setFailed(src)}
    />
  );
}

/** A track's thumbnail from its `cover_url`. */
export function CoverThumb({ url, px = 32 }: Readonly<{ url: string | null | undefined; px?: number }>) {
  return <CoverImage src={coverSrc(url, true)} px={px} />;
}

/** The thumbnail of a track known by its ID only (decks, queue). */
export function TrackCoverThumb({ trackId, px = 40 }: Readonly<{ trackId: number | null | undefined; px?: number }>) {
  const src = trackId == null ? null : `/api/v1/tracks/${trackId}/cover?size=96`;
  return <CoverImage src={src} px={px} />;
}

/** Sends an image file, or an image address, to a route that takes either. */
export async function sendImage(url: string, image: File | string): Promise<Response> {
  const response = await fetch(url, {
    method: "PUT",
    credentials: "same-origin",
    headers: {
      "X-Requested-With": "heron-web",
      "Content-Type": typeof image === "string" ? "application/json" : image.type || "application/octet-stream",
    },
    body: typeof image === "string" ? JSON.stringify({ url: image }) : image,
  });
  if (!response.ok) {
    let message = t("cover.notSaved");
    let code = "http";
    try {
      const body = (await response.json()) as { message?: string; error?: string };
      message = body.message ?? message;
      code = body.error ?? code;
    } catch {
      // Not JSON: the generic message.
    }
    throw new ApiError(response.status, code, message);
  }
  return response;
}

const SOURCES: Record<Schemas["CoverSource"], PlainKey> = {
  hand: "cover.source.hand",
  embedded: "cover.source.embedded",
  file: "cover.source.file",
  cover_art_archive: "cover.source.coverArtArchive",
};

/** Where a cover comes from, for people. */
export function coverSource(cover: Schemas["TrackCover"]): string {
  if (cover.source === "file" && cover.detail) {
    return cover.detail;
  }
  return t(SOURCES[cover.source]);
}

/** Polls a library task and calls `onDone` once it is done. */
function useTaskDone(onDone: () => void) {
  const [taskId, setTaskId] = useState<number | null>(null);
  const task = useTask(taskId);
  const toast = useToast();
  const finished = useRef<number | null>(null);
  const status = task.data?.status;
  useEffect(() => {
    if (taskId === null || finished.current === taskId) {
      return;
    }
    if (status === "done") {
      finished.current = taskId;
      onDone();
    } else if (status === "failed") {
      finished.current = taskId;
      toast(task.data?.error ?? t("cover.notSaved"), true);
    }
  }, [taskId, status, task.data?.error, onDone, toast]);
  const busy = taskId !== null && status !== "done" && status !== "failed";
  return { start: setTaskId, task: task.data, busy };
}

/** Candidate releases, each with its cover, to pick one. */
export function CoverCandidates({
  candidates,
  trackId,
  onAccepted,
  extra,
}: Readonly<{
  candidates: CoverCandidate[];
  /** Given: for this track only. */
  trackId?: number | undefined;
  onAccepted: () => void;
  extra?: ReactNode;
}>) {
  const canEdit = useCan("operator");
  const { start, task, busy } = useTaskDone(onAccepted);
  const accept = useAction(
    (candidate: CoverCandidate) =>
      unwrap(
        api.POST("/api/v1/covers/candidates/{id}/accept", {
          params: { path: { id: candidate.id } },
          body: trackId === undefined ? {} : { track_id: trackId },
        }),
      ),
    { onSuccess: (queued) => start(queued.id) },
  );
  return (
    <div className="stack">
      <ul className="cover-candidates">
        {candidates.map((candidate) => (
          <li key={candidate.id}>
            <CoverImage src={candidate.thumbnail_url} px={96} alt={candidate.title ?? ""} />
            <div className="stack" style={{ gap: "0.15rem", minWidth: 0 }}>
              <strong className="ellipsis">{candidate.title ?? candidate.release_id}</strong>
              <span className="small ellipsis">{candidate.artist}</span>
              <span className="small muted">
                {[candidate.date, candidate.country].filter(Boolean).join(" · ")}
              </span>
              <span className="small muted">{t("cover.score", { score: candidate.score })}</span>
              {canEdit && (
                <button
                  type="button"
                  className="btn small"
                  disabled={busy || accept.isPending}
                  onClick={() => accept.mutate(candidate)}
                >
                  {t("cover.useThis")}
                </button>
              )}
            </div>
          </li>
        ))}
      </ul>
      <div className="row">
        {extra}
        <TaskState task={task} />
      </div>
    </div>
  );
}

/** A track's cover on its page: where it comes from, and how to change it. */
export function CoverCard({ track }: Readonly<{ track: TrackDetail }>) {
  const canEdit = useCan("operator");
  const client = useQueryClient();
  const toast = useToast();
  const file = useRef<HTMLInputElement>(null);
  const [scope, setScope] = useState<CoverScope>(track.cover?.scope ?? "album");
  const [address, setAddress] = useState<string | null>(null);
  const refresh = () => {
    void client.invalidateQueries({ queryKey: ["track", track.id] });
    void client.invalidateQueries({ queryKey: ["tracks"] });
    void client.invalidateQueries({ queryKey: ["cover-candidates"] });
  };
  // No album: only a cover of its own.
  const noAlbum = !track.album && !track.file.musicbrainz_release_id;
  const chosen: CoverScope = noAlbum ? "track" : scope;
  const coverUrl = `/api/v1/tracks/${track.id}/cover?scope=${chosen}`;
  const upload = useAction((image: File | string) => sendImage(coverUrl, image), {
    success: t("cover.saved"),
    onSuccess: () => {
      setAddress(null);
      refresh();
    },
  });
  const remove = useAction(
    () =>
      done(
        api.DELETE("/api/v1/tracks/{id}/cover", {
          params: { path: { id: track.id }, query: { scope: track.cover?.scope ?? chosen } },
        }),
      ),
    { success: t("cover.removed"), onSuccess: refresh },
  );
  const search = useTaskDone(refresh);
  const ask = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/covers/fetch", {
          body: { selection: { track_ids: [track.id] }, candidates: true },
        }),
      ),
    { onSuccess: (queued) => search.start(queued.id) },
  );
  useEffect(() => {
    if (search.task?.status === "done" && search.task.covers?.candidates === 0) {
      toast(t("cover.nothingFound"));
    }
  }, [search.task?.status, search.task?.covers?.candidates, toast]);

  const cover = track.cover;
  return (
    <Card title={t("cover.title")}>
      <div className="stack">
        <div className="row" style={{ alignItems: "flex-start", gap: "1rem" }}>
          <CoverImage
            src={cover ? cover.url : null}
            px={160}
            alt={cover ? t("cover.alt", { album: track.album ?? track.title ?? "" }) : t("cover.none")}
            className="large"
          />
          <dl className="details" style={{ margin: 0, minWidth: 0 }}>
            {cover ? (
              <>
                <dt>{t("cover.from")}</dt>
                <dd className="ellipsis">{coverSource(cover)}</dd>
                <dt>{t("cover.for")}</dt>
                <dd>{cover.scope === "album" ? t("cover.forAlbum") : t("cover.forTrack")}</dd>
                <dt>{t("cover.size")}</dt>
                <dd>
                  {cover.width} × {cover.height}
                </dd>
              </>
            ) : (
              <dd className="muted">{t("cover.none")}</dd>
            )}
          </dl>
        </div>
        {canEdit && !track.deleted_at_unix_ms && (
          <>
            {!noAlbum && (
              <Segmented<CoverScope>
                label={t("cover.for")}
                value={scope}
                onChange={setScope}
                options={[
                  { value: "album", label: t("cover.forAlbum") },
                  { value: "track", label: t("cover.forTrackOnly") },
                ]}
              />
            )}
            <div className="row">
              <input
                ref={file}
                type="file"
                accept="image/jpeg,image/png,image/webp"
                hidden
                onChange={(e) => {
                  const picked = e.target.files?.[0];
                  e.target.value = "";
                  if (picked) {
                    upload.mutate(picked);
                  }
                }}
              />
              <button type="button" className="btn small" disabled={upload.isPending} onClick={() => file.current?.click()}>
                <Icon name="upload" /> {t("cover.upload")}
              </button>
              <button type="button" className="btn small" onClick={() => setAddress(address === null ? "" : null)}>
                <Icon name="link" /> {t("cover.fromAddress")}
              </button>
              <button
                type="button"
                className="btn small"
                disabled={search.busy || ask.isPending || noAlbum}
                title={noAlbum ? t("cover.searchNeedsAlbum") : undefined}
                onClick={() => ask.mutate(undefined)}
              >
                <Icon name="search" /> {t("cover.search")}
              </button>
              {cover && (
                <ConfirmButton
                  className="btn small danger"
                  confirm={t("cover.remove")}
                  question={cover.scope === "album" ? t("cover.removeAlbumQuestion") : t("cover.removeTrackQuestion")}
                  onConfirm={() => remove.mutate(undefined)}
                >
                  {t("cover.remove")}
                </ConfirmButton>
              )}
              <TaskState task={search.task} />
            </div>
            {address !== null && (
              <form
                className="row"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (address.trim()) {
                    upload.mutate(address.trim());
                  }
                }}
              >
                <label style={{ flex: "1 1 16rem" }}>
                  <span className="visually-hidden">{t("cover.address")}</span>
                  <input
                    className="input"
                    type="url"
                    placeholder="https://…"
                    value={address}
                    onChange={(e) => setAddress(e.target.value)}
                  />
                </label>
                <button type="submit" className="btn small primary" disabled={!address.trim() || upload.isPending}>
                  {t("cover.fetch")}
                </button>
              </form>
            )}
            <span className="muted small">{t("cover.uploadHint")}</span>
          </>
        )}
        {track.cover_candidates.length > 0 && (
          <>
            <h3 className="small" style={{ margin: 0 }}>
              {t("cover.candidates")}
            </h3>
            <CoverCandidates
              candidates={track.cover_candidates}
              trackId={chosen === "track" ? track.id : undefined}
              onAccepted={refresh}
            />
          </>
        )}
      </div>
    </Card>
  );
}
