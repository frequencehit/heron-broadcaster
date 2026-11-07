# Listener interaction

Listeners take part from the station's website: they ask for a song with a dedication, choose the next song, say whether they like the track playing. Each of these is a **show module** (*Dedications*, *Match*, *Top 30*, and *Likes*), built on the common base described here: a public gateway, opening hours, a page and a widget for the website, and notifications. Built in [M18](roadmap/m18-listener-interaction.md); each show adds its own part on top.

## The pieces

```
 listeners' browsers ──► website (any host)
        │  <script src=".../public/stations/main/dedications.js">
        ▼
 ┌──────────────┐   one request a second   ┌───────────┐  gRPC  ┌──────────────┐
 │ heron-public │ ───── GET /gateway/state ──► heron-api ├──────► │ heron-engine │
 │   gateway    │ ── checked writes, batches ──►         │        └──────────────┘
 └──────────────┘    (gateway token only)  └───────────┘
   internet side                             private network
```

- **`heron-public`** is a service of its own, the only part of Heron listeners reach. It can run on another host, facing the internet, while `heron-api` and the engines stay on the private network. It has no database access: it talks to the API over HTTP with a **gateway token**, which only the gateway routes accept.
- **`heron-api`** keeps each show's settings, decides what is accepted and plays, and sends the notifications.
- **The engine** is never reached from the public side.

## The public gateway

### Reads, from its cache

- Once a second, the gateway reads `GET /api/v1/gateway/state` from the API: every enabled station with its now playing and the public state of its shows that are on. That one request feeds every widget: 5,000 open widgets cost the API what one costs.
- Widgets follow a station with **Server-Sent Events** (`GET …/stations/{code}/events`): the station's state when they connect, then each change (a new track, a show opening or closing, a module's live data). The gateway decides itself whether a show is open, from its hours and the station's time zone, so a show opens and closes on time even while the API does not answer.
- The public endpoints of the API (`now-playing`, `queue`, `history`) are served from the gateway's cache too, for 5, 5 and 15 seconds.
- While the API does not answer, the gateway serves what it last read; `GET /healthz` of the gateway says since when.

### Writes, checked first

A write (a dedication, a vote) goes through, in order:

1. **The address's limit**: requests per minute per address, reads and writes counted apart. Over it: `429`.
2. **The show**: its module on for the station, the write one it takes, and the show open now (else `404` or `409`). A website not listed in `HERON_PUBLIC_ALLOWED_ORIGINS` is refused before anything (`403`).
3. **The anti-robot proof**: a proof of work computed by the browser (below). Missing or wrong: `403`.
4. **The fields**: each text field has a maximum size; unknown fields are refused (`422`).
5. **The device's limit**: what one device may send, set by each show (one dedication per opening range, one vote per match). Over it: `429`.
6. **The API's limit**: writes sent on to the API per second, for every listener together. Over it: `429`, and the API sees nothing more.

