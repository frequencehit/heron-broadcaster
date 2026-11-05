// SPDX-License-Identifier: AGPL-3.0-or-later

// Medleys (docs/medleys.md): what the web shows before a render.

import { t } from "../i18n";
import { timeZone } from "./format";

/** The longest medley name, as the API cuts it (heron_core::medley::NAME_CHARS). */
const NAME_CHARS = 249;

/**
 * The name a medley gets at its render, as the API gives it: `Medley
 * 2026-10-29 (A - B - C)`, each artist (or title) once, the list cut with `…`
 * when too long. The API may add ` (2)` when another medley has it.
 */
export function medleyName(date: string, names: readonly (string | null | undefined)[]): string {
  const base = `Medley ${date}`;
  const seen = new Set<string>();
  const unique: string[] = [];
  for (const raw of names) {
    const name = (raw ?? "").trim();
    if (name && !seen.has(name.toLowerCase())) {
      seen.add(name.toLowerCase());
      unique.push(name);
    }
  }
  if (unique.length === 0) {
    return base;
  }
  const length = (text: string) => [...text].length;
  for (let count = unique.length; count > 0; count -= 1) {
    const list = unique.slice(0, count).join(" - ") + (count < unique.length ? " - …" : "");
    const name = `${base} (${list})`;
    if (length(name) <= NAME_CHARS) {
      return name;
    }
  }
  const room = NAME_CHARS - length(base) - length(" (…)");
  return `${base} (${[...(unique[0] ?? "")].slice(0, room).join("")}…)`;
}

/** The day of a date in the station's time zone: `2026-10-29`. */
export function renderDay(date: Date, zone: string | undefined = timeZone()): string {
  const format = { year: "numeric", month: "2-digit", day: "2-digit", timeZone: zone } as const;
  const parts = new Intl.DateTimeFormat("en", format).formatToParts(date);
  const part = (type: string) => parts.find((p) => p.type === type)?.value ?? "";
  return `${part("year")}-${part("month")}-${part("day")}`;
}

/** A medley's name; a stand-in until its first render names it. */
export function displayName(medley: { id: number; name?: string | null }): string {
  return medley.name ?? t("medleys.unnamed", { id: String(medley.id) });
}

/** Where the liner at `index` goes: before every hook, after every hook, or between. */
export function linerSlot(parts: readonly { kind: string }[], index: number): "start" | "between" | "end" {
  if (!parts.slice(0, index).some((part) => part.kind === "hook")) {
    return "start";
  }
  if (!parts.slice(index + 1).some((part) => part.kind === "hook")) {
    return "end";
  }
  return "between";
}

/** The music goes down under a liner, and back up, over this (DUCK_RAMP_MS). */
export const DUCK_RAMP_MS = 150;
/** Every part fades in and out over at least this (EDGE_FADE_MS). */
const EDGE_FADE_MS = 20;

export type FadeCurve = "linear" | "fast_then_slow" | "slow_then_fast" | "s_curve";

/** Where the music goes down, or comes back up, around a liner over it (DuckPlace). */
export type DuckPlace = "outside" | "across" | "inside";

/** How the music goes down under liners over it, and comes back up (Ducking). */
export interface Ducking {
  attack_ms: number;
  attack_at: DuckPlace;
  release_ms: number;
  release_at: DuckPlace;
}

/** How much of the going down is before the liner. */
const duckShare = (at: string, ms: number) => {
  if (at === "outside") return ms;
  return at === "across" ? Math.trunc(ms / 2) : 0;
};

export const duckBefore = (ducking: Ducking) => duckShare(ducking.attack_at, ducking.attack_ms);

/** How much of the coming back up is after the liner. */
export const duckAfter = (ducking: Ducking) => duckShare(ducking.release_at, ducking.release_ms);

/**
 * For liners from `start` to `stop`: where the music starts going down, is
 * down, starts coming back up and is back (Ducking::ramps). What is inside
 * the liners never crosses: when they are too short, the two share them.
 */
export function duckRamps(ducking: Ducking, start: number, stop: number): [number, number, number, number] {
  const downFrom = start - duckBefore(ducking);
  let downTo = downFrom + ducking.attack_ms;
  const upTo = stop + duckAfter(ducking);
  let upFrom = upTo - ducking.release_ms;
  const insideDown = Math.max(0, downTo - start);
  const insideUp = Math.max(0, stop - upFrom);
  const room = Math.max(0, stop - start);
  if (insideDown + insideUp > room && insideDown + insideUp > 0) {
    const scale = room / (insideDown + insideUp);
    downTo = Math.min(downTo, start + insideDown * scale);
    upFrom = Math.max(upFrom, stop - insideUp * scale);
  }
  return [downFrom, Math.max(downTo, downFrom), Math.min(upFrom, upTo), upTo];
}

