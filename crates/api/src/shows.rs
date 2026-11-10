// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shows (`docs/listener-interaction.md`): their settings per station
//! (`station_settings`, `show_<module>`), what the gateway reads of them,
//! and their notifications through the station's webhooks.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use heron_core::shows::{
    self, Change, HourRange, NotificationSettings, SHOWS, Show, ShowPublic, ShowSettings,
    ShowStatus, WebsiteSettings, Week,
};
use heron_core::{Module, Modules};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use sqlx::MySqlPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::app::AppState;
use crate::auth::{CurrentUser, Role};
use crate::error::{ApiError, ApiResult, ErrorBody};
use crate::settings::{self, StationSettings};
use crate::stations::{Station, Stations};
use crate::webhooks::{Payload, PayloadShow, WebhookEvent, Webhooks};

/// How often the notifications look for changes.
const TICK: Duration = Duration::from_secs(15);

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_shows))
        .routes(routes!(get_show, put_show))
}

/// A webhook a show can send to.
#[derive(Clone, Debug, Serialize, ToSchema, sqlx::FromRow)]
pub struct WebhookChoice {
    pub id: u64,
    pub name: String,
}

/// A show on a station.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ShowView {
    pub module: Module,
    pub settings: ShowSettings,
    /// Now.
    pub status: ShowStatus,
    /// The station's time zone (IANA), which the hours follow.
    pub timezone: String,
    /// The station's webhooks, by name, to choose from.
    pub webhooks: Vec<WebhookChoice>,
}

/// The shows set by the module's settings: all but the tops, which have
/// their own (`/tops`).
fn module_shows() -> impl Iterator<Item = &'static Show> {
    SHOWS.iter().filter(|show| show.module != Module::Top30)
}

/// The show of a path's `{module}`.
fn show_of(key: &str) -> ApiResult<&'static Show> {
    Module::from_key(key)
        .and_then(Show::of)
        .filter(|show| show.module != Module::Top30)
        .ok_or_else(|| ApiError::NotFound(format!("there is no show {key:?}")))
}

/// A show's settings on a station, every event in it.
pub async fn load(pool: &MySqlPool, station_id: u64, show: &Show) -> ApiResult<ShowSettings> {
    let mut settings: ShowSettings =
        settings::load_for(pool, station_id, show.settings_name()).await?;
    settings.complete(show);
    Ok(settings)
}

async fn station_settings(pool: &MySqlPool, station_id: u64) -> ApiResult<StationSettings> {
    settings::load_for(pool, station_id, StationSettings::SETTINGS_NAME).await
}

/// The shows on for a station, as the gateway and the widgets see them.
pub async fn public_shows(pool: &MySqlPool, station_id: u64) -> ApiResult<Vec<ShowPublic>> {
    let on = crate::modules::on_for(pool, station_id).await?;
    let timezone = station_settings(pool, station_id).await?.timezone;
    let mut shows = Vec::new();
    for show in SHOWS.iter().filter(|show| on.is_on(show.module)) {
        if show.module == Module::Top30 {
            // Each top is a show of its own; one that fails never hides
            // the others' shows.
            match crate::tops::public_shows(pool, station_id, &timezone).await {
                Ok(tops) => shows.extend(tops),
                Err(error) => {
                    tracing::warn!(station = station_id, %error, "the tops cannot be read")
                }
            }
            continue;
        }
        let settings = load(pool, station_id, show).await?;
        // Each show adds its live data here (`state`).
        let mut public = ShowPublic::new(show.module, &settings, &timezone);
        if show.module == Module::Likes {
            public.state = crate::likes::public_state(pool, station_id).await?;
        }
        if show.module == Module::Dedications {
            public.state =
                crate::dedications::public_state(pool, station_id, &settings, &timezone).await?;
        }
        if show.module == Module::Match {
            public.state = crate::matches::public_state(pool, station_id, &timezone).await?;
        }
        shows.push(public);
    }
    Ok(shows)
}

