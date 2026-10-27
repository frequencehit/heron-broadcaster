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
- An **output** is one destination: a sink kind, its connection settings (host, port, mount, credentials…), the encoder profile it uses, metadata settings and an enabled flag.
- Several outputs can share one encoder profile: identical encodings are produced **once** and fanned out to every output that uses them.
- Any number of outputs can be configured, to the same or different stream servers.

## Encoders

The user picks the encoder for each profile.

| Encoder | Codec | Implementation | License |
|---|---|---|---|
| **MP3** | MP3 (CBR/VBR) | Built in, LAME | LGPL |
| **AAC** | AAC-LC | Built in, FFmpeg's native `aac` encoder (libavcodec built without `--enable-nonfree`) | LGPL (GPL builds are also compatible) |
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

### External FFmpeg encoder

- Heron **never ships, builds or links fdk-aac**. Selecting an fdk-aac codec requires the user to provide their own fdk-aac-enabled `ffmpeg` binary (configured path, e.g. mounted into the `heron-engine` container or in a custom image).
- The engine spawns the binary, writes raw PCM (f32le, stereo, 48 kHz) to its stdin and reads the encoded stream from its stdout, which is then sent to the outputs like any built-in encoder.
- Heron checks at configuration time that the binary exists and supports the chosen codec (`ffmpeg -encoders`), and reports a clear error otherwise.
- If the process dies, it is restarted with backoff; other encoders and outputs are not affected.

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

The default profiles are seeded by a migration in [M2](roadmap/m2-engine-core.md); AAC arrives in [M11](roadmap/m11-sinks-live-dj.md), so `AAC 96k` is seeded with it.

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

Source client for Icecast 2.4 and later (HTTP `PUT`). Metadata goes through `/admin/metadata` as `Artist - Title`, with the source credentials. Settings (`outputs.config`):

| Setting | Default |
|---|---|
| `host` | required |
| `port` | 8000 |
| `mount` | required, starts with `/` |
| `username` | `source` |
| `password` | required |
| `name`, `description`, `genre`, `url` | none (shown in stream directories) |
| `public` | `false` (list in public directories) |
| `public_url` | none: `http://host:port` (server address as listeners reach it, e.g. `https://stream.example.org`) |

The stream plays at the public URL followed by the mount. The **Outputs** and **On air** pages link to it (**Listen**).

Listener statistics read the server's status page, and its listener list with the server's **admin** credentials: an optional setting of the output, kept apart from `outputs.config` and never pushed to the engine ([Listener statistics](statistics.md#settings)).

TLS is not supported yet.

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

Manage them through the API (`/api/v1/encoder-profiles`, `/api/v1/outputs`, admins only; [API and control plane](api.md#engine-configuration)) until the web UI's editor ([M5](roadmap/m5-web-ui.md)). For example, to the local Icecast of the `icecast` Compose profile:

```json
POST /api/v1/outputs
{"name": "Local Icecast", "kind": "icecast", "encoder_profile_id": 1,
 "config": {"host": "icecast", "port": 8000, "mount": "/live", "password": "hackme", "name": "My station",
            "public_url": "http://localhost:8010"}}
```

The API never sends passwords back; an update without one keeps the stored password. Each output in `GET /api/v1/outputs` carries its live state from the engine and, for Icecast, its `listen_url`.

## Delivery

- [M2 – Engine core](roadmap/m2-engine-core.md): encoder/output model, multiple outputs, MP3 encoder, Icecast sink.
- [M4 – API & control plane](roadmap/m4-api-control-plane.md): profiles and outputs stored in MariaDB, edited through the API and pushed to the engine.
- [M11 – More encoders, sinks & live DJ](roadmap/m11-sinks-live-dj.md): AAC-LC, Opus, Vorbis, external FFmpeg encoder, HLS, Shoutcast, file recording, pipe.
