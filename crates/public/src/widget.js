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
  var match = /^(.*\/api\/v1\/public\/stations\/([^/]+))\/([a-z0-9_-]+)\.js(?:\?.*)?$/.exec(script.src);
  if (!match) return;
  var base = match[1];
  // The show's key: its module's (`match`), or a top's (`top-4`; `top30`
  // is the first top).
  var key = match[3];
  var module = /^top-[0-9]+$/.test(key) ? "top30" : key;
  var mode = script.getAttribute("data-mode") === "page" ? "page" : "widget";

  var TEXT = {
    en: {
      names: { dedications: "Dedications", match: "The match", top30: "Top", likes: "Do you like it?" },
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
      dedications: {
        remaining: "{n} more requests possible",
        full: "Full for today",
        track: "The track",
        search: "A title, an artist…",
        searchMore: "Type at least 2 letters.",
        noResult: "No track found.",
        from: "Your first name",
        to: "For whom?",
        message: "Your message",
        send: "Send my dedication",
        sending: "Sending…",
        pick: "Choose a track first.",
        note: "Your message is read before it is shown. {per} per range. First names and messages are deleted after {days} days.",
        perOne: "One request",
        perMany: "{n} requests",
        tooMany: "You have already sent your request for this range.",
        fullError: "Full for today: no more requests in this range.",
        closedError: "Dedications have just closed.",
        failed: "Your request could not be sent. Try again.",
        yours: "Your dedication",
        said: "{who}: “{message}”",
        forFrom: "For {to}, from {from}",
        sent: "Sent",
        accepted: "Accepted",
        onAirAt: "On air around {hour}",
        played: "Played at {hour}",
        keepOpen: "Keep this page open: it updates itself.",
        refused: "Not this time",
        reasons: {
          just_played: "This track has just played. Try another one!",
          unavailable: "This track cannot play now. Try another one!",
          closed: "The dedications closed before it could be chosen.",
          removed: "It could not be played in the end.",
        },
        followLink: "Follow link:",
        another: "Send another one",
        today: "Today's dedications",
        playedTag: "played",
        upcoming: "coming up",
        onlyAccepted: "Only accepted dedications are shown.",
      },
      match: {
        sub: "Choose the next track",
        question: "Which track next?",
        versus: "VS",
        votes: "{n} votes",
        oneVote: "1 vote",
        mine: "Your choice",
        change: "You can change your mind until the end.",
        sending: "Sending…",
        refused: "Your vote could not be counted. Try again.",
        closed: "The vote is closed: the result on air!",
        next: "The next vote is coming in a few tracks.",
        won: "{title} won with {percent}",
        last: "Last match",
        today: "Today's results",
        against: "against {track}",
        tie: "tie, least recently played",
        noVotes: "no vote",
      },
      top30: {
        live: "Live: the places to come are revealed on air",
        week: "Chart of {date}",
        hidden: "? ? ?",
        answer: "The answer on air",
        onAir: "ON AIR",
        isNew: "NEW",
        reentry: "RE-ENTRY",
        weeks: "{n} charts",
        newRelease: "New release",
        none: "No chart yet.",
        votesFor: "Your vote counts for {name}, {day}.",
      },
    },
    fr: {
      names: { dedications: "Dédicaces", match: "Le match", top30: "Top", likes: "Vous aimez ?" },
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
      dedications: {
        remaining: "encore {n} demandes possibles",
        full: "Complet pour aujourd'hui",
        track: "Le titre",
        search: "Un titre, un artiste…",
        searchMore: "Tapez au moins 2 lettres.",
        noResult: "Aucun titre trouvé.",
        from: "Votre prénom",
        to: "Pour qui ?",
        message: "Votre message",
        send: "Envoyer ma dédicace",
        sending: "Envoi…",
        pick: "Choisissez d'abord un titre.",
        note: "Votre message est relu avant d'être affiché. {per} par plage. Prénoms et messages effacés après {days} jours.",
        perOne: "Une demande",
        perMany: "{n} demandes",
        tooMany: "Vous avez déjà envoyé votre demande pour cette plage.",
        fullError: "Complet pour aujourd'hui : plus de demandes sur cette plage.",
        closedError: "Les dédicaces viennent de fermer.",
        failed: "Votre demande n'a pas pu être envoyée. Réessayez.",
        yours: "Votre dédicace",
        said: "{who} : « {message} »",
        forFrom: "Pour {to}, de la part de {from}",
        sent: "Envoyée",
        accepted: "Acceptée",
        onAirAt: "À l'antenne vers {hour}",
        played: "Diffusée à {hour}",
        keepOpen: "Gardez cette page ouverte : elle s'actualise seule.",
        refused: "Pas retenue cette fois",
        reasons: {
          just_played: "Ce titre vient de passer. Essayez-en un autre !",
          unavailable: "Ce titre ne peut pas passer en ce moment. Essayez-en un autre !",
          closed: "Les dédicaces ont fermé avant qu'elle soit choisie.",
          removed: "Elle n'a finalement pas pu passer.",
        },
        followLink: "Lien de suivi :",
        another: "Envoyer une autre dédicace",
        today: "Les dédicaces du jour",
        playedTag: "diffusée",
        upcoming: "à venir",
        onlyAccepted: "Seules les dédicaces acceptées s'affichent.",
      },
      match: {
        sub: "Choisissez le prochain titre",
        question: "Quel titre juste après ?",
        versus: "CONTRE",
        votes: "{n} votes",
        oneVote: "1 vote",
        mine: "Votre choix",
        change: "Vous pouvez changer d'avis jusqu'à la fin.",
        sending: "Envoi…",
        refused: "Votre vote n'a pas pu être compté. Réessayez.",
        closed: "Vote fermé : le résultat à l'antenne !",
        next: "Le prochain vote arrive dans quelques titres.",
        won: "{title} l'emporte avec {percent}",
        last: "Dernier match",
        today: "Les résultats du jour",
        against: "contre {track}",
        tie: "égalité, le moins récent",
        noVotes: "aucun vote",
      },
      top30: {
        live: "En direct : les places à venir se découvrent à l'antenne",
        week: "Classement du {date}",
        hidden: "? ? ?",
        answer: "Réponse à l'antenne",
        onAir: "EN ONDE",
        isNew: "NOUVEAU",
        reentry: "RETOUR",
        weeks: "{n} classements",
        newRelease: "La nouveauté",
        none: "Pas encore de classement.",
        votesFor: "Votre vote compte pour {name}, {day}.",
      },
    },
    es: {
      names: { dedications: "Dedicatorias", match: "El duelo", top30: "Top", likes: "¿Te gusta?" },
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
      dedications: {
        remaining: "quedan {n} peticiones",
        full: "Completo por hoy",
        track: "La canción",
        search: "Una canción, un artista…",
        searchMore: "Escribe al menos 2 letras.",
        noResult: "No se encontró ninguna canción.",
        from: "Tu nombre",
        to: "¿Para quién?",
        message: "Tu mensaje",
        send: "Enviar mi dedicatoria",
        sending: "Enviando…",
        pick: "Elige primero una canción.",
        note: "Tu mensaje se revisa antes de mostrarse. {per} por franja. Los nombres y mensajes se borran a los {days} días.",
        perOne: "Una petición",
        perMany: "{n} peticiones",
        tooMany: "Ya enviaste tu petición para esta franja.",
        fullError: "Completo por hoy: no hay más peticiones en esta franja.",
        closedError: "Las dedicatorias acaban de cerrar.",
        failed: "No se pudo enviar tu petición. Inténtalo de nuevo.",
        yours: "Tu dedicatoria",
        said: "{who}: «{message}»",
        forFrom: "Para {to}, de parte de {from}",
        sent: "Enviada",
        accepted: "Aceptada",
        onAirAt: "En antena hacia las {hour}",
        played: "Emitida a las {hour}",
        keepOpen: "Deja esta página abierta: se actualiza sola.",
        refused: "Esta vez no",
        reasons: {
          just_played: "Esta canción acaba de sonar. ¡Prueba con otra!",
          unavailable: "Esta canción no puede sonar ahora. ¡Prueba con otra!",
          closed: "Las dedicatorias cerraron antes de elegirla.",
          removed: "Al final no pudo sonar.",
        },
        followLink: "Enlace de seguimiento:",
        another: "Enviar otra",
        today: "Las dedicatorias de hoy",
        playedTag: "emitida",
        upcoming: "por venir",
        onlyAccepted: "Solo se muestran las dedicatorias aceptadas.",
      },
      match: {
        sub: "Elige la próxima canción",
        question: "¿Qué canción después?",
        versus: "CONTRA",
        votes: "{n} votos",
        oneVote: "1 voto",
        mine: "Tu elección",
        change: "Puedes cambiar de opinión hasta el final.",
        sending: "Enviando…",
        refused: "No se pudo contar tu voto. Inténtalo de nuevo.",
        closed: "Votación cerrada: ¡el resultado en antena!",
        next: "La próxima votación llega en unas canciones.",
        won: "{title} gana con {percent}",
        last: "Último duelo",
        today: "Los resultados de hoy",
        against: "contra {track}",
        tie: "empate, la menos reciente",
        noVotes: "ningún voto",
      },
      top30: {
        live: "En directo: los puestos que faltan se descubren en antena",
        week: "Lista del {date}",
        hidden: "? ? ?",
        answer: "La respuesta en antena",
        onAir: "EN ANTENA",
        isNew: "NUEVO",
        reentry: "VUELVE",
        weeks: "{n} listas",
        newRelease: "La novedad",
        none: "Todavía no hay lista.",
        votesFor: "Tu voto cuenta para {name}, el {day}.",
      },
    },
    de: {
      names: { dedications: "Widmungen", match: "Das Duell", top30: "Top", likes: "Gefällt's dir?" },
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
      dedications: {
        remaining: "noch {n} Wünsche möglich",
        full: "Für heute voll",
        track: "Der Titel",
        search: "Ein Titel, ein Interpret…",
        searchMore: "Mindestens 2 Buchstaben eingeben.",
        noResult: "Kein Titel gefunden.",
        from: "Dein Vorname",
        to: "Für wen?",
        message: "Deine Nachricht",
        send: "Meine Widmung senden",
        sending: "Wird gesendet…",
        pick: "Wähle zuerst einen Titel.",
        note: "Deine Nachricht wird gelesen, bevor sie erscheint. {per} pro Zeitfenster. Vornamen und Nachrichten werden nach {days} Tagen gelöscht.",
        perOne: "Ein Wunsch",
        perMany: "{n} Wünsche",
        tooMany: "Du hast deinen Wunsch für dieses Zeitfenster schon gesendet.",
        fullError: "Für heute voll: keine Wünsche mehr in diesem Zeitfenster.",
        closedError: "Die Widmungen sind gerade geschlossen.",
        failed: "Dein Wunsch konnte nicht gesendet werden. Versuch es noch einmal.",
        yours: "Deine Widmung",
        said: "{who}: „{message}“",
        forFrom: "Für {to}, von {from}",
        sent: "Gesendet",
        accepted: "Angenommen",
        onAirAt: "Im Radio gegen {hour}",
        played: "Gespielt um {hour}",
        keepOpen: "Lass diese Seite offen: Sie aktualisiert sich selbst.",
        refused: "Diesmal nicht",
        reasons: {
          just_played: "Dieser Titel lief gerade. Probier einen anderen!",
          unavailable: "Dieser Titel kann gerade nicht laufen. Probier einen anderen!",
          closed: "Die Widmungen haben geschlossen, bevor er gewählt wurde.",
          removed: "Er konnte am Ende nicht gespielt werden.",
        },
        followLink: "Link zum Verfolgen:",
        another: "Noch eine senden",
        today: "Die Widmungen von heute",
        playedTag: "gespielt",
        upcoming: "kommt noch",
        onlyAccepted: "Nur angenommene Widmungen werden gezeigt.",
      },
      match: {
        sub: "Wähle den nächsten Titel",
        question: "Welcher Titel als Nächstes?",
        versus: "GEGEN",
        votes: "{n} Stimmen",
        oneVote: "1 Stimme",
        mine: "Deine Wahl",
        change: "Du kannst deine Meinung bis zum Schluss ändern.",
        sending: "Wird gesendet…",
        refused: "Deine Stimme konnte nicht gezählt werden. Versuche es noch einmal.",
        closed: "Abstimmung beendet: das Ergebnis im Programm!",
        next: "Die nächste Abstimmung kommt in ein paar Titeln.",
        won: "{title} gewinnt mit {percent}",
        last: "Letztes Duell",
        today: "Die Ergebnisse von heute",
        against: "gegen {track}",
        tie: "Gleichstand, der am längsten nicht gespielte",
        noVotes: "keine Stimme",
      },
      top30: {
        live: "Live: die nächsten Plätze gibt es im Radio",
        week: "Charts vom {date}",
        hidden: "? ? ?",
        answer: "Die Antwort im Radio",
        onAir: "LÄUFT",
        isNew: "NEU",
        reentry: "WIEDER DA",
        weeks: "{n} Charts",
        newRelease: "Die Neuheit",
        none: "Noch keine Charts.",
        votesFor: "Deine Stimme zählt für {name} am {day}.",
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

  // The parts the website styles with ::part() (docs/listener-interaction.md#styling-it-from-the-website):
  // names that stay, whatever the classes become.
  var PARTS = {
    page: "page", widget: "widget", card: "card", head: "header", title: "title", sub: "subtitle",
    body: "body", closed: "closed", np: "now-playing", track: "track", art: "cover",
    duel: "duel", question: "question", choice: "choice", versus: "versus", pct: "percent",
    bar: "bar", clock: "countdown", votes: "votes", vote: "vote-button", chart: "chart",
    rank: "rank", mv: "movement", tag: "tag", release: "new-release", field: "field",
    results: "results", result: "result", send: "send-button", quote: "quote", steps: "steps",
    today: "today", pill: "pill", link: "link", error: "error",
  };
  var TAG_PARTS = { img: "cover", input: "input", textarea: "input" };

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    var part = className ? PARTS[className.split(" ")[0]] : TAG_PARTS[tag];
    if (part) node.setAttribute("part", part);
    if (text !== undefined) node.textContent = text;
    return node;
  }

  var host = document.createElement("div");
  host.className = "heron-widget heron-" + module + (key !== module ? " heron-" + key : "");
  host.setAttribute("data-mode", mode);
  var target = script.getAttribute("data-target");
  var parent = target && document.querySelector(target);
  if (parent) parent.appendChild(host);
  else script.parentNode.insertBefore(host, script.nextSibling);
  var root = host.attachShadow ? host.attachShadow({ mode: "open" }) : host;
  var style = el("style");
  style.textContent =
    ":host{all:initial;display:block}" +
    ".card{font:var(--heron-font-size,15px)/1.4 var(--heron-font,system-ui,-apple-system,'Segoe UI',sans-serif);" +
    "color:var(--heron-text,#1d1d1b);background:var(--heron-bg,#fff);border:1px solid var(--heron-border,#e3e3df);" +
    "border-radius:var(--heron-radius,14px);overflow:hidden;max-width:var(--heron-max-width,420px)}" +
    ".page .card{max-width:var(--heron-max-width,none)}" +
    ".head{background:var(--color);color:#fff;padding:14px 18px}" +
    ".title{font-size:20px;font-weight:700;margin:0}" +
    ".sub{margin:2px 0 0;opacity:.92}" +
    ".body{padding:16px 18px}" +
    ".closed{text-align:center;padding:22px 18px}" +
    ".closed svg{width:34px;height:34px;color:var(--heron-muted,#5f6067)}" +
    ".closed strong{display:block;font-size:17px;margin:8px 0 4px}" +
    ".muted{color:var(--heron-muted,var(--heron-muted,#5f6067));margin:0}" +
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
    ".chart li{display:flex;gap:12px;align-items:center;padding:8px 0;border-top:1px solid var(--heron-border,#e3e3df)}" +
    ".chart li:first-child{border-top:0}" +
    ".chart li.on{background:#1d1d1b;color:#fff;margin:0 -18px;padding:8px 18px}" +
    ".rank{font-weight:800;font-size:20px;min-width:2ch;text-align:right}" +
    ".page .rank{font-size:26px}" +
    ".mv{font-size:12px;font-weight:700;min-width:5.5em;color:var(--heron-muted,#5f6067)}" +
    ".mv.up{color:#2e7d32}.mv.down{color:#c62828}.mv.new{color:var(--color)}" +
    ".who{flex:1;min-width:0}.who strong{display:block}.chart .q{color:#9a9a95;letter-spacing:.3em}" +
    ".tag{font-size:11px;font-weight:700;background:var(--color);color:#fff;border-radius:6px;padding:1px 6px;margin-left:6px}" +
    ".release{margin-top:12px;border:2px solid var(--color);border-radius:10px;padding:10px 14px}" +
    ".field{display:block;margin:0 0 12px}.field>span{display:block;font-weight:600;margin-bottom:4px}" +
    "input,textarea{font:inherit;color:inherit;background:transparent;box-sizing:border-box;width:100%;" +
    "border:1.5px solid #c9c9c4;border-radius:10px;padding:9px 12px}" +
    "textarea{min-height:84px;resize:vertical}" +
    "input:focus,textarea:focus{outline:2px solid var(--color);outline-offset:1px}" +
    ".count{text-align:right;font-size:13px}" +
    ".row2{display:grid;grid-template-columns:1fr 1fr;gap:12px}" +
    ".widget .row2{display:block}" +
    ".results{list-style:none;margin:0 0 12px;padding:0;border:1px solid var(--heron-border,#e3e3df);border-radius:10px;overflow:hidden}" +
    ".page .results{display:flex;flex-wrap:wrap;gap:10px;border:0;overflow:visible}" +
    ".result{font:inherit;color:inherit;text-align:left;width:100%;display:block;background:transparent;" +
    "border:0;border-top:1px solid var(--heron-border,#e3e3df);padding:8px 12px;cursor:pointer}" +
    ".results li:first-child .result{border-top:0}" +
    ".page .result{border:1.5px solid var(--heron-border,#e3e3df);border-radius:10px;width:auto;min-width:200px}" +
    ".result[aria-pressed=true]{box-shadow:inset 3px 0 var(--color);background:rgba(127,127,127,.1)}" +
    ".page .result[aria-pressed=true]{border-color:var(--color);box-shadow:none}" +
    ".result strong{display:block}" +
    ".send{font:inherit;font-weight:700;font-size:17px;width:100%;padding:12px;border:0;border-radius:10px;" +
    "background:var(--color);color:#fff;cursor:pointer;margin:4px 0 10px}" +
    ".send:disabled{opacity:.6;cursor:default}" +
    ".error{color:#c62828;margin:0 0 10px}.small{font-size:13px}" +
    ".quote{border-left:4px solid var(--color);background:rgba(127,127,127,.08);padding:8px 12px;margin:0 0 14px}" +
    ".steps{list-style:none;margin:0 0 12px;padding:0}" +
    ".steps li{display:flex;gap:12px;align-items:flex-start;margin-bottom:10px}" +
    ".dot{flex:none;width:26px;height:26px;border-radius:50%;border:2px solid var(--color);color:var(--color);" +
    "display:inline-flex;align-items:center;justify-content:center;font-size:13px;font-weight:700;box-sizing:border-box}" +
    ".dot.done{background:var(--color);color:#fff}" +
    ".link{font:inherit;color:inherit;background:transparent;border:0;padding:0;text-decoration:underline;cursor:pointer}" +
    ".today{list-style:none;margin:0;padding:0}" +
    ".today li{display:flex;gap:12px;padding:10px 0;border-top:1px solid var(--heron-border,#e3e3df)}" +
    ".today li:first-child{border-top:0}.today time{color:var(--heron-muted,#5f6067);min-width:3.2em}" +
    ".today em{display:block}" +
    ".pill{font-size:12px;font-weight:700;border-radius:999px;padding:1px 8px;margin-left:6px;background:rgba(46,125,50,.14);color:#2e7d32}" +
    ".pill.soon{background:rgba(21,101,192,.12);color:#1565c0}" +
    ".head.timed{display:flex;justify-content:space-between;align-items:flex-start;gap:12px}" +
    ".clock{font-weight:800;font-variant-numeric:tabular-nums;background:rgba(255,255,255,.22);border-radius:8px;padding:3px 10px}" +
    ".duel{display:flex;flex-direction:column;gap:8px}" +
    ".versus{text-align:center;font-size:13px;font-weight:800;letter-spacing:.12em;color:var(--heron-muted,#5f6067)}" +
    ".choice{font:inherit;color:inherit;text-align:left;display:flex;align-items:center;gap:12px;width:100%;" +
    "padding:10px 14px;border:2px solid var(--heron-border,#e3e3df);border-radius:12px;cursor:pointer;box-sizing:border-box;" +
    "background:linear-gradient(90deg,color-mix(in srgb,var(--color) 14%,transparent) var(--share,0%),transparent var(--share,0%))}" +
    ".choice[aria-pressed=true]{border-color:var(--color)}" +
    ".choice:disabled{cursor:default}" +
    ".art{flex:none;width:52px;height:52px;border-radius:8px;object-fit:cover;background:var(--color);color:#fff;" +
    "display:inline-flex;align-items:center;justify-content:center;font-size:22px;font-weight:800}" +
    ".choice .who strong{font-size:17px}.pct{font-size:22px;font-weight:800;font-variant-numeric:tabular-nums}" +
    ".page .duel{display:grid;grid-template-columns:1fr auto 1fr;align-items:center;gap:20px}" +
    ".page .choice{flex-direction:column;align-items:stretch;padding:16px}" +
    ".page .art{width:100%;height:auto;aspect-ratio:2/1;font-size:64px}" +
    ".page .choice .who strong{font-size:24px}.page .pct{font-size:34px}" +
    ".bar{height:8px;border-radius:4px;background:var(--heron-border,#e3e3df);overflow:hidden}.bar span{display:block;height:100%;background:var(--color)}" +
    ".page .question{display:flex;justify-content:space-between;align-items:center;margin:0 0 16px}" +
    ".page .question strong{font-size:24px}.page .question .clock{font-size:44px;background:none;color:var(--color);padding:0}" +
    ".center{text-align:center}" +
    "@keyframes hurry{50%{opacity:.3}}" +
    ".hurry .clock,.hurry .choice{animation:hurry 1s steps(1,end) infinite}" +
    "@media (prefers-reduced-motion: reduce){.hurry .clock,.hurry .choice{animation:none;outline:3px solid var(--color);outline-offset:2px}" +
    ".head .clock{outline-color:#fff}}" +
    "@media (max-width:560px){.page .duel{grid-template-columns:1fr}}" +
    "@media (prefers-color-scheme: dark){.card{background:var(--heron-bg,#1f1f1e);color:var(--heron-text,#ededea);" +
    "border-color:var(--heron-border,#3a3a37)}.muted,.closed svg{color:var(--heron-muted,#b5b5b0)}}";
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
    // The votes of now count for the top whose next show plays them first.
    var top = null;
    (state.shows || []).forEach(function (s) {
      var at = s.module === "top30" && s.enabled && s.state && s.state.votes_count_for_unix_ms;
      if (at && (!top || at < top.state.votes_count_for_unix_ms)) top = s;
    });
    if (top) {
      var counts = top.state.votes_count_for_unix_ms;
      var weekday = new Intl.DateTimeFormat(lang, { weekday: "long", timeZone: top.timezone || state.timezone }).format(counts);
      body.appendChild(el("p", "muted", fill(t.top30.votesFor, { name: top.title || top.state.name || t.names.top30, day: weekday })));
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
      date: new Intl.DateTimeFormat(lang, { day: "numeric", month: "long", timeZone: "UTC" }).format(Date.parse(top.chart.key + "T00:00:00Z")),
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
      if (!entry.hidden && entry.charts && mode === "page") row.appendChild(el("span", "muted", fill(t.top30.weeks, { n: entry.charts })));
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


  // Dedications (docs/listener-interaction.md#dedications): search a track
  // the station can play, then a first name, who it is for and a message;
  // once sent, its follow link. The form's fields are built once, so that
  // what is typed stays while the station's state changes.
  var ded = {
    query: "",
    results: null,
    picked: null,
    searching: null,
    error: null,
    sending: false,
    follow: null,
    followed: null,
    polling: null,
  };
  var fields = null;

  function field(label, input) {
    var wrap = el("label", "field");
    wrap.appendChild(el("span", "", label));
    wrap.appendChild(input);
    return wrap;
  }

  function buildFields() {
    var search = el("input");
    search.type = "search";
    search.maxLength = 60;
    search.placeholder = t.dedications.search;
    search.autocomplete = "off";
    search.addEventListener("input", function () {
      ded.query = search.value.trim();
      clearTimeout(ded.searching);
      ded.searching = setTimeout(searchTracks, 300);
    });
    var from = el("input");
    from.maxLength = 40;
    from.autocomplete = "given-name";
    var to = el("input");
    to.maxLength = 60;
    var message = el("textarea");
    message.addEventListener("input", function () { render(latest); });
    return { search: search, from: from, to: to, message: message };
  }

  function searchTracks() {
    if (ded.query.length < 2) {
      ded.results = null;
      render(latest);
      return;
    }
    var asked = ded.query;
    fetch(base + "/dedications/search?q=" + encodeURIComponent(asked))
      .then(function (r) { return r.ok ? r.json() : []; })
      .then(function (found) {
        if (asked !== ded.query) return;
        ded.results = found;
        render(latest);
      }, function () {});
  }

  function trackText(track) {
    return [track.title, track.artist].filter(Boolean).join(" – ");
  }

  function followHere() {
    var found = /(?:^#|&)heron-dedication=([0-9a-f]{32})/.exec(location.hash);
    return found ? found[1] : null;
  }

  function startFollow(key) {
    ded.follow = key;
    clearInterval(ded.polling);
    var poll = function () {
      fetch(base + "/dedications/follow?id=" + key)
        .then(function (r) { return r.ok ? r.json() : null; })
        .then(function (followed) {
          if (ded.follow !== key) return;
          if (followed) ded.followed = followed;
          if (followed && (followed.status === "played" || followed.status === "refused")) clearInterval(ded.polling);
          if (latest) render(latest);
        }, function () {});
    };
    poll();
    ded.polling = setInterval(poll, 10000);
  }

  function sendDedication(show) {
    var f = fields;
    if (!ded.picked) {
      ded.error = t.dedications.pick;
      render(latest);
      return;
    }
    ded.sending = true;
    ded.error = null;
    render(latest);
    api.send("", {
      track_id: ded.picked.track_id,
      from: f.from.value.trim(),
      to: f.to.value.trim(),
      message: f.message.value.trim(),
    }).then(function (response) {
      return response.json().catch(function () { return {}; }).then(function (body) {
        ded.sending = false;
        if (response.status === 201 && body.follow) {
          ded.followed = body;
          f.search.value = f.message.value = "";
          ded.query = "";
          ded.results = null;
          ded.picked = null;
          startFollow(body.follow);
        } else if (response.status === 429) {
          ded.error = t.dedications.tooMany;
        } else if (response.status === 409) {
          ded.error = show.open ? t.dedications.fullError : t.dedications.closedError;
        } else if (response.status === 422 && body.message) {
          ded.error = body.message;
        } else {
          ded.error = t.dedications.failed;
        }
        render(latest);
      });
    }, function () {
      ded.sending = false;
      ded.error = t.dedications.failed;
      render(latest);
    });
  }

  function step(list, done, mark, label, sub) {
    var li = el("li");
    var dot = el("span", "dot" + (done ? " done" : ""), mark);
    li.appendChild(dot);
    var words = el("div");
    words.appendChild(el("div", "", label));
    if (sub) words.appendChild(el("div", "muted small", sub));
    li.appendChild(words);
    list.appendChild(li);
  }

  function followCard(show, zone) {
    var f = ded.followed;
    var card = el("div", "card");
    var color = /^#[0-9a-fA-F]{6}$/.test(show.color) ? show.color : "#c62828";
    card.style.setProperty("--color", f && f.status === "refused" ? "#5f6067" : color);
    var head = el("div", "head");
    head.appendChild(el("p", "title", t.dedications.yours));
    if (f) head.appendChild(el("p", "sub", trackText(f)));
    card.appendChild(head);
    var body = el("div", "body");
    if (!f) {
      body.appendChild(el("p", "muted", t.dedications.sending));
      card.appendChild(body);
      return card;
    }
    var quote = fill(t.dedications.forFrom, { to: f.to, from: f.from });
    body.appendChild(el("p", "quote", f.message ? fill(t.dedications.said, { who: quote, message: f.message }) : quote));
    var steps = el("ol", "steps");
    if (f.status === "refused") {
      step(steps, false, "✕", t.dedications.refused, t.dedications.reasons[f.reason] || "");
    } else {
      step(steps, true, "✓", t.dedications.sent, hour(f.submitted_at_unix_ms, zone));
      if (f.status === "accepted" || f.status === "played") {
        step(steps, true, "✓", t.dedications.accepted, f.decided_at_unix_ms ? hour(f.decided_at_unix_ms, zone) : "");
      } else {
        step(steps, false, "2", t.dedications.accepted, "");
      }
      if (f.status === "played") {
        step(steps, true, "✓", fill(t.dedications.played, { hour: hour(f.played_at_unix_ms, zone) }), "");
      } else if (f.planned_at_unix_ms) {
        step(steps, false, "3", fill(t.dedications.onAirAt, { hour: hour(f.planned_at_unix_ms, zone) }), t.dedications.keepOpen);
      } else {
        step(steps, false, "3", fill(t.dedications.onAirAt, { hour: "…" }), t.dedications.keepOpen);
      }
    }
    body.appendChild(steps);
    var url = location.href.split("#")[0] + "#heron-dedication=" + ded.follow;
    var link = el("p", "muted small", t.dedications.followLink + " ");
    var a = el("a", "", url.replace(/^https?:\/\//, "").slice(0, 48) + (url.length > 56 ? "…" : ""));
    a.href = url;
    link.appendChild(a);
    body.appendChild(link);
    if (f.status === "refused" || f.status === "played") {
      var again = el("button", "link", t.dedications.another);
      again.type = "button";
      again.addEventListener("click", function () {
        clearInterval(ded.polling);
        ded.follow = null;
        ded.followed = null;
        if (followHere()) history.replaceState(null, "", location.href.split("#")[0]);
        render(latest);
      });
      body.appendChild(again);
    }
    card.appendChild(body);
    return card;
  }

  function todayList(show, zone) {
    var today = (show.state && show.state.today) || [];
    if (mode !== "page" || !today.length) return null;
    var card = el("div", "card");
    card.style.marginTop = "16px";
    var body = el("div", "body");
    body.appendChild(el("p", "title", t.dedications.today));
    var list = el("ul", "today");
    today.forEach(function (d) {
      var li = el("li");
      li.appendChild(el("time", "", d.at_unix_ms ? hour(d.at_unix_ms, zone) : ""));
      var words = el("div");
      var title = el("strong", "", [d.artist, d.title].filter(Boolean).join(" – "));
      title.appendChild(el("span", d.played ? "pill" : "pill soon", d.played ? t.dedications.playedTag : t.dedications.upcoming));
      words.appendChild(title);
      var quote = fill(t.dedications.forFrom, { to: d.to, from: d.from });
      words.appendChild(el("em", "muted", d.message ? fill(t.dedications.said, { who: quote, message: d.message }) : quote));
      li.appendChild(words);
      list.appendChild(li);
    });
    body.appendChild(list);
    body.appendChild(el("p", "muted small", t.dedications.onlyAccepted));
    card.appendChild(body);
    return card;
  }

  RENDERERS.dedications = function (body, show) {
    if (!fields) fields = buildFields();
    var f = fields;
    var state = show.state || {};
    var max = state.message_max || 140;
    f.message.maxLength = max;
    body.appendChild(field(t.dedications.track, f.search));
    if (ded.query.length > 0 && ded.query.length < 2) body.appendChild(el("p", "muted small", t.dedications.searchMore));
    if (ded.results) {
      if (!ded.results.length) body.appendChild(el("p", "muted", t.dedications.noResult));
      var list = el("ul", "results");
      ded.results.forEach(function (track) {
        var li = el("li");
        var button = el("button", "result");
        button.type = "button";
        button.setAttribute("aria-pressed", ded.picked && ded.picked.track_id === track.track_id ? "true" : "false");
        button.appendChild(el("strong", "", track.title || ""));
        if (track.artist) button.appendChild(el("span", "muted", track.artist));
        button.addEventListener("click", function () {
          ded.picked = track;
          ded.error = null;
          render(latest);
        });
        li.appendChild(button);
        list.appendChild(li);
      });
      body.appendChild(list);
    }
    var names = el("div", "row2");
    names.appendChild(field(t.dedications.from, f.from));
    names.appendChild(field(t.dedications.to, f.to));
    body.appendChild(names);
    body.appendChild(field(t.dedications.message, f.message));
    body.appendChild(el("p", "muted count", f.message.value.length + " / " + max));
    if (ded.error) body.appendChild(el("p", "error", ded.error));
    var send = el("button", "send", ded.sending ? t.dedications.sending : t.dedications.send);
    send.type = "button";
    send.disabled = ded.sending || state.remaining === 0;
    send.addEventListener("click", function () { sendDedication(show); });
    body.appendChild(send);
    var per = state.per_device > 1 ? fill(t.dedications.perMany, { n: state.per_device }) : t.dedications.perOne;
    body.appendChild(el("p", "muted small", fill(t.dedications.note, { per: per, days: state.retention_days || 30 })));
  };

  RENDERERS.top30 = function (body, show) {
    chart(body, show);
  };

  // The match (docs/listener-interaction.md#match): the two tracks, a vote
  // for one, the countdown to the close, blinking in its last 5 seconds.
  // The vote is kept for its match only, in memory.
  var ballot = { match: null, choice: null, note: null };
  var clockOffset = 0;

  function remaining(show) {
    var closes = show && show.state && show.state.closes_at_unix_ms;
    if (!closes) return null;
    return Math.max(0, closes - (Date.now() + clockOffset));
  }

  function clockText(ms) {
    var s = Math.ceil(ms / 1000);
    return Math.floor(s / 60) + ":" + (s % 60 < 10 ? "0" : "") + (s % 60);
  }

  // Updates the countdowns between two events, without drawing again.
  function tick() {
    if (module !== "match") return;
    var show = latest && (latest.shows || []).filter(function (s) { return s.module === "match"; })[0];
    var left = remaining(show);
    var clocks = root.querySelectorAll(".clock");
    for (var i = 0; i < clocks.length; i++) clocks[i].textContent = left === null ? "" : clockText(left);
    var card = root.querySelector(".card");
    if (card) card.classList.toggle("hurry", left !== null && left > 0 && left <= 5000);
    var buttons = root.querySelectorAll(".choice");
    for (var j = 0; j < buttons.length; j++) buttons[j].disabled = left === 0;
  }

  function votesText(n) {
    return n === 1 ? t.match.oneVote : fill(t.match.votes, { n: n });
  }

  function art(track) {
    if (track.cover_url) {
      var img = el("img", "art");
      img.alt = "";
      img.src = track.cover_url + (track.cover_url.indexOf("?") < 0 ? "?size=" + (mode === "page" ? 600 : 160) : "");
      return img;
    }
    return el("span", "art", (track.title || "?").charAt(0).toUpperCase());
  }

  function trackName(track) {
    return [track.artist, track.title].filter(Boolean).join(" – ");
  }

  function resultLine(result, zone) {
    var li = el("li");
    if (result.closed_at_unix_ms) li.appendChild(el("time", "", hour(result.closed_at_unix_ms, zone)));
    var who = el("div", "who");
    var line = el("strong", "", trackName(result.winner));
    var how = result.decided_by === "tie" ? t.match.tie
      : result.decided_by === "no_votes" ? t.match.noVotes : result.percent + " %";
    line.appendChild(el("span", "pill", how));
    who.appendChild(line);
    who.appendChild(el("span", "muted", fill(t.match.against, { track: trackName(result.loser) })));
    li.appendChild(who);
    return li;
  }

  RENDERERS.match = function (body, show, state, api) {
    var match = show.state || {};
    var zone = show.timezone || state.timezone;
    if (state.now_playing && state.now_playing.server_time_unix_ms) {
      clockOffset = state.now_playing.server_time_unix_ms - Date.now();
    }
    if (!match.match_id) {
      var last = match.last;
      if (last) {
        body.appendChild(el("p", "muted small", t.match.last));
        var row = el("div", "np");
        row.appendChild(art(last.winner));
        var words = el("div");
        words.appendChild(el("strong", "", fill(t.match.won, { title: last.winner.title || "", percent: last.percent + " %" })));
        words.appendChild(el("div", "muted", fill(t.match.against, { track: trackName(last.loser) })));
        row.appendChild(words);
        body.appendChild(row);
      }
      body.appendChild(el("p", "muted", ballot.match ? t.match.closed : t.match.next));
      return;
    }
    if (ballot.match !== match.match_id) ballot = { match: match.match_id, choice: null, note: null };
    var left = remaining(show);
    if (mode === "page") {
      var question = el("div", "question");
      question.appendChild(el("strong", "", t.match.question));
      question.appendChild(el("span", "clock", left === null ? "" : clockText(left)));
      body.appendChild(question);
    }
    var duel = el("div", "duel");
    (match.choices || []).forEach(function (choice, index) {
      if (index === 1) duel.appendChild(el("div", "versus", t.match.versus));
      var button = el("button", "choice");
      button.type = "button";
      button.disabled = left === 0;
      button.setAttribute("aria-pressed", ballot.choice === choice.choice ? "true" : "false");
      var live = typeof choice.percent === "number";
      if (live) button.style.setProperty("--share", choice.percent + "%");
      button.appendChild(art(choice));
      var who = el("span", "who");
      who.appendChild(el("strong", "", choice.title || ""));
      if (choice.artist) who.appendChild(el("span", "muted", choice.artist));
      if (mode === "page" && live) {
        var bar = el("span", "bar");
        var fillBar = el("span");
        fillBar.style.width = choice.percent + "%";
        bar.appendChild(fillBar);
        who.appendChild(bar);
      }
      button.appendChild(who);
      if (live) button.appendChild(el("span", "pct", choice.percent + " %"));
      button.addEventListener("click", function () {
        if (ballot.choice === choice.choice) return;
        var mine = ballot;
        mine.choice = choice.choice;
        mine.note = t.match.sending;
        render(latest);
        api.send("vote", { match_id: match.match_id, choice: choice.choice }).then(
          function (response) {
            mine.note = response.ok ? t.match.change : response.status === 409 ? t.match.closed : t.match.refused;
            if (!response.ok) mine.choice = null;
            render(latest);
          },
          function () {
            mine.note = t.match.refused;
            mine.choice = null;
            render(latest);
          }
        );
      });
      duel.appendChild(button);
    });
    body.appendChild(duel);
    if (typeof match.votes === "number") body.appendChild(el("p", "muted center small", votesText(match.votes)));
    if (ballot.note) body.appendChild(el("p", "muted center small", ballot.note));
  };

  // Today's results, under the page.
  function matchResults(show, zone) {
    var today = show.state && show.state.today;
    if (mode !== "page" || !today || !today.length) return null;
    var card = el("div", "card");
    card.style.marginTop = "16px";
    var body = el("div", "body");
    body.appendChild(el("p", "title", t.match.today));
    var list = el("ul", "today");
    today.forEach(function (result) { list.appendChild(resultLine(result, zone)); });
    body.appendChild(list);
    card.appendChild(body);
    return card;
  }

  if (module === "match") setInterval(tick, 250);

  function render(state) {
    if (!state) return;
    // What is typed keeps its focus and caret when the card is drawn again.
    var focused = root.activeElement || null;
    var caret = focused && typeof focused.selectionStart === "number"
      ? [focused.selectionStart, focused.selectionEnd] : null;
    box.textContent = "";
    var shows = state.shows || [];
    var show = shows.filter(function (s) { return (s.key || s.module) === key; })[0];
    // `top30.js`: the first top.
    if (!show && key === "top30") show = shows.filter(function (s) { return s.module === "top30"; })[0];
    if (!show) return;
    draw(state, show);
    if (focused && focused.isConnected && root.activeElement !== focused) {
      focused.focus();
      if (caret) {
        try { focused.setSelectionRange(caret[0], caret[1]); } catch (error) { /* not a text field */ }
      }
    }
  }

  function draw(state, show) {
    if (mode === "widget" && !show.small_widget) return;
    if (!show.open && show.closed === "nothing") return;
    var name = show.title || t.names[module] || module;
    var zone = show.timezone || state.timezone;
    // A dedication sent: its follow link, open or closed.
    if (module === "dedications" && ded.follow) {
      box.appendChild(followCard(show, zone));
      var past = todayList(show, zone);
      if (past) box.appendChild(past);
      return;
    }
    var card = el("div", "card");
    var color = /^#[0-9a-fA-F]{6}$/.test(show.color) ? show.color : "#c62828";
    card.style.setProperty("--color", show.open ? color : "#5f6067");
    var head = el("div", "head");
    head.appendChild(el("p", "title", name));
    if (show.open && module === "likes") {
      head.appendChild(el("p", "sub", t.likes.sub));
    } else if (show.open && module === "match") {
      // The countdown, in the head of the small widget; nothing more.
      var words = el("div");
      words.appendChild(head.firstChild);
      words.appendChild(el("p", "sub", t.match.sub));
      head.appendChild(words);
      head.className = "head timed";
      var left = remaining(show);
      if (mode === "widget" && left !== null) head.appendChild(el("span", "clock", clockText(left)));
    } else if (show.open && show.closes_at_unix_ms) {
      var until = fill(t.until, { hour: hour(show.closes_at_unix_ms, zone) });
      var left = module === "dedications" && show.state ? show.state.remaining : null;
      if (left === 0) until += " · " + t.dedications.full;
      else if (typeof left === "number") until += " · " + fill(t.dedications.remaining, { n: left });
      head.appendChild(el("p", "sub", until));
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
    if (module === "dedications") {
      var today = todayList(show, zone);
      if (today) box.appendChild(today);
    }
    if (module === "match" && show.open) {
      var results = matchResults(show, zone);
      if (results) box.appendChild(results);
    }
    tick();
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
  if (module === "dedications" && followHere()) startFollow(followHere());
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
