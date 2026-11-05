// SPDX-License-Identifier: AGPL-3.0-or-later

// Translations of the web UI (see docs/web.md#languages). English is the
// source: `en.ts` defines the messages and their parameters, the other
// catalogs are typed against it and loaded only when chosen. Dates, numbers
// and plurals come from `Intl` in the current language.
//
// `App` re-renders the whole tree when the language changes (`useLanguage`),
// so components and the helpers they call read `t()` as they render.

import { Fragment, type ReactNode, createElement, useSyncExternalStore } from "react";
import en from "./en";

/** A message: text with `{named}` parameters, or plural forms chosen by `{count}`. */
export type Message = string | Plural;
export type Plural = Partial<Record<Intl.LDMLPluralRule, string>> & { other: string };

export type Messages = typeof en;
export type Key = keyof Messages;
/** A catalog: every English key, translated. */
export type Catalog = { readonly [K in Key]: Message };

// Adding a language: its catalog next to this file, and a line here.
export const LANGUAGES = [
  { code: "en", name: "English", load: () => Promise.resolve(en) },
  { code: "fr", name: "Français", load: async () => (await import("./fr")).default },
  { code: "es", name: "Español", load: async () => (await import("./es")).default },
  { code: "de", name: "Deutsch", load: async () => (await import("./de")).default },
] as const satisfies readonly { code: string; name: string; load: () => Promise<Catalog> }[];

export type Language = (typeof LANGUAGES)[number]["code"];
/** What a user chose: a language, or `system` for the browser's. */
export type LanguageSetting = Language | "system";

// The pseudo-locale (`?lang=pseudo`, in development): accented, lengthened
// English that shows text missed by the catalogs and layouts too tight.
const PSEUDO = "pseudo";
type Current = Language | typeof PSEUDO;

// Parameters --------------------------------------------------------------

type Names<S> = S extends `${string}{${infer Name}}${infer Rest}` ? Name | Names<Rest> : never;
type ParamNames<M> = M extends string ? Names<M> : Names<M[keyof M]> | "count";
type Value = string | number;
type ParamsOf<K extends Key> = { [P in ParamNames<Messages[K]>]: Value };
type Args<K extends Key> = [ParamNames<Messages[K]>] extends [never] ? [] : [ParamsOf<K>];
/** The keys of messages without parameters, for labels kept in tables. */
export type PlainKey = { [K in Key]: [ParamNames<Messages[K]>] extends [never] ? K : never }[Key];

// State -------------------------------------------------------------------

const KEY = "heron.language";

let current: Current = "en";
let tag = "en";
let catalog: Catalog = en;
let setting: LanguageSetting = "system";
let numbers = new Intl.NumberFormat("en");
let plurals = new Intl.PluralRules("en");
const listeners = new Set<() => void>();

function isLanguage(code: string): code is Language {
  return LANGUAGES.some((language) => language.code === code);
}

/** The language for `navigator.languages`: the first one we have, English otherwise. */
export function systemLanguage(preferred: readonly string[] = navigator.languages): Language {
  for (const tag of preferred) {
    const code = tag.toLowerCase().split("-")[0] ?? "";
    if (isLanguage(code)) {
      return code;
    }
  }
  return "en";
}

/** The language a setting stands for. */
export function resolve(choice: LanguageSetting): Language {
  return choice === "system" ? systemLanguage() : choice;
}

/** Reads a setting saved by the API or this browser; unknown languages follow the system. */
export function toSetting(value: string | null | undefined): LanguageSetting {
  return value && isLanguage(value) ? value : "system";
}

/**
 * The BCP 47 tag `Intl` formats with: the browser's own variant of the
 * language when it lists one (`en-GB` keeps 24-hour times, `fr-CA` its
 * dates), else the language.
 */
export function locale(): string {
  return tag;
}

function regional(code: string, preferred: readonly string[] = navigator.languages): string {
  const variant = preferred.find((entry) => entry.toLowerCase().split("-")[0] === code);
  try {
    return variant ? (Intl.getCanonicalLocales(variant)[0] ?? code) : code;
  } catch {
    return code;
  }
}

