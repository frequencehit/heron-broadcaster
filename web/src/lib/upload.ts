// SPDX-License-Identifier: AGPL-3.0-or-later

// Uploads into the buffer zone, in chunks that resume after a dropped
// connection: the server keeps what it received, and after an error the
// client asks how far it got and carries on from there.

import { ApiError, api, unwrap } from "../api";
import { locale, t } from "../i18n";

/** Bytes sent per request (the API takes up to 16 MiB). */
export const CHUNK = 8 * 1024 * 1024;

/** How the uploader talks to the API, replaced in tests. */
export interface Transport {
  /** Announces a file; returns its ID. */
  create(groupId: number, name: string, size: number): Promise<number>;
  /** Sends bytes at `offset`; returns the bytes the server now has. */
  send(id: number, offset: number, chunk: Blob, signal?: AbortSignal): Promise<number>;
  /** The bytes the server has. */
  received(id: number): Promise<number>;
}

export interface UploadOptions {
  chunk?: number;
  /** Attempts in a row without progress before giving up. */
  attempts?: number;
  /** Wait before attempt `n` (from 1), in ms. */
  backoff?: (attempt: number) => number;
  signal?: AbortSignal;
  onProgress?: (sent: number) => void;
  /** Called once the file is announced. */
  onCreated?: (id: number) => void;
}

/**
 * Errors worth trying again: the network, the server, a chunk out of place.
 * Not the station's set-up (a buffer zone the API cannot write to): trying
 * again changes nothing, and its message says what to fix.
 */
export function isTransient(error: unknown): boolean {
  if (error instanceof ApiError) {
    if (error.code === "misconfigured") {
      return false;
    }
    return error.status >= 500 || error.status === 409 || error.status === 408 || error.status === 429;
  }
  return !(error instanceof DOMException && error.name === "AbortError");
}

const sleep = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => {
      clearTimeout(timer);
      reject(new DOMException("Upload cancelled", "AbortError"));
    });
  });

/**
 * Uploads `file` as `name` into the group. Resolves with the file's ID once
 * the server has every byte.
 */
export async function uploadFile(
  transport: Transport,
  groupId: number,
  file: Blob,
  name: string,
  options: UploadOptions = {},
): Promise<number> {
  const chunk = options.chunk ?? CHUNK;
  const attempts = options.attempts ?? 8;
  const backoff = options.backoff ?? ((n: number) => Math.min(30_000, 500 * 2 ** (n - 1)));
  const id = await transport.create(groupId, name, file.size);
  options.onCreated?.(id);
  let offset = 0;
  let failures = 0;
  while (offset < file.size) {
    options.signal?.throwIfAborted();
    try {
      const received = await transport.send(id, offset, file.slice(offset, offset + chunk), options.signal);
      if (received > offset) {
        failures = 0;
      }
      offset = received;
      options.onProgress?.(offset);
    } catch (error) {
      if (!isTransient(error)) {
        throw error;
      }
      failures += 1;
      if (failures >= attempts) {
        throw error;
      }
      await sleep(backoff(failures), options.signal);
      offset = await resync(transport, id, offset, options);
    }
  }
  return id;
}

/** Where the server got to, before carrying on; where we were if it cannot say. */
async function resync(transport: Transport, id: number, offset: number, options: UploadOptions): Promise<number> {
  try {
    const received = await transport.received(id);
    options.onProgress?.(received);
    return received;
  } catch (error) {
    if (!isTransient(error)) {
      throw error;
    }
    return offset;
  }
}

/** The API, as the uploader uses it. */
export const apiTransport: Transport = {
  async create(groupId, name, size) {
    const file = await unwrap(api.POST("/api/v1/staging/uploads", { body: { group_id: groupId, name, size } }));
    return file.id;
  },
  async send(id, offset, chunk, signal) {
    const response = await fetch(`/api/v1/staging/uploads/${id}?offset=${offset}`, {
      method: "PUT",
      headers: { "X-Requested-With": "heron-web", "Content-Type": "application/octet-stream" },
      body: chunk,
      signal: signal ?? null,
    });
    const body = await response.json().catch(() => null);
    if (!response.ok) {
      throw new ApiError(
        response.status,
        body?.error ?? "error",
        body?.message ?? `The API answered ${response.status}`,
      );
    }
    return body.received ?? body.size;
  },
  async received(id) {
    const file = await unwrap(api.GET("/api/v1/staging/uploads/{id}", { params: { path: { id } } }));
    return file.received ?? file.size;
  },
};

/** A file to upload, with its path in what was dropped. */
export interface Picked {
  file: File;
  path: string;
}

/** Files of a drop, folders walked, with their relative paths. */
export async function droppedFiles(items: DataTransferItemList): Promise<Picked[]> {
  const entries: FileSystemEntry[] = [];
  for (const item of Array.from(items)) {
    const entry = item.webkitGetAsEntry?.();
    if (entry) {
      entries.push(entry);
    }
  }
  const picked: Picked[] = [];
  const walk = async (entry: FileSystemEntry, prefix: string): Promise<void> => {
    if (entry.isFile) {
      const file = await new Promise<File>((resolve, reject) =>
        (entry as FileSystemFileEntry).file(resolve, reject),
      );
      picked.push({ file, path: `${prefix}${entry.name}` });
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      // readEntries answers in batches until it returns none.
      for (;;) {
        const batch = await new Promise<FileSystemEntry[]>((resolve, reject) =>
          reader.readEntries(resolve, reject),
        );
        if (batch.length === 0) {
          break;
        }
        for (const child of batch) {
          await walk(child, `${prefix}${entry.name}/`);
        }
      }
    }
  };
  for (const entry of entries) {
    await walk(entry, "");
  }
  return picked;
}

/** Hidden files and what is not audio are left out of an upload. */
export function isUploadable(path: string, extensions: readonly string[]): boolean {
  const name = path.split("/").pop() ?? path;
  if (name.startsWith(".")) {
    return false;
  }
  const extension = name.includes(".") ? (name.split(".").pop() ?? "").toLowerCase() : "";
  return extensions.includes(extension);
}

/** What the scanner and the librarian import (`heron_library::files`). */
export const AUDIO_EXTENSIONS = [
  "mp3",
  "m4a",
  "mp4",
  "aac",
  "flac",
  "ogg",
  "oga",
  "opus",
  "wav",
  "aif",
  "aiff",
] as const;

/** A name for the group of a drop: its folder, when it is one. */
export function groupName(picked: Picked[], now = new Date()): string {
  const tops = new Set(picked.map((p) => (p.path.includes("/") ? p.path.split("/")[0] : "")));
  if (tops.size === 1) {
    const [top] = tops;
    if (top) {
      return top;
    }
  }
  return t("buffer.uploadOf", {
    date: now.toLocaleString(locale(), { dateStyle: "medium", timeStyle: "short" }),
  });
}