A show can also ask that a field be one of a few values (`like` or `dislike`), and that a field match its live state (`play_id`: the track playing, see [Likes](#likes)): else `422` or `409`.

Then a write is either **sent on** to the API at once (a dedication: one request, answered with the API's answer), **added up** in the gateway and sent every second in one batch (votes: `202` for the listener), or **kept per device**, the device's last write only, and sent every second in one batch with each device's hash (likes: `202`).

When the gateway is flooded, its limits fill and listeners get `429`: the API and the engines see no more than the usual batches, and the air carries on.

### Listeners, without storing who they are

- **The proof of work**: `GET …/public/challenge` gives a challenge (valid 5 minutes, used once) and a difficulty; the browser finds a `nonce` such that `SHA-256(challenge + ":" + nonce)` starts with that many zero bits (16 by default: about a second on a phone), and sends `X-Heron-Proof: <challenge>:<nonce>` with its write. It needs no outside service. Turnstile and hCaptcha are not built in.
- **The device**: a keyed hash (HMAC-SHA-256) of the address, the browser's `User-Agent` and `Accept-Language`, with a key the gateway draws when it starts and never writes down. It counts the device's writes, in the gateway's memory only, until the opening range ends; the API receives it (`X-Heron-Device`) so that a show can refuse a second dedication from the same device in a range. **No IP address is stored**, by the gateway or the API. A gateway restarted draws a new key: the counts start again.
- No cookie, no fingerprinting script, nothing kept in the browser.

### Cross-origin requests

Reads answer any origin (`Access-Control-Allow-Origin: *`), like the API's public endpoints. Writes answer only the websites listed in `HERON_PUBLIC_ALLOWED_ORIGINS` (`https://radio.example.org`, comma-separated): a browser on another origin is refused (`403`). An empty list allows any origin.

### Settings

`heron-public` reads its environment:

| Variable | Default | What it is |
|---|---|---|
| `HERON_PUBLIC_API_URL` | `http://heron-api:8080` | Where it reaches the API |
| `HERON_PUBLIC_TOKEN` | — | The gateway token; the same value as `heron-api`'s `HERON_PUBLIC_TOKEN` |
| `HERON_PUBLIC_ALLOWED_ORIGINS` | empty: any | The websites that may send writes |
| `HERON_PUBLIC_CLIENT_IP_HEADER` | empty: the peer's address | Behind a reverse proxy, the header it puts the listener's address in (`X-Forwarded-For`: its last value is used, `X-Real-IP`) |
| `HERON_PUBLIC_READS_PER_MINUTE` | 300 | Reads per address per minute |
| `HERON_PUBLIC_WRITES_PER_MINUTE` | 20 | Writes (and challenges) per address per minute |
| `HERON_PUBLIC_WRITES_PER_SECOND` | 50 | Writes sent on to the API per second, everyone together |
| `HERON_PUBLIC_STREAMS_PER_ADDRESS` | 10 | Event streams open per address |
| `HERON_PUBLIC_MAX_STREAMS` | 10000 | Event streams open in all |
| `HERON_PUBLIC_PROOF_BITS` | 16 | Difficulty of the proof of work, 0 (off) to 24 |
| `HERON_HTTP_ADDR` | `0.0.0.0:8084` | The public port: the website's requests |
| `HERON_OPS_ADDR` | `0.0.0.0:8085` | `/healthz` and `/metrics`, kept off the public port |

`docker-compose.yml` starts the gateway with the `public` profile only (`docker compose --profile public up -d`): an installation without shows opens no public port. The token is any long random string, `openssl rand -hex 32` for example, put in `.env` as `HERON_PUBLIC_TOKEN`: `docker-compose.yml` gives it to both services. Without it, the API refuses the gateway (`401`) and the gateway answers `503`. Changing it means restarting both.

## Shows and their hours

The shows are [modules](modules.md): *Dedications* (`dedications`), *Match* (`match`), *Top 30* (`top30`), off until an admin turns them on, and *Likes* (`likes`), on by default. Each has, per station, its settings (`GET`, `PUT /stations/{id}/shows/{module}`, operators):

- **Opening hours**, in **whole hours**, in the station's time zone: ranges of days and hours (Monday to Friday, 17:00 to 18:00; up to 24:00). `enabled` off closes the show whatever its hours.
- **An hour belongs to one show at most**, on a station: saving hours that another show of the station has is refused (`409`, saying which hours and which show). The hours of a show that is off or not enabled still count, so that turning it back on finds them free.
- **Outside its hours**, a show's API refuses writes (`409`) and its widget says when it opens next.

[Likes](#likes) have no hours: they are open all the time (while *enabled*), and do not take hours from the others. Saving hours for them is refused (`422`).

### Closed

Per show, outside its hours:

- **a message**, set by the station, with `{day}` and `{hour}` for the next opening (`Dedications are back {day} at {hour}` → *Dedications are back Monday 9 November at 17:00*), written in the listener's language by the widget; empty, the widget's own sentence;
- **or nothing at all**: the page and the small widget show nothing.

## On the website

Each show has, served by the gateway:

- **a dedicated page**, for a page of the website of its own:

  ```html
  <script src="https://public.radio.example.org/api/v1/public/stations/main/dedications.js" data-mode="page" async></script>
  ```

- **a small widget**, the same script without `data-mode`, for a column of any page. The station can turn it off (*small widget*): the script then shows nothing.
- **a page of its own**, `…/stations/{code}/{module}.html`, the dedicated page alone, to try it or to link to.

The script draws where it is put (or in the element `data-target="#id"` names), in a shadow DOM so that the website's styles do not reach it, in the show's **color** and with its **title**. It writes in the language of `data-lang`, else of the page (`<html lang>`), else of the browser: English, French, Spanish or German. It follows the station's events, so it changes without reloading.

What a show shows when it is open is the show's own (the dedication form, the vote…). Until it brings it, the widget shows that the show is open, until when, and the track playing.

The address of the gateway, as the website reaches it, is the station's **gateway address** (*Settings › Station*, `gateway_url`): the lines to copy start with it.

The website can also build its own pages on the [public API of the gateway](api.md#the-public-gateway).

## Notifications

Each show tells the team or the community what happens, through the station's [webhooks](api.md#webhooks), whose body templates already format messages for Discord, Slack or any other service. On each show's page:

- **which webhooks** receive it (the station's, chosen by name);
- **each event on or off, with its message**:
  - `show.upcoming`: a set time before it opens (60 minutes by default, 1 to 1440);
  - `show.started`, with the time it ends;
  - `show.ended`, with the next opening;
  - and each show's own events (the [Top 30](#top-30)'s, #131, #132);
- **the language** of the dates in the messages (English, French, Spanish, German).

Placeholders in the messages: `{name}` (the show's title on the website), `{link}` (the website's page of the show, else its page on the gateway), `{end}` (the time it closes, `18:00`), `{next}` (the next opening, `Monday 9 November 17:00`). An unknown placeholder is refused (`422`). A show adds its own (`{artist}`, `{title}`…).

A show event is sent to the webhooks chosen on the show's page, whatever events the webhook itself lists. Its JSON body carries `show` (`module`, `name`, `link`, `end_unix_ms`, `next_unix_ms`) and `message`, the message written out; a body template uses `{{message}}`: `{"content": "{{message}}"}` for Discord, `{"text": "{{message}}"}` for Slack. A webhook whose template only knows the track (`{{artist}}`) sends an empty line for a show event: give the shows a webhook of their own, with the same address.

Events are not sent while the webhooks or the show are turned off, nor for times that passed while the API was stopped.

## Likes

*Likes* (`likes`) let listeners say whether they like the track playing. A show with no hours, **on by default**: open all the time while the module is on and the show *enabled*. The weekly [Top 30](#top-30) ranks the tracks on these votes. Like every show, it reaches listeners through the gateway: nothing is voted on until `heron-public` runs.

### Voting

- **Only the track playing.** The show's live state (`state` in the gateway's state and events) names the play listeners can vote on, `{"play_id": 1234}`, or is `null` while nothing can be voted on. A vote names that play: a vote for another one, a track that ended, is refused (`409`).
- **Only music**: a track of the library on deck A or B, not one that sends no metadata (jingles, ads, medleys, voice tracks) and not one from the fallback directory. While a jingle plays, nothing can be voted on.
- **Like or dislike**: `POST …/stations/{code}/likes/vote` with `{"play_id": 1234, "vote": "like"}` (or `"dislike"`) and the proof of work, answered `202`.
- **One vote per device per play.** A device can change its mind while the track plays: its last vote counts, once. The same track played again is another play, voted on again.
- **Kept per device in the gateway**, the last vote of each device only, and sent to the API every second in one batch, with each device's hash. The API counts a vote while its play is on air and up to 10 seconds after it ends (for the batch on its way); it drops the others.

### The score

The **share of likes**, `likes / (likes + dislikes)`, shown with the number of votes (*79 %, 611 votes*). A track is ranked, among the week's most and least liked, from **10 votes** in the period; below, its share is shown but it is *not ranked yet*.

### What is kept

- **Per play**, the likes and dislikes of each play, with its station, its track and when it started, kept as long as the history. Heron adds them up per track and per station: over this week, 30 days or always, the week's most and least liked tracks, and the week the [Top 30](#top-30) ranks.
- **Per device**, the device's hash and its vote on a play, so that a device changes its vote instead of adding one: deleted an hour after the play ends, and a day after the vote at the latest. No IP address.

The votes are in the database dump of the [backups](backups.md), with the history, not in the configuration file.

### In Heron

- **On air**: the likes and dislikes of the track playing, as they come.
- **The track's page**: for the station shown, this week, 30 days and always (share and votes), and its latest plays with their votes. Hovering a play in the plays grid gives its votes.
- **The track list**: a *Liked* column (share and votes over 7, 30 or 365 days, on the station listed), its sort, and filters on the share and the number of votes (*under 40 %, at least 20 votes*: the tracks to rest, or whose [ratio](rotation.md#play-ratio) to lower).
- **Shows › Likes** and the **Statistics** page: the most liked and least liked tracks of the week.

### Notifications

No notification per vote: there would be too many. Likes have no hours, so they send no `show.upcoming`, `show.started` or `show.ended` either. The week's results go out with the [Top 30](#top-30).

### On the website

The page and the small widget show the track playing with two buttons, *I like* and *I don't like*. The one chosen stays pressed until the track ends; another click changes the vote. While nothing can be voted on (a jingle, nothing on air), they say so. Nothing is kept in the browser.

## Top 30

*Top 30* (`top30`) is a weekly show: once a week, for two hours, the station plays the 30 tracks its listeners liked most that week, counting down from 30 to 1, then a new release. Heron builds the chart from the [likes](#likes) and plays the show on its own. Off until an admin turns the module on.

### The chart

- **The week**: from Monday 00:00 to its **close**, a day and an hour of the week (Sunday 17:00 by default), in the station's time zone. The plays that started in it count. Votes on plays after the close and before Monday count for no chart.
- **Ranked by the score of the likes**: the share of likes, among the tracks with at least the **minimum votes** (10 by default) over the week. Ties go to the track with the most votes, then the oldest track.
- **Eligible tracks**: the tracks of the **eligible categories**, chosen by the station; none chosen: every category of the station whose type sends metadata (its music). Only music is voted on anyway.
- **Its size**: 30 tracks by default, 1 to 50. With fewer ranked tracks the chart is shorter: 12 ranked tracks make a Top 12, and the show counts down from 12.
- **Frozen at the close**: the chart is kept as it stands then, and does not change any more. Charts are kept for good, one per week, and can be browsed.
- **Movements**, against the chart of the week before: up or down so many places, the same place, **new** (never in a chart before) or **re-entry** (in an earlier chart, not last week's). And the **weeks in the chart**, this one included.
- **One chart per station**, from the votes on that station: stations that share their music still have their own listeners.
- **No limit of weeks**: a track can stay at number 1 as long as listeners like it most. There is no *gold* list for now.

**Taking a track out.** A programmer can take a track out of the chart of the week, or of a closed chart whose show has not started yet (a track removed from the library, a mistake). The tracks after it move up, and the chart keeps the tracks taken out, with who did it and when. It can be put back under the same condition. Once the show has started from a chart, the chart cannot change (`409`).

### The show

The show's **hours are its slot** in the schedule: Sunday 18:00 to 20:00, say. They are the show's [hours](#shows-and-their-hours), which no other show can take; outside them, the clocks play as usual.

- **The programme is built when the show starts**: when the queue manager needs the first track planned in the slot (with the queue's depth, a few minutes before at most). It plays the **latest chart closed before the slot starts**: with a close at Sunday 17:00 and a show at 18:00, the chart of the week that just ended.
- **The order**: a jingle, number 30, a jingle, number 29… number 1, a jingle, then **the new release**. The jingles come from the **jingle category** of the show: a jingle of a rank when the category has one (its title ends with the rank after a dash or `#`: *Top 30 – 12*, *Top 30 #12*), else the category's other jingles in turn, the least recently played first. Without a jingle category, the tracks follow one another.
- **Fitted to the slot**: when the programme is longer than the slot, the lowest ranks are left out, with their jingles, until it fits (the new release stays). The show's page says so before the show. When it is shorter, rotation fills the rest of the slot, as the hour's clock says.
- **Rules**: separation rules do not apply inside the show (a track at number 1 for weeks plays every week, and two tracks of the same artist can follow each other). **Restrictions do**: a track a restriction forbids at its planned time is passed over, with its jingle, and the show's page says why. So is a track that cannot play any more (disabled, missing, out of the station's library).
- **In the queue**: the programme's tracks are queued in order as the queue needs them, like rotation's (`queue.min_queued` ahead), each jingle linked to its track. With autofill off, nothing is queued: the show does not play either. They are rotation's plays in the history, not in the rotation log: the show's page lists the programme. A programme started plays to its end, even when the slot is over, and is not built again.
- Nothing is built while the chart is empty: rotation plays the slot.

**The new release** comes from the **new release category** of the show. Proposed: the most liked track of that category over the chart's week, among those not in the chart (any number of votes; ties go to the most votes); none voted on: the track of the category added last to the library. A programmer can choose another track of the category before the show, on the show's page; the programme takes the choice it finds when it is built. Without a new release category, the show ends at number 1.

### Notifications

Besides `show.upcoming`, `show.started` and `show.ended`:

- `top.published`, on by default: when the week closes, with `{artist}` and `{title}` (number 1) and `{top3}` (*1. Artist – Title, 2. …*). Default message: *This week's number 1: {artist} – {title}*. It tells the result before the show: turn it off to keep the suspense, or send it to the team's webhook only.
- `top.rank`, off by default: each rank as it starts playing in the show, with `{rank}`, `{artist}` and `{title}`. Default message: *Number {rank}: {artist} – {title}*.

### On the website

The page and the small widget show:

- **During the show**: the chart being played. The ranks still to come are **hidden** (*? ? ?, the answer on air*): the gateway's state does not hold their tracks, only their rank. A rank shows once it starts playing, or once a rank after it does (a rank passed over shows with the next). The rank on air is highlighted, and the new release shows when it plays.
- **Outside the show**: the chart of the **last show that started**, with its movements and its new release, under the closed message (*Back Sunday at 18:00*). A chart that closed but has not been played yet is never shown. Before the first show, the closed message alone. With *nothing* outside the hours, nothing at all.

The small widget shows the first five ranks (during the show: the rank on air and the next one to come); the page shows them all. The [likes](#likes) widget says while a vote counts for the next Top 30 (*Your vote counts for Sunday's Top 30*), and the track's page in Heron gives the track's rank in the chart of the week as it stands.

### What is kept

The charts (each rank with its track, its likes, dislikes and plays with votes, its movement), the tracks taken out, the new release chosen and the programme of each show, for good, in the database dump of the [backups](backups.md). The settings are in the configuration file. Nothing about listeners: the votes are the likes'.

## Personal data

- **The gateway** keeps nothing on disk. In memory: the hash of each device that wrote, with its count, until the opening range ends (at most a day); a device's last like until the next batch, a second; the challenges given, 5 minutes. Addresses are counted for the per-address limits for a minute, and dropped.
- **The API** stores what a show keeps (a dedication's first name and message, the device's hash), and deletes it after the time the show says, written on the show's page; no IP address. Each show's part of these docs says what it keeps and for how long.
- **Nothing a listener writes reaches the air, the website or a webhook without a person's approval.**

## Building a show on the base

What a show module adds, and where:

| Where | What |
|---|---|
| `heron_core::Module` and `Modules` | Its key, and its default (off for a show, on for Likes) |
| `heron_core::shows::Show` | Its entry: key, whether it has hours, its own notification events with their default messages, its own placeholders |
| `heron-api`, `shows.rs` | Its part of the public state (`ShowPublic::state`, any JSON: the current match, the day's dedications), read once a second by the gateway |
| `heron-api`, `queue.rs` | A show that plays on air (the [Top 30](#top-30)): the queue manager asks it for its programme before rotation picks (`top30::due`) |
| `heron-api`, gateway routes | Its writes, under `/api/v1/gateway/stations/{code}/{module}/…`, behind `GatewayClient` and `gateway::open_show`, which check the token, the module and the hours |
| `heron-public`, `actions.rs` | Its writes as the gateway takes them: path, fields and their sizes (and their values, or the field of its state they must match), the device's limit, sent on, added up or kept per device |
| `heron-public`, `widget.js` | Its renderer: what the page and the small widget show while it is open |
| `web/src/pages/Show.tsx` | Its tabs (the live view: requests, the match in progress) and its settings, next to the common ones |
| `docs/` | Its part: what it keeps, its routes, its events |
