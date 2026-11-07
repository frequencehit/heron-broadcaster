// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The page and the small widget of a show, for the station's website
// (docs/listener-interaction.md#on-the-website):
//
//   <script src="https://…/api/v1/public/stations/main/dedications.js" async></script>
//   <script src="…/dedications.js" data-mode="page" async></script>
//
// Optional: data-lang="fr" (else the page's, else the browser's),
// data-target="#id" (else right after the script). It follows the
// station's events and draws in a shadow DOM, so the website's styles do
// not reach it. Each show adds its renderer to RENDERERS: what it shows
// while it is open.
(function () {
  "use strict";

  var script = document.currentScript;
  if (!script) return;
  var match = /^(.*\/api\/v1\/public\/stations\/([^/]+))\/([a-z0-9_]+)\.js(?:\?.*)?$/.exec(script.src);
  if (!match) return;
  var base = match[1];
  var module = match[3];
  var mode = script.getAttribute("data-mode") === "page" ? "page" : "widget";

  var TEXT = {
    en: {
      names: { dedications: "Dedications", match: "The match", top30: "Top 30", likes: "Do you like it?" },
      until: "Until {hour}",
      likes: {
        sub: "Only the track playing can be voted on",
        like: "I like it",
        dislike: "I don't like it",
        nothing: "Nothing to vote on right now.",
        sending: "Sending…",
        thanks: "Thank you! You can change your mind while it plays.",
        refused: "Your vote could not be counted. Try again.",
        closed: "This track has ended: the vote is closed.",
      },
      open: "Open now",
      closed: "{name}: closed",
      back: "Back {day} at {hour}.",
      nowPlaying: "Now playing",
      sep: " to ",
      top30: {
        live: "Live: the places to come are revealed on air",
        week: "Week of {date}",
        hidden: "? ? ?",
        answer: "The answer on air",
        onAir: "ON AIR",
        isNew: "NEW",
        reentry: "RE-ENTRY",
        weeks: "{n} wk",
        newRelease: "New release",
        none: "No chart yet.",
        votesFor: "Your vote counts for {day}'s Top 30.",
      },
    },
    fr: {
      names: { dedications: "Dédicaces", match: "Le match", top30: "Top 30", likes: "Vous aimez ?" },
      until: "Jusqu'à {hour}",
      likes: {
        sub: "On ne vote que pour le titre en cours",
        like: "J'aime",
        dislike: "J'aime pas",
        nothing: "Rien à voter pour l'instant.",
        sending: "Envoi…",
        thanks: "Merci ! Vous pouvez changer d'avis tant que le titre passe.",
        refused: "Votre vote n'a pas pu être compté. Réessayez.",
        closed: "Ce titre est fini : vote fermé.",
      },
      open: "Ouvert en ce moment",
      closed: "{name} : fermé",
      back: "Réouverture {day} à {hour}.",
      nowPlaying: "En ce moment",
      sep: " au ",
      top30: {
        live: "En direct : les places à venir se découvrent à l'antenne",
        week: "Semaine du {date}",
        hidden: "? ? ?",
        answer: "Réponse à l'antenne",
        onAir: "EN ONDE",
        isNew: "NOUVEAU",
        reentry: "RETOUR",
        weeks: "{n} sem.",
        newRelease: "La nouveauté",
        none: "Pas encore de classement.",
        votesFor: "Votre vote compte pour le Top 30 de {day}.",
      },
    },
    es: {
      names: { dedications: "Dedicatorias", match: "El duelo", top30: "Top 30", likes: "¿Te gusta?" },
      until: "Hasta las {hour}",
      likes: {
        sub: "Solo se vota la canción que suena",
        like: "Me gusta",
        dislike: "No me gusta",
        nothing: "Nada que votar ahora.",
        sending: "Enviando…",
        thanks: "¡Gracias! Puedes cambiar de opinión mientras suena.",
        refused: "No se pudo contar tu voto. Inténtalo de nuevo.",
        closed: "Esta canción terminó: votación cerrada.",
      },
      open: "Abierto ahora",
      closed: "{name}: cerrado",
      back: "Vuelve el {day} a las {hour}.",
      nowPlaying: "Suena ahora",
      sep: " a ",
      top30: {
        live: "En directo: los puestos que faltan se descubren en antena",
        week: "Semana del {date}",
        hidden: "? ? ?",
        answer: "La respuesta en antena",
        onAir: "EN ANTENA",
        isNew: "NUEVO",
        reentry: "VUELVE",
        weeks: "{n} sem.",
        newRelease: "La novedad",
        none: "Todavía no hay lista.",
        votesFor: "Tu voto cuenta para el Top 30 del {day}.",
      },
    },
    de: {
      names: { dedications: "Widmungen", match: "Das Duell", top30: "Top 30", likes: "Gefällt's dir?" },
      until: "Bis {hour}",
      likes: {
        sub: "Abgestimmt wird nur über den laufenden Titel",
        like: "Gefällt mir",
        dislike: "Gefällt mir nicht",
        nothing: "Gerade gibt es nichts abzustimmen.",
        sending: "Wird gesendet…",
        thanks: "Danke! Solange der Titel läuft, kannst du deine Meinung ändern.",
        refused: "Deine Stimme konnte nicht gezählt werden. Versuch es noch einmal.",
        closed: "Dieser Titel ist vorbei: Abstimmung geschlossen.",
      },
      open: "Jetzt geöffnet",
      closed: "{name}: geschlossen",
      back: "Wieder da am {day} um {hour}.",
      nowPlaying: "Jetzt läuft",
      sep: " bis ",
      top30: {
        live: "Live: die nächsten Plätze gibt es im Radio",
        week: "Woche vom {date}",
        hidden: "? ? ?",
        answer: "Die Antwort im Radio",
        onAir: "LÄUFT",
        isNew: "NEU",
        reentry: "WIEDER DA",
        weeks: "{n} Wo.",
        newRelease: "Die Neuheit",
        none: "Noch keine Charts.",
        votesFor: "Deine Stimme zählt für die Top 30 am {day}.",
      },
    },
  };
  var wanted = script.getAttribute("data-lang") || document.documentElement.lang || navigator.language || "en";
  var lang = wanted.slice(0, 2).toLowerCase();
  var t = TEXT[lang] || TEXT.en;
  if (!TEXT[lang]) lang = "en";

  function fill(text, values) {
    return text.replace(/\{([a-z]+)\}/g, function (all, name) {
      return Object.prototype.hasOwnProperty.call(values, name) ? values[name] : all;
    });
  }

  function hour(ms, zone) {
    return new Intl.DateTimeFormat(lang, { hour: "2-digit", minute: "2-digit", timeZone: zone }).format(ms);
  }

  function day(ms, zone) {
    return new Intl.DateTimeFormat(lang, { weekday: "long", day: "numeric", month: "long", timeZone: zone }).format(ms);
  }

  // Monday 9 November 2026: the day names of the hours.
  function dayName(n) {
    return new Intl.DateTimeFormat(lang, { weekday: "long", timeZone: "UTC" }).format(Date.UTC(2026, 10, 8 + n));
  }

  function hoursText(hours) {
    return hours.map(function (range) {
      var days = range.days.slice().sort();
      var consecutive = days.every(function (d, i) { return i === 0 || d === days[i - 1] + 1; });
      var named = days.length > 2 && consecutive
        ? dayName(days[0]) + t.sep + dayName(days[days.length - 1])
        : days.map(dayName).join(", ");
      var pad = function (h) { return (h < 10 ? "0" : "") + h + ":00"; };
      return named + ", " + pad(range.start_hour) + "–" + pad(range.end_hour);
    }).join(" · ");
  }

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  var host = document.createElement("div");
  var target = script.getAttribute("data-target");
  var parent = target && document.querySelector(target);
  if (parent) parent.appendChild(host);
  else script.parentNode.insertBefore(host, script.nextSibling);
  var root = host.attachShadow ? host.attachShadow({ mode: "open" }) : host;
  var style = el("style");
  style.textContent =
    ":host{all:initial;display:block}" +
    ".card{font:15px/1.4 system-ui,-apple-system,'Segoe UI',sans-serif;color:#1d1d1b;background:#fff;" +
    "border:1px solid #e3e3df;border-radius:14px;overflow:hidden;max-width:420px}" +
    ".page .card{max-width:none}" +
    ".head{background:var(--color);color:#fff;padding:14px 18px}" +
    ".title{font-size:20px;font-weight:700;margin:0}" +
    ".sub{margin:2px 0 0;opacity:.92}" +
    ".body{padding:16px 18px}" +
    ".closed{text-align:center;padding:22px 18px}" +
    ".closed svg{width:34px;height:34px;color:#5f6067}" +
    ".closed strong{display:block;font-size:17px;margin:8px 0 4px}" +
    ".muted{color:#5f6067;margin:0}" +
    ".np{display:flex;gap:10px;align-items:center;margin-top:10px}" +
    ".np img{width:48px;height:48px;border-radius:6px;object-fit:cover}" +
    ".track{margin:0}.track strong{display:block;font-size:17px}" +
    ".page .track strong{font-size:22px}" +
    ".votes{display:flex;gap:10px;margin:12px 0 8px;flex-wrap:wrap}" +
    ".vote{font:inherit;font-weight:600;display:inline-flex;align-items:center;gap:8px;padding:9px 18px;" +
    "border-radius:999px;border:1.5px solid #c9c9c4;background:transparent;color:inherit;cursor:pointer}" +
    ".vote[aria-pressed=true]{background:var(--color);border-color:var(--color);color:#fff}" +
    ".vote svg{width:20px;height:20px}" +
    ".page .vote{font-size:18px;padding:12px 24px}" +
    ".chart{list-style:none;margin:0;padding:0}" +
    ".chart li{display:flex;gap:12px;align-items:center;padding:8px 0;border-top:1px solid #e3e3df}" +
    ".chart li:first-child{border-top:0}" +
    ".chart li.on{background:#1d1d1b;color:#fff;margin:0 -18px;padding:8px 18px}" +
    ".rank{font-weight:800;font-size:20px;min-width:2ch;text-align:right}" +
    ".page .rank{font-size:26px}" +
    ".mv{font-size:12px;font-weight:700;min-width:5.5em;color:#5f6067}" +
    ".mv.up{color:#2e7d32}.mv.down{color:#c62828}.mv.new{color:var(--color)}" +
    ".who{flex:1;min-width:0}.who strong{display:block}.chart .q{color:#9a9a95;letter-spacing:.3em}" +
    ".tag{font-size:11px;font-weight:700;background:var(--color);color:#fff;border-radius:6px;padding:1px 6px;margin-left:6px}" +
    ".release{margin-top:12px;border:2px solid var(--color);border-radius:10px;padding:10px 14px}" +
    "@media (prefers-color-scheme: dark){.card{background:#1f1f1e;color:#ededea;border-color:#3a3a37}.muted,.closed svg{color:#b5b5b0}}";
  root.appendChild(style);
  var box = el("div", mode);
  root.appendChild(box);

  // What a show shows while it is open; each show adds its own here.
  var RENDERERS = {};

  function generic(body, show, state) {
    body.appendChild(el("p", "muted", t.open));
    var now = state.now_playing && state.now_playing.now;
    if (now) {
      var np = el("div", "np");
      if (now.cover_url) {
        var img = el("img");
        img.alt = "";
        img.src = now.cover_url + (now.cover_url.indexOf("?") < 0 ? "?size=96" : "");
        np.appendChild(img);
      }
      var words = el("div");
      words.appendChild(el("div", "muted", t.nowPlaying));
      words.appendChild(el("strong", "", [now.artist, now.title].filter(Boolean).join(" – ")));
      np.appendChild(words);
      body.appendChild(np);
    }
  }

  // Likes (docs/listener-interaction.md#likes): the track playing, liked or
  // not. The vote is kept for its play only, in memory.
  var THUMBS = {
    like:
      "M7 10v12M15 5.88 14 10h5.83a2 2 0 0 1 1.92 2.56l-2.33 8A2 2 0 0 1 17.5 22H4a2 2 0 0 1-2-2v-8a2 2 0 0 1 " +
      "2-2h2.76a2 2 0 0 0 1.79-1.11L12 2a3.13 3.13 0 0 1 3 3.88Z",
    dislike:
      "M17 14V2M9 18.12 10 14H4.17a2 2 0 0 1-1.92-2.56l2.33-8A2 2 0 0 1 6.5 2H20a2 2 0 0 1 2 2v8a2 2 0 0 1-2 " +
      "2h-2.76a2 2 0 0 0-1.79 1.11L12 22a3.13 3.13 0 0 1-3-3.88Z",
  };
  var mine = { play: null, vote: null, note: null };

  RENDERERS.likes = function (body, show, state, api) {
    var play = show.state && show.state.play_id;
    var now = state.now_playing && state.now_playing.now;
    if (!play || !now) {
      body.appendChild(el("p", "muted", mine.vote ? t.likes.closed : t.likes.nothing));
      return;
    }
    if (mine.play !== play) mine = { play: play, vote: null, note: null };
    var track = el("p", "track");
    track.appendChild(el("strong", "", now.title || ""));
    if (now.artist) track.appendChild(el("span", "muted", now.artist));
    body.appendChild(track);
    var votes = el("div", "votes");
    ["like", "dislike"].forEach(function (vote) {
      var button = el("button", "vote");
      button.type = "button";
      button.setAttribute("aria-pressed", mine.vote === vote ? "true" : "false");
      button.insertAdjacentHTML(
        "beforeend",
        '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" ' +
          'stroke-linejoin="round" aria-hidden="true"><path d="' + THUMBS[vote] + '"/></svg>'
      );
      button.appendChild(document.createTextNode(t.likes[vote]));
      button.addEventListener("click", function () {
        if (mine.vote === vote) return;
        var voted = mine;
        voted.vote = vote;
        voted.note = t.likes.sending;
        render(latest);
        api.send("vote", { play_id: play, vote: vote }).then(
          function (response) {
            voted.note = response.ok ? t.likes.thanks : response.status === 409 ? t.likes.closed : t.likes.refused;
            if (!response.ok) voted.vote = null;
            render(latest);
          },
          function () {
            voted.note = t.likes.refused;
            voted.vote = null;
            render(latest);
          }
        );
      });
      votes.appendChild(button);
    });
    body.appendChild(votes);
    body.appendChild(el("p", "muted", mine.note || t.likes.sub + "."));
    // The votes of now count for the next Top 30.
    var top = (state.shows || []).filter(function (s) { return s.module === "top30"; })[0];
    var counts = top && top.enabled && top.state && top.state.votes_count_for_unix_ms;
    if (counts) {
      var weekday = new Intl.DateTimeFormat(lang, { weekday: "long", timeZone: top.timezone || state.timezone }).format(counts);
      body.appendChild(el("p", "muted", fill(t.top30.votesFor, { day: weekday })));
    }
  };

  // Top 30 (docs/listener-interaction.md#top-30): the chart of the last show,
  // or of the show playing, whose ranks to come are hidden (the gateway's
  // state does not have them).
  function movementText(entry) {
    switch (entry.movement) {
      case "up": return "▲ " + (entry.previous_rank - entry.rank);
      case "down": return "▼ " + (entry.rank - entry.previous_rank);
      case "same": return "=";
      case "new": return t.top30.isNew;
      case "reentry": return t.top30.reentry;
      default: return "";
    }
  }

  function chart(body, show) {
    var top = show.state || {};
    if (!top.chart) {
      if (show.open) body.appendChild(el("p", "muted", t.top30.none));
      return;
    }
    if (top.live) body.appendChild(el("p", "muted", t.top30.live));
    else body.appendChild(el("p", "muted", fill(t.top30.week, {
      date: new Intl.DateTimeFormat(lang, { day: "numeric", month: "long", timeZone: "UTC" }).format(Date.parse(top.chart.week + "T00:00:00Z")),
    })));
    var entries = top.chart.entries || [];
    if (mode === "widget") {
      if (top.live && top.on_air_rank) {
        // The rank on air and the next one to come.
        entries = entries.filter(function (e) { return e.rank === top.on_air_rank || e.rank === top.on_air_rank - 1; });
      } else {
        entries = entries.slice(0, 5);
      }
    }
    var list = el("ol", "chart");
    entries.forEach(function (entry) {
      var row = el("li", entry.on_air ? "on" : "");
      row.appendChild(el("span", "rank", String(entry.rank)));
      var who = el("span", "who");
      if (entry.hidden) {
        row.appendChild(el("span", "mv", ""));
        who.appendChild(el("strong", "q", t.top30.hidden));
        who.appendChild(el("span", "muted", t.top30.answer));
      } else {
        row.appendChild(el("span", "mv " + (entry.movement || ""), movementText(entry)));
        var title = el("strong", "", entry.title || "");
        if (entry.on_air) title.appendChild(el("span", "tag", t.top30.onAir));
        who.appendChild(title);
        if (entry.artist) who.appendChild(el("span", entry.on_air ? "" : "muted", entry.artist));
      }
      row.appendChild(who);
      if (!entry.hidden && entry.weeks && mode === "page") row.appendChild(el("span", "muted", fill(t.top30.weeks, { n: entry.weeks })));
      list.appendChild(row);
    });
    body.appendChild(list);
    var release = top.new_release;
    if (release && !release.hidden && mode === "page") {
      var box = el("div", "release");
      box.appendChild(el("span", "tag", t.top30.newRelease));
      box.appendChild(el("strong", "", " " + [release.artist, release.title].filter(Boolean).join(" – ")));
      body.appendChild(box);
    }
  }

  RENDERERS.top30 = function (body, show) {
    chart(body, show);
  };

  function render(state) {
    box.textContent = "";
    var show = (state.shows || []).filter(function (s) { return s.module === module; })[0];
    if (!show) return;
    if (mode === "widget" && !show.small_widget) return;
    if (!show.open && show.closed === "nothing") return;
    var name = show.title || t.names[module] || module;
    var card = el("div", "card");
    var color = /^#[0-9a-fA-F]{6}$/.test(show.color) ? show.color : "#c62828";
    card.style.setProperty("--color", show.open ? color : "#5f6067");
    var head = el("div", "head");
    head.appendChild(el("p", "title", name));
    var zone = show.timezone || state.timezone;
    if (show.open && module === "likes") {
      head.appendChild(el("p", "sub", t.likes.sub));
    } else if (show.open && show.closes_at_unix_ms) {
      head.appendChild(el("p", "sub", fill(t.until, { hour: hour(show.closes_at_unix_ms, zone) })));
    } else if (!show.open) {
      head.appendChild(el("p", "sub", state.name || ""));
    }
    card.appendChild(head);
    if (show.open) {
      var body = el("div", "body");
      (RENDERERS[module] || generic)(body, show, state, api);
      card.appendChild(body);
    } else {
      var closed = el("div", "closed");
      closed.insertAdjacentHTML(
        "beforeend",
        '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">' +
          '<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>'
      );
      closed.appendChild(el("strong", "", fill(t.closed, { name: name })));
      var next = show.next_opening_unix_ms;
      var values = next ? { day: day(next, zone), hour: hour(next, zone) } : { day: "", hour: "" };
      if (show.closed_message) closed.appendChild(el("p", "muted", fill(show.closed_message, values)));
      else if (next) closed.appendChild(el("p", "muted", fill(t.back, values)));
      if (show.hours && show.hours.length) closed.appendChild(el("p", "muted", hoursText(show.hours)));
      card.appendChild(closed);
      // Outside the show, the Top 30 shows the chart of the last one.
      if (module === "top30" && show.state && show.state.chart) {
        var last = el("div", "body");
        chart(last, show);
        card.appendChild(last);
      }
    }
    box.appendChild(card);
  }

  // For the shows' writes: a proof of work, then the write.
  function leadingZeroBits(bytes) {
    var bits = 0;
    for (var i = 0; i < bytes.length; i++) {
      if (bytes[i] === 0) { bits += 8; continue; }
      return bits + Math.clz32(bytes[i]) - 24;
    }
    return bits;
  }

  function solve(challenge, bits) {
    var encoder = new TextEncoder();
    var nonce = 0;
    function next() {
      var tries = [];
      for (var i = 0; i < 64; i++, nonce++) {
        tries.push((function (n) {
          return crypto.subtle.digest("SHA-256", encoder.encode(challenge + ":" + n)).then(function (hash) {
            return leadingZeroBits(new Uint8Array(hash)) >= bits ? String(n) : null;
          });
        })(nonce));
      }
      return Promise.all(tries).then(function (found) {
        var hit = found.filter(function (n) { return n !== null; })[0];
        return hit !== undefined ? hit : next();
      });
    }
    return next();
  }

  var api = {
    base: base,
    module: module,
    lang: lang,
    /** POST …/{module}[/{action}] with a proof of work; resolves with the response. */
    send: function (action, fields) {
      var path = base + "/" + module + (action ? "/" + action : "");
      return fetch(base.replace(/\/stations\/[^/]+$/, "") + "/challenge")
        .then(function (r) { return r.json(); })
        .then(function (c) {
          var proof = c.bits ? solve(c.challenge, c.bits) : Promise.resolve("0");
          return proof.then(function (nonce) {
            return fetch(path, {
              method: "POST",
              headers: { "Content-Type": "application/json", "X-Heron-Proof": c.challenge + ":" + nonce },
              body: JSON.stringify(fields),
            });
          });
        });
    },
  };

  var latest = null;
  var events = new EventSource(base + "/events");
  events.addEventListener("state", function (event) {
    try {
      latest = JSON.parse(event.data);
    } catch (error) {
      return;
    }
    render(latest);
  });
})();