async fn view(pool: &MySqlPool, station_id: u64, show: &Show) -> ApiResult<ShowView> {
    let settings = load(pool, station_id, show).await?;
    let timezone = station_settings(pool, station_id).await?.timezone;
    let status = ShowPublic::new(show.module, &settings, &timezone).status(Timestamp::now());
    let webhooks =
        sqlx::query_as("SELECT id, name FROM webhooks WHERE station_id = ? ORDER BY name, id")
            .bind(station_id)
            .fetch_all(pool)
            .await?;
    Ok(ShowView {
        module: show.module,
        settings,
        status,
        timezone,
        webhooks,
    })
}

/// The shows on for the station, with their settings and whether they are
/// open now.
#[utoipa::path(get, path = "/shows", tag = "shows",
    responses((status = 200, body = Vec<ShowView>), (status = "4XX", body = ErrorBody)))]
async fn list_shows(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
) -> ApiResult<Json<Vec<ShowView>>> {
    let on = crate::modules::on_for(&state.pool, station.id()).await?;
    let mut views = Vec::new();
    for show in module_shows().filter(|show| on.is_on(show.module)) {
        views.push(view(&state.pool, station.id(), show).await?);
    }
    Ok(Json(views))
}

/// A show's settings on the station, and whether it is open now.
#[utoipa::path(get, path = "/shows/{module}", tag = "shows",
    params(("module" = String, Path, description = "dedications, match or likes")),
    responses((status = 200, body = ShowView), (status = "4XX", body = ErrorBody)))]
async fn get_show(
    _user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(module): Path<String>,
) -> ApiResult<Json<ShowView>> {
    let show = show_of(&module)?;
    Ok(Json(view(&state.pool, station.id(), show).await?))
}

/// Replaces a show's settings on the station. Hours another show of the
/// station has, or a top's show, are refused (`409`). Operators.
#[utoipa::path(put, path = "/shows/{module}", tag = "shows",
    params(("module" = String, Path, description = "dedications, match or likes")),
    request_body = ShowSettings,
    responses((status = 200, body = ShowView), (status = "4XX", body = ErrorBody)))]
async fn put_show(
    user: CurrentUser,
    State(state): State<AppState>,
    station: Station,
    Path(module): Path<String>,
    Json(mut settings): Json<ShowSettings>,
) -> ApiResult<Json<ShowView>> {
    user.require(Role::Operator)?;
    let show = show_of(&module)?;
    settings.check(show).map_err(ApiError::Invalid)?;
    settings.complete(show);
    let pool = &state.pool;
    // A show not enabled holds no hours: it is checked when enabled again.
    if settings.enabled {
        for range in &settings.hours {
            check_free(pool, station.id(), range, Some(show.module)).await?;
        }
    }
    let webhooks: Vec<u64> = sqlx::query_scalar("SELECT id FROM webhooks WHERE station_id = ?")
        .bind(station.id())
        .fetch_all(pool)
        .await?;
    if let Some(id) = settings
        .notifications
        .webhook_ids
        .iter()
        .find(|id| !webhooks.contains(id))
    {
        return Err(ApiError::Invalid(format!(
            "webhook {id} is not one of the station's"
        )));
    }
    settings.notifications.webhook_ids.sort_unstable();
    settings.notifications.webhook_ids.dedup();
    settings::save_for(pool, station.id(), show.settings_name(), &settings).await?;
    tracing::info!(user = %user.username, station = station.id(), show = show.module.key(), "show settings changed");
    Ok(Json(view(pool, station.id(), show).await?))
}

/// Refuses hours that another show of the station has (`409`, saying
/// which): an hour belongs to one show at most. `module` is the show they
/// are for (`None`: a top). Only the shows that are on count: a show whose
/// module is off, or that is not enabled, leaves its hours to the others
/// (turning it back on is checked then, [`check_turned_on`]). The tops'
/// shows count with the whole hours they touch.
pub(crate) async fn check_free(
    pool: &MySqlPool,
    station_id: u64,
    range: &HourRange,
    module: Option<Module>,
) -> ApiResult<()> {
    let week = Week::of(std::slice::from_ref(range));
    let on = crate::modules::on_for(pool, station_id).await?;
    for taker in hours_held(pool, station_id, &on).await? {
        let theirs = taker.module == Module::Top30;
        // A top against the tops: their own rule (`tops::same_slot`).
        if (module.is_none() && theirs) || Some(taker.module) == module {
            continue;
        }
        if let Some((day, start, end)) = week.overlap(&taker.week).first() {
            return Err(ApiError::Conflict(format!(
                "{start:02}:00–{end:02}:00 on {} belongs to {}: an hour belongs to one show at most",
                shows::day_name(*day),
                taker.name,
            )));
        }
    }
    Ok(())
}

