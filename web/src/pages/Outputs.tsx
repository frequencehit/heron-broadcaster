// SPDX-License-Identifier: AGPL-3.0-or-later

// Encoder profiles and outputs, with each output's live state.

import { useModule } from "../lib/modules";
import { useQuery } from "@tanstack/react-query";
import { type FormEvent, type ReactNode, useState } from "react";
import { type EncoderProfileView, type OutputView, type Schemas, api, done, unwrap } from "../api";
import { type DetailTexts, overridesFrom, parseRelayMounts, relayMountsText, serverAddress } from "../lib/servers";
import {
  Banner,
  Card,
  ConfirmButton,
  Empty,
  ErrorView,
  Field,
  Icon,
  Loading,
  Modal,
  NumberInput,
  PageHead,
} from "../components/ui";
import { number, t } from "../i18n";
import { bytes, clockTime } from "../lib/format";
import { statisticsInput, useListenersNow } from "../lib/listeners";
import { useLive } from "../lib/live";
import { useAction, useCan } from "../lib/session";
import { ListenLink, OutputState } from "./Dashboard";
import { here, useStation } from "../lib/station";

type Encoder = Schemas["EncoderKind"];
type Kind = Schemas["OutputKind"];
type StreamServerView = Schemas["StreamServerView"];

// Available so far: MP3 and AAC-LC built in, and the external FFmpeg encoder
// (the station's own ffmpeg, for HE-AAC with libfdk_aac); the others arrive in
// M11 and the engine refuses them.
// Labels are functions: some are words of the current language.
const ENCODERS: { value: Encoder; label: () => string; available: boolean }[] = [
  { value: "mp3", label: () => "MP3 (LAME)", available: true },
  { value: "aac", label: () => "AAC-LC", available: true },
  { value: "opus", label: () => "Opus", available: false },
  { value: "vorbis", label: () => "Vorbis", available: false },
  { value: "external_ffmpeg", label: () => t("outputs.externalFfmpeg"), available: true },
];

const KINDS: { value: Kind; label: () => string; available: boolean }[] = [
  { value: "icecast", label: () => "Icecast", available: true },
  { value: "shoutcast_v1", label: () => "Shoutcast v1", available: false },
  { value: "shoutcast_v2", label: () => "Shoutcast v2", available: false },
  { value: "hls", label: () => "HLS", available: false },
  { value: "file", label: () => t("outputs.kind.file"), available: false },
  { value: "pipe", label: () => t("outputs.kind.pipe"), available: false },
];

