# Listener interaction

Listeners take part from the station's website: they ask for a song with a dedication, choose the next song, say whether they like the track playing. Each of these is a **show module** (*Dedications*, *Match*, *Tops*, *Featured artist* and *Likes*), built on the common base described here: a public gateway, opening hours, a page and a widget for the website, and notifications. Built in [M18](roadmap/m18-listener-interaction.md); each show adds its own part on top.

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
5. **The device's limit**: what one device may send, set by each show (a few dedications per opening range, one vote per match). Over it: `429`.
6. **The API's limit**: writes sent on to the API per second, for every listener together. Over it: `429`, and the API sees nothing more.

A show can also ask that a field be one of a few values (`like` or `dislike`), and that a field match its live state (`play_id`: the track playing, see [Likes](#likes)): else `422` or `409`.

Then a write is either **sent on** to the API at once (a dedication: one request, answered with the API's answer), **added up** in the gateway and sent every second in one batch (`202` for the listener), or **kept per device**, the device's last write only, and sent every second in one batch with each device's hash (likes and the match's votes, which a device can change: `202`).

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

The shows are [modules](modules.md): *Dedications* (`dedications`), *Match* (`match`), *Tops* (`top30`), *Featured artist* (`featured_artist`), off until an admin turns them on, and *Likes* (`likes`), on by default. Each has, per station, its settings (`GET`, `PUT /stations/{id}/shows/{module}`, operators):

- **Opening hours**, in **whole hours**, in the station's time zone: ranges of days and hours (Monday to Friday, 17:00 to 18:00; up to 24:00). `enabled` off closes the show whatever its hours.
- **An hour belongs to one show at most**, among the shows that are on: saving hours that another show of the station has is refused (`409`, saying which hours and which show). A show that is **off** (its module turned off for the station or the installation) or **not enabled** holds no hours: another show can take them. Turning it back on (its module, or `enabled`) is then refused (`409`, saying which hours and which show) until one of the two moves.
- **Outside its hours**, a show's API refuses writes (`409`) and its widget says when it opens next.

The [tops](#tops) are shows too, several per station, each with its own settings (`/stations/{id}/tops/…`) and its show as a recurrence with minutes (*every Sunday, 18:00 to 20:00*) instead of whole hours; a Match or Dedications hour a top's show touches is theirs as well.

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

### Styling it from the website

The website's styles do not reach inside the widget, but its own stylesheet can change it through names that stay the same from one version to the next, whatever the widget's inner classes become. The element the script draws in has the classes `heron-widget` and `heron-{module}` (`heron-match`; a top has `heron-top30` and its key, `heron-top-4`), and `data-mode` (`page` or `widget`).

**Variables**, set on that element (or any parent), each with its default:

| Variable | Default | What it changes |
|---|---|---|
| `--heron-font` | the system font | The font family |
| `--heron-font-size` | `15px` | The text size (titles and figures keep their own: style them by their part) |
| `--heron-text` | dark grey, light grey in a dark color scheme | The text color |
| `--heron-muted` | mid grey | Secondary text |
| `--heron-bg` | white, near black in a dark color scheme | The background (one value for both schemes once set) |
| `--heron-border` | light grey | Borders and separators |
| `--heron-radius` | `14px` | The corners of the card |
| `--heron-max-width` | `420px` for the small widget, none for the page | The card's width at most |

**Parts**, for anything else (size, spacing, colors of one zone), with `::part()`:

| Part | What it is |
|---|---|
| `page`, `widget` | The whole widget, as a page or as the small widget |
| `card`, `header`, `title`, `subtitle`, `body` | The card, its colored header with the show's title and its line under it, and its content |
| `closed` | The message outside the hours |
| `now-playing`, `track`, `cover` | The track playing, a track's name, a cover |
| `duel`, `question`, `choice`, `versus`, `percent`, `bar`, `countdown` | The match: the two tracks, the question above them, each track to vote for, *against* between them, its share, its bar, the countdown |
| `votes`, `vote-button` | The likes' buttons |
| `chart`, `rank`, `movement`, `tag`, `new-release` | A top: the chart, a rank, its movement, a badge, the new release |
| `field`, `input`, `results`, `result`, `send-button`, `quote`, `steps`, `today` | The dedications: a field, a text box, the tracks found, one of them, the button sending it, the message, the steps of a request, today's dedications |
| `pill`, `link`, `error` | A small status badge, a link-like button, an error |

```css
.heron-match { --heron-font: "Inter", sans-serif; --heron-radius: 6px; }
.heron-match::part(title) { font-size: 28px; }
.heron-match::part(choice) { padding: 24px; }
.heron-widget[data-mode="widget"] { --heron-max-width: 100%; }
```

The show's color stays its own (*On the website*); the widget's layout for small screens and reduced motion stays too.

## Notifications

Each show tells the team or the community what happens, through the station's [webhooks](api.md#webhooks), whose body templates already format messages for Discord, Slack or any other service. On each show's page:

- **which webhooks** receive it (the station's, chosen by name);
- **each event on or off, with its message**:
  - `show.upcoming`: a set time before it opens (60 minutes by default, 1 to 1440);
  - `show.started`, with the time it ends;
  - `show.ended`, with the next opening;
  - and each show's own events (the [Dedications](#dedications)', the [Match](#match)'s and the [Tops](#tops)');
- **the language** of the dates in the messages (English, French, Spanish, German).

Placeholders in the messages: `{name}` (the show's title on the website), `{link}` (the website's page of the show, else its page on the gateway), `{end}` (the time it closes, `18:00`), `{next}` (the next opening, `Monday 9 November 17:00`). An unknown placeholder is refused (`422`). A show adds its own (`{artist}`, `{title}`…).

A show event is sent to the webhooks chosen on the show's page, whatever events the webhook itself lists. Its JSON body carries `show` (`module`, `name`, `link`, `end_unix_ms`, `next_unix_ms`) and `message`, the message written out; a body template uses `{{message}}`: `{"content": "{{message}}"}` for Discord, `{"text": "{{message}}"}` for Slack. A webhook whose template only knows the track (`{{artist}}`) sends an empty line for a show event: give the shows a webhook of their own, with the same address.

Events are not sent while the webhooks or the show are turned off, nor for times that passed while the API was stopped.

## Likes

*Likes* (`likes`) let listeners say whether they like the track playing. A show with no hours, **on by default**: open all the time while the module is on and the show *enabled*. The [Tops](#tops) rank the tracks on these votes and their plays. Like every show, it reaches listeners through the gateway: nothing is voted on until `heron-public` runs.

### Voting

- **Only the track playing.** The show's live state (`state` in the gateway's state and events) names the play listeners can vote on, `{"play_id": 1234}`, or is `null` while nothing can be voted on. A vote names that play: a vote for another one, a track that ended, is refused (`409`).
- **Only music**: a track of the library on deck A or B, not one that sends no metadata (jingles, ads, medleys, voice tracks) and not one from the fallback directory. While a jingle plays, nothing can be voted on.
- **Like or dislike**: `POST …/stations/{code}/likes/vote` with `{"play_id": 1234, "vote": "like"}` (or `"dislike"`) and the proof of work, answered `202`.
- **One vote per device per play.** A device can change its mind while the track plays: its last vote counts, once. The same track played again is another play, voted on again.
- **Kept per device in the gateway**, the last vote of each device only, and sent to the API every second in one batch, with each device's hash. The API counts a vote while its play is on air and up to 10 seconds after it ends (for the batch on its way); it drops the others.

### The score

The **share of likes**, `likes / (likes + dislikes)`, shown with the number of votes (*79 %, 611 votes*). A track is ranked, among the week's most and least liked, from **10 votes** in the period; below, its share is shown but it is *not ranked yet*.

### What is kept

- **Per play**, the likes and dislikes of each play, with its station, its track and when it started, kept as long as the history. Heron adds them up per track and per station: over this week, 30 days or always, the week's most and least liked tracks, and the periods the [Tops](#tops) rank.
- **Per device**, the device's hash and its vote on a play, so that a device changes its vote instead of adding one: deleted an hour after the play ends, and a day after the vote at the latest. No IP address.

The votes are in the database dump of the [backups](backups.md), with the history, not in the configuration file.

### In Heron

- **On air**: the likes and dislikes of the track playing, as they come.
- **The track's page**: for the station shown, this week, 30 days and always (share and votes), and its latest plays with their votes. Hovering a play in the plays grid gives its votes.
- **The track list**: a *Liked* column (share and votes over 7, 30 or 365 days, on the station listed), its sort, and filters on the share and the number of votes (*under 40 %, at least 20 votes*: the tracks to rest, or whose [ratio](rotation.md#play-ratio) to lower).
- **Shows › Likes** and the **Statistics** page: the most liked and least liked tracks of the week.

### Notifications

No notification per vote: there would be too many. Likes have no hours, so they send no `show.upcoming`, `show.started` or `show.ended` either. The results go out with the [Tops](#tops).

### On the website

The page and the small widget show the track playing with two buttons, *I like* and *I don't like*. The one chosen stays pressed until the track ends; another click changes the vote. While nothing can be voted on (a jingle, nothing on air), they say so. Nothing is kept in the browser.

## Tops

*Tops* (`top30`) are the station's charts, played on air: each **top** ranks the tracks of a period by their plays and their [likes](#likes), freezes its chart when the period ends, and plays it in its show, counting down from its last rank to number 1, then a new release. A station has as many tops as it wants (*the Top 30 of the week*, *the Top 3 of the evening*, *the Top 50 of the month*), each with its own chart, period and show. Off until an admin turns the module on (its key stays `top30`).

### A top

Each top has, per station:

- **a name** (60 characters, *Top 3 of the evening*), its title on the website unless *On the website* gives another;
- **a size**: 3, 5, 10, 20, 30, 40 or 50, shown *Top 30*. With fewer ranked tracks the chart is shorter: 12 ranked tracks make a Top 12, and the show counts down from 12;
- **a period**, **a score**, **eligible tracks**, **a show** and its jingles and new release, described below;
- **On the website** and **notifications**, as every show has them, its own.

*Enabled* off, a top plays nothing, its page and widget on the website stay closed and it sends nothing; its charts are still frozen. A top can be deleted, with its charts.

### The period

What the chart counts, in the station's time zone:

- **The last 7 days** (`rolling_7_days`): frozen just before each show of the top, when its programme is built, over the 7 days before the show opens. A top without a show freezes nothing: its chart is the last 7 days, live.
- **The week** (`week`): from a **reset**, a day of the week and a time (Sunday 17:00), to the next.
- **The month** (`month`): from a reset, a day of the month (1 to 28) and a time (the 1st at 00:00), to the next.

**The reset is the freeze**: the chart of a week or a month runs from one reset to the next, and is frozen at that moment, while the next one starts. Every play counts for one chart: there is no gap between two periods.

A chart is named by the day it is frozen (`2026-11-08`, in the station's time zone): the reset that ends it, or the show it is frozen for.

### The score

```
score = p × plays + (1 − p) × likes
```

- **`p`**, *plays or votes*, 0 to 100 % by steps of 10: at 100 %, the most played tracks; at 0 %, the most liked, as the Top 30 ranked them before.
- **plays**: the track's plays in the period, divided by the plays of the most played eligible track (which counts 100 %). Plays during a top's show (its slot, from when it opens to when it closes, any top of the station) do not count: a top would feed itself.
- **likes**: the share of likes, `likes / (likes + dislikes)`, of the plays in the period. A track without a vote counts 0.
- **Minimum votes** to enter the chart (10 by default, 0 to 1000): 0, plays alone are enough. A track below it is listed apart, *not enough votes*, with the rank its score would give it.
- **Ties** go to the track with the most votes, then the most plays, then the oldest track. A track scoring 0 is not ranked.
- **Eligible tracks**: the tracks of the **eligible categories** of the top; none chosen: every category of the station whose type sends metadata (its music).

The chart keeps each rank's plays, its share of the most played, its likes and dislikes, and its score: the page draws the score as two parts, the plays' and the likes'.

### The chart

- **Live** until it is frozen, then kept as it stood, for good. The page also shows the next ranks after the size, greyed, to see who is just behind, and the tracks without enough votes.
- **Movements**, against the top's previous chart: up or down so many places, the same place, **new** (never in one of its charts before) or **re-entry** (in an earlier one, not the previous). And the **charts it was in**, this one included.
- **One chart per top and per station**, from the plays and votes on that station: stations that share their music still have their own listeners.
- **No limit**: a track can stay at number 1 as long as it scores the most. There is no *gold* list for now.

**Taking a track out.** A programmer can take a track out of the live chart, or of a frozen chart whose show has not started yet (a track removed from the library, a mistake). The tracks after it move up, and the chart keeps the tracks taken out, with who did it and when. It can be put back under the same condition. Once a show has started from a chart, the chart cannot change (`409`).

### The show

A top's show is **a recurrence and hours**, in the station's time zone:

- **days of the week**, chosen one by one (Monday to Sunday: one day, three, all seven), **every week** or **the 1st, 2nd, 3rd, 4th or last** of them in the month (*the 1st Saturday and Sunday of the month*);
- **from** a time **to** a time (18:00 to 20:00, minutes included; up to 24:00, not across midnight).

No show: the top's charts are kept, and nothing plays.

- **It plays the latest chart frozen before the show opens**: a weekly top reset on Sunday at 17:00 and played at 18:00 plays the week that just ended; a monthly top played on the 1st Sunday plays last month's. A top of the last 7 days freezes its chart for the show, when the programme is built.
- **Two tops on the same slot** (their hours overlap on a day both play): the **rarer recurrence takes it that day**: a top of the month (the 1st Sunday) before a top of every week. The other gives way for that day: it does not play at all then, and the list of tops says so. Two tops of the same rarity whose hours overlap on a day both can play (every Monday to Sunday and every Sunday, the 1st Sunday and the 1st Sunday, the 4th Sunday and the last) are refused when saved (`422`), saying which: a top of every evening and a top of Sunday evening at the same time take Monday to Saturday and Sunday. A top that is not enabled takes no slot from the others and is not counted in that check either: enabling it is then refused (`422`) while a top of the same rarity has its hours.
- **Its hours are its own**, as every show's: a Match or Dedications hour (whole hours) that a top's show touches, on a day it can play, is refused when either is saved (`409`, saying which). As for every show, only the shows that are on count: a top not enabled, or the tops turned off, leave their hours to the Match and the Dedications, and the other way round. Outside the tops' shows, the clocks play as usual.
- **The programme is built when the show starts**: when the queue manager needs the first track planned in the slot (with the queue's depth, a few minutes before at most).
- **The order**: a jingle, the last rank, a jingle… number 1, a jingle, then **the new release**. The jingles come from the **jingle category** of the top: a jingle of a rank when the category has one (its title ends with the rank after a dash or `#`: *Top 30 – 12*, *Top 30 #12*), else the category's other jingles in turn, the least recently played first. Without a jingle category, the tracks follow one another.
- **Fitted to the slot**: when the programme is longer than the slot, the lowest ranks are left out, with their jingles, until it fits (the new release stays). The show's tab says so before the show. When it is shorter, rotation fills the rest of the slot, as the hour's clock says.
- **Rules**: separation rules do not apply inside the show (a track at number 1 for weeks plays every week, and two tracks of the same artist can follow each other). **Restrictions do**: a track a restriction forbids at its planned time is passed over, with its jingle, and the show's tab says why. So is a track that cannot play any more (disabled, missing, out of the station's library, nothing audible).
- **In the queue**: the programme's tracks are queued in order as the queue needs them, like rotation's (`queue.min_queued` ahead), each jingle linked to its track. With autofill off, nothing is queued: the show does not play either. They are rotation's plays in the history, not in the rotation log: the show's tab lists the programme. A programme started plays to its end, even when the slot is over, and is not built again.
- Nothing is built while the chart is empty: rotation plays the slot.

**The new release** comes from the **new release category** of the top (optional). Proposed: the most liked track of that category over the chart's period, among those not in the chart (any number of votes; ties go to the most votes); none voted on: the track of the category added last to the library. A programmer can choose another track of the category before the show, on its tab; the programme takes the choice it finds when it is built. Without a new release category, the show ends at number 1.

### Never off air

A top runs inside the queue fill, so **nothing a top does may stop the music**:

- A top that fails (a table it cannot read, settings it cannot parse after a restore or a downgrade) is logged as a warning and left out: rotation fills the queue as without it, and the other tops play.
- An item of the programme that cannot play when it is queued (disabled, missing, nothing audible since its cue points changed) is passed over, with its jingle, and the programme goes on with the next one. The check is the one every queued track goes through.

### Notifications

Each top has its own notifications, as every show: `show.upcoming`, `show.started` and `show.ended` at its show's times (not on a day it gives way), and:

- `top.published`, on by default: when a chart is frozen (the reset, or the show's opening for a top of the last 7 days), with `{artist}` and `{title}` (number 1) and `{top3}` (*1. Artist – Title, 2. …*). Default message: *{name}: number 1 is {artist} – {title}*. It tells the result before the show: turn it off to keep the suspense, or send it to the team's webhook only.
- `top.rank`, off by default: each rank as it starts playing in the show, with `{rank}`, `{artist}` and `{title}`. Default message: *Number {rank}: {artist} – {title}*.

`{name}` is the top's title on the website, else its name.

### On the website

Each top has its page and its small widget, by its key, `top-{id}`: `…/stations/main/top-4.js`, `top-4.html`. The first top of the list is also `top30` (`top30.js`, `top30.html`), so the lines already on a website keep working. They show:

- **During the show**: the chart being played. The ranks still to come are **hidden** (*? ? ?, the answer on air*), unless the top shows them (*hide the ranks not played yet* off): the gateway's state does not hold their tracks, only their rank. A rank shows once it starts playing, or once a rank after it does (a rank passed over shows with the next). The rank on air is highlighted, and the new release shows when it plays.
- **Outside the show**: the chart of its **last show that started**, with its movements and its new release, under the closed message (*Back Sunday at 18:00*). A frozen chart that has not been played yet is never shown. Before the first show, the closed message alone. With *nothing* outside the hours, nothing at all.

The small widget shows the first five ranks (during the show: the rank on air and the next one to come); the page shows them all. The [likes](#likes) widget says which top the votes of now count for next (*Your vote counts for Top 30, Sunday*): the top whose next show playing them comes first. The track's page in Heron gives the track's rank in each top's live chart.

### What is kept

The tops' settings (the configuration file of the [backups](backups.md), with the website and notifications); their charts (each rank with its track, its likes, dislikes, plays and score, its movement), the tracks taken out, the new release chosen and the programme of each show, for good, in the database dump. Nothing about listeners: the votes are the likes'.

### From the Top 30

The single weekly Top 30 of earlier versions becomes the first top of each station that had set it up (its settings, its hours or a chart), with its history: charts, tracks taken out, new releases chosen, programmes, settings, website and notifications. Its period becomes the **week**, reset at its former close (Sunday 17:00 by default), and its score 100 % likes. Its show's hours become its show: the first range's days every week, from its first to its last hour. Its charts keep their dates as they were: from Monday 00:00 to the close. Votes are no longer lost between the close and Monday: they count for the next chart.

## Dedications

*Dedications* (`dedications`) let listeners ask for a song with a message during the show's hours (17:00 to 18:00 on weekdays, say). An operator accepts or refuses each request; an accepted song goes into the queue, and its dedication shows on the website when it plays. Off until an admin turns the module on.

### Asking

- **A track the station can play**: the listener searches the station's requestable tracks by artist or title (`GET …/dedications/search?q=neon`, 2 to 60 characters, 8 tracks at most) and picks one. The requestable tracks are those of the **requestable categories** the station chooses; none chosen: every category of the station whose type sends metadata (its music). Only tracks the station can play now count: enabled, on file, analyzed, not a medley, not disabled on the station. No free text.
- **The dedication**: the listener's **first name** or pseudonym (`from`, 40 characters at most), **who it is for** (`to`, 60 characters) and **a message** (140 characters by default, 20 to 500, set by the station; it may be empty). No email, no phone number.
- Sent with `POST …/stations/{code}/dedications` and the proof of work: `{"track_id": 42, "from": "Marc", "to": "Julie", "message": "Happy birthday!"}`. The answer carries a **follow link**: the request waiting, accepted (planned at about 17:40), played, or refused.
- **Limits**: requests **per device per opening range** (1 by default, 1 to 10), and **one waiting at a time** per device; requests **per opening range**, for every listener together (30 by default, 1 to 1000): past it, *full for today* (`409`). A track already waiting from another listener can be asked again: the operator sees both.
- Outside the hours, the API refuses requests (`409`) and the search too.

### Deciding

On *Shows › Dedications*, for the station's operators, requests come in as they arrive, without reloading: the track, who it is from and for, the message, how long ago, and **what the rules say** about it now:

- *rules kept*;
- *artist played 12 min ago (separation: 20 min)*, *title played…*, *track played…*: the station's separation rules, against what played and what is queued;
- *already in the queue, at 17:52*;
- *held back by a restriction (not before 20:00)*;
- *cannot play* (disabled, missing since it was asked).

**Accept** puts the track in the queue, with the source `request`:

- **next**: right after the track on air;
- **within the hour**: at the end of the tracks queued now, before the show closes;
- **at the best place**: the first place in the queue where the separation rules hold, else within the hour.

An accepted request **may break the separation rules**: a person approved it, and the page said so before. **Restrictions always apply**: a request a restriction forbids at its planned time cannot be accepted (`409`), and *at the best place* looks for a place where none forbids it. The play ratio does not hold a request back. Nothing is read on air: the message shows on the *On air* page while the track plays, for a host who is there.

**Refuse**, with a reason or none: *the track just played*, *the track is not available*, *inappropriate message*. The listener's follow link says *not this time*, with the reason (except an inappropriate message). **A refused request is never shown publicly.**

Requests still waiting when the range closes are refused on their own, *the range closed*. Accepted ones not played yet stay in the queue.

### Once accepted

- The queue holds the track as any other: an operator can move it or take it out (the follow link then says *not played*).
- **When the track plays**, the dedication goes with it: `dedication` (`from`, `to`, `message`) in the public `now-playing` and the gateway's state, in the `track.started` webhook (`{{dedication_from}}`, `{{dedication_to}}`, `{{dedication_message}}`), and on the *On air* page. The request is then *played*, with its play in the history, as long as it is kept.
- The page and the small widget list **today's dedications**: those accepted or played since midnight, with the planned or played time, the track, *For Julie, from Marc* and the message. Waiting and refused requests never show.

The message on the stream's metadata: no, the website only.

### Notifications

Besides `show.upcoming`, `show.started` and `show.ended`:

- `dedication.played`, on by default: when an accepted dedication goes on air, with `{artist}`, `{title}`, `{from}`, `{to}`, `{message}` and `{time}`. Default message: *♥ {artist} – {title}, for {to} from {from}: « {message} »*.
- `dedication.accepted`, off by default: when a request is accepted, with the same placeholders, `{time}` being when it is planned. Default message: *Dedication accepted for {to}, around {time}*.

Only accepted dedications are sent, with the message as the operator read it. No email or message goes to the listener: the follow link is enough.

### What is kept

- **Each request**: the track, `from`, `to`, the message, the opening range it was sent in, its status (waiting, accepted, played, refused, with the reason, who decided and when), its place in the queue and its play: deleted **30 days** after it was sent by default (1 to 365, the station's *Keep first names and messages*), whether the module is on or not.
- **The device's hash**, to count its requests in the range: deleted when the range closes. No IP address, no email.
- **The follow link**'s key, random, with the request: deleted with it.

The requests are in the database dump of the [backups](backups.md); the settings are in the configuration file.

## Match

*Match* (`match`) lets listeners choose the next song between two. During the show's hours, every few tracks or every track, two tracks face each other: listeners vote on the website while the track before them plays, and the one with the most votes plays next. The two tracks come from the slot of the hour's clock the winner takes, or from one category. Off until an admin turns the module on.

### When a match runs

- **During the show's hours** (18:00 to 19:00 on weekdays, say), which no other show of the station has.
- **Every N tracks** (4 by default, 1 to 20): the winner of a match is every Nth track. The vote runs during the track before it, so with 4: *winner, track, track, the vote's track, winner…* Only the tracks rotation queues count; a dedication or a track queued by hand is not counted.
- **The first winner is the first track of the range:** the first match of a range votes during the **last track before it** (the one starting at 16:56 for a range from 17:00), so that its winner starts the range, after what the clock puts at its start (an exact time at 17:00:00: the vote closes 10 seconds before it, and the winner follows it). The show counts as open from that first vote: the website shows it, and `match.started` is sent when it opens; `show.started` stays at the range's start. With every N tracks, the counting starts from that first winner.
- **Every track** (N = 1): each winner is the vote's track of the next match, so every track of the hours is chosen by the listeners: *the first track, winner 1 (the vote for 2 open), winner 2 (the vote for 3 open)…* The next match's two tracks are queued behind the winner when its vote closes, and the next vote opens when the winner starts. When a match cannot run (a slot without two tracks, say), its slot plays the track rotation picks, which carries the next vote: the chain goes on.
- **What the clock puts between** the vote's track and the winner (a jingle slot, a jingle [linked](rotation.md#linked-slots) to the winner's slot, a slot with an [exact time](rotation.md#exact-times)) plays between them, as the clock says: the vote closes 10 seconds before the first of them starts, and the winner follows them.
- **The vote's track** is the track rotation picks there, as usual: music (a category whose type sends its metadata) of 60 seconds or more, long enough for a vote of 30 seconds at least. A jingle, or a track too short to vote during, does not carry a vote: the match waits for the next track.
- **The last match of a range** is the last one whose winner **starts** before the range closes; a match whose winner would start after it does not run, and rotation plays on.
- **One match at a time**, and **none without somewhere to take the tracks from**: the hour's clock (with *the tracks from the clock*), or the match's category. Without either, no match runs, and the show's page says so.
- **With autofill only**, like any pick of rotation: with autofill off, no match runs.

### The two tracks

Where they come from is the station's choice (*the tracks of the match*):

- **From the clock** (`clock`): the winner takes the place of the **next music slot of the hour's clock**, and both tracks are picked **as that slot would pick its track, twice**: its category, its selection method, its *enforce rules*, its separation overrides and its draw depth. A clock of *90s, Hits, jingle 90s + 90s, New releases* makes a match of two 90s tracks, then of two hits, then the jingle and a match of two 90s, then of two new releases: the matches follow the hour's balance, its jingles and its exact times. The winner counts as the slot's pick: the clock goes on with the slot after it. A slot that is not music (a jingle, an ad: a category whose type sends no metadata) never makes a match: it plays as the clock says. An hour without a clock, which rotates the station's library, takes the match's category; without one, no match runs in that hour.
- **From one category** (`category`, the default, as until now): the match's category (*Hits*, say), chosen with the station's rules as a slot calling that category would.

Either way: separation, restrictions and the play ratio at the time the winner should start, the least recently played first, relaxed step by step if needed (the fallback category does not count: a match needs two tracks of its category). The second is picked with the first counted as played then: two different tracks, of two different artists while the separation rules allow it.
- **The loser of a match does not come back the same day** (in the station's time zone), unless the station allows it (*the loser can come back the same day*). A track that won can, as the rules allow.
- **One play in the plan:** the two tracks are planned at the same start, and what follows after the longer one, since only one of them plays: the hour is never late, whichever wins. Before an exact time, the second is chosen with a length close to the first's, so that either lands the hour in the exact time's window ([Rotation – Exact times](rotation.md#exact-times)).
- **Never a queued track again:** the tracks in the queue count as played at their planned times, so the separation rules keep them out of the match like any recent play. The rules may be relaxed step by step down to title separation dropped (step 8), so the second can be another version of a title, or an artist, already queued when nothing else is left; a track itself is kept out by track separation until step 9 (halved), and the fallback category never gives one.
- Without two tracks, no match: rotation plays on, and tries again for the next track.

### On air

- **An announcement**, every match or every few (*announce every X matches*, 1 to 20, 1 by default: with 2 and every track, one vote out of two is announced): a jingle of the station's **announcement category** (*Jingles "vote on the website"*, say) plays over the intro of the vote's track, linked to it as the jingle of a [linked slot](rotation.md#linked-slots) is (it takes the place of the slot's own linked jingle, if any). The category's type plays over the intro, so it plays on the FX player. Its jingles are picked in turn, the least recently played first, whatever played before: over the intro, it is not two jingles in a row. Without an announcement category, no jingle.
- **In the queue**: the announcement, the vote's track, then **both tracks**, one after the other. They are both read ahead by the engine, so whichever wins starts without a gap.
- **The vote opens** when the vote's track starts on air, and **closes 10 seconds before the next transition** (5 to 60, set by the station): where the next track starts mixing in, the vote's track's mix point, else its end.
- **At the close**, the loser is taken out of the queue: the winner is next. The engine had both ready, and the 10 seconds leave it time to cue the winner.
- **A tie, or no vote**: the least recently played of the two on the station wins, else one at random.
- **The loser** never plays: it is not counted as played, and goes back to its category (out of matches for the rest of the day, by default).
- In the **rotation log**, both tracks are logged as picks of their slot (from the clock) or of the match's category when they are queued; the loser's pick is deleted at the close, the winner's stays with its play.

What an operator can do (*Shows › Match*, and *On air*): **close the vote now**, which decides with the votes so far, and **cancel the match**, which keeps the first track in the queue, as an ordinary track, and takes the second out. A new engine cancels the match the same way.

**The match's tracks are locked in the queue** while it waits or its vote is open: the announcement, the vote's track, what the clock put between it and the winner, and the two tracks. Only the match takes one out (the loser, at the close) or the operator's *cancel the match*:

- **Removing, moving or playing next** one of them is refused (`409`, *this track belongs to the match in progress: cancel the match first*). The Queue page and On air mark them *Match*, without their handle, *play next* or *remove*.
- **Nothing goes between them**: queueing or moving a track to a place among them is refused the same way. Before them, or after the second track, is free.
- **Clearing the queue** takes out every other track and keeps the match's: the match goes on.
- **Skipping the vote's track** while its vote is open closes the vote first, with the votes so far (*closed early*), then starts the winner. Skipping it before its vote opens, or skipping another track, skips as usual.

A track the queue manager takes out by itself (its file left the library, or it was disabled) still counts as taken out: the other one wins when the vote closes, and the match is cancelled without either.

### Voting

- **The match open now**: the show's live state names it (`match_id`); a vote names it too. A vote for a match that closed is refused (`409`).
- **One vote per device per match**: `POST …/stations/{code}/match/vote` with `{"match_id": 12, "choice": "a"}` (`a` or `b`) and the proof of work, answered `202`. A device can change its mind until the vote closes: its last vote counts, once.
- **Kept per device in the gateway**, the last vote of each device only, and sent to the API every second in one batch, with each device's hash. The API counts the votes it receives while the match is open: the close waits a second for the batch on its way, and drops what comes later.
- **Live results**: the votes of each track as they come, in the gateway's state and on the website, unless the station turns them off (*live results on the website*): then only the result shows, at the close.

### Notifications

Besides `show.upcoming`, `show.started` and `show.ended`, both on by default, with the name on the website (`{name}`):

- `match.started`: a new vote, when it opens, with `{title1}`, `{artist1}`, `{title2}`, `{artist2}` and `{close}` (when it closes, `18:05`). Default message: *New vote: {title1} or {title2}? Choose before {close}: {link}*.
- `match.closed`: the result, at the close, with the same placeholders, `{winner}` and `{loser}` (their titles), `{percent}` (the winner's share of the votes, `58 %`) and `{votes}` (the votes in all). Default message: *★ {winner} wins with {percent} of {votes} votes, playing now!* A cancelled match sends nothing.

### On the website

The page and the small widget show, during the hours:

- **While a vote is open**: *Choose the next track*, the two tracks (cover, title, artist) one against the other, each a button to vote for it, and a **countdown** to the close. The countdown says nothing else: no sentence about when the vote closes. With live results, each track's share of the votes as a bar and a percentage, and the votes in all. The track chosen stays marked; another click changes the vote.
- **In the last 5 seconds**, the countdown and the two choices **blink**, so that listeners who have not voted do it now. When the browser asks for reduced motion, they do not blink: a strong outline shows instead.
- **Between two votes**: the last result, and *the next vote is coming in a few tracks*. Every track, a vote is always open: the last result shows above it while the winner plays.
- The page also lists **today's results**: the time, the winner with its share, against the loser.

Outside the hours, the closed message or nothing, as every show.

When the winner plays, the public `now-playing` (and the gateway's state) says so: `match` (`name`, the show's name on the website, `percent` and `votes`), so that a website can say *chosen by the listeners, 62 % of the votes*.

### What is kept

- **Each match**, for good: its station, its category (the slot's, from the clock), the vote's track, the two tracks, when it opened and closed, the votes of each, the winner and how it was decided (votes, a tie, no vote, closed early), or that it was cancelled. They are shown on the show's *History* tab, with the week's matches and votes.
- **Per device**, the device's hash and its vote on a match, so that a device changes its vote instead of adding one: deleted an hour after the match closes, and a day after the vote at the latest. No IP address.

The matches are in the database dump of the [backups](backups.md); the settings are in the configuration file.

## Featured artist

A station puts an artist forward: on the days and hours it chooses, at set minutes of each hour (*midday, 13:00, 14:00 and 15:00*, or *a quarter past, half past and a quarter to*), a track of **the artist of the week**, or **of the day**, plays at an exact time, after a jingle if the station has one. Module `featured_artist`, off until an admin turns it on.

### When it plays

- **Its hours** are a show's [opening hours](#shows-and-their-hours): days of the week and whole hours, in the station's time zone (*Monday to Friday, 12:00 to 16:00*). Like every show's, they belong to it: hours the Match, the Dedications or a top's show have are refused (`409`, saying which), and the other way round.
- **Its minutes**, in each of those hours: `:00`, `:15`, `:30`, `:45`, any of them (one at least). *12:00 to 16:00* with `:00` plays at 12:00, 13:00, 14:00 and 15:00; with `:15`, `:30` and `:45`, three times an hour. These are **its times**.
- **The artist** comes from its **planning**, filled in ahead: **by week** (an artist from Monday to Sunday) or **by day** (an artist per date), as the station chooses. A week or a day without an artist plays nothing at its times: the clocks play as usual, and the show's page lists the coming weeks or days without one. Changing from weeks to days, or back, keeps what was planned the other way, unused until the station goes back to it.

### On air

- **An exact time the show adds to the hour**, soft, as a clock slot's ([Rotation – Exact times](rotation.md#exact-times)): rotation plans the tracks before it so that the last one ends in its **window** (from 10 seconds before the time to 30 seconds after it by default, 0 to 300 each, set by the show), with the fills and their fit search, and the station's medleys or filler category when nothing lands. A track that would end after the window is faded out at its end, over 1.5 s.
- **The clock pauses**: at the time, the jingle and the artist's track play, then the clock goes on with the slot it was at. It is not a slot of the clock: the clock's own slots, their exact times and its balance stay as they are, two or four minutes later.
- **A clock's exact time comes first**: when the window of a clock slot's exact time (a news flash at `:00:00`, an ad break at `:15:00`) meets the show's, the clock's slot plays at its time, and the show's track right after it (and after its linked slot), without a window of its own.
- **The track**, among the station's library: the tracks of the artist (any of its [names](rotation.md#artists-with-several-names), featuring included) in a category whose type sends its metadata (music), the one played least recently on the station first: a track played that day is left for the others while there are any. **Restrictions apply** at its planned time; **separation rules do not** to it (the artist plays every quarter of an hour, if the station wants), but they count it for the tracks around it, as any play: rotation keeps the artist and the track out of the picks next to it. A track that cannot play (disabled, missing, nothing audible) is passed over. None left: nothing plays at that time, the show's *Today* tab says so, and the clock goes on.
- **The jingle**, optional: a jingle of the show's **jingle category** (*Jingles "featured artist"*, say), linked to the track as the jingle of a [linked slot](rotation.md#linked-slots) is: it starts at the time, the pair landing in the window, and never plays without the track. Without a jingle left, the track plays alone. Its jingles are picked in turn, the least recently played first. Its category's type says where it plays (over the intro: the FX player). Without a jingle category, the track plays alone.
- **In the queue**: the jingle and the track are queued as rotation's picks, as the queue needs them, and logged in the rotation log as the show's. They are rotation's plays in the history. With autofill off, nothing is queued: the show does not play either.
- **Never off air**: a show that fails (settings it cannot read, a planning it cannot parse) is logged as a warning and left out: rotation fills the queue as without it.

### In Heron

*Shows › Featured artist*, for operators:

- **Planning**: this week and the next 12 (or today and the next 31 days), an artist chosen for each among the library's artists, by name, or taken out.
- **Today**: the day's times, with the track planned or played at each, its jingle, and what was passed over and why.
- **Settings**: hours, minutes, by week or by day, the window, the jingle category, and the website and notifications every show has.

### Notifications

The common `show.upcoming`, `show.started` and `show.ended`, at the show's hours, with `{artist}`: the artist of the week or the day.

### On the website

The page and the small widget (`featured_artist`) show the artist of the week or the day, and the next time it plays (*Featured artist of the week: Daft Punk, next at 14:15*). Outside its hours, or without an artist, the closed message, or nothing.

### What is kept

- **The planning**, with each week or day and its artist: a year after its week or day is over, it is deleted.
- Its plays are rotation's, in the history and the rotation log, kept as theirs.

The planning is in the database dump of the [backups](backups.md); the settings are in the configuration file.

## Personal data

- **The gateway** keeps nothing on disk. In memory: the hash of each device that wrote, with its count, until the opening range ends (at most a day); a device's last like or match vote until the next batch, a second; the challenges given, 5 minutes. Addresses are counted for the per-address limits for a minute, and dropped.
- **The API** stores what a show keeps (a dedication's first name and message, the device's hash), and deletes it after the time the show says, written on the show's page; no IP address. Each show's part of these docs says what it keeps and for how long.
- **Nothing a listener writes reaches the air, the website or a webhook without a person's approval.**

## Building a show on the base

What a show module adds, and where:

| Where | What |
|---|---|
| `heron_core::Module` and `Modules` | Its key, and its default (off for a show, on for Likes) |
| `heron_core::shows::Show` | Its entry: key, whether it has hours, its own notification events with their default messages, its own placeholders |
| `heron-api`, `shows.rs` | Its part of the public state (`ShowPublic::state`, any JSON: the current match, the day's dedications), read once a second by the gateway |
| `heron-api`, `queue.rs` | A show that plays on air (the [Tops](#tops)): the queue manager asks it for its programme before rotation picks (`top30::due`), and fills as without it when it fails; a show that changes rotation's picks (the [Match](#match)) is asked as rotation fills the queue (`matches::plan`) |
| `heron-api`, gateway routes | Its writes, under `/api/v1/gateway/stations/{code}/{module}/…`, behind `GatewayClient` and `gateway::open_show`, which check the token, the module and the hours |
| `heron-public`, `actions.rs` | Its writes as the gateway takes them: path, fields and their sizes (and their values, or the field of its state they must match), the device's limit, sent on, added up or kept per device |
| `heron-public`, `widget.js` | Its renderer: what the page and the small widget show while it is open |
| `web/src/pages/Show.tsx` | Its tabs (the live view: requests, the match in progress) and its settings, next to the common ones |
| `docs/` | Its part: what it keeps, its routes, its events |
