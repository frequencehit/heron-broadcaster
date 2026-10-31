// SPDX-License-Identifier: AGPL-3.0-or-later

// Whether the web app and the API come from the same version: an image
// pulled or started without the other serves pages the API cannot answer.

declare global {
  /** The commit a published image was built from; empty in a local build. */
  const __APP_REVISION__: string;
}

export interface Build {
  version: string;
  /** The commit; missing or empty when built locally. */
  revision?: string | null;
}

/** The web app's own build. */
export function webBuild(): Build {
  return { version: __APP_VERSION__, revision: __APP_REVISION__ };
}

/**
 * Whether `web` and `api` differ: their commits when both have one, else
 * their versions.
 */
export function differentBuilds(web: Build, api: Build): boolean {
  if (web.revision && api.revision) {
    return web.revision !== api.revision;
  }
  return web.version !== api.version;
}

/** How a build is shown: its version, and its commit when known. */
export function buildLabel(build: Build): string {
  return build.revision ? `${build.version}+${build.revision}` : build.version;
}