/// A show on, and the hours it holds.
struct Taker {
    module: Module,
    /// As messages name it: `dedications`, `the top “Hits”`.
    name: String,
    week: Week,
}

/// The hours each show on (`on`) and enabled holds on a station: a show
/// off holds none.
async fn hours_held(pool: &MySqlPool, station_id: u64, on: &Modules) -> ApiResult<Vec<Taker>> {
    let mut takers = Vec::new();
    for show in module_shows().filter(|show| on.is_on(show.module)) {
        let settings = load(pool, station_id, show).await?;
        if settings.enabled && !settings.hours.is_empty() {
            takers.push(Taker {
                module: show.module,
                name: show.module.name().to_owned(),
                week: Week::of(&settings.hours),
            });
        }
    }
    if on.is_on(Module::Top30) {
        for (name, hours) in crate::tops::hours_taken(pool, station_id).await? {
            takers.push(Taker {
                module: Module::Top30,
                name: format!("the top “{name}”"),
                week: Week::of(&[hours]),
            });
        }
    }
    Ok(takers)
}

/// Refuses turning shows on (from `before` to `after`, what is on for the
/// station) whose hours another show on has (`409`, saying which): a show
/// off left them to the others. Two tops follow their own rule.
pub(crate) async fn check_turned_on(
    pool: &MySqlPool,
    station_id: u64,
    before: &Modules,
    after: &Modules,
) -> ApiResult<()> {
    let turned_on = |module: Module| !before.is_on(module) && after.is_on(module);
    if !SHOWS.iter().any(|show| turned_on(show.module)) {
        return Ok(());
    }
    let takers = hours_held(pool, station_id, after).await?;
    for (i, mine) in takers.iter().enumerate() {
        for theirs in &takers[i + 1..] {
            let both_tops = mine.module == Module::Top30 && theirs.module == Module::Top30;
            if both_tops || !(turned_on(mine.module) || turned_on(theirs.module)) {
                continue;
            }
            let (new, held) = if turned_on(mine.module) {
                (mine, theirs)
            } else {
                (theirs, mine)
            };
            if let Some((day, start, end)) = new.week.overlap(&held.week).first() {
                return Err(ApiError::Conflict(format!(
                    "{} cannot be turned on: {start:02}:00–{end:02}:00 on {} belongs to {}: an hour belongs to one show at most",
                    new.name,
                    shows::day_name(*day),
                    held.name,
                )));
            }
        }
    }
    Ok(())
}

/// Sends the shows' notifications (`show.upcoming`, `show.started`,
/// `show.ended`) as their times come. Times that passed while the API was
/// stopped are not sent.
pub async fn notify(pool: MySqlPool, stations: Arc<Stations>, webhooks: Arc<Webhooks>) {
    let mut last = Timestamp::now();
    loop {
        tokio::time::sleep(TICK).await;
        let now = Timestamp::now();
        for station in stations.all() {
            let code = station.code();
            if let Err(error) =
                notify_station(&pool, &webhooks, station.id(), &code, last, now).await
            {
                tracing::warn!(station = %code, %error, "cannot send the shows' notifications");
            }
        }
        last = now;
    }
}