/** A fade-out's gain `t` of the way through it (FadeCurve::gain): 1 down to 0. */
export function fadeGain(curve: FadeCurve, t: number): number {
  const x = Math.min(1, Math.max(0, t));
  switch (curve) {
    case "fast_then_slow":
      return (1 - x) * (1 - x);
    case "slow_then_fast":
      return 1 - x * x;
    case "s_curve":
      return 0.5 * (1 + Math.cos(Math.PI * x));
    default:
      return 1 - x;
  }
}

export interface LayoutPart {
  kind: string;
  from_ms: number;
  to_ms: number;
  shift_ms?: number;
  /** How far it was moved from where the recipe puts it, negative for earlier. */
  move_ms?: number;
  /** The file's length, which extensions stop at. */
  duration_ms?: number | null;
}

export interface Placed {
  kind: string;
  /** Where it starts in the medley. */
  at_ms: number;
  /** The span of its file played, extensions under liners included. */
  from_ms: number;
  to_ms: number;
  /** A liner over the music. */
  over: boolean;
  /** Its own fades, at the edges and over the crossfades. */
  fade_in_ms: number;
  fade_out_ms: number;
}

export interface Layout {
  parts: Placed[];
  length_ms: number;
  /** Where the music starts and ends: the fade-in starts, the fade-out ends there. */
  music_start_ms?: number;
  music_end_ms: number;
  fade_in_ms?: number;
  fade_in_curve?: FadeCurve;
  fade_out_ms?: number;
  fade_out_curve?: FadeCurve;
  duck_db?: number;
  /** A layout saved before these were set: down over 0.15 s before the liners, back up during them. */
  duck_attack_ms?: number;
  duck_attack_at?: DuckPlace;
  duck_release_ms?: number;
  duck_release_at?: DuckPlace;
}

/** How the music went down and came back up in `medley`. */
export const layoutDucking = (medley: Layout): Ducking => ({
  attack_ms: medley.duck_attack_ms ?? DUCK_RAMP_MS,
  attack_at: medley.duck_attack_at ?? "outside",
  release_ms: medley.duck_release_ms ?? 0,
  release_at: medley.duck_release_at ?? "inside",
});

interface MixRecipe {
  crossfade_ms: number;
  liner_mode: string;
  fade_in_ms?: number;
  fade_in_curve?: FadeCurve;
  fade_out_ms?: number;
  fade_out_curve?: FadeCurve;
  duck_db?: number;
  duck_attack_ms?: number;
  duck_attack_at?: DuckPlace;
  duck_release_ms?: number;
  duck_release_at?: DuckPlace;
}

/** How a recipe makes the music go down: by default during the liners over 0.3 s, back up across their end over 0.3 s. */
const recipeDucking = (recipe: MixRecipe): Ducking => ({
  attack_ms: recipe.duck_attack_ms ?? 300,
  attack_at: recipe.duck_attack_at ?? "inside",
  release_ms: recipe.duck_release_ms ?? 300,
  release_at: recipe.duck_release_at ?? "across",
});

const end = (placed: Placed) => placed.at_ms + placed.to_ms - placed.from_ms;

/**
 * The medley as it will sound, as the librarian mixes it
 * (heron_core::medley::layout): parts overlap by the crossfade; liners over
 * the music fall in gaps made by the hooks around them playing longer.
 */
