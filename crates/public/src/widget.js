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
      names: { dedications: "Dedications", match: "The match", top30: "Top 30" },
      until: "Until {hour}",
      open: "Open now",
      closed: "{name}: closed",
      back: "Back {day} at {hour}.",
      nowPlaying: "Now playing",
      sep: " to ",
    },
    fr: {
      names: { dedications: "Dédicaces", match: "Le match", top30: "Top 30" },
      until: "Jusqu'à {hour}",
      open: "Ouvert en ce moment",
      closed: "{name} : fermé",
      back: "Réouverture {day} à {hour}.",
      nowPlaying: "En ce moment",
      sep: " au ",
    },
    es: {
      names: { dedications: "Dedicatorias", match: "El duelo", top30: "Top 30" },
      until: "Hasta las {hour}",
      open: "Abierto ahora",
      closed: "{name}: cerrado",
      back: "Vuelve el {day} a las {hour}.",
      nowPlaying: "Suena ahora",
      sep: " a ",
    },
    de: {
      names: { dedications: "Widmungen", match: "Das Duell", top30: "Top 30" },
      until: "Bis {hour}",
      open: "Jetzt geöffnet",
      closed: "{name}: geschlossen",
      back: "Wieder da am {day} um {hour}.",
      nowPlaying: "Jetzt läuft",
      sep: " bis ",
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
    if (show.open && show.closes_at_unix_ms) {
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
