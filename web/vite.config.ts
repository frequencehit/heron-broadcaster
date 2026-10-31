// SPDX-License-Identifier: AGPL-3.0-or-later

import { readFileSync } from "node:fs";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

const version: string = JSON.parse(readFileSync(new URL("package.json", import.meta.url), "utf8")).version;

// In development, /api (and its WebSocket) is proxied to a heron-api
// running locally, or to a station's heron-web with HERON_API_URL.
export default defineConfig({
  plugins: [react()],
  build: {
    rolldownOptions: {
      output: {
        // Dependencies change less often than the app: their own chunk stays cached.
        codeSplitting: { groups: [{ name: "vendor", test: /node_modules/ }] },
      },
    },
  },
  define: {
    __APP_VERSION__: JSON.stringify(version),
    // The commit of a published image; empty in a local build.
    __APP_REVISION__: JSON.stringify(process.env.HERON_REVISION ?? ""),
  },
  server: {
    proxy: {
      "/api": { target: process.env.HERON_API_URL ?? "http://localhost:8080", ws: true },
    },
  },
  test: {
    include: ["src/**/*.test.ts"],
    // `npm run coverage`: LCOV for SonarQube Cloud, paths from the repository root.
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: ["src/api/schema.ts", "src/**/*.test.ts"],
      reporter: [["lcov", { projectRoot: ".." }], "text-summary"],
    },
  },
});
