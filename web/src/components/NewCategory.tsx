// SPDX-License-Identifier: AGPL-3.0-or-later

// A category created on the spot, where tracks are put in categories: the
// selection's and the buffer's dialog, and the track page. It belongs to the
// station shown (docs/stations.md#web-ui), takes the first type, as on the
// Categories page, and a color no other category has yet.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { type Category, api, unwrap } from "../api";
import { t } from "../i18n";
import { nextColor } from "../lib/colors";
import { useAction } from "../lib/session";
import { useStation } from "../lib/station";
import { useCategoryTypes } from "../pages/CategoryTypes";
import { useStations } from "./Stations";

export function NewCategory({ onCreated }: Readonly<{ onCreated: (category: Category) => void }>) {
  const [name, setName] = useState("");
  const station = useStation();
  const stations = useStations();
  const types = useCategoryTypes();
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const shown = stations.length > 1 ? stations.find((s) => s.id === station) : undefined;
  const create = useAction(
    (name: string) =>
      unwrap(
        api.POST("/api/v1/categories", {
          body: {
            name,
            color: nextColor((categories.data ?? []).map((category) => category.color)),
            type_id: types.data?.[0]?.id ?? null,
            ...(shown ? { station_ids: [shown.id] } : {}),
          },
        }),
      ),
    {
      success: (category) => t("categories.createdOnTheSpot", { name: category.name }),
      invalidate: [["categories"]],
      onSuccess: (category) => {
        setName("");
        onCreated(category);
      },
    },
  );
  const submit = () => {
    if (name.trim() && !create.isPending) {
      create.mutate(name.trim());
    }
  };
  return (
    <div className="stack" style={{ gap: "0.25rem" }}>
      <div className="row">
        <input
          className="input"
          style={{ flex: 1 }}
          placeholder={t("categories.newOnTheSpot")}
          aria-label={t("categories.newOnTheSpot")}
          value={name}
          onChange={(event) => setName(event.target.value)}
          onKeyDown={(event) => {
            // Not a form: the dialogs and cards around it may be one.
            if (event.key === "Enter") {
              event.preventDefault();
              submit();
            }
          }}
        />
        <button type="button" className="btn" disabled={!name.trim() || create.isPending} onClick={submit}>
          {t("selection.create")}
        </button>
      </div>
      {shown && (
        <p className="muted small" style={{ margin: 0 }}>
          {t("categories.newOnTheSpotStation", { station: shown.name })}
        </p>
      )}
    </div>
  );
}