export function layout(parts: readonly LayoutPart[], recipe: MixRecipe): Layout {
  const crossfade = recipe.crossfade_ms;
  const isOver = (part: LayoutPart) => recipe.liner_mode === "over" && part.kind === "liner";
  const sequence: number[] = [];
  const gaps: number[][] = [[]];
  parts.forEach((part, index) => {
    if (isOver(part)) {
      gaps.at(-1)?.push(index);
    } else {
      sequence.push(index);
      gaps.push([]);
    }
  });
  const earlier = parts.map(() => 0);
  const later = parts.map(() => 0);
  const roomAfter = (index: number) => {
    const part = parts[index];
    return part?.duration_ms != null ? Math.max(0, part.duration_ms - part.to_ms) : Number.POSITIVE_INFINITY;
  };
  const length = (index: number) => {
    const part = parts[index];
    return part ? part.to_ms - part.from_ms : 0;
  };
  const count = sequence.length;
  // What the going down takes before the liners, and the coming back up
  // after them: the hooks' spans stay clear of both.
  const ducking = recipeDucking(recipe);
  const pre = duckBefore(ducking);
  const post = duckAfter(ducking);
  gaps.forEach((liners, gap) => {
    const first = liners[0];
    if (first === undefined || count === 0) {
      return;
    }
    const voice = liners.reduce((sum, index) => sum + length(index), 0);
    if (gap === 0) {
      const hook = sequence[0] ?? 0;
      // The span starts once the music is back up.
      earlier[hook] = Math.min(voice + post, parts[hook]?.from_ms ?? 0);
    } else if (gap === count) {
      const hook = sequence[count - 1] ?? 0;
      later[hook] = Math.min(voice + pre, roomAfter(hook));
    } else {
      const before = sequence[gap - 1] ?? 0;
      const after = sequence[gap] ?? 0;
      const total = voice + crossfade + pre + post;
      const shift = parts[first]?.shift_ms ?? 0;
      // Centred on the liners, whatever the ducking takes before and after them.
      let on = Math.min(Math.max(Math.trunc((total + pre - post) / 2) + shift, 0), total);
      let ahead = total - on;
      const roomOn = roomAfter(before);
      const roomAhead = parts[after]?.from_ms ?? 0;
      if (on > roomOn) {
        ahead += on - roomOn;
        on = roomOn;
      }
      if (ahead > roomAhead) {
        on = Math.min(on + (ahead - roomAhead), roomOn);
        ahead = roomAhead;
      }
      later[before] = on;
      earlier[after] = ahead;
    }
  });
  const placed: (Placed | undefined)[] = parts.map(() => undefined);
  let cursor = 0;
  let previous: Placed | undefined;
  for (const index of sequence) {
    const part = parts[index];
    if (!part) {
      continue;
    }
    const item: Placed = {
      kind: part.kind,
      at_ms: 0,
      from_ms: part.from_ms - (earlier[index] ?? 0),
      to_ms: part.to_ms + (later[index] ?? 0),
      over: false,
      fade_in_ms: EDGE_FADE_MS,
      fade_out_ms: EDGE_FADE_MS,
    };
    if (previous) {
      // Overlaps the part before by the crossfade, less its move: from 0 to the shorter of the two.
      const longest = Math.min(item.to_ms - item.from_ms, previous.to_ms - previous.from_ms);
      const overlap = Math.min(Math.max(crossfade - (part.move_ms ?? 0), 0), longest);
      item.at_ms = cursor - overlap;
      const blend = Math.max(overlap, EDGE_FADE_MS);
      item.fade_in_ms = blend;
      previous.fade_out_ms = blend;
    }
    cursor = end(item);
    placed[index] = item;
    previous = item;
  }
  gaps.forEach((liners, gap) => {
    let at = 0;
    if (gap > 0) {
      const index = sequence[gap - 1] ?? 0;
      const hook = placed[index];
      at = hook ? hook.at_ms + ((parts[index]?.to_ms ?? 0) - hook.from_ms) + pre : 0;
    }
    for (const index of liners) {
      const part = parts[index];
      if (!part) {
        continue;
      }
      const item: Placed = {
        kind: part.kind,
        at_ms: Math.max(0, at + (part.move_ms ?? 0)),
        from_ms: part.from_ms,
        to_ms: part.to_ms,
        over: true,
        fade_in_ms: EDGE_FADE_MS,
        fade_out_ms: EDGE_FADE_MS,
      };
      at += part.to_ms - part.from_ms;
      placed[index] = item;
    }
  });
  const all = placed.filter((item): item is Placed => item !== undefined);
  const hooks = all.filter((item) => item.kind === "hook");
  return {
    parts: all,
    length_ms: Math.max(0, ...all.map(end)),
    music_start_ms: hooks.length > 0 ? Math.min(...hooks.map((item) => item.at_ms)) : 0,
    music_end_ms: Math.max(0, ...hooks.map(end)),
    fade_in_ms: recipe.fade_in_ms ?? 0,
    fade_in_curve: recipe.fade_in_curve ?? "linear",
    fade_out_ms: recipe.fade_out_ms ?? 0,
    fade_out_curve: recipe.fade_out_curve ?? "linear",
    duck_db: recipe.duck_db ?? -10,
    duck_attack_ms: ducking.attack_ms,
    duck_attack_at: ducking.attack_at,
    duck_release_ms: ducking.release_ms,
    duck_release_at: ducking.release_at,
  };
}

