// SPDX-License-Identifier: AGPL-3.0-or-later

// Artists: their names, similar artists and the links suggested for review.
// Separation compares artists, not names: names found in the tags are
// linked automatically when they only differ by case, accents or spaces;
// anything else is linked here.

import { PlaysByStation } from "../components/Stations";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { type Artist, api, done, query, unwrap } from "../api";
import { ArtistPicker } from "../components/ArtistPicker";
import { useDebounced } from "../components/TrackSearch";
import { Card, ConfirmButton, Empty, ErrorView, Field, Icon, Loading, Modal, PageHead } from "../components/ui";
import { rich, t } from "../i18n";
import { useModule } from "../lib/modules";
import { Link, setSearch, useLocation } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { RestrictionDialog } from "./Restrictions";

const PAGE = 50;

export function Artists() {
  const { search } = useLocation();
  const tab = search.get("tab") === "suggestions" ? "suggestions" : "artists";
  const suggestions = useQuery({
    queryKey: ["artist-suggestions"],
    queryFn: () => unwrap(api.GET("/api/v1/artist-suggestions")),
  });
  const waiting = suggestions.data?.length ?? 0;
  return (
    <>
      <PageHead
        title={t("nav.artists")}
        description={t("artists.description")}
      />
      <div className="tabs" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "artists"}
          onClick={() => setSearch({ tab: null })}
        >
          {t("nav.artists")}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "suggestions"}
          onClick={() => setSearch({ tab: "suggestions", artist: null })}
        >
          {t("artists.suggestions")} {waiting > 0 && <span className="count">{waiting}</span>}
        </button>
      </div>
      {tab === "artists" ? <ArtistList /> : <Suggestions />}
    </>
  );
}