pub(crate) async fn notify_station(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station_id: u64,
    code: &str,
    from: Timestamp,
    to: Timestamp,
) -> ApiResult<()> {
    let on = crate::modules::on_for(pool, station_id).await?;
    if !on.webhooks {
        return Ok(());
    }
    let station = station_settings(pool, station_id).await?;
    let tz = station.time_zone();
    if on.is_on(Module::Top30)
        && let Err(error) =
            crate::tops::notify_shows(pool, webhooks, station_id, code, &tz, from, to).await
    {
        tracing::warn!(station = %code, %error, "cannot send the tops' notifications");
    }
    for show in module_shows().filter(|show| on.is_on(show.module)) {
        let settings = load(pool, station_id, show).await?;
        let notifications = &settings.notifications;
        if notifications.webhook_ids.is_empty() {
            continue;
        }
        let week = settings.open_week();
        let lead = SignedDuration::from_mins(i64::from(notifications.lead_minutes));
        for (change, at) in shows::changes(&week, &tz, from, to, lead) {
            let (event, name) = match change {
                Change::Upcoming { .. } => (WebhookEvent::ShowUpcoming, shows::SHOW_UPCOMING),
                Change::Started => (WebhookEvent::ShowStarted, shows::SHOW_STARTED),
                Change::Ended => (WebhookEvent::ShowEnded, shows::SHOW_ENDED),
            };
            let Some(wanted) = notifications.events.get(name).filter(|event| event.enabled) else {
                continue;
            };
            let times = end_and_next(&settings, &tz, change, at);
            let payload = message(
                &Notice::of(show, &settings),
                &station,
                code,
                &tz,
                times,
                &wanted.message,
                &[],
            );
            let payload = Payload::show(event, payload.0, payload.1);
            let sent = webhooks
                .queue_show(station_id, &notifications.webhook_ids, payload)
                .await?;
            tracing::info!(
                station = code,
                show = show.module.key(),
                event = name,
                sent,
                "show notification"
            );
        }
    }
    Ok(())
}

/// A show as its notifications name it: a module's, or a top.
pub(crate) struct Notice<'a> {
    pub show: &'a Show,
    /// Its name in the gateway's routes, for `{link}`.
    pub key: String,
    /// `{name}`: its title on the website, else its name.
    pub name: String,
    pub website: &'a WebsiteSettings,
    pub notifications: &'a NotificationSettings,
}

impl<'a> Notice<'a> {
    pub fn of(show: &'a Show, settings: &'a ShowSettings) -> Self {
        Self {
            show,
            key: show.module.key().to_owned(),
            name: Some(settings.website.title.trim())
                .filter(|title| !title.is_empty())
                .unwrap_or(show.name)
                .to_owned(),
            website: &settings.website,
            notifications: &settings.notifications,
        }
    }
}

/// When a module's show closes and opens next, as a change at `at` says.
fn end_and_next(
    settings: &ShowSettings,
    tz: &TimeZone,
    change: Change,
    at: Timestamp,
) -> (Option<i64>, Option<i64>) {
    let week = settings.open_week();
    // Upcoming: as the show will be once it opens.
    let reference = match change {
        Change::Upcoming { opens } => opens,
        _ => at,
    };
    let status = shows::status(&week, tz, reference);
    let next = match change {
        Change::Upcoming { opens } => Some(opens.as_millisecond()),
        _ => status.next_opening_unix_ms,
    };
    (status.closes_at_unix_ms, next)
}

/// Sends one of a show's own events (`dedication.played`), with its own
/// placeholders, to the webhooks the show chose: not while the webhooks or
/// the event are off. Returns the deliveries queued.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_own_event(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station_id: u64,
    code: &str,
    show: &Show,
    event: WebhookEvent,
    at: Timestamp,
    extra: &[(&str, String)],
) -> ApiResult<usize> {
    let settings = load(pool, station_id, show).await?;
    let tz = station_settings(pool, station_id).await?.time_zone();
    let times = end_and_next(&settings, &tz, Change::Started, at);
    send_notice(
        pool,
        webhooks,
        station_id,
        code,
        &Notice::of(show, &settings),
        event,
        times,
        extra,
    )
    .await
}

