// SPDX-License-Identifier: AGPL-3.0-or-later

// Listeners' likes (docs/listener-interaction.md#likes): the share of likes
// with its bar, the votes on air, a track's votes, the week's most and
// least liked tracks. Hidden while the module is off.

import { useQuery } from "@tanstack/react-query";
import { api, unwrap } from "../api";
import { number, t } from "../i18n";
import { dateTime } from "../lib/format";
import { MIN_VOTES, isRanked, likeShare, shareLabel } from "../lib/likes";
import { useModule } from "../lib/modules";
import { Link } from "../lib/router";
import { here, useStation } from "../lib/station";
import { TopRanks } from "./Tops";
import { Card, Empty, ErrorView, Icon, Loading } from "./ui";

/** Likes in green, dislikes in red; grey without votes. */
export function LikeBar({ likes, dislikes }: Readonly<{ likes: number; dislikes: number }>) {
  const share = likeShare(likes, dislikes);
  return (
    <span className={share === null ? "like-bar empty" : "like-bar"} aria-hidden="true">
      {share !== null && <i style={{ width: `${share * 100}%` }} />}
    </span>
  );
}

/** `79 % 611 votes`, or the votes alone below the minimum. */
export function LikedCell({ likes, dislikes }: Readonly<{ likes: number; dislikes: number }>) {
  const share = likeShare(likes, dislikes);
  const votes = likes + dislikes;
  if (share === null) {
    return <span className="muted">–</span>;
  }
  const title = t("likes.counts", { likes: number(likes), dislikes: number(dislikes) });
  return (
    <span className="liked" title={title}>
      <LikeBar likes={likes} dislikes={dislikes} />
      {isRanked(likes, dislikes) ? (
        <>
          <strong className="num">{shareLabel(share)}</strong>
          <span className="muted small">{t("likes.votes", { count: votes })}</span>
        </>
      ) : (
        <span className="muted small">{t("likes.notRanked", { count: votes })}</span>
      )}
    </span>
  );
}

/** The votes on the track playing, as they come. */
export function NowLikes() {
  const on = useModule("likes");
  const station = useStation();
  const now = useQuery({
    queryKey: ["likes", "current", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/likes/current", { params: { path: here() } })),
    enabled: on && station !== null,
    refetchInterval: 5_000,
  });
  if (!on || !now.data) {
    return null;
  }
  const { likes, dislikes, play_id } = now.data;
  return (
    <Card title={t("likes.onAir")}>
      {play_id === null || play_id === undefined ? (
        <p className="muted small">{t("likes.nothingToVote")}</p>
      ) : (
        <div className="stack" style={{ gap: 6 }}>
          <div className="row" style={{ gap: 16 }}>
            <span className="row">
              <Icon name="like" /> <strong className="num">{number(likes)}</strong>
            </span>
            <span className="row">
              <Icon name="dislike" /> <strong className="num">{number(dislikes)}</strong>
            </span>
            {likes + dislikes > 0 && <span className="muted">{shareLabel(likeShare(likes, dislikes) ?? 0)}</span>}
          </div>
          <LikeBar likes={likes} dislikes={dislikes} />
        </div>
      )}
    </Card>
  );
}

/** A track's votes on the station shown: this week, 30 days, always, and per play. */
export function TrackLikesCard({ trackId }: Readonly<{ trackId: number }>) {
  const on = useModule("likes");
  const station = useStation();
  const likes = useQuery({
    queryKey: ["likes", "track", station, trackId],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/likes/tracks/{id}", { params: { path: here({ id: trackId }) } })),
    enabled: on && station !== null,
  });
  if (!on) {
    return null;
  }
  return (
    <div className="grid-2">
      <Card title={t("likes.opinion")}>
        {likes.isPending ? (
          <Loading />
        ) : likes.isError ? (
          <ErrorView error={likes.error} />
        ) : (
          <>
            <div className="like-periods">
              {(
                [
                  ["likes.thisWeek", likes.data.week],
                  ["likes.days30", likes.data.days_30],
                  ["likes.always", likes.data.total],
                ] as const
              ).map(([label, votes]) => {
                const share = likeShare(votes.likes, votes.dislikes);
                return (
                  <div key={label} className="like-period">
                    <span className="muted small">{t(label)}</span>
                    <strong className="like-share">{share === null ? "–" : shareLabel(share)}</strong>
                    <span className="muted small">{t("likes.votes", { count: votes.likes + votes.dislikes })}</span>
                    <LikeBar likes={votes.likes} dislikes={votes.dislikes} />
                  </div>
                );
              })}
            </div>
            <p className="muted small">{t("likes.opinionHelp", { count: likes.data.min_votes })}</p>
            <TopRanks trackId={trackId} />
          </>
        )}
      </Card>
      <Card title={t("likes.perPlay")}>
        {likes.data && likes.data.plays.length === 0 ? (
          <Empty>{t("likes.noVotes")}</Empty>
        ) : (
          <ul className="like-plays">
            {likes.data?.plays.map((play) => (
              <li key={play.play_id}>
                <span className="small">{dateTime(play.started_at_unix_ms)}</span>
                <LikeBar likes={play.likes} dislikes={play.dislikes} />
                <span className="num small" title={t("likes.counts", { likes: number(play.likes), dislikes: number(play.dislikes) })}>
                  <strong>{number(play.likes)}</strong> · {number(play.dislikes)}
                </span>
              </li>
            ))}
          </ul>
        )}
        <p className="muted small">{t("likes.perPlayHelp")}</p>
      </Card>
    </div>
  );
}

/** The week's most and least liked tracks of the station shown. */
export function LikesWeekCard() {
  const on = useModule("likes");
  const station = useStation();
  const week = useQuery({
    queryKey: ["likes", "week", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/likes/week", { params: { path: here() } })),
    enabled: on && station !== null,
    refetchInterval: 60_000,
  });
  if (!on) {
    return null;
  }
  return (
    <Card title={t("likes.week")}>
      {week.isPending ? (
        <Loading />
      ) : week.isError ? (
        <ErrorView error={week.error} />
      ) : (
        <>
          <p className="muted small">
            {t("likes.weekHelp", { date: dateTime(week.data.from_unix_ms), count: week.data.min_votes ?? MIN_VOTES })}
          </p>
          <div className="grid-2">
            {(
              [
                ["likes.mostLiked", week.data.most_liked],
                ["likes.leastLiked", week.data.least_liked],
              ] as const
            ).map(([label, tracks]) => (
              <div key={label} className="stack" style={{ gap: 4 }}>
                <h3 className="small">{t(label)}</h3>
                {tracks.length === 0 ? (
                  <span className="muted small">{t("likes.noneRanked")}</span>
                ) : (
                  <ol className="like-ranking">
                    {tracks.map((track) => (
                      <li key={track.track_id}>
                        <Link to={`/library/${track.track_id}`}>
                          {[track.artist, track.title].filter(Boolean).join(" – ") || `#${track.track_id}`}
                        </Link>
                        <LikedCell likes={track.likes} dislikes={track.dislikes} />
                      </li>
                    ))}
                  </ol>
                )}
              </div>
            ))}
          </div>
        </>
      )}
    </Card>
  );
}
