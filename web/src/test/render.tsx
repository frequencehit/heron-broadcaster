// SPDX-License-Identifier: AGPL-3.0-or-later

// Components in tests (jsdom): the app's providers, a signed-in user, and an
// API answered by the test instead of heron-api.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, vi } from "vitest";
import { type Role, type User, api } from "../api";
import { ToastProvider } from "../components/ui";
import { UserContext } from "../lib/session";

// jsdom has <dialog> without its modal methods.
if (typeof HTMLDialogElement !== "undefined" && !HTMLDialogElement.prototype.showModal) {
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.open = true;
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.open = false;
    this.dispatchEvent(new Event("close"));
  };
}

/** An answer the API gives with an error, as heron-api writes it. */
export class Failure {
  constructor(
    readonly status: number,
    readonly code: string,
    readonly message: string,
  ) {}
}

/** What a request is answered with: data, a `Failure`, or either from its options. */
type Answer = unknown;

type Method = "GET" | "POST" | "PUT" | "DELETE";

/** A request the component sent. */
export interface Call {
  method: Method;
  path: string;
  options: Record<string, unknown> | undefined;
}

export const calls: Call[] = [];

/**
 * Answers the requests to `"METHOD /api/v1/path"`; a function is called with
 * the request's options. `undefined` answers 204. A request without an
 * answer fails the test.
 */
export function answer(answers: Record<string, Answer>): void {
  const reply = (method: Method) => async (path: string, options?: Record<string, unknown>) => {
    calls.push({ method, path, options });
    const route = `${method} ${path}`;
    if (!(route in answers)) {
      throw new Error(`no answer for ${route}`);
    }
    const given = answers[route];
    const result = typeof given === "function" ? (given as (options: unknown) => unknown)(options) : given;
    if (result instanceof Failure) {
      return {
        error: { error: result.code, message: result.message },
        response: new Response(null, { status: result.status }),
      };
    }
    return { data: result, response: new Response(null, { status: result === undefined ? 204 : 200 }) };
  };
  for (const method of ["GET", "POST", "PUT", "DELETE"] as const) {
    vi.spyOn(api, method).mockImplementation(reply(method) as never);
  }
}

/** The requests sent to `"METHOD /api/v1/path"`. */
export function sent(route: string): Call[] {
  return calls.filter((call) => `${call.method} ${call.path}` === route);
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  calls.length = 0;
});

export function user(role: Role = "admin"): User {
  return {
    id: 1,
    username: "ada",
    role,
    enabled: true,
    theme: "system",
    language: "en",
    full_width: false,
    created_at_unix_ms: 0,
    station_roles: [],
  };
}

/** Renders `ui` as the app does, signed in as `as`. */
export function renderApp(ui: ReactNode, as: User = user()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ToastProvider>
        <UserContext.Provider value={as}>{ui}</UserContext.Provider>
      </ToastProvider>
    </QueryClientProvider>,
  );
}
