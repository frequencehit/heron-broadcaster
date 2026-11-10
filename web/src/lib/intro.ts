// SPDX-License-Identifier: AGPL-3.0-or-later

// Intros: where the vocals start in a track, and where a jingle played over
// the intro starts (as the engine places it: ending where the vocals start).

/** What these need of a queued track. */
export interface IntroTrack {
  cue_in_ms: number;
  cue_out_ms: number;
  intro_ms?: number | null;
}

/** Where the vocals start, from the cue-in (deck positions count from there); null when unknown. */
export function introFromCueIn(track: IntroTrack): number | null {
  if (track.intro_ms == null) {
    return null;
  }
  const intro = track.intro_ms - track.cue_in_ms;
  return intro > 0 && intro <= track.cue_out_ms - track.cue_in_ms ? intro : null;
}

/** Where `jingle` starts over `song`'s intro, from the song's cue-in; null when it does not fit. */
export function jingleStart(song: IntroTrack, jingle: IntroTrack): number | null {
  const intro = introFromCueIn(song);
  if (intro === null) {
    return null;
  }
  const start = intro - (jingle.cue_out_ms - jingle.cue_in_ms);
  return start >= 0 ? start : null;
}
