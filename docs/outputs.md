# Encoders and outputs

How audio leaves `heron-engine`. Outputs are pluggable ("bring your own streamer"), and a station can feed **several stream servers at once**, each with its own encoder settings.

## Model

```
                                  ┌──────────────────────┐     ┌─────────────────────────┐
                              ┌──►│ Encoder: MP3 128k    │──┬─►│ Output: Icecast A /live │
                              │   └──────────────────────┘  └─►│ Output: Shoutcast B     │
 master bus ──► encoder fan-out   ┌──────────────────────┐     └─────────────────────────┘
 (f32 stereo 48 kHz)          ├──►│ Encoder: AAC-LC 96k  │──┬─►│ Output: Icecast C /aac  │
                              │   └──────────────────────┘  └─►│ Output: HLS             │
                              │   ┌──────────────────────┐     └─────────────────────────┘
                              └──►│ Encoder: external    │────►│ Output: Icecast D /he   │
                                  │ ffmpeg (user binary) │     └─────────────────────────┘
                                  └──────────────────────┘
```

- An **encoder profile** describes one encoding: encoder kind, bitrate, sample rate, channels and encoder-specific options.
- A **stream server** is a server a station streams to (Icecast today): its address, port and credentials, kept once per station and shared by its outputs ([Stream servers](#stream-servers)).
- An **output** is one destination: a sink kind, the stream server it goes to and what is its own there (Icecast: its mount), the encoder profile it uses, the station details it sends, metadata settings and an enabled flag.
- Several outputs can share one encoder profile: identical encodings are produced **once** and fanned out to every output that uses them.
- Any number of outputs can be configured, to the same or different stream servers.

## Encoders

The user picks the encoder for each profile.

| Encoder | Codec | Implementation | License |
|---|---|---|---|
| **MP3** | MP3 (CBR/VBR) | Built in, LAME | LGPL |
| **AAC** | AAC-LC | Built in, optional: FFmpeg's native `aac` encoder from the system's libavcodec, linked dynamically (built without `--enable-nonfree`) | LGPL (GPL builds are also compatible) |
| **Opus** | Opus (Ogg) | Built in, libopus | BSD |
| **Vorbis** | Vorbis (Ogg) | Built in, libvorbis | BSD |
| **External FFmpeg** | Anything the provided binary supports, e.g. HE-AAC v1/v2 with `libfdk_aac` | User-provided `ffmpeg` binary, run as a child process | User's responsibility |

### MP3

Built in with LAME, compiled from source with Heron (LGPL, linked statically). Profile fields: `bitrate_kbps` (8 to 320, MP3 bitrates only), `sample_rate` (8 to 48 kHz, MP3 rates only; the master bus is resampled from 48 kHz), `channels` (1 or 2). Options (`encoder_profiles.options`):

| Option | Values | Default |
|---|---|---|
| `mode` | `cbr`, `vbr` | `cbr` |
| `quality` | LAME's algorithm quality, 0 (best, slowest) to 9 | 3 |
| `vbr_quality` | With `vbr`, 0 (best) to 9 | 4 |

### Built-in AAC

Plain AAC-LC without HE (SBR/PS) is sufficient for current players and bitrates, so HE-AAC is not a built-in feature.

Unlike LAME, there is no AAC-LC encoder to compile into Heron: FFmpeg's native `aac` encoder lives in libavcodec. It is therefore **optional at build time**. The `aac` Cargo feature of `heron-outputs` (forwarded by `heron-engine`, and enabled in Heron's Docker image builds) links the system's libavcodec dynamically (LGPL), through the `ffmpeg-next` bindings; it is off by default so that a plain `cargo build` needs no FFmpeg library. Heron's engine image is built with it, and installs libavcodec (Debian's build, without fdk-aac). The API never links it: it checks an AAC profile like any other. An engine built without the feature accepts the profile too, and reports `this Heron was built without AAC-LC` on the profile's outputs (*Outputs* and *On air*) while the other profiles keep streaming.

To build it yourself, install FFmpeg 7.1's development libraries (`libavcodec-dev`, `libavformat-dev`, `libavutil-dev`, `libswresample-dev` and `libclang-dev` on Debian, `pkg-config`), then `cargo build --features heron-engine/aac`.

libavcodec returns raw AAC frames; Heron frames them as ADTS (`audio/aac`), the stream format Icecast servers and players expect.

Profile fields: `bitrate_kbps` (8 to 512, required, CBR), `sample_rate` (the AAC rates, 7.35 to 96 kHz; the master bus is resampled from 48 kHz), `channels` (1 or 2). No options: `encoder_profiles.options` must be empty.

### External FFmpeg encoder

- Heron **never ships, builds or links fdk-aac**. Selecting an fdk-aac codec requires the user to provide their own fdk-aac-enabled `ffmpeg` binary (configured path, e.g. mounted into the `heron-engine` container or in a custom image).
- The engine spawns the binary, writes raw PCM (f32le, stereo, 48 kHz) to its stdin and reads the encoded stream from its stdout, which is then sent to the outputs like any built-in encoder.
- When it starts the binary, the engine checks that it runs and has the chosen codec (`ffmpeg -encoders`), and reports a clear error otherwise, on the profile's outputs (*Outputs* and *On air*): `ffmpeg has no libfdk_aac encoder (…)`.
- If the process dies, or writes nothing for 10 s while it is fed, it is killed and started again after 1 s, then 2, 4… up to 30 s; other encoders and outputs are not affected. The engine never waits on it: audio it does not take in time is dropped (`heron_external_encoder_dropped_chunks_total`), failures are counted (`heron_external_encoder_failures_total`).
- On an engine upgrade, the new engine starts its own `ffmpeg` ([Upgrading](upgrading.md)).

**The binary** is a deployment setting of the engine, never an API one: `HERON_FFMPEG` (`ffmpeg` on the engine's `PATH` when unset; the engine's image has none). A static build avoids libraries missing from the engine's Debian image. With Docker Compose, mount it with a `docker-compose.override.yml` next to `docker-compose.yml`:

```yaml
services:
  heron-engine:
    volumes:
      - /opt/ffmpeg-fdk:/opt/ffmpeg-fdk:ro
```

and set `HERON_FFMPEG=/opt/ffmpeg-fdk/ffmpeg` in `.env`. Such a binary is built with `--enable-libfdk-aac --enable-nonfree`, which makes it non-redistributable: build it for the station's own use.

**Profile fields**: `bitrate_kbps` (8 to 512, unless `vbr`), `sample_rate` (8 to 96 kHz; ffmpeg resamples the 48 kHz master bus), `channels` (1 or 2; HE-AAC v2 needs 2). Options (`encoder_profiles.options`), and nothing else: no free command-line arguments, so a profile cannot make the engine run anything but the configured binary with these values.

| Option | Values | Default |
|---|---|---|
| `codec` | An FFmpeg encoder name, as `ffmpeg -encoders` lists it: `libfdk_aac`, `aac`, `libopus`, `libmp3lame`… (letters, digits, `_`) | required |
| `profile` | The encoder's profile (`-profile:a`): `aac_low`, `aac_he` (HE-AAC, AAC+), `aac_he_v2` (HE-AAC v2, for 32 to 48 kbps) | the encoder's |
| `vbr` | Variable bitrate mode (`-vbr`, `libfdk_aac`), 1 to 5, instead of `bitrate_kbps` | none |
| `format` | The stream's container: `adts` (AAC, `audio/aac`), `mp3` (`audio/mpeg`), `ogg` (`audio/ogg`) | `adts` for AAC encoders, `mp3` for MP3, `ogg` otherwise |

An AAC+ stream for mobile listeners: `{"codec": "libfdk_aac", "profile": "aac_he_v2"}` at 48 kbps, 44.1 kHz, stereo.

### Default profiles

A fresh install creates two encoder profiles, so a new station only has to enter its server details:

| Profile | Encoder | Mode | Bitrate | Sample rate | Channels | Default for |
|---|---|---|---|---|---|---|
| `MP3 128k` | MP3 (LAME) | CBR | 128 kbps | 44.1 kHz | stereo | new Icecast / Shoutcast outputs |
| `AAC 96k` | AAC-LC | CBR | 96 kbps | 48 kHz | stereo | new HLS outputs |

Why these:

- **MP3 128 kbps** plays everywhere, including hardware internet radios, smart speakers and stream directories. CBR keeps the bitrate shown in directories accurate. 44.1 kHz is the rate these devices expect for MP3; resampling from the internal 48 kHz is cheap.
- **AAC-LC 96 kbps** sounds about as good as MP3 128 kbps with 25 % less bandwidth, and is the codec HLS players expect. It stays at the internal 48 kHz.
- **Opus** is not created by default: it is the most efficient codec, but some hardware players and stream directories still do not support it. Stations can add an Opus profile in one click.
- No outputs are created by default: they need the station's own server address and credentials.

The default profiles are seeded by migrations: `MP3 128k` in [M2](roadmap/m2-engine-core.md), `AAC 96k` with the AAC-LC encoder in [M11](roadmap/m11-sinks-live-dj.md).

## Outputs (sinks)

All sinks implement the `OutputSink` trait: they receive encoded frames and now-playing metadata.

| Sink | Notes |
|---|---|
| **Icecast** | Source client, metadata updates. First sink to be built. |
| **Shoutcast v1/v2** | Source client, metadata updates. |
| **HLS** | Segments the encoded stream (AAC-LC by default; MP3 also possible) and writes segments plus a rolling `.m3u8` playlist to a directory, to be served by any HTTP server or CDN. Configurable segment duration and playlist window. |
| **File recording** | Writes the encoded stream to files, with rotation. |
| **Pipe** | Sends an encoded stream to an external process. |

### Icecast

Source client for Icecast 2.4 and later (HTTP `PUT`). Metadata goes through `/admin/metadata` as `Artist - Title`, with the source credentials. An Icecast output picks a [stream server](#stream-servers) (`server_id`) and has only its own settings in `outputs.config`:

| Setting | Default |
|---|---|
| `mount` | required, starts with `/` |

What it sends to the server about the station (name, description, genre, website, public or not) are the [station details](#station-details), each one overridable on the output.

The stream plays at the server's public address followed by the mount. The **Outputs** and **On air** pages link to it (**Listen**).

TLS is not supported yet for sending: a server outputs send to cannot have TLS on. A relay server, which Heron only reads, can.

### Stream servers

A station keeps a list of **stream servers** (*Outputs › Servers*). Several outputs going to the same server (an MP3 and an AAC stream, two mounts) pick it instead of each typing its address and passwords.

| Setting | Default | Meaning |
|---|---|---|
| `name` | required | Unique on the station; shown in the outputs list |
| `kind` | `icecast` | Icecast only for now |
| `host` | required | The server's address, as the engine and the API reach it |
| `port` | 8000 | |
| `tls` | `false` | Relays only for now (see above) |
| `source_username` | `source` | The source login outputs connect with |
| `source_password` | required (except relays) | Never sent back by the API |
| `admin_username` | `admin` | The admin login, for the [listener statistics](statistics.md#settings) |
| `admin_password` | none | Never sent back; without it, Heron only counts concurrent listeners |
| `public_url` | none: `http://host:port` | The server's address as listeners reach it, e.g. `https://stream.example.org` |
| `relay_of` | none | The master server, for a [relay](statistics.md#relay-servers) |
| `relay_mounts` | none: all of the master's | A relay's mounts: `[{"mount": "/live"}, {"mount": "/aac", "relay_mount": "/aac-relay"}]`, with the mount's name on the relay when it differs |
| `access_log` | none | Where Heron reads the server's access log for [audience certification](certification.md#where-it-is-read-from): a file or an address |
| `relay_subtract_one` | `false` | A relay whose address the master does not see (behind NAT): its connection to the master counts as 1 listener per mount, subtracted |

- **The engine still gets the full configuration of each output**, as before: the API joins the server's address, port and source credentials, the output's mount and the station details into the output's settings when it pushes them ([Configuration flow](#configuration-flow)). The engine never reads the database.
- **Changing a server changes every output using it**: the API pushes the outputs again, and the engine reconnects those whose settings changed (a new address or source password). The admin credentials, the public address and the relay settings are not pushed: changing only them reconnects nothing.
- A server used by an output, or the master of a relay, cannot be deleted: `409`, with the outputs and relays that use it.
- A **relay** (`relay_of` set) is a server repeating the master's mounts: Heron sends nothing to it and no output can pick it; it is only read for the statistics, and needs no source credentials. Its master must be a server of the same station that is not itself a relay.

### Station details

What an output tells the server about the station, shown in stream directories: `name`, `description`, `genre`, `url` (the website) and `public` (listed in public directories, `false` by default). They **come from the station** (*Settings › Station*, `GET`, `PUT /settings/stream-details`): set once, every output sends them.

An output can **override each of them** (`details` of the output: a field set there replaces the station's, one left out follows it, so it changes when the station's does). An empty text overrides with nothing: that output sends none. The 256k stream can be called "My station HD" while the others send the station's name.

### Duplicate

**Duplicate** (`POST /outputs/{id}/duplicate`) copies an output: the same server, encoder profile, metadata setting and station details, the name followed by `(copy)` and the mount by `-copy` (`/live-copy`; `-copy-2` and on when taken), and **disabled**, so that two outputs never fight for one mount. Change its mount and profile, then enable it.

### Metadata

Every output with metadata on gets the artist and title of the track on air when it starts, from decks A and B (the FX player never sends any). A track whose category's **type** sends no metadata (*Categories › Category types › Sends metadata*, off for Jingle and Ad from the start, see [Rotation – Category types](rotation.md#category-types)) **sends none**: while it plays, listeners' players keep showing the song before it. The public now-playing shows that song too, the public history leaves such tracks out, and webhooks report them only when they include the FX player ([API](api.md#webhooks)).

## Isolation and reliability

- Encoders and outputs run **outside the audio thread**. The mixer writes to a lock-free ring buffer per encoder; a slow or failed encoder or output never blocks the mixer or other outputs.
- Each network output reconnects on its own with exponential backoff (1 s, doubling up to 60 s). After a reconnection the stream starts with fresh audio, not a backlog.
- **Network outputs connect through `heron-relay`** when `HERON_RELAY_SOCKET` is set (it is in `docker-compose.yml`): the engine reaches the relay through a Unix socket, and the relay holds the TCP connection to the stream server, so it outlives the engine's container. Metadata updates go through it too: Icecast accepts them (with the source's credentials) only from the source's address. When the relay cannot be reached, outputs connect directly.
- **On an engine upgrade, connections are handed over, not reopened** ([Audio engine – Upgrades and handover](audio-engine.md#upgrades-and-handover)): the old engine flushes its encoders and passes each connected output's connection to the new engine ([Engine handover](handover.md#handing-over-the-connections)), which goes on writing to it without a new source request. Icecast (and Shoutcast, from [M11](roadmap/m11-sinks-live-dj.md)) keeps the source and its listeners. The new engine's encoders start fresh: MP3 continues frame after frame, Ogg (Opus, Vorbis) as a new chained stream. An output that was reconnecting, and outputs that are not a connection (HLS, file recording, pipes, external FFmpeg encoders), start again in the new engine.
- An output's state (connected, reconnecting, error, bytes sent) is exposed through gRPC, the API and `/metrics`. Listener counts come from the stream servers' statistics, collected by `heron-api` ([M7b](roadmap/m7b-listener-statistics.md)).

## Configuration flow

Encoder profiles and outputs are edited in the web UI and stored by `heron-api` in MariaDB. The API pushes the configuration to the engine over gRPC (`SetOutputs`, JSON with `encoder_profiles` and `outputs` in the shape of the tables); the engine keeps a local copy so it restarts with its outputs even when the API or database is down (the engine never reads the database, see [Locked decisions](decisions.md)).

A new configuration is validated as a whole: an invalid one changes nothing. Outputs whose settings and encoder profile are unchanged keep streaming without a reconnection; an encoder runs only while an enabled output uses it.

Manage them on the **Outputs** page, or through the API (`/api/v1/encoder-profiles`, `/api/v1/stream-servers`, `/api/v1/outputs`, admins only; [API and control plane](api.md#engine-configuration)). For example, to the local Icecast of the `icecast` Compose profile:

```json
POST /api/v1/stream-servers
{"name": "Local Icecast", "host": "icecast", "port": 8000, "source_password": "hackme",
 "public_url": "http://localhost:8010"}

POST /api/v1/outputs
{"name": "Live MP3", "kind": "icecast", "encoder_profile_id": 1, "server_id": 1,
 "config": {"mount": "/live"}, "details": {"name": "My station HD"}}
```

The API never sends passwords back; an update without one keeps the stored password. Each output in `GET /api/v1/outputs` carries its live state from the engine and, for Icecast, its `listen_url`.

When it pushes the outputs, the API keeps the engine's own settings of an output when they mean the same as the database's (the same server, mount and details, written differently: a default left out, an empty text): such an output is unchanged for the engine, and keeps streaming.

## Delivery

- [M2 – Engine core](roadmap/m2-engine-core.md): encoder/output model, multiple outputs, MP3 encoder, Icecast sink.
- [M4 – API & control plane](roadmap/m4-api-control-plane.md): profiles and outputs stored in MariaDB, edited through the API and pushed to the engine.
- [M11 – More encoders, sinks & live DJ](roadmap/m11-sinks-live-dj.md): AAC-LC, Opus, Vorbis, external FFmpeg encoder, HLS, Shoutcast, file recording, pipe.