const MP3_BITRATES = [32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const FFMPEG_BITRATES = [24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 256, 320];
// Suggested encoders of the external FFmpeg; any name its ffmpeg has works.
const FFMPEG_CODECS = ["libfdk_aac", "aac", "libopus", "libmp3lame", "libvorbis"];
const AAC_PROFILES = ["aac_low", "aac_he", "aac_he_v2"];
const SAMPLE_RATES = [22_050, 32_000, 44_100, 48_000];

/** An output's listeners at the last minute, and whether sessions are counted too. */
function ListenerCount({ now, sessions }: Readonly<{ now: Schemas["OutputListeners"] | undefined; sessions: boolean }>) {
  if (!now) {
    return <span className="muted">–</span>;
  }
  if (now.listeners === null || now.listeners === undefined) {
    return (
      <span className="warn small" title={now.error ?? undefined}>
        {t("outputs.statsUnread")}
      </span>
    );
  }
  return (
    <>
      <div>{number(now.listeners)}</div>
      <div className="muted small" title={now.error ?? undefined}>
        {sessions && now.sessions ? t("outputs.withSessions") : t("outputs.countOnly")}
      </div>
    </>
  );
}

export function Outputs() {
  const isAdmin = useCan("admin");
  const stationId = useStation();
  const outputs = useQuery({ queryKey: ["outputs"], queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/outputs", { params: { path: here() } })) });
  const servers = useQuery({
    queryKey: ["stream-servers", stationId],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/stream-servers", { params: { path: here() } })),
  });
  const details = useQuery({
    queryKey: ["settings", "stream-details", stationId],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/stream-details", { params: { path: here() } })),
  });
  const profiles = useQuery({
    queryKey: ["encoder-profiles"],
    queryFn: () => unwrap(api.GET("/api/v1/encoder-profiles")),
  });
  const live = useLive((state) => state.player?.outputs);
  const listeners = useListenersNow();
  const statistics = useModule("listener_statistics");
  const [editingOutput, setEditingOutput] = useState<OutputView | "new" | null>(null);
  const [editingProfile, setEditingProfile] = useState<EncoderProfileView | "new" | null>(null);
  const removeOutput = useAction(
    (id: number) => done(api.DELETE("/api/v1/stations/{station_id}/outputs/{id}", { params: { path: here({ id }) } })),
    { success: t("outputs.deleted"), invalidate: [["outputs"], ["encoder-profiles"], ["stream-servers"]] },
  );
  const duplicate = useAction(
    (id: number) => unwrap(api.POST("/api/v1/stations/{station_id}/outputs/{id}/duplicate", { params: { path: here({ id }) } })),
    { success: t("outputs.duplicated"), invalidate: [["outputs"], ["encoder-profiles"], ["stream-servers"]] },
  );
  const removeProfile = useAction(
    (id: number) => done(api.DELETE("/api/v1/encoder-profiles/{id}", { params: { path: { id } } })),
    { success: t("outputs.profileDeleted"), invalidate: [["encoder-profiles"]] },
  );
  const toggle = useAction(
    (output: OutputView) =>
      unwrap(
        api.PUT("/api/v1/stations/{station_id}/outputs/{id}", {
          params: { path: here({ id: output.id }) },
          body: {
            name: output.name,
            kind: output.kind,
            server_id: output.server_id ?? null,
            encoder_profile_id: output.encoder_profile_id,
            config: output.config,
            details: output.details,
            metadata_enabled: output.metadata_enabled,
            enabled: !output.enabled,
          },
        }),
      ),
    { invalidate: [["outputs"]] },
  );

  if (outputs.isPending || profiles.isPending || servers.isPending) {
    return <Loading />;
  }
  if (outputs.isError || profiles.isError || servers.isError) {
    return <ErrorView error={outputs.error ?? profiles.error ?? servers.error} />;
  }
  const profileName = (id: number) => profiles.data.find((p) => p.id === id)?.name ?? `#${id}`;
  const serverName = (id: number | null | undefined) => servers.data.find((s) => s.id === id)?.name;

  return (
    <>
      <PageHead
        title={t("nav.outputs")}
        description={t("outputs.description")}
        actions={
          isAdmin && (
            <button type="button" className="btn primary" onClick={() => setEditingOutput("new")}>
              <Icon name="plus" /> {t("outputs.new")}
            </button>
          )
        }
      />
      <Card title={t("nav.outputs")} flush>
        {outputs.data.length === 0 ? (
          <Empty>{t("outputs.none")}</Empty>
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("categories.name")}</th>
                  <th className="hide-narrow">{t("outputs.server")}</th>
                  <th className="hide-narrow">{t("outputs.encoding")}</th>
                  <th>{t("review.state")}</th>
                  {statistics && <th className="num">{t("statistics.listeners")}</th>}
                  <th>
                    <span className="visually-hidden">{t("library.actions")}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {outputs.data.map((output) => {
                  const status = live?.find((s) => s.id === output.id) ?? output.status;
                  const config = output.config as Record<string, unknown>;
                  return (
                    <tr key={output.id}>
                      <td>
                        <div>{output.name}</div>
                        <div className="muted small">{KINDS.find((k) => k.value === output.kind)?.label()}</div>
                      </td>
                      <td className="hide-narrow">
                        <div>{serverName(output.server_id) ?? "–"}</div>
                        {typeof config.mount === "string" && <div className="mono small muted">{config.mount}</div>}
                      </td>
                      <td className="hide-narrow">{profileName(output.encoder_profile_id)}</td>
                      <td>
                        <OutputState status={status} />
                        {status?.state === "connected" && status.connected_since_unix_ms && (
                          <div className="muted small">
                            {t("outputs.since", { time: clockTime(status.connected_since_unix_ms), sent: bytes(status.bytes_sent) })}
                          </div>
                        )}
                        {status?.error && <div className="bad small">{status.error}</div>}
                      </td>
                      {statistics && (
                        <td className="num">
                          <ListenerCount
                            now={listeners.data?.outputs.find((o) => o.output_id === output.id)}
                            sessions={servers.data.find((s) => s.id === output.server_id)?.admin_password_set ?? false}
                          />
                        </td>
                      )}
                      <td className="actions">
                        {output.listen_url && <ListenLink url={output.listen_url} />}{" "}
                        {isAdmin && (
                          <>
                            <button type="button" className="btn small" onClick={() => toggle.mutate(output)}>
                              {output.enabled ? t("track.disable") : t("track.enable")}
                            </button>
                            <button type="button" className="btn small" onClick={() => duplicate.mutate(output.id)} disabled={duplicate.isPending}>
                              {t("outputs.duplicate")}
                            </button>
                            <button
                              type="button"
                              className="btn ghost small icon"
                              aria-label={t("categories.edit", { name: output.name })}
                              onClick={() => setEditingOutput(output)}
                            >
                              <Icon name="edit" />
                            </button>
                            <ConfirmButton
                              className="btn ghost small icon danger"
                              title={t("categories.delete", { name: output.name })}
                              confirm={t("categories.deleteButton")}
                              question={<p>{t("outputs.deleteQuestion", { name: output.name })}</p>}
                              onConfirm={() => removeOutput.mutate(output.id)}
                            >
                              <Icon name="trash" />
                            </ConfirmButton>
                          </>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Card>

      <ServersCard servers={servers.data} />

      <Card
        title={t("outputs.profiles")}
        actions={
          isAdmin && (
            <button type="button" className="btn small" onClick={() => setEditingProfile("new")}>
              <Icon name="plus" size={14} /> {t("outputs.newProfile")}
            </button>
          )
        }
        flush
      >
        <table className="table">
          <thead>
            <tr>
              <th>{t("categories.name")}</th>
              <th>{t("outputs.encoding")}</th>
              <th>{t("nav.outputs")}</th>
              <th>
                <span className="visually-hidden">{t("library.actions")}</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {profiles.data.map((profile) => (
              <tr key={profile.id}>
                <td>{profile.name}</td>
                <td className="small">{describeProfile(profile)}</td>
                <td className="num">{number(profile.outputs)}</td>
                <td className="actions">
                  {isAdmin && (
                    <>
                      <button
                        type="button"
                        className="btn ghost small icon"
                        aria-label={t("categories.edit", { name: profile.name })}
                        onClick={() => setEditingProfile(profile)}
                      >
                        <Icon name="edit" />
                      </button>
                      <ConfirmButton
                        className="btn ghost small icon danger"
                        title={profile.outputs > 0 ? t("outputs.profileUsed") : t("categories.delete", { name: profile.name })}
                        disabled={profile.outputs > 0}
                        confirm={t("categories.deleteButton")}
                        question={<p>{t("outputs.deleteProfileQuestion", { name: profile.name })}</p>}
                        onConfirm={() => removeProfile.mutate(profile.id)}
                      >
                        <Icon name="trash" />
                      </ConfirmButton>
                    </>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>

      {editingOutput && (
        <OutputDialog
          output={editingOutput === "new" ? null : editingOutput}
          profiles={profiles.data}
          servers={servers.data}
          station={details.data}
          onClose={() => setEditingOutput(null)}
        />
      )}
      {editingProfile && (
        <ProfileDialog
          profile={editingProfile === "new" ? null : editingProfile}
          onClose={() => setEditingProfile(null)}
        />
      )}
    </>
  );
}

/** The station's stream servers and relays (Outputs › Servers). */
function ServersCard({ servers }: Readonly<{ servers: StreamServerView[] }>) {
  const isAdmin = useCan("admin");
  const statistics = useModule("listener_statistics");
  const [editing, setEditing] = useState<StreamServerView | "new" | null>(null);
  const remove = useAction(
    (id: number) => done(api.DELETE("/api/v1/stations/{station_id}/stream-servers/{id}", { params: { path: here({ id }) } })),
    { success: t("outputs.serverDeleted"), invalidate: [["stream-servers"]] },
  );
  const name = (id: number | null | undefined) => servers.find((s) => s.id === id)?.name ?? `#${String(id)}`;
  return (
    <Card
      title={t("outputs.servers")}
      actions={
        isAdmin && (
          <button type="button" className="btn small" onClick={() => setEditing("new")}>
            <Icon name="plus" size={14} /> {t("outputs.newServer")}
          </button>
        )
      }
      flush
    >
      <p className="muted small" style={{ margin: "0 1rem 0.75rem" }}>
        {t("outputs.serversHelp")}
      </p>
      {servers.length === 0 ? (
        <Empty>{t("outputs.noServers")}</Empty>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>{t("categories.name")}</th>
                <th className="hide-narrow">{t("outputs.address")}</th>
                <th>{t("outputs.usedBy")}</th>
                {statistics && <th className="hide-narrow">{t("outputs.statistics")}</th>}
                <th>
                  <span className="visually-hidden">{t("library.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {servers.map((server) => {
                const used = server.outputs.length + server.relays.length > 0;
                return (
                  <tr key={server.id}>
                    <td>
                      <div>{server.name}</div>
                      {server.relay_of != null && <div className="muted small">{t("outputs.relayOf", { name: name(server.relay_of) })}</div>}
                    </td>
                    <td className="hide-narrow mono small">{serverAddress(server)}</td>
                    <td className="small">
                      {server.relay_of != null
                        ? server.relay_mounts?.map((m) => m.mount).join(", ") ?? t("outputs.allMounts")
                        : [...server.outputs, ...server.relays].map((u) => u.name).join(", ") || "–"}
                    </td>
                    {statistics && (
                      <td className="hide-narrow small muted">{server.admin_password_set ? t("outputs.withSessions") : t("outputs.countOnly")}</td>
                    )}
                    <td className="actions">
                      {isAdmin && (
                        <>
                          <button
                            type="button"
                            className="btn ghost small icon"
                            aria-label={t("categories.edit", { name: server.name })}
                            onClick={() => setEditing(server)}
                          >
                            <Icon name="edit" />
                          </button>
                          <ConfirmButton
                            className="btn ghost small icon danger"
                            title={used ? t("outputs.serverUsed") : t("categories.delete", { name: server.name })}
                            disabled={used}
                            confirm={t("categories.deleteButton")}
                            question={<p>{t("outputs.deleteServerQuestion", { name: server.name })}</p>}
                            onConfirm={() => remove.mutate(server.id)}
                          >
                            <Icon name="trash" />
                          </ConfirmButton>
                        </>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      {editing && <ServerDialog server={editing === "new" ? null : editing} servers={servers} onClose={() => setEditing(null)} />}
    </Card>
  );
}

/** HE-AAC profiles by name; other names as they are. */
function aacProfileLabel(name: string): string {
  switch (name) {
    case "aac_low":
      return t("outputs.aacProfile.aac_low");
    case "aac_he":
      return t("outputs.aacProfile.aac_he");
    case "aac_he_v2":
      return t("outputs.aacProfile.aac_he_v2");
    default:
      return name;
  }
}

function describeProfile(profile: EncoderProfileView): string {
  const encoder = ENCODERS.find((e) => e.value === profile.encoder)?.label() ?? profile.encoder;
  const options = profile.options as Record<string, unknown>;
  const mode =
    profile.encoder === "external_ffmpeg"
      ? [options.codec, options.profile].filter((part) => typeof part === "string").map((part) => ` ${part}`).join("")
      : typeof options.mode === "string"
        ? ` ${options.mode.toUpperCase()}`
        : "";
  const bitrate = profile.bitrate_kbps ? ` ${t("outputs.kbps", { rate: profile.bitrate_kbps })}` : "";
  const rate = t("outputs.khz", {
    rate: number(profile.sample_rate / 1000, { minimumFractionDigits: 1, maximumFractionDigits: 1 }),
  });
  const channels = profile.channels === 1 ? t("outputs.mono") : t("outputs.stereo");
  return `${encoder}${mode}${bitrate}, ${rate} ${channels}`;
}

function SaveFooter({ form, onClose, busy }: { form: string; onClose: () => void; busy: boolean }) {
  return (
    <>
      <button type="button" className="btn" onClick={onClose}>
        {t("common.cancel")}
      </button>
      <button type="submit" form={form} className="btn primary" disabled={busy}>
        {t("common.save")}
      </button>
    </>
  );
}

/** Text of an optional string setting. */
const text = (value: unknown) => (typeof value === "string" ? value : "");

function OutputDialog({
  output,
  profiles,
  servers,
  station,
  onClose,
}: {
  output: OutputView | null;
  profiles: EncoderProfileView[];
  servers: StreamServerView[];
  station: Schemas["StreamDetails"] | undefined;
  onClose: () => void;
}) {
  const initial = (output?.config ?? {}) as Record<string, unknown>;
  const masters = servers.filter((s) => s.relay_of == null);
  const [name, setName] = useState(output?.name ?? "");
  const [kind, setKind] = useState<Kind>(output?.kind ?? "icecast");
  const [profile, setProfile] = useState(
    output?.encoder_profile_id ?? profiles.find((p) => p.name === "MP3 128k")?.id ?? profiles[0]?.id ?? 0,
  );
  const [server, setServer] = useState(output?.server_id ?? masters[0]?.id ?? 0);
  const [mount, setMount] = useState(text(initial.mount) || "/live");
  const [enabled, setEnabled] = useState(output?.enabled ?? true);
  const [metadata, setMetadata] = useState(output?.metadata_enabled ?? true);
  const own = output?.details ?? {};
  const [details, setDetails] = useState<DetailTexts>({
    name: own.name ?? "",
    description: own.description ?? "",
    genre: own.genre ?? "",
    url: own.url ?? "",
  });
  const [listed, setListed] = useState<boolean | null>(own.public ?? null);
  const [json, setJson] = useState(JSON.stringify(initial, null, 2));
  const [jsonError, setJsonError] = useState<string | null>(null);
  const setDetail = (key: keyof DetailTexts, value: string) => setDetails((d) => ({ ...d, [key]: value }));

  const save = useAction(
    () => {
      const icecast = kind === "icecast";
      const input = {
        name,
        kind,
        server_id: icecast ? server : null,
        encoder_profile_id: profile,
        config: (icecast ? { mount } : JSON.parse(json)) as Record<string, never>,
        details: icecast ? overridesFrom(details, listed) : {},
        enabled,
        metadata_enabled: metadata,
      };
      return output
        ? unwrap(api.PUT("/api/v1/stations/{station_id}/outputs/{id}", { params: { path: here({ id: output.id }) }, body: input }))
        : unwrap(api.POST("/api/v1/stations/{station_id}/outputs", { params: { path: here() }, body: input }));
    },
    {
      success: t("outputs.saved"),
      invalidate: [["outputs"], ["encoder-profiles"], ["stream-servers"], ["engine"]],
      onSuccess: onClose,
    },
  );

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (kind !== "icecast") {
      try {
        JSON.parse(json);
        setJsonError(null);
      } catch (error) {
        setJsonError(String(error));
        return;
      }
    }
    save.mutate(undefined);
  };
  const detail = (key: keyof DetailTexts, label: string, type = "text") => (
    <Field label={label}>
      <input
        className="input"
        type={type}
        value={details[key]}
        placeholder={station?.[key] ?? ""}
        onChange={(e) => setDetail(key, e.target.value)}
      />
    </Field>
  );

  return (
    <Modal
      title={output ? t("categories.edit", { name: output.name }) : t("outputs.new")}
      open
      wide
      onClose={onClose}
      footer={<SaveFooter form="output-form" onClose={onClose} busy={save.isPending} />}
    >
      <form id="output-form" className="stack" onSubmit={submit}>
        <div className="form-grid">
          <Field label={t("categories.name")}>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} required />
          </Field>
          <Field label={t("outputs.kind")}>
            <select className="input" value={kind} onChange={(e) => setKind(e.target.value as Kind)}>
              {KINDS.map((k) => (
                <option key={k.value} value={k.value} disabled={!k.available && k.value !== output?.kind}>
                  {k.available ? k.label() : t("outputs.comingLater", { name: k.label() })}
                </option>
              ))}
            </select>
          </Field>
          <Field label={t("outputs.profile")}>
            <select className="input" value={profile} onChange={(e) => setProfile(Number(e.target.value))}>
              {profiles.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <div className="row">
          <label className="check">
            <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} /> {t("outputs.enabled")}
          </label>
          <label className="check">
            <input type="checkbox" checked={metadata} onChange={(e) => setMetadata(e.target.checked)} />{" "}
            {t("outputs.metadata")}
          </label>
        </div>
        {kind === "icecast" ? (
          <>
            {masters.length === 0 && <Banner kind="warn">{t("outputs.noServerYet")}</Banner>}
            <div className="form-grid">
              <Field label={t("outputs.serverField")}>
                <select className="input" value={server} onChange={(e) => setServer(Number(e.target.value))} required>
                  {masters.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.name} ({serverAddress(s)})
                    </option>
                  ))}
                </select>
              </Field>
              <Field label={t("outputs.mount")} hint={t("outputs.mountHint")}>
                <input className="input" value={mount} onChange={(e) => setMount(e.target.value)} required pattern="/.*" />
              </Field>
            </div>
            <fieldset>
              <legend>{t("outputs.stationDetails")}</legend>
              <p className="muted small" style={{ marginTop: 0 }}>
                {t("outputs.stationDetailsHelp")}
              </p>
              <div className="form-grid">
                {detail("name", t("outputs.stationName"))}
                {detail("genre", t("outputs.genre"))}
                {detail("url", t("outputs.website"), "url")}
                {detail("description", t("outputs.descriptionField"))}
                <Field label={t("outputs.public")}>
                  <select
                    className="input"
                    value={listed === null ? "" : String(listed)}
                    onChange={(e) => setListed(e.target.value === "" ? null : e.target.value === "true")}
                  >
                    <option value="">
                      {t("outputs.publicFollow", { value: station?.public ? t("outputs.yes") : t("outputs.no") })}
                    </option>
                    <option value="true">{t("outputs.publicYes")}</option>
                    <option value="false">{t("outputs.publicNo")}</option>
                  </select>
                </Field>
              </div>
            </fieldset>
          </>
        ) : (
          <Field label={t("outputs.json")} hint={t("outputs.jsonHint")}>
            <textarea className="input" value={json} onChange={(e) => setJson(e.target.value)} rows={8} />
          </Field>
        )}
        {jsonError && <Banner kind="bad">{jsonError}</Banner>}
      </form>
    </Modal>
  );
}

/** A stream server, or a relay of one. */
function ServerDialog({
  server,
  servers,
  onClose,
}: {
  server: StreamServerView | null;
  servers: StreamServerView[];
  onClose: () => void;
}) {
  // A server outputs use, or with relays, cannot become a relay.
  const masters = servers.filter((s) => s.relay_of == null && s.id !== server?.id);
  const canRelay = !server || (server.outputs.length === 0 && server.relays.length === 0);
  const [name, setName] = useState(server?.name ?? "");
  const [relayOf, setRelayOf] = useState<number | null>(server?.relay_of ?? null);
  const [host, setHost] = useState(server?.host ?? "");
  const [port, setPort] = useState(server?.port ?? 8000);
  const [tls, setTls] = useState(server?.tls ?? false);
  const [sourceUser, setSourceUser] = useState(server?.source_username ?? "");
  const [sourcePassword, setSourcePassword] = useState("");
  const [publicUrl, setPublicUrl] = useState(server?.public_url ?? "");
  const [mounts, setMounts] = useState(relayMountsText(server?.relay_mounts));
  const [subtractOne, setSubtractOne] = useState(server?.relay_subtract_one ?? false);
  const adminPasswordSet = server?.admin_password_set ?? false;
  const [adminUser, setAdminUser] = useState(server?.admin_username ?? "");
  const [adminPassword, setAdminPassword] = useState("");
  const [removeAdmin, setRemoveAdmin] = useState(false);
  const relay = relayOf !== null;
  const sourcePasswordSet = server?.source_password_set ?? false;

  const save = useAction(
    () => {
      const input: Schemas["StreamServerInput"] = {
        name,
        host,
        port,
        tls: relay && tls,
        source_username: relay ? null : sourceUser.trim() || null,
        public_url: publicUrl.trim() || null,
        relay_of: relayOf,
        relay_mounts: relay ? parseRelayMounts(mounts) : null,
        relay_subtract_one: relay && subtractOne,
        ...statisticsInput(adminUser, adminPassword, removeAdmin),
      };
      if (!relay && sourcePassword) {
        input.source_password = sourcePassword;
      }
      return server
        ? unwrap(
            api.PUT("/api/v1/stations/{station_id}/stream-servers/{id}", { params: { path: here({ id: server.id }) }, body: input }),
          )
        : unwrap(api.POST("/api/v1/stations/{station_id}/stream-servers", { params: { path: here() }, body: input }));
    },
    { success: t("outputs.saved"), invalidate: [["stream-servers"], ["outputs"], ["engine"]], onSuccess: onClose },
  );

  return (
    <Modal
      title={server ? t("categories.edit", { name: server.name }) : t("outputs.newServerTitle")}
      open
      wide
      onClose={onClose}
      footer={<SaveFooter form="server-form" onClose={onClose} busy={save.isPending} />}
    >
      <form
        id="server-form"
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          save.mutate(undefined);
        }}
      >
        <div className="form-grid">
          <Field label={t("categories.name")}>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} required />
          </Field>
          <Field label={t("outputs.kind")}>
            <select className="input" value="icecast" disabled>
              <option value="icecast">Icecast</option>
            </select>
          </Field>
          <Field label={t("outputs.relayOfField")}>
            <select
              className="input"
              value={relayOf ?? ""}
              disabled={!canRelay}
              onChange={(e) => setRelayOf(e.target.value === "" ? null : Number(e.target.value))}
            >
              <option value="">{t("outputs.notRelay")}</option>
              {masters.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
          </Field>
        </div>
        {relay && (
          <p className="muted small" style={{ margin: 0 }}>
            {t("outputs.relayHelp")}
          </p>
        )}
        <fieldset>
          <legend>{t("outputs.icecastServer")}</legend>
          <div className="form-grid">
            <Field label={t("outputs.host")}>
              <input className="input" value={host} onChange={(e) => setHost(e.target.value)} required />
            </Field>
            <Field label={t("outputs.port")}>
              <NumberInput value={port} min={1} max={65535} step={1} onChange={setPort} />
            </Field>
            {!relay && (
              <>
                <Field label={t("outputs.sourceUser")}>
                  <input className="input" value={sourceUser} placeholder="source" onChange={(e) => setSourceUser(e.target.value)} />
                </Field>
                <Field label={t("outputs.sourcePassword")} hint={sourcePasswordSet ? t("outputs.keepPassword") : undefined}>
                  <input
                    className="input"
                    type="password"
                    autoComplete="new-password"
                    value={sourcePassword}
                    placeholder={sourcePasswordSet ? "••••••••" : ""}
                    onChange={(e) => setSourcePassword(e.target.value)}
                    required={!sourcePasswordSet}
                  />
                </Field>
              </>
            )}
            <Field label={t("outputs.publicUrl")} hint={t("outputs.publicUrlHint")}>
              <input
                className="input"
                type="url"
                value={publicUrl}
                placeholder={`http://${host || "host"}:${String(port)}`}
                onChange={(e) => setPublicUrl(e.target.value)}
              />
            </Field>
          </div>
          {relay && (
            <label className="check" style={{ marginTop: "0.75rem" }} title={t("outputs.tlsHint")}>
              <input type="checkbox" checked={tls} onChange={(e) => setTls(e.target.checked)} />
              {t("outputs.tls")}
            </label>
          )}
        </fieldset>
        {relay && (
          <fieldset>
            <legend>{t("outputs.relayMounts")}</legend>
            <Field label={t("outputs.relayMounts")} hint={t("outputs.relayMountsHint")}>
              <textarea className="input mono" rows={3} value={mounts} placeholder={t("outputs.allMounts")} onChange={(e) => setMounts(e.target.value)} />
            </Field>
            <label className="check" style={{ marginTop: "0.75rem" }} title={t("outputs.relaySubtractOneHint")}>
              <input type="checkbox" checked={subtractOne} onChange={(e) => setSubtractOne(e.target.checked)} />
              {t("outputs.relaySubtractOne")}
            </label>
            <p className="muted small">{t("outputs.relaySubtractOneHint")}</p>
          </fieldset>
        )}
        <fieldset>
          <legend>{t("outputs.statistics")}</legend>
          <p className="muted small" style={{ marginTop: 0 }}>
            {t("outputs.statisticsHelp")}
          </p>
          <div className="form-grid">
            <Field label={t("outputs.adminUser")}>
              <input className="input" value={adminUser} placeholder="admin" autoComplete="off" onChange={(e) => setAdminUser(e.target.value)} />
            </Field>
            <Field label={t("outputs.adminPassword")} hint={adminPasswordSet ? t("outputs.keepPassword") : undefined}>
              <input
                className="input"
                type="password"
                autoComplete="new-password"
                value={adminPassword}
                placeholder={adminPasswordSet && !removeAdmin ? "••••••••" : ""}
                disabled={removeAdmin}
                onChange={(e) => setAdminPassword(e.target.value)}
              />
            </Field>
          </div>
          {adminPasswordSet && (
            <label className="check" style={{ marginTop: "0.75rem" }}>
              <input type="checkbox" checked={removeAdmin} onChange={(e) => setRemoveAdmin(e.target.checked)} />
              {t("outputs.removeAdminPassword")}
            </label>
          )}
        </fieldset>
      </form>
    </Modal>
  );
}

function ProfileDialog({ profile, onClose }: { profile: EncoderProfileView | null; onClose: () => void }) {
  const options = (profile?.options ?? {}) as Record<string, unknown>;
  const [name, setName] = useState(profile?.name ?? "");
  const [encoder, setEncoder] = useState<Encoder>(profile?.encoder ?? "mp3");
  const [bitrate, setBitrate] = useState(profile?.bitrate_kbps ?? 128);
  const [sampleRate, setSampleRate] = useState(profile?.sample_rate ?? 44_100);
  const [channels, setChannels] = useState(profile?.channels ?? 2);
  const [mode, setMode] = useState(text(options.mode) || "cbr");
  const [quality, setQuality] = useState(Number(options.quality ?? 3));
  const [vbrQuality, setVbrQuality] = useState(Number(options.vbr_quality ?? 4));
  // The external FFmpeg encoder: HE-AAC v2 by default, AAC+ at 48 kbps.
  const [codec, setCodec] = useState(text(options.codec) || "libfdk_aac");
  const [aacProfile, setAacProfile] = useState(text(options.profile) || (profile ? "" : "aac_he_v2"));
  const [ffmpegVbr, setFfmpegVbr] = useState(typeof options.vbr === "number" ? options.vbr : 0);
  const [format, setFormat] = useState(text(options.format));
  const external = encoder === "external_ffmpeg";
  const chooseEncoder = (next: Encoder) => {
    setEncoder(next);
    if (next === "external_ffmpeg" && !profile) {
      setBitrate(48);
    }
    if (next === "aac" && !profile) {
      setBitrate(96);
      setSampleRate(48_000);
    }
  };
  const ffmpegOptions = () => {
    const chosen: Record<string, string | number> = { codec: codec.trim() };
    if (aacProfile) {
      chosen.profile = aacProfile;
    }
    if (ffmpegVbr > 0) {
      chosen.vbr = ffmpegVbr;
    }
    if (format) {
      chosen.format = format;
    }
    return chosen;
  };
  const bitrates = external || encoder === "aac" ? FFMPEG_BITRATES : MP3_BITRATES;
  const save = useAction(
    () => {
      const body = {
        name,
        encoder,
        bitrate_kbps: external && ffmpegVbr > 0 ? null : bitrate,
        sample_rate: sampleRate,
        channels,
        options: (encoder === "mp3"
          ? mode === "vbr"
            ? { mode, quality, vbr_quality: vbrQuality }
            : { mode, quality }
          : external
            ? ffmpegOptions()
            : options) as Record<string, never>,
      };
      return profile
        ? unwrap(api.PUT("/api/v1/encoder-profiles/{id}", { params: { path: { id: profile.id } }, body }))
        : unwrap(api.POST("/api/v1/encoder-profiles", { body }));
    },
    { success: t("categories.saved"), invalidate: [["encoder-profiles"], ["engine"]], onSuccess: onClose },
  );
  const row = (label: string, control: ReactNode, hint?: string) => (
    <Field label={label} hint={hint}>
      {control}
    </Field>
  );
  return (
    <Modal
      title={profile ? t("categories.edit", { name: profile.name }) : t("outputs.newProfileTitle")}
      open
      onClose={onClose}
      footer={<SaveFooter form="profile-form" onClose={onClose} busy={save.isPending} />}
    >
      <form
        id="profile-form"
        className="stack"
        onSubmit={(event) => {
          event.preventDefault();
          save.mutate(undefined);
        }}
      >
        {profile && profile.outputs > 0 && (
          <Banner kind="warn">
            {t("outputs.profileInUse", { count: profile.outputs })}
          </Banner>
        )}
        <div className="form-grid">
          {row(t("categories.name"), <input className="input" value={name} onChange={(e) => setName(e.target.value)} required />)}
          {row(
            t("outputs.encoder"),
            <select className="input" value={encoder} onChange={(e) => chooseEncoder(e.target.value as Encoder)}>
              {ENCODERS.map((e) => (
                <option key={e.value} value={e.value} disabled={!e.available && e.value !== profile?.encoder}>
                  {e.available ? e.label() : t("outputs.comingLater", { name: e.label() })}
                </option>
              ))}
            </select>,
          )}
          {row(
            (external ? ffmpegVbr > 0 : mode === "vbr") ? t("outputs.bitrateVbr") : t("outputs.bitrate"),
            <select className="input" value={bitrate} onChange={(e) => setBitrate(Number(e.target.value))}>
              {[...new Set([...bitrates, bitrate])].sort((a, b) => a - b).map((b) => (
                <option key={b} value={b}>
                  {t("outputs.kbps", { rate: b })}
                </option>
              ))}
            </select>,
          )}
          {row(
            t("outputs.sampleRate"),
            <select className="input" value={sampleRate} onChange={(e) => setSampleRate(Number(e.target.value))}>
              {SAMPLE_RATES.map((rate) => (
                <option key={rate} value={rate}>
                  {t("outputs.khz", {
                    rate: number(rate / 1000, { minimumFractionDigits: rate % 1000 ? 2 : 0, maximumFractionDigits: 2 }),
                  })}
                </option>
              ))}
            </select>,
          )}
          {row(
            t("outputs.channels"),
            <select className="input" value={channels} onChange={(e) => setChannels(Number(e.target.value))}>
              <option value={2}>{t("outputs.stereoOption")}</option>
              <option value={1}>{t("outputs.monoOption")}</option>
            </select>,
          )}
        </div>
        {external && (
          <fieldset>
            <legend>FFmpeg</legend>
            <p className="muted small" style={{ marginTop: 0 }}>
              {t("outputs.ffmpegHelp")}
            </p>
            <div className="form-grid">
              {row(
                t("outputs.ffmpegCodec"),
                <>
                  <input
                    className="input mono"
                    value={codec}
                    list="ffmpeg-codecs"
                    required
                    pattern="[a-z0-9_]{1,32}"
                    onChange={(e) => setCodec(e.target.value)}
                  />
                  <datalist id="ffmpeg-codecs">
                    {FFMPEG_CODECS.map((name) => (
                      <option key={name} value={name} />
                    ))}
                  </datalist>
                </>,
                t("outputs.ffmpegCodecHint"),
              )}
              {row(
                t("outputs.ffmpegProfile"),
                <select className="input" value={aacProfile} onChange={(e) => setAacProfile(e.target.value)}>
                  <option value="">{t("outputs.ffmpegDefault")}</option>
                  {[...new Set([...AAC_PROFILES, aacProfile].filter(Boolean))].map((name) => (
                    <option key={name} value={name}>
                      {aacProfileLabel(name)}
                    </option>
                  ))}
                </select>,
                t("outputs.ffmpegProfileHint"),
              )}
              {row(
                t("outputs.ffmpegVbr"),
                <select className="input" value={ffmpegVbr} onChange={(e) => setFfmpegVbr(Number(e.target.value))}>
                  <option value={0}>{t("outputs.ffmpegVbrOff")}</option>
                  {[1, 2, 3, 4, 5].map((vbr) => (
                    <option key={vbr} value={vbr}>
                      {vbr}
                    </option>
                  ))}
                </select>,
                t("outputs.ffmpegVbrHint"),
              )}
              {row(
                t("outputs.ffmpegFormat"),
                <select className="input" value={format} onChange={(e) => setFormat(e.target.value)}>
                  <option value="">{t("outputs.ffmpegFormatAuto")}</option>
                  <option value="adts">ADTS (AAC)</option>
                  <option value="mp3">MP3</option>
                  <option value="ogg">Ogg</option>
                </select>,
              )}
            </div>
          </fieldset>
        )}
        {encoder === "mp3" && (
          <fieldset>
            <legend>MP3</legend>
            <div className="form-grid">
              {row(
                t("outputs.mode"),
                <select className="input" value={mode} onChange={(e) => setMode(e.target.value)}>
                  <option value="cbr">{t("outputs.cbr")}</option>
                  <option value="vbr">{t("outputs.vbr")}</option>
                </select>,
                t("outputs.cbrHint"),
              )}
              {row(
                t("outputs.quality"),
                <NumberInput value={quality} min={0} max={9} step={1} onChange={setQuality} />,
                t("outputs.qualityHint"),
              )}
              {mode === "vbr" &&
                row(
                  t("outputs.vbrQuality"),
                  <NumberInput value={vbrQuality} min={0} max={9} step={1} onChange={setVbrQuality} />,
                  t("outputs.vbrQualityHint"),
                )}
            </div>
          </fieldset>
        )}
      </form>
    </Modal>
  );
}