function ArtistList() {
  const { search } = useLocation();
  const [text, setText] = useState(search.get("q") ?? "");
  const q = useDebounced(text.trim());
  const [offset, setOffset] = useState(0);
  const open = search.get("artist") ? Number(search.get("artist")) : null;
  const artists = useQuery({
    queryKey: ["artists", "list", q, offset],
    queryFn: () =>
      unwrap(api.GET("/api/v1/artists", { params: { query: query({ q, limit: PAGE, offset }) } })),
    placeholderData: keepPreviousData,
  });
  return (
    <Card flush>
      <div className="card-pad">
        <label className="row" style={{ flexWrap: "nowrap" }}>
          <span className="visually-hidden">{t("artistPicker.search")}</span>
          <Icon name="search" />
          <input
            className="input"
            type="search"
            placeholder={t("artists.searchAny")}
            value={text}
            onChange={(event) => {
              setText(event.target.value);
              setOffset(0);
            }}
          />
        </label>
      </div>
      {artists.isPending && <Loading />}
      {artists.isError && <ErrorView error={artists.error} />}
      {artists.data?.total === 0 && (
        <Empty>{q ? t("artistPicker.none") : t("artists.noneYet")}</Empty>
      )}
      {artists.data && artists.data.total > 0 && (
        <>
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("library.sort.artist")}</th>
                  <th>{t("artists.aka")}</th>
                  <th>{t("artists.similar")}</th>
                  <th>{t("nav.tracks")}</th>
                </tr>
              </thead>
              <tbody>
                {artists.data.artists.map((artist) => (
                  <tr key={artist.id}>
                    <td className="title-cell">
                      <button type="button" className="link-button" onClick={() => setSearch({ artist: artist.id })}>
                        {artist.name}
                      </button>
                    </td>
                    <td className="small muted">
                      {artist.names
                        .filter((name) => name.name !== artist.name)
                        .map((name) => name.name)
                        .join(", ")}
                    </td>
                    <td className="small">{artist.similar.map((other) => other.name).join(", ")}</td>
                    <td className="num">
                      <Link to={`/library?q=${encodeURIComponent(artist.name)}`}>{artist.tracks}</Link>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="pager">
            <span className="muted small">
              {t("artists.range", { first: offset + 1, last: Math.min(offset + PAGE, artists.data.total), total: artists.data.total })}
            </span>
            <div className="row">
              <button type="button" className="btn small" disabled={offset === 0} onClick={() => setOffset(offset - PAGE)}>
                {t("library.previous")}
              </button>
              <button
                type="button"
                className="btn small"
                disabled={offset + PAGE >= artists.data.total}
                onClick={() => setOffset(offset + PAGE)}
              >
                {t("library.nextPage")}
              </button>
            </div>
          </div>
        </>
      )}
      {open !== null && <ArtistDialog id={open} onClose={() => setSearch({ artist: null })} />}
    </Card>
  );
}

function ArtistDialog({ id, onClose }: { id: number; onClose: () => void }) {
  const restrictionsOn = useModule("restrictions");
  const canEdit = useCan("operator");
  const artist = useQuery({
    queryKey: ["artists", id],
    queryFn: () => unwrap(api.GET("/api/v1/artists/{id}", { params: { path: { id } } })),
  });
  const [adding, setAdding] = useState<"similar" | "merge" | null>(null);
  const [moving, setMoving] = useState<number | null>(null);
  const [restricting, setRestricting] = useState(false);
  const [name, setName] = useState<string | null>(null);
  const invalidate = [["artists"], ["artist-suggestions"]];
  const rename = useAction(
    (value: string) => unwrap(api.PATCH("/api/v1/artists/{id}", { params: { path: { id } }, body: { name: value } })),
    { success: t("artists.renamed"), invalidate, onSuccess: () => setName(null) },
  );
  const similar = useAction(
    ({ other, linked }: { other: number; linked: boolean }) =>
      done(
        (linked ? api.PUT : api.DELETE)("/api/v1/artists/{id}/similar/{other}", {
          params: { path: { id, other } },
        }),
      ),
    { invalidate, onSuccess: () => setAdding(null) },
  );
  const merge = useAction(
    (other: Artist) =>
      unwrap(api.POST("/api/v1/artists/{id}/merge", { params: { path: { id } }, body: { artist_id: other.id } })),
    { success: (merged) => t("artists.merged", { name: merged.name }), invalidate, onSuccess: () => setAdding(null) },
  );
  const move = useAction(
    ({ nameId, to }: { nameId: number; to: number | null }) =>
      unwrap(
        api.PUT("/api/v1/artist-names/{id}", {
          params: { path: { id: nameId } },
          body: to === null ? {} : { artist_id: to },
        }),
      ),
    {
      success: (destination) => t("artists.moved", { name: destination.name }),
      invalidate,
      onSuccess: () => setMoving(null),
    },
  );

  const data = artist.data;
  return (
    <Modal title={data?.name ?? t("library.sort.artist")} open wide onClose={onClose}>
      {artist.isPending && <Loading />}
      {artist.isError && <ErrorView error={artist.error} />}
      {data && (
        <div className="stack">
          {name !== null ? (
            <form
              className="row"
              onSubmit={(event) => {
                event.preventDefault();
                rename.mutate(name);
              }}
            >
              <input
                className="input"
                aria-label={t("artists.nameShown")}
                value={name}
                maxLength={512}
                onChange={(event) => setName(event.target.value)}
              />
              <button type="submit" className="btn small primary" disabled={!name.trim()}>
                {t("common.save")}
              </button>
              <button type="button" className="btn small" onClick={() => setName(null)}>
                {t("common.cancel")}
              </button>
            </form>
          ) : (
            <div className="row">
              <span className="muted">
                {t("common.tracks", { count: data.tracks })}
                {data.plays && (
                  <>
                    {" · "}
                    {t("track.playCount", { count: data.plays.play_count })}{" "}
                    <PlaysByStation plays={data.plays} />
                  </>
                )}
                {data.musicbrainz_id && (
                  <>
                    {" · "}
                    <a href={`https://musicbrainz.org/artist/${data.musicbrainz_id}`} target="_blank" rel="noreferrer">
                      MusicBrainz
                    </a>
                  </>
                )}
              </span>
              <span className="spacer" />
              {restrictionsOn && (
                <Link to={`/restrictions?target=artist&target_id=${data.id}`} className="small">
                  {t("nav.restrictions")}
                </Link>
              )}
              {canEdit && (
                <>
                  {restrictionsOn && (
                    <button type="button" className="btn small" onClick={() => setRestricting(true)}>
                      <Icon name="ban" /> {t("artists.restrict")}
                    </button>
                  )}
                  <button type="button" className="btn small" onClick={() => setName(data.name)}>
                    <Icon name="edit" /> {t("artists.rename")}
                  </button>
                </>
              )}
            </div>
          )}

          <Field label={t("artists.names")} hint={t("artists.namesHelp")}>
            <ul className="list">
              {data.names.map((artistName) => (
                <li key={artistName.id} className="queue-item">
                  <div className="meta">
                    {artistName.name} {artistName.by_hand && <span className="badge">{t("artists.byHand")}</span>}
                  </div>
                  {canEdit && (
                    <div className="row" style={{ flexWrap: "nowrap" }}>
                      <button type="button" className="btn small" onClick={() => setMoving(artistName.id)}>
                        {t("artists.move")}
                      </button>
                      {data.names.length > 1 && (
                        <button
                          type="button"
                          className="btn small"
                          title={t("artists.splitHelp")}
                          onClick={() => move.mutate({ nameId: artistName.id, to: null })}
                        >
                          {t("artists.split")}
                        </button>
                      )}
                    </div>
                  )}
                </li>
              ))}
            </ul>
          </Field>
          {moving !== null && (
            <div className="stack">
              <p className="muted small" style={{ margin: 0 }}>
                {t("artists.moveTo", { name: data.names.find((n) => n.id === moving)?.name ?? "" })}
              </p>
              <ArtistPicker
                exclude={[data.id]}
                actions={(other) => (
                  <button
                    type="button"
                    className="btn small"
                    onClick={() => move.mutate({ nameId: moving, to: other.id })}
                  >
                    {t("artists.moveHere")}
                  </button>
                )}
              />
              <button type="button" className="link-button" onClick={() => setMoving(null)}>
                {t("common.cancel")}
              </button>
            </div>
          )}

          <Field
            label={t("artists.similarArtists")}
            hint={t("artists.similarHelp")}
          >
            {data.similar.length === 0 ? (
              <span className="muted small">{t("artists.none")}</span>
            ) : (
              <ul className="list">
                {data.similar.map((other) => (
                  <li key={other.id} className="queue-item">
                    <div className="meta">
                      <button type="button" className="link-button" onClick={() => setSearch({ artist: other.id })}>
                        {other.name}
                      </button>
                    </div>
                    {canEdit && (
                      <button
                        type="button"
                        className="btn ghost small icon danger"
                        aria-label={t("artists.notSimilar", { name: other.name })}
                        onClick={() => similar.mutate({ other: other.id, linked: false })}
                      >
                        <Icon name="close" />
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </Field>

          {canEdit && adding === null && (
            <div className="row">
              <button type="button" className="btn small" onClick={() => setAdding("similar")}>
                <Icon name="link" /> {t("artists.addSimilar")}
              </button>
              <button type="button" className="btn small" onClick={() => setAdding("merge")}>
                {t("artists.mergeAnother")}
              </button>
            </div>
          )}
          {adding === "similar" && (
            <ArtistPicker
              exclude={[data.id, ...data.similar.map((s) => s.id)]}
              placeholder={t("artists.searchSimilar")}
              actions={(other) => (
                <button
                  type="button"
                  className="btn small"
                  onClick={() => similar.mutate({ other: other.id, linked: true })}
                >
                  {t("artists.similarButton")}
                </button>
              )}
            />
          )}
          {adding === "merge" && (
            <ArtistPicker
              exclude={[data.id]}
              placeholder={t("artists.searchMerge")}
              actions={(other) => (
                <ConfirmButton
                  className="btn small"
                  confirm={t("artists.merge")}
                  question={
                    <p>
                      {rich("artists.mergeQuestion", {
                        other: <strong>{other.name}</strong>,
                        artist: <strong>{data.name}</strong>,
                        artistName: data.name,
                        otherName: other.name,
                      })}
                    </p>
                  }
                  onConfirm={() => merge.mutate(other)}
                >
                  {t("artists.merge")}
                </ConfirmButton>
              )}
            />
          )}
          {adding !== null && (
            <button type="button" className="link-button" onClick={() => setAdding(null)}>
              {t("common.cancel")}
            </button>
          )}
          {restricting && (
            <RestrictionDialog
              restriction={null}
              target={{ target: { kind: "artist", id: data.id }, name: data.name }}
              onClose={() => setRestricting(false)}
            />
          )}
        </div>
      )}
    </Modal>
  );
}

function Suggestions() {
  const canEdit = useCan("operator");
  const suggestions = useQuery({
    queryKey: ["artist-suggestions"],
    queryFn: () => unwrap(api.GET("/api/v1/artist-suggestions")),
  });
  const review = useAction(
    ({ id, accept }: { id: number; accept: boolean }) =>
      done(api.POST("/api/v1/artist-suggestions/{id}", { params: { path: { id } }, body: { accept } })),
    { invalidate: [["artist-suggestions"], ["artists"]] },
  );
  const lookUp = useAction(() => unwrap(api.POST("/api/v1/artist-suggestions/musicbrainz")), {
    success: t("artists.lookingUp"),
    invalidate: [["library-tasks"]],
  });
  return (
    <Card
      flush
      title={t("artists.linksToReview")}
      actions={
        canEdit && (
          <button
            type="button"
            className="btn small"
            disabled={lookUp.isPending}
            title={t("artists.lookUpHelp")}
            onClick={() => lookUp.mutate(undefined)}
          >
            {t("artists.lookUp")}
          </button>
        )
      }
    >
      {suggestions.isPending && <Loading />}
      {suggestions.isError && <ErrorView error={suggestions.error} />}
      {suggestions.data?.length === 0 && <Empty>{t("artists.nothingToReview")}</Empty>}
      {suggestions.data && suggestions.data.length > 0 && (
        <ul className="list">
          {suggestions.data.map((suggestion) => (
            <li key={suggestion.id} className="queue-item">
              <div className="meta">
                <div className="ellipsis">
                  {rich("artists.pair", {
                    artist: <Link to={`/artists?artist=${suggestion.artist.id}`}>{suggestion.artist.name}</Link>,
                    other: <Link to={`/artists?artist=${suggestion.other.id}`}>{suggestion.other.name}</Link>,
                  })}
                </div>
                <div className="muted small ellipsis">
                  {t("artists.reason", {
                    source: suggestion.source === "musicbrainz" ? "MusicBrainz" : t("artists.names"),
                    reason: suggestion.reason,
                  })}
                </div>
              </div>
              {canEdit && (
                <div className="row" style={{ flexWrap: "nowrap" }}>
                  <button
                    type="button"
                    className="btn small primary"
                    disabled={review.isPending}
                    onClick={() => review.mutate({ id: suggestion.id, accept: true })}
                  >
                    {t("artists.similarButton")}
                  </button>
                  <button
                    type="button"
                    className="btn small"
                    disabled={review.isPending}
                    onClick={() => review.mutate({ id: suggestion.id, accept: false })}
                  >
                    {t("artists.dismiss")}
                  </button>
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}
