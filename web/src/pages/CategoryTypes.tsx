// SPDX-License-Identifier: AGPL-3.0-or-later

// Category types: what a category holds. Song, Jingle and Ad exist by
// default; the station renames them, adds its own and deletes those no
// category uses. Shown under the categories.

import { useQuery } from "@tanstack/react-query";
import { type FormEvent, useState } from "react";
import { type Schemas, api, done, unwrap } from "../api";
import { Card, ConfirmButton, Icon, Loading } from "../components/ui";
import { number, t } from "../i18n";
import { useAction, useCan } from "../lib/session";

type CategoryType = Schemas["CategoryType"];

export function useCategoryTypes() {
  return useQuery({
    queryKey: ["category-types"],
    queryFn: () => unwrap(api.GET("/api/v1/category-types")),
  });
}

export function TypesCard() {
  const canEdit = useCan("operator");
  const types = useCategoryTypes();
  const [name, setName] = useState("");
  const create = useAction(
    () => unwrap(api.POST("/api/v1/category-types", { body: { name } })),
    { success: t("categories.typeAdded"), invalidate: [["category-types"]], onSuccess: () => setName("") },
  );
  const submit = (event: FormEvent) => {
    event.preventDefault();
    create.mutate(undefined);
  };
  return (
    <Card title={t("categories.typesTitle")}>
      <div className="stack">
        <p className="muted small" style={{ margin: 0 }}>
          {t("categories.typesHelp")}
        </p>
        {types.isPending && <Loading />}
        {types.data && (
          <table className="table">
            <thead>
              <tr>
                <th>{t("categories.name")}</th>
                <th title={t("categories.noTwoInARowHint")}>{t("categories.noTwoInARow")}</th>
                <th title={t("categories.sendsMetadataHint")}>{t("categories.sendsMetadata")}</th>
                <th className="num">{t("nav.categories")}</th>
                <th>
                  <span className="visually-hidden">{t("library.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {types.data.map((categoryType) => (
                <TypeRow key={categoryType.id} categoryType={categoryType} canEdit={canEdit} />
              ))}
            </tbody>
          </table>
        )}
        {canEdit && (
          <form className="row" onSubmit={submit}>
            <input
              className="input"
              value={name}
              maxLength={64}
              placeholder={t("categories.newTypePlaceholder")}
              aria-label={t("categories.newType")}
              onChange={(event) => setName(event.target.value)}
            />
            <button type="submit" className="btn" disabled={!name.trim() || create.isPending}>
              <Icon name="plus" /> {t("categories.addType")}
            </button>
          </form>
        )}
      </div>
    </Card>
  );
}

function TypeRow({ categoryType, canEdit }: { categoryType: CategoryType; canEdit: boolean }) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const rename = useAction(
    () =>
      unwrap(
        api.PUT("/api/v1/category-types/{id}", {
          params: { path: { id: categoryType.id } },
          body: { name: renaming ?? categoryType.name },
        }),
      ),
    { invalidate: [["category-types"], ["categories"]], onSuccess: () => setRenaming(null) },
  );
  const setRule = useAction(
    (rules: { no_two_in_a_row?: boolean; sends_metadata?: boolean }) =>
      unwrap(
        api.PUT("/api/v1/category-types/{id}", {
          params: { path: { id: categoryType.id } },
          body: { name: categoryType.name, ...rules },
        }),
      ),
    { invalidate: [["category-types"]] },
  );
  const remove = useAction(
    () => done(api.DELETE("/api/v1/category-types/{id}", { params: { path: { id: categoryType.id } } })),
    { success: t("categories.typeDeleted"), invalidate: [["category-types"]] },
  );
  return (
    <tr>
      <td>
        {renaming === null ? (
          categoryType.name
        ) : (
          <form
            className="row"
            onSubmit={(event) => {
              event.preventDefault();
              rename.mutate(undefined);
            }}
          >
            <input
              className="input"
              value={renaming}
              maxLength={64}
              aria-label={t("categories.renameType", { name: categoryType.name })}
              onChange={(event) => setRenaming(event.target.value)}
            />
            <button type="submit" className="btn small primary" disabled={!renaming.trim() || rename.isPending}>
              {t("common.save")}
            </button>
            <button type="button" className="btn small" onClick={() => setRenaming(null)}>
              {t("common.cancel")}
            </button>
          </form>
        )}
      </td>
      <td>
        <input
          type="checkbox"
          checked={categoryType.no_two_in_a_row}
          disabled={!canEdit || setRule.isPending}
          aria-label={t("categories.noTwoInARowFor", { name: categoryType.name })}
          onChange={(event) => setRule.mutate({ no_two_in_a_row: event.target.checked })}
        />
      </td>
      <td>
        <input
          type="checkbox"
          checked={categoryType.sends_metadata}
          disabled={!canEdit || setRule.isPending}
          aria-label={t("categories.sendsMetadataFor", { name: categoryType.name })}
          onChange={(event) => setRule.mutate({ sends_metadata: event.target.checked })}
        />
      </td>
      <td className="num">{number(categoryType.categories)}</td>
      <td className="actions">
        {canEdit && renaming === null && (
          <>
            <button
              type="button"
              className="btn ghost small icon"
              aria-label={t("categories.renameType", { name: categoryType.name })}
              onClick={() => setRenaming(categoryType.name)}
            >
              <Icon name="edit" />
            </button>
            <ConfirmButton
              className="btn ghost small icon danger"
              title={
                categoryType.categories > 0
                  ? t("categories.typeInUse", { count: categoryType.categories })
                  : t("categories.deleteType", { name: categoryType.name })
              }
              disabled={categoryType.categories > 0}
              confirm={t("categories.deleteButton")}
              question={<p>{t("categories.deleteType", { name: categoryType.name })}</p>}
              onConfirm={() => remove.mutate(undefined)}
            >
              <Icon name="trash" />
            </ConfirmButton>
          </>
        )}
      </td>
    </tr>
  );
}
