// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { ApiError, errorMessage } from "../api";
import { AUDIO_EXTENSIONS, type Transport, groupName, isUploadable, uploadFile } from "./upload";

/** A server that keeps what it receives, and a connection that drops. */
function flakyServer(drops: Set<number>) {
  let stored: Uint8Array = new Uint8Array(0);
  let calls = 0;
  const transport: Transport = {
    create: async () => 7,
    async send(_id, offset, chunk) {
      calls += 1;
      if (offset !== stored.length) {
        throw new ApiError(409, "conflict", `${stored.length} bytes received so far`);
      }
      const bytes = new Uint8Array(await chunk.arrayBuffer());
      if (drops.has(calls)) {
        // Half the chunk arrived before the connection dropped.
        const half = bytes.slice(0, Math.floor(bytes.length / 2));
        stored = concat(stored, half);
        throw new TypeError("Failed to fetch");
      }
      stored = concat(stored, bytes);
      return stored.length;
    },
    received: async () => stored.length,
  };
  return { transport, stored: () => stored };
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a);
  out.set(b, a.length);
  return out;
}

const content = new Uint8Array(1000).map((_, n) => n % 251);

describe("uploadFile", () => {
  it("sends every byte, in chunks", async () => {
    const server = flakyServer(new Set());
    const progress: number[] = [];
    const id = await uploadFile(server.transport, 1, new Blob([content]), "a.flac", {
      chunk: 300,
      onProgress: (sent) => progress.push(sent),
    });
    expect(id).toBe(7);
    expect(progress).toEqual([300, 600, 900, 1000]);
    expect(server.stored()).toEqual(content);
  });

  it("resumes where the server got to after a dropped connection", async () => {
    const server = flakyServer(new Set([2, 3]));
    await uploadFile(server.transport, 1, new Blob([content]), "a.flac", {
      chunk: 300,
      backoff: () => 0,
    });
    expect(server.stored()).toEqual(content);
  });

  it("gives up after failing again and again", async () => {
    const transport: Transport = {
      create: async () => 1,
      send: async () => {
        throw new TypeError("Failed to fetch");
      },
      received: async () => 0,
    };
    await expect(
      uploadFile(transport, 1, new Blob([content]), "a.flac", { attempts: 3, backoff: () => 0 }),
    ).rejects.toThrow("Failed to fetch");
  });

  it("does not retry what the server refuses", async () => {
    let sends = 0;
    const transport: Transport = {
      create: async () => 1,
      send: async () => {
        sends += 1;
        throw new ApiError(422, "invalid", "too large");
      },
      received: async () => 0,
    };
    await expect(uploadFile(transport, 1, new Blob([content]), "a.flac")).rejects.toThrow("too large");
    expect(sends).toBe(1);
  });

  it("does not retry a buffer zone the API cannot write to, and says why", async () => {
    let sends = 0;
    const message = "the buffer zone (/staging) is not writable by uid 997, gid 988: give it to HERON_LIBRARIAN_USER (chown -R) (Permission denied (os error 13))";
    const transport: Transport = {
      create: async () => 1,
      send: async () => {
        sends += 1;
        throw new ApiError(503, "misconfigured", message);
      },
      received: async () => 0,
    };
    const upload = uploadFile(transport, 1, new Blob([content]), "a.flac", { backoff: () => 0 });
    await expect(upload).rejects.toThrow(message);
    expect(sends).toBe(1);
    await upload.catch((error: unknown) => expect(errorMessage(error)).toBe(message));
  });
});

describe("what is uploaded", () => {
  it("keeps audio files only, never hidden ones", () => {
    expect(isUploadable("Album/01 Song.FLAC", AUDIO_EXTENSIONS)).toBe(true);
    expect(isUploadable("Album/cover.jpg", AUDIO_EXTENSIONS)).toBe(false);
    expect(isUploadable("Album/._01 Song.flac", AUDIO_EXTENSIONS)).toBe(false);
    expect(isUploadable("mp3", AUDIO_EXTENSIONS)).toBe(false);
  });

  it("names a group after the folder dropped", () => {
    const file = new File([], "x");
    expect(groupName([{ file, path: "Album/1.mp3" }, { file, path: "Album/2.mp3" }])).toBe("Album");
    expect(groupName([{ file, path: "1.mp3" }], new Date(0))).toMatch(/^Upload of /);
  });
});