/** When the music is lowered (heron_core::medley::duck_windows): liners in a row as one. */
export function duckWindows(medley: Layout): [number, number][] {
  // A gap shorter than going down and back up takes: the music stays down.
  const ducking = layoutDucking(medley);
  const reach = ducking.attack_ms + ducking.release_ms;
  const liners = medley.parts
    .filter((part) => part.over)
    .map((part): [number, number] => [part.at_ms, end(part)])
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const windows: [number, number][] = [];
  for (const [start, stop] of liners) {
    const last = windows.at(-1);
    if (last && start < last[1] + reach) {
      last[1] = Math.max(last[1], stop);
    } else {
      windows.push([start, stop]);
    }
  }
  return windows;
}

/**
 * The level of part `index` at `at_ms` on the medley's timeline, as the
 * librarian mixes it (heron_core::medley::mix), from 0 to 1: its own fades,
 * and for the music, the ducking under liners over it and the medley's
 * fade-in and fade-out.
 */
export function levelAt(medley: Layout, index: number, at_ms: number): number {
  const part = medley.parts[index];
  if (!part) {
    return 0;
  }
  const length = part.to_ms - part.from_ms;
  const t = at_ms - part.at_ms;
  if (t < 0 || t > length) {
    return 0;
  }
  // Its own fades: linear, each at most half the part.
  const fadeIn = Math.max(1, Math.min(part.fade_in_ms, length / 2));
  const fadeOut = Math.max(1, Math.min(part.fade_out_ms, length / 2));
  let level = Math.min(1, t / fadeIn) * Math.min(1, (length - t) / fadeOut);
  if (part.kind !== "hook") {
    return level;
  }
  const duck = 10 ** ((medley.duck_db ?? -10) / 20);
  let gain = 1;
  const ducking = layoutDucking(medley);
  for (const [start, stop] of duckWindows(medley)) {
    const [downFrom, downTo, upFrom, upTo] = duckRamps(ducking, start, stop);
    let here = 1;
    if (at_ms >= downFrom && at_ms < downTo) {
      here = 1 + (duck - 1) * ((at_ms - downFrom) / Math.max(1, downTo - downFrom));
    } else if (at_ms >= downTo && at_ms < upFrom) {
      here = duck;
    } else if (at_ms >= upFrom && at_ms < upTo) {
      here = duck + (1 - duck) * ((at_ms - upFrom) / Math.max(1, upTo - upFrom));
    }
    gain = Math.min(gain, here);
  }
  level *= gain;
  const start = medley.music_start_ms ?? 0;
  const fadeInMs = medley.fade_in_ms ?? 0;
  if (fadeInMs > 0 && at_ms >= start && at_ms < start + fadeInMs) {
    level *= 1 - fadeGain(medley.fade_in_curve ?? "linear", (at_ms - start) / fadeInMs);
  }
  const fadeOutMs = Math.min(medley.fade_out_ms ?? 0, medley.music_end_ms);
  if (fadeOutMs > 0 && at_ms >= medley.music_end_ms - fadeOutMs) {
    level *= fadeGain(medley.fade_out_curve ?? "linear", (at_ms - (medley.music_end_ms - fadeOutMs)) / fadeOutMs);
  }
  return level;
}

/** How long the parts last together once mixed. */
export function plannedLength(parts: readonly LayoutPart[], recipe: MixRecipe): number {
  return layout(parts, recipe).length_ms;
}

/**
 * The `move_ms` that puts the part at `index` at `at_ms` in the medley, as
 * near as it can go: a part after another overlaps it from 0 up to the
 * shorter of the two; a liner over the music starts at 0 at the earliest.
 */
export function moveTo(
  parts: readonly LayoutPart[],
  recipe: MixRecipe,
  index: number,
  at_ms: number,
): number {
  const unmoved = parts.map((part, i) => (i === index ? { ...part, move_ms: 0 } : part));
  const placed = layout(unmoved, recipe).parts;
  const item = placed[index];
  if (!item) {
    return 0;
  }
  if (item.over) {
    return Math.round(Math.max(at_ms, 0) - item.at_ms);
  }
  // The part played before it.
  let before = index - 1;
  while (before >= 0 && placed[before]?.over) {
    before -= 1;
  }
  const previous = placed[before];
  if (!previous) {
    return 0;
  }
  const cursor = previous.at_ms + previous.to_ms - previous.from_ms;
  const longest = Math.min(item.to_ms - item.from_ms, previous.to_ms - previous.from_ms);
  const overlap = Math.min(Math.max(cursor - at_ms, 0), longest);
  return Math.round(recipe.crossfade_ms - overlap);
}
