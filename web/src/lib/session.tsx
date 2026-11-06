// SPDX-License-Identifier: AGPL-3.0-or-later

// The signed-in user, and mutations that report their errors.

import { type QueryKey, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createContext, useContext } from "react";
import { type Role, type User, can, errorMessage, fetchMe } from "../api";
import { useToast } from "../components/ui";
import { stationRole, useStation } from "./station";

export const ME_KEY = ["me"] as const;

export function useMeQuery() {
  return useQuery({ queryKey: ME_KEY, queryFn: fetchMe, staleTime: 60_000, retry: 1 });
}

export const UserContext = createContext<User | null>(null);

/** The signed-in user (inside the signed-in app). */
export function useUser(): User {
  const user = useContext(UserContext);
  if (!user) {
    throw new Error("useUser outside the signed-in app");
  }
  return user;
}

/** Whether a page is one of a station's: its actions follow the role on that station. */
export const StationScope = createContext(false);

/**
 * Whether the signed-in user's role allows what `needed` can do: their role
 * on the station shown on a station's pages, their shared role elsewhere.
 */
export function useCan(needed: Role): boolean {
  const user = useContext(UserContext);
  const onStationPage = useContext(StationScope);
  const station = useStation();
  if (!user) {
    return false;
  }
  return can(onStationPage ? stationRole(user, station) : user.role, needed);
}

/**
 * Whether the signed-in user can do what `needed` can on each station: their
 * role on it (a shared admin is admin everywhere). For what a shared page
 * changes on one station, a track's play ratio there.
 */
export function useCanOn(needed: Role): (stationId: number) => boolean {
  const user = useContext(UserContext);
  return (stationId) => (user ? can(stationRole(user, stationId), needed) : false);
}

/**
 * A mutation that shows a toast on success (when given a message) and on
 * failure, and refreshes the queries under `invalidate`.
 */
export function useAction<V, R>(
  action: (variables: V) => Promise<R>,
  options: {
    success?: string | ((result: R) => string);
    invalidate?: QueryKey[];
    onSuccess?: (result: R, variables: V) => void;
  } = {},
) {
  const client = useQueryClient();
  const toast = useToast();
  return useMutation({
    mutationFn: action,
    onSuccess: async (result, variables) => {
      for (const key of options.invalidate ?? []) {
        await client.invalidateQueries({ queryKey: key });
      }
      if (options.success) {
        toast(typeof options.success === "string" ? options.success : options.success(result));
      }
      options.onSuccess?.(result, variables);
    },
    onError: (error) => toast(errorMessage(error), true),
  });
}