/// Sends an event of a show (a module's or a top), with its placeholders
/// and when it closes and opens next, to the webhooks it chose: not while
/// the webhooks or the event are off. Returns the deliveries queued.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_notice(
    pool: &MySqlPool,
    webhooks: &Webhooks,
    station_id: u64,
    code: &str,
    notice: &Notice<'_>,
    event: WebhookEvent,
    times: (Option<i64>, Option<i64>),
    extra: &[(&str, String)],
) -> ApiResult<usize> {
    if !crate::modules::on_for(pool, station_id).await?.webhooks {
        return Ok(0);
    }
    let notifications = notice.notifications;
    let Some(wanted) = notifications
        .events
        .get(event.as_str())
        .filter(|wanted| wanted.enabled)
    else {
        return Ok(0);
    };
    if notifications.webhook_ids.is_empty() {
        return Ok(0);
    }
    let station = station_settings(pool, station_id).await?;
    let tz = station.time_zone();
    let (show_payload, text) = message(notice, &station, code, &tz, times, &wanted.message, extra);
    let payload = Payload::show(event, show_payload, text);
    let sent = webhooks
        .queue_show(station_id, &notifications.webhook_ids, payload)
        .await?;
    tracing::info!(
        station = code,
        show = %notice.key,
        event = event.as_str(),
        sent,
        "show notification"
    );
    Ok(sent)
}

/// A show event's show and message, written out.
fn message(
    notice: &Notice<'_>,
    station: &StationSettings,
    code: &str,
    tz: &TimeZone,
    (end, next): (Option<i64>, Option<i64>),
    template: &str,
    extra: &[(&str, String)],
) -> (PayloadShow, String) {
    let name = notice.name.clone();
    let link = notice.website.page_url.clone().or_else(|| {
        station.gateway_url.as_ref().map(|gateway| {
            format!(
                "{gateway}/api/v1/public/stations/{code}/{}.html",
                notice.key
            )
        })
    });
    let time = |ms: Option<i64>| ms.and_then(|ms| Timestamp::from_millisecond(ms).ok());
    let language = &notice.notifications.language;
    let mut values = vec![
        ("name", name.clone()),
        ("link", link.clone().unwrap_or_default()),
        (
            "end",
            time(end)
                .map(|at| shows::hour_text(at, tz))
                .unwrap_or_default(),
        ),
        (
            "next",
            time(next)
                .map(|at| shows::date_text(at, tz, language))
                .unwrap_or_default(),
        ),
    ];
    values.extend(extra.iter().cloned());
    (
        PayloadShow {
            module: notice.show.module,
            name,
            link,
            end_unix_ms: end,
            next_unix_ms: next,
        },
        shows::render(template, &values),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use heron_core::shows::HourRange;

    #[test]
    fn messages_say_when_the_show_ends_and_opens_next() {
        let show = Show::of(Module::Dedications).unwrap();
        let mut settings = ShowSettings {
            hours: vec![HourRange {
                days: vec![1, 2],
                start_hour: 17,
                end_hour: 18,
            }],
            ..ShowSettings::default()
        };
        settings.notifications.language = "fr".to_owned();
        let station = StationSettings {
            timezone: "Europe/Paris".to_owned(),
            public_url: None,
            gateway_url: Some("https://public.radio.example.org".to_owned()),
        };
        let tz = station.time_zone();
        let at = |text: &str| {
            format!("{text}[Europe/Paris]")
                .parse::<jiff::Zoned>()
                .unwrap()
                .timestamp()
        };
        let template = "{name} until {end}, back {next}: {link}";
        let started = end_and_next(&settings, &tz, Change::Started, at("2026-11-09T17:00"));
        let (payload, text) = message(
            &Notice::of(show, &settings),
            &station,
            "main",
            &tz,
            started,
            template,
            &[],
        );
        assert_eq!(
            text,
            "Dedications until 18:00, back mardi 10 novembre 17:00: \
             https://public.radio.example.org/api/v1/public/stations/main/dedications.html"
        );
        assert_eq!(
            payload.end_unix_ms,
            Some(at("2026-11-09T18:00").as_millisecond())
        );
        settings.website.title = "Vos dédicaces".to_owned();
        settings.website.page_url = Some("https://radio.example.org/dedicaces".to_owned());
        let opens = at("2026-11-09T17:00");
        let upcoming = end_and_next(
            &settings,
            &tz,
            Change::Upcoming { opens },
            at("2026-11-09T16:00"),
        );
        let (_, text) = message(
            &Notice::of(show, &settings),
            &station,
            "main",
            &tz,
            upcoming,
            template,
            &[],
        );
        assert_eq!(
            text,
            "Vos dédicaces until 18:00, back lundi 9 novembre 17:00: https://radio.example.org/dedicaces"
        );
    }
}
