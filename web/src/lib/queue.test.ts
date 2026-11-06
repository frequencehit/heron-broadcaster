// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { linkRoles, moveEntries } from "./queue";

// b, then a jingle (2) linked to its track (3), then c.
const entries = [
  { entry_id: 1 },
  { entry_id: 2, linked_to: 3 },
  { entry_id: 3, linked_to: null },
  { entry_id: 4 },
];
const ids = (list: { entry_id: number }[]) => list.map((entry) => entry.entry_id);

describe("linked entries", () => {
  it("are a jingle right before its track", () => {
    expect(linkRoles(entries)).toEqual([null, "jingle", "track", null]);
    // Apart, they are not shown together.
    expect(linkRoles([entries[1]!, entries[0]!, entries[2]!])).toEqual([null, null, null]);
  });

  it("move together, the jingle first", () => {
    expect(ids(moveEntries(entries, 2, 0))).toEqual([2, 3, 1, 4]);
    expect(ids(moveEntries(entries, 2, 3))).toEqual([1, 4, 2, 3]);
    expect(ids(moveEntries(entries, 1, 3))).toEqual([1, 4, 2, 3]);
    // Past its own track, the jingle stays.
    expect(ids(moveEntries(entries, 1, 2))).toEqual([1, 2, 3, 4]);
  });

  it("let nothing between them", () => {
    expect(ids(moveEntries(entries, 3, 2))).toEqual([1, 4, 2, 3]);
    expect(ids(moveEntries(entries, 0, 1))).toEqual([1, 2, 3, 4]);
    // Unlinked entries move as before.
    const plain = [{ entry_id: 1 }, { entry_id: 2 }, { entry_id: 3 }];
    expect(ids(moveEntries(plain, 0, 2))).toEqual([2, 3, 1]);
    expect(ids(moveEntries(plain, 5, 0))).toEqual([1, 2, 3]);
  });
});
