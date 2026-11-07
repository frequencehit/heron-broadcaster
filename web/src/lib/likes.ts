// SPDX-License-Identifier: AGPL-3.0-or-later

// Likes (docs/listener-interaction.md#likes): listeners like or dislike the
// track playing. The score is the share of likes among the votes, and a
// track is ranked from 10 votes.

import { number } from "../i18n";

/** Votes a track needs to be ranked. */
export const MIN_VOTES = 10;

/** The share of likes, 0 to 1; `null` without votes. */
export function likeShare(likes: number, dislikes: number): number | null {
  const votes = likes + dislikes;
  return votes > 0 ? likes / votes : null;
}

/** Whether the votes are enough to rank the track. */
export function isRanked(likes: number, dislikes: number): boolean {
  return likes + dislikes >= MIN_VOTES;
}

/** `79 %`, in the current language. */
export function shareLabel(share: number): string {
  return number(share, { style: "percent", maximumFractionDigits: 0 });
}

/** The filters of the track list on the share of likes. */
export type LikedFilter = "high" | "low";

const LIKED_FILTERS: Record<LikedFilter, { liked_min?: number; liked_max?: number; votes_min: number }> = {
  // 70 % and more.
  high: { liked_min: 70, votes_min: MIN_VOTES },
  // Under 40 %: the tracks to rest.
  low: { liked_max: 39, votes_min: MIN_VOTES },
};

/** The API's filters of `liked` in the URL, and its period (`likes_days`). */
export function likedFilter(
  value: string | null,
  days: string | null,
): { liked_min?: number; liked_max?: number; votes_min?: number; likes_days?: number } {
  const filter = value && Object.hasOwn(LIKED_FILTERS, value) ? LIKED_FILTERS[value as LikedFilter] : {};
  const period = Number(days);
  return LIKE_PERIODS.includes(period) ? { ...filter, likes_days: period } : filter;
}

/** The periods of the likes in the track list, in days; 30 by default. */
export const LIKE_PERIODS = [7, 30, 365];
export const DEFAULT_LIKE_PERIOD = 30;
