// SPDX-License-Identifier: AGPL-3.0-or-later

// Linked queue entries: a jingle of a clock slot linked to the next one,
// and the track it announces right after it. The engine keeps them
// together; these mirror what it does, so the queue shows a drag at once.

interface Linked {
  entry_id: number;
  linked_to?: number | null;
}

/** Where a waiting entry sits in a linked pair, if it does. */
export type LinkRole = "jingle" | "track" | null;

/** Each entry's place in a linked pair: the jingle, right before its track. */
export function linkRoles(entries: readonly Linked[]): LinkRole[] {
  return entries.map((entry, index) => {
    const next = entries[index + 1];
    const previous = entries[index - 1];
    if (next && entry.linked_to != null && entry.linked_to === next.entry_id) {
      return "jingle";
    }
    if (previous && previous.linked_to != null && previous.linked_to === entry.entry_id) {
      return "track";
    }
    return null;
  });
}

/**
 * The entries with the one at `from` moved to `to`, as the engine moves
 * them: a linked jingle and its track move together, the jingle first, and
 * nothing is moved between a jingle and its track (it goes before the
 * jingle).
 */
export function moveEntries<T extends Linked>(entries: readonly T[], from: number, to: number): T[] {
  const moved = entries[from];
  if (!moved) {
    return [...entries];
  }
  const list = [...entries];
  list.splice(from, 1);
  list.splice(to, 0, moved);
  const track =
    moved.linked_to != null ? entries.find((entry) => entry.entry_id === moved.linked_to) : undefined;
  const jingle = track ? undefined : entries.find((entry) => entry.linked_to === moved.entry_id);
  const group = [jingle, moved, track].filter((entry): entry is T => !!entry);
  const place = list
    .slice(0, list.indexOf(moved))
    .filter((entry) => !group.includes(entry)).length;
  const rest = list.filter((entry) => !group.includes(entry));
  let at = place;
  while (at > 0 && at < rest.length && rest[at - 1]?.linked_to === rest[at]?.entry_id) {
    at -= 1;
  }
  rest.splice(at, 0, ...group);
  return rest;
}