/** The current language's catalog, loading it the first time. */
async function load(code: Current): Promise<Catalog> {
  // Only in development, so production builds leave it out.
  if (import.meta.env.DEV && code === PSEUDO) {
    return (await import("./pseudo")).default;
  }
  const language = LANGUAGES.find((entry) => entry.code === code) ?? LANGUAGES[0];
  return language.load();
}

let switching = 0;
// The language and regional variant last asked for, loaded or still
// loading: a change of region alone (en-US to en-GB) changes formats too.
let wanted: string | null = null;

async function show(code: Current) {
  const nextTag = regional(code === PSEUDO ? "en" : code);
  if (`${code} ${nextTag}` === wanted) {
    return;
  }
  wanted = `${code} ${nextTag}`;
  const attempt = ++switching;
  const next = await load(code).catch(() => null);
  // A later choice won, or the catalog could not be loaded: keep the current one.
  if (attempt !== switching) {
    return;
  }
  if (next === null) {
    wanted = `${current} ${tag}`;
    return;
  }
  current = code;
  tag = nextTag;
  catalog = next;
  numbers = new Intl.NumberFormat(locale());
  plurals = new Intl.PluralRules(locale());
  if (typeof document !== "undefined") {
    document.documentElement.lang = code === PSEUDO ? "en" : code;
  }
  for (const listener of listeners) {
    listener();
  }
}

function pseudo(): boolean {
  return import.meta.env.DEV && new URLSearchParams(window.location.search).get("lang") === PSEUDO;
}

/** Switches to what the user chose and remembers it in this browser. */
export async function setLanguage(choice: LanguageSetting) {
  setting = choice;
  try {
    window.localStorage.setItem(KEY, choice);
  } catch {
    // Not remembered in this browser; the API still has it.
  }
  if (current === PSEUDO) {
    return;
  }
  await show(resolve(choice));
}

/** Before the first paint: the language this browser remembers, or the pseudo-locale. */
export async function startLanguage() {
  try {
    setting = toSetting(window.localStorage.getItem(KEY));
  } catch {
    // Storage blocked: follow the system.
  }
  // The browser's languages changed: another language, or another region.
  window.addEventListener("languagechange", () => {
    if (setting === "system" && current !== PSEUDO) {
      void show(systemLanguage());
    }
  });
  await show(pseudo() ? PSEUDO : resolve(setting));
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The current language and regional variant (`fr fr-CA`); re-renders when either changes. */
export function useLanguage(): string {
  return useSyncExternalStore(subscribe, () => `${current} ${tag}`);
}

// Messages ----------------------------------------------------------------

function pick(message: Message, params: Record<string, unknown>): string {
  if (typeof message === "string") {
    return message;
  }
  const count = Number(params.count);
  return message[plurals.select(count)] ?? message.other;
}

function value(param: unknown): string {
  return typeof param === "number" ? numbers.format(param) : String(param);
}

/** A message in the current language: `t("queue.waiting", { count: 3 })`. */
export function t<K extends Key>(key: K, ...args: Args<K>): string {
  const params: Record<string, unknown> = args[0] ?? {};
  return pick(catalog[key] ?? en[key], params).replace(/\{(\w+)\}/g, (all, name: string) =>
    name in params ? value(params[name]) : all,
  );
}

type RichParams<K extends Key> = { [P in ParamNames<Messages[K]>]: ReactNode };

/** A message with elements for parameters: links, emphasis… */
export function rich<K extends Key>(key: K, params: RichParams<K>): ReactNode {
  const values: Record<string, unknown> = params;
  const parts = pick(catalog[key] ?? en[key], values).split(/\{(\w+)\}/g);
  return createElement(
    Fragment,
    null,
    ...parts.map((part, index) => {
      if (index % 2 === 0) {
        return part;
      }
      const param = values[part];
      return typeof param === "number" ? numbers.format(param) : (param as ReactNode);
    }),
  );
}

/** A number in the current language: `1 234,5` in French. */
export function number(value: number, options?: Intl.NumberFormatOptions): string {
  return options ? new Intl.NumberFormat(locale(), options).format(value) : numbers.format(value);
}
