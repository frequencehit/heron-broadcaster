// SPDX-License-Identifier: AGPL-3.0-or-later

//! Medleys: song hooks and liners picked from a recipe ([`plan`]), then
//! mixed into one stretch of audio ([`mix`]). The API plans, the librarian
//! decodes the parts and renders the file. See `docs/medleys.md`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Sample rate of the parts handed to [`mix`] and of the rendered file.
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// Every part fades in and out over at least this, so no part clicks.
pub const EDGE_FADE_MS: u32 = 20;
/// The music goes down under a liner (and back up) over this.
pub const DUCK_RAMP_MS: u32 = 150;
/// The longest time the music takes to go down, or to come back up, under a
/// liner.
pub const MAX_DUCK_RELEASE_MS: u32 = 5_000;

/// Where the music goes down, or comes back up, around a liner over it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema), schema(as = MedleyDuckPlace))]
#[serde(rename_all = "snake_case")]
pub enum DuckPlace {
    /// Before the liner starts, or after it ends.
    Outside,
    /// Centred on the liner's start, or its end: a crossfade.
    #[default]
    Across,
    /// After the liner starts, or before it ends.
    Inside,
}

/// How the music goes down under the liners over it, and comes back up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ducking {
    pub attack_ms: u32,
    pub attack_at: DuckPlace,
    pub release_ms: u32,
    pub release_at: DuckPlace,
}

impl Ducking {
    /// How much of the going down is before the liner.
    pub fn before(&self) -> u32 {
        match self.attack_at {
            DuckPlace::Outside => self.attack_ms,
            DuckPlace::Across => self.attack_ms / 2,
            DuckPlace::Inside => 0,
        }
    }

    /// How much of the coming back up is after the liner.
    pub fn after(&self) -> u32 {
        match self.release_at {
            DuckPlace::Outside => self.release_ms,
            DuckPlace::Across => self.release_ms / 2,
            DuckPlace::Inside => 0,
        }
    }

    /// For liners from `start` to `end` (ms): where the music starts going
    /// down, is down, starts coming back up, and is back. What is inside
    /// the liners never crosses: when they are too short, the two share them.
    pub fn ramps(&self, start: u32, end: u32) -> [f64; 4] {
        let (start, end) = (f64::from(start), f64::from(end));
        let down_from = start - f64::from(self.before());
        let mut down_to = down_from + f64::from(self.attack_ms);
        let up_to = end + f64::from(self.after());
        let mut up_from = up_to - f64::from(self.release_ms);
        let inside_down = (down_to - start).max(0.0);
        let inside_up = (end - up_from).max(0.0);
        let room = (end - start).max(0.0);
        if inside_down + inside_up > room && inside_down + inside_up > 0.0 {
            let scale = room / (inside_down + inside_up);
            down_to = down_to.min(start + inside_down * scale);
            up_from = up_from.max(end - inside_up * scale);
        }
        [down_from, down_to.max(down_from), up_from.min(up_to), up_to]
    }
}
/// The mix is scaled down when its peak would go over this.
pub const PEAK_LIMIT: f32 = 0.98;

/// Folder of the rendered medleys, in the library.
pub const FOLDER: &str = "medleys";

/// Folder of the previews: hidden, in the medleys' own folder, where the
/// librarian writes already (the scanner never sees hidden folders).
pub const PREVIEW_FOLDER: &str = "medleys/.previews";

/// Where a medley's rendered file goes, relative to the library root.
pub fn medley_path(medley_id: u64) -> String {
    format!("{FOLDER}/medley-{medley_id}.wav")
}

/// Where a medley's preview goes, relative to the library root.
pub fn preview_path(medley_id: u64) -> String {
    format!("{PREVIEW_FOLDER}/medley-{medley_id}.wav")
}

/// The longest medley name, from its hooks: a track title holds 255
/// characters, and a medley named like another gets ` (2)`, ` (3)`… too.
pub const NAME_CHARS: usize = 249;

/// A medley's name: `Medley 2026-10-29 (A - B - C)`, `date` the day of its
/// render, `names` its hooks' artists (or titles) in order.
/// Each comes once (ignoring case), empty ones are left out, and the list
/// ends with `…` after the last name that fits in [`NAME_CHARS`].
pub fn name(date: &str, names: &[&str]) -> String {
    let base = format!("Medley {date}");
    let mut seen = HashSet::new();
    let names: Vec<&str> = names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty() && seen.insert(name.to_lowercase()))
        .collect();
    if names.is_empty() {
        return base;
    }
    for count in (1..=names.len()).rev() {
        let mut list = names[..count].join(" - ");
        if count < names.len() {
            list.push_str(" - …");
        }
        let name = format!("{base} ({list})");
        if name.chars().count() <= NAME_CHARS {
            return name;
        }
    }
    // A single name too long: cut it.
    let room = NAME_CHARS - base.chars().count() - " (…)".chars().count();
    let first: String = names[0].chars().take(room).collect();
    format!("{base} ({first}…)")
}

/// How a medley is built.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default, deny_unknown_fields)]
pub struct Recipe {
    /// Categories the songs come from; none: every category the medley's
    /// stations can use.
    pub category_ids: Vec<u64>,
    /// Only the songs with this label, when set.
    pub label_id: Option<u64>,
    /// How many hooks.
    pub hooks: u32,
    pub order: HookOrder,
    /// Hooks shorter than this are left out.
    pub hook_min_ms: u32,
    /// Hooks longer than this are left out too: a hook is never cut.
    pub hook_max_ms: u32,
    /// Overlap between two parts.
    pub crossfade_ms: u32,
    /// Liners between or over the hooks, from the categories of the medley
    /// settings; off for none.
    pub liners: bool,
    /// A liner after every this many hooks.
    pub liner_every: u32,
    pub liner_at_start: bool,
    pub liner_at_end: bool,
    pub liner_mode: LinerMode,
    /// How much the music goes down under a liner played over it.
    pub duck_db: f64,
    /// How long the music takes to go down under a liner over it, and
    /// where.
    pub duck_attack_ms: u32,
    pub duck_attack_at: DuckPlace,
    /// How long the music takes to come back up, and where.
    pub duck_release_ms: u32,
    pub duck_release_at: DuckPlace,
    /// How long the music fades in at the start; 0 for no fade-in.
    pub fade_in_ms: u32,
    pub fade_in_curve: FadeCurve,
    /// How long the music fades out at the end; 0 for no fade-out.
    pub fade_out_ms: u32,
    pub fade_out_curve: FadeCurve,
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            category_ids: Vec::new(),
            label_id: None,
            hooks: 3,
            order: HookOrder::Picked,
            hook_min_ms: 4_000,
            hook_max_ms: 8_000,
            crossfade_ms: 500,
            liners: true,
            liner_every: 1,
            liner_at_start: true,
            liner_at_end: true,
            liner_mode: LinerMode::Between,
            duck_db: -10.0,
            duck_attack_ms: 300,
            duck_attack_at: DuckPlace::Inside,
            duck_release_ms: 300,
            duck_release_at: DuckPlace::Across,
            fade_in_ms: 0,
            fade_in_curve: FadeCurve::Linear,
            fade_out_ms: 0,
            fade_out_curve: FadeCurve::Linear,
        }
    }
}

impl Recipe {
    /// How the music goes down under its liners over it.
    pub fn ducking(&self) -> Ducking {
        Ducking {
            attack_ms: self.duck_attack_ms,
            attack_at: self.duck_attack_at,
            release_ms: self.duck_release_ms,
            release_at: self.duck_release_at,
        }
    }

    /// Why the recipe cannot be built, if it cannot. No categories means
    /// every category the medley's stations can use.
    pub fn check(&self) -> Result<(), String> {
        self.check_settings()
    }

    /// [`Recipe::check`] but for the songs: what the defaults of a new
    /// medley must pass.
    pub fn check_settings(&self) -> Result<(), String> {
        if !(2..=30).contains(&self.hooks) {
            return Err("a medley has 2 to 30 hooks".to_owned());
        }
        if self.hook_min_ms < 1_000
            || self.hook_min_ms > self.hook_max_ms
            || self.hook_max_ms > 60_000
        {
            return Err("hooks last 1 to 60 s, the minimum at most the maximum".to_owned());
        }
        if self.crossfade_ms > 5_000 || self.crossfade_ms >= self.hook_min_ms {
            return Err("the crossfade is at most 5 s, and shorter than the hooks".to_owned());
        }
        if self.liner_every == 0 {
            return Err("liners come every 1 hook or more".to_owned());
        }
        if !(-40.0..=0.0).contains(&self.duck_db) {
            return Err("ducking is between -40 and 0 dB".to_owned());
        }
        if self.duck_attack_ms > MAX_DUCK_RELEASE_MS || self.duck_release_ms > MAX_DUCK_RELEASE_MS {
            return Err(format!(
                "the music goes down and comes back up over at most {} s",
                MAX_DUCK_RELEASE_MS / 1_000
            ));
        }
        if self.fade_in_ms > MAX_FADE_OUT_MS || self.fade_out_ms > MAX_FADE_OUT_MS {
            return Err(format!(
                "the fade-in and the fade-out last at most {} s",
                MAX_FADE_OUT_MS / 1_000
            ));
        }
        Ok(())
    }
}

/// The longest fade-in, or fade-out, of a medley.
pub const MAX_FADE_OUT_MS: u32 = 30_000;

/// How the music fades in at the start of a medley, or out at its end.
/// For a fade-in, "fast then slow" comes up quickly at first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
// Named apart from the playout's own fade curves in the OpenAPI document.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema), schema(as = MedleyFadeCurve))]
#[serde(rename_all = "snake_case")]
pub enum FadeCurve {
    #[default]
    Linear,
    /// Down quickly, then slowly.
    FastThenSlow,
    /// Down slowly, then quickly.
    SlowThenFast,
    /// Slowly, quickly, slowly.
    SCurve,
}

impl FadeCurve {
    /// The gain `t` of the way through the fade (0 to 1): from 1 down to 0.
    pub fn gain(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => 1.0 - t,
            Self::FastThenSlow => (1.0 - t) * (1.0 - t),
            Self::SlowThenFast => 1.0 - t * t,
            Self::SCurve => 0.5 * (1.0 + (std::f32::consts::PI * t).cos()),
        }
    }

    /// The gain `t` of the way through a fade-in: from 0 up to 1.
    pub fn rise(self, t: f32) -> f32 {
        1.0 - self.gain(t)
    }
}

/// The order of the hooks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum HookOrder {
    /// As picked (at random).
    #[default]
    Picked,
    /// Oldest first.
    Year,
    /// Slowest first.
    Bpm,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LinerMode {
    /// Liners take their own time between the hooks.
    #[default]
    Between,
    /// Liners start on the transition, the music lowered under them.
    Over,
}

/// Where a liner goes in a medley: its liners come from a category of
/// their own at the start and at the end, when the settings have one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LinerSlot {
    Start,
    #[default]
    Between,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    Hook,
    Liner,
}

/// A span of a track's file in the medley.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Part {
    pub kind: PartKind,
    pub track_id: u64,
    /// On the file's timeline.
    pub from_ms: u32,
    pub to_ms: u32,
    /// A liner over the music: where in its gap the music passes from the
    /// hook before to the hook after, from the middle; positive for more of
    /// the liner over the hook before. See [`layout`].
    #[serde(default)]
    pub shift_ms: i32,
    /// How far the part was moved in the medley from where the recipe puts
    /// it, negative for earlier. See [`layout`].
    #[serde(default)]
    pub move_ms: i32,
}

impl Part {
    pub fn length_ms(&self) -> u32 {
        self.to_ms.saturating_sub(self.from_ms)
    }
}

/// A track that can be part of a medley: a song with its hook, or a liner
/// with its cue points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub track_id: u64,
    pub from_ms: u32,
    pub to_ms: u32,
    pub year: Option<u16>,
    pub bpm: Option<f64>,
}

/// The liners a medley can take, by where they go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Liners {
    pub between: Vec<Candidate>,
    /// For the liner at the start: `between` when empty.
    pub start: Vec<Candidate>,
    /// For the liner at the end: `between` when empty.
    pub end: Vec<Candidate>,
}

impl Liners {
    /// The liners that can go in `slot`.
    pub fn at(&self, slot: LinerSlot) -> &[Candidate] {
        match slot {
            LinerSlot::Start if !self.start.is_empty() => &self.start,
            LinerSlot::End if !self.end.is_empty() => &self.end,
            _ => &self.between,
        }
    }
}

/// A small random generator (xorshift): picks need no cryptographic randomness.
pub struct Picker(u64);

impl Picker {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

/// Whether a song's hook lasts between the recipe's shortest and longest.
fn fits(recipe: &Recipe, song: &Candidate) -> bool {
    (recipe.hook_min_ms..=recipe.hook_max_ms).contains(&song.to_ms.saturating_sub(song.from_ms))
}

/// The songs whose hook fits, shuffled, those not in `avoid` (the medley's
/// current version) first.
fn usable_songs(
    recipe: &Recipe,
    songs: &[Candidate],
    avoid: &HashSet<u64>,
    picker: &mut Picker,
) -> Vec<Candidate> {
    let mut usable: Vec<Candidate> = songs
        .iter()
        .filter(|song| fits(recipe, song))
        .copied()
        .collect();
    picker.shuffle(&mut usable);
    // Stable: the shuffle holds within each group.
    usable.sort_by_key(|song| avoid.contains(&song.track_id));
    usable
}

/// A song's hook, whole.
fn hook(song: &Candidate) -> Part {
    Part {
        kind: PartKind::Hook,
        track_id: song.track_id,
        from_ms: song.from_ms,
        to_ms: song.to_ms,
        shift_ms: 0,
        move_ms: 0,
    }
}

fn liner(candidate: &Candidate) -> Part {
    Part {
        kind: PartKind::Liner,
        track_id: candidate.track_id,
        from_ms: candidate.from_ms,
        to_ms: candidate.to_ms,
        shift_ms: 0,
        move_ms: 0,
    }
}

/// How many songs have a hook that fits.
pub fn qualified(recipe: &Recipe, songs: &[Candidate]) -> usize {
    songs.iter().filter(|song| fits(recipe, song)).count()
}

/// Picks the parts of a medley: `recipe.hooks` hooks (fewer when not
/// enough songs qualify), never the same song twice, songs not in `avoid`
/// first; liners where the recipe puts them (from their own category at the
/// start and at the end, when there is one), never the same twice in a row.
pub fn plan(
    recipe: &Recipe,
    songs: &[Candidate],
    liners: &Liners,
    avoid: &HashSet<u64>,
    seed: u64,
) -> Vec<Part> {
    let mut picker = Picker::new(seed);
    let mut picked: Vec<Candidate> = usable_songs(recipe, songs, avoid, &mut picker)
        .into_iter()
        .take(recipe.hooks as usize)
        .collect();
    match recipe.order {
        HookOrder::Picked => {}
        // Unknown values last.
        HookOrder::Year => picked.sort_by_key(|song| song.year.unwrap_or(u16::MAX)),
        HookOrder::Bpm => picked.sort_by(|a, b| {
            a.bpm
                .unwrap_or(f64::MAX)
                .total_cmp(&b.bpm.unwrap_or(f64::MAX))
        }),
    }

    // Each slot's liners, shuffled, and where the next pick is in them.
    let mut pools: Vec<(LinerSlot, Vec<Candidate>, usize)> =
        [LinerSlot::Start, LinerSlot::Between, LinerSlot::End]
            .into_iter()
            .map(|slot| {
                let mut pool: Vec<Candidate> = if recipe.liners {
                    liners
                        .at(slot)
                        .iter()
                        .filter(|l| l.to_ms > l.from_ms)
                        .copied()
                        .collect()
                } else {
                    Vec::new()
                };
                picker.shuffle(&mut pool);
                (slot, pool, 0)
            })
            .collect();
    let mut previous: Option<u64> = None;
    let mut take_liner = |parts: &mut Vec<Part>, slot: LinerSlot| {
        let Some((_, pool, next)) = pools.iter_mut().find(|(s, _, _)| *s == slot) else {
            return;
        };
        if pool.is_empty() {
            return;
        }
        let mut candidate = pool[*next % pool.len()];
        *next += 1;
        if previous == Some(candidate.track_id) && pool.len() > 1 {
            candidate = pool[*next % pool.len()];
            *next += 1;
        }
        previous = Some(candidate.track_id);
        parts.push(liner(&candidate));
    };

    let mut parts = Vec::new();
    if recipe.liner_at_start && !picked.is_empty() {
        take_liner(&mut parts, LinerSlot::Start);
    }
    let count = picked.len();
    for (index, song) in picked.iter().enumerate() {
        parts.push(hook(song));
        let done = index + 1;
        if done < count && done % recipe.liner_every as usize == 0 {
            take_liner(&mut parts, LinerSlot::Between);
        }
    }
    if recipe.liner_at_end && count > 0 {
        take_liner(&mut parts, LinerSlot::End);
    }
    parts
}

/// Every part of `kind` the candidates can give, in their order: the hooks
/// that fit the recipe, liners whole.
pub fn choices(recipe: &Recipe, kind: PartKind, candidates: &[Candidate]) -> Vec<Part> {
    candidates
        .iter()
        .filter(|candidate| match kind {
            PartKind::Hook => fits(recipe, candidate),
            PartKind::Liner => candidate.to_ms > candidate.from_ms,
        })
        .map(|candidate| match kind {
            PartKind::Hook => hook(candidate),
            PartKind::Liner => liner(candidate),
        })
        .collect()
}

/// Another part of `kind`, none of `exclude`: to replace one in a plan.
pub fn pick_one(
    recipe: &Recipe,
    kind: PartKind,
    candidates: &[Candidate],
    exclude: &HashSet<u64>,
    avoid: &HashSet<u64>,
    seed: u64,
) -> Option<Part> {
    let mut picker = Picker::new(seed);
    match kind {
        PartKind::Hook => usable_songs(recipe, candidates, avoid, &mut picker)
            .into_iter()
            .find(|song| !exclude.contains(&song.track_id))
            .map(|song| hook(&song)),
        PartKind::Liner => {
            let mut liners: Vec<Candidate> = candidates
                .iter()
                .filter(|l| l.to_ms > l.from_ms && !exclude.contains(&l.track_id))
                .copied()
                .collect();
            picker.shuffle(&mut liners);
            liners.first().map(liner)
        }
    }
}

/// The audio of a part: interleaved stereo at [`SAMPLE_RATE`], and the gain
/// bringing it to the station's loudness target.
pub struct PartAudio {
    pub kind: PartKind,
    pub samples: Vec<f32>,
    pub gain: f32,
}

pub fn ms_to_frames(ms: u32) -> usize {
    (u64::from(ms) * u64::from(SAMPLE_RATE) / 1_000) as usize
}

/// Adds `part` to `out` from frame `at`, fading in over `fade_in` frames
/// and out over `fade_out`, times `gain`.
fn add(out: &mut Vec<f32>, at: usize, part: &PartAudio, fade_in: usize, fade_out: usize) {
    let frames = part.samples.len() / CHANNELS;
    let end = at + frames;
    if out.len() < end * CHANNELS {
        out.resize(end * CHANNELS, 0.0);
    }
    let fade_in = fade_in.min(frames / 2).max(1);
    let fade_out = fade_out.min(frames / 2).max(1);
    for frame in 0..frames {
        let mut gain = part.gain;
        if frame < fade_in {
            gain *= frame as f32 / fade_in as f32;
        }
        if frame + fade_out > frames {
            gain *= (frames - frame) as f32 / fade_out as f32;
        }
        for channel in 0..CHANNELS {
            out[(at + frame) * CHANNELS + channel] +=
                part.samples[frame * CHANNELS + channel] * gain;
        }
    }
}

/// Where a part goes in the medley, in ms on the medley's timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Placed {
    pub kind: PartKind,
    /// Where it starts in the medley.
    pub at_ms: u32,
    /// The span of its file played: the part's own, with the extensions
    /// under liners over the music.
    pub from_ms: u32,
    pub to_ms: u32,
    pub fade_in_ms: u32,
    pub fade_out_ms: u32,
    /// A liner over the music: the music is lowered while it plays.
    pub over: bool,
}

impl Placed {
    pub fn end_ms(&self) -> u32 {
        self.at_ms + self.to_ms.saturating_sub(self.from_ms)
    }
}

/// The medley as it will sound: each part where it goes, in the parts'
/// order. Saved with a medley when it is rendered, so its timeline is the
/// file's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Layout {
    pub parts: Vec<Placed>,
    /// How long the medley lasts.
    pub length_ms: u32,
    /// Where the music (the hooks) starts: the fade-in starts there.
    #[serde(default)]
    pub music_start_ms: u32,
    /// Where the music (the hooks) ends: the fade-out ends there.
    pub music_end_ms: u32,
    /// The recipe's fade-in.
    #[serde(default)]
    pub fade_in_ms: u32,
    #[serde(default)]
    pub fade_in_curve: FadeCurve,
    /// The recipe's fade-out.
    #[serde(default)]
    pub fade_out_ms: u32,
    #[serde(default)]
    pub fade_out_curve: FadeCurve,
    /// How much the music goes down under a liner over it.
    #[serde(default)]
    pub duck_db: f64,
    /// How the music went down and came back up; a layout saved before
    /// these were set went down before the liners over 150 ms, and came
    /// back up during them.
    #[serde(default = "old_attack_ms")]
    pub duck_attack_ms: u32,
    #[serde(default = "old_attack_at")]
    pub duck_attack_at: DuckPlace,
    #[serde(default)]
    pub duck_release_ms: u32,
    #[serde(default = "old_release_at")]
    pub duck_release_at: DuckPlace,
}

fn old_attack_ms() -> u32 {
    DUCK_RAMP_MS
}

fn old_attack_at() -> DuckPlace {
    DuckPlace::Outside
}

fn old_release_at() -> DuckPlace {
    DuckPlace::Inside
}

impl Layout {
    /// How the music went down and came back up.
    pub fn ducking(&self) -> Ducking {
        Ducking {
            attack_ms: self.duck_attack_ms,
            attack_at: self.duck_attack_at,
            release_ms: self.duck_release_ms,
            release_at: self.duck_release_at,
        }
    }
}

/// Lays the parts out on the medley's timeline: consecutive parts overlap
/// by the crossfade. With liners over the music, the hooks around each
/// group of liners play longer so the liners fall in the gap and the
/// hooks' own spans are heard whole: between two hooks the gap lasts the
/// liners, the crossfade and the two ducking ramps, shared between the two
/// hooks as the first liner's `shift_ms` says (half each at 0, more of it
/// at the end of the hook before when positive); before the first hook or
/// after the last, that hook alone plays longer. An extension stops at the
/// file's start, or its end (`lengths`, each part's file length when known),
/// the other hook taking what is missing.
///
/// A part's `move_ms` then moves it in the medley: a part played after
/// another overlaps it by the crossfade minus `move_ms`, from 0 (back to
/// back) up to the shorter of the two, the parts after it following; a
/// liner over the music moves over the music.
pub fn layout(recipe: &Recipe, parts: &[Part], lengths: &[Option<u32>]) -> Layout {
    let crossfade = recipe.crossfade_ms;
    let over = recipe.liner_mode == LinerMode::Over;
    let is_over = |part: &Part| over && part.kind == PartKind::Liner;
    // The parts played one after another, and the liners over them by gap:
    // gap k is before the k-th of them (the last one after them all).
    let sequence: Vec<usize> = (0..parts.len()).filter(|&i| !is_over(&parts[i])).collect();
    let mut gaps: Vec<Vec<usize>> = vec![Vec::new(); sequence.len() + 1];
    let mut seen = 0;
    for (index, part) in parts.iter().enumerate() {
        if is_over(part) {
            gaps[seen].push(index);
        } else {
            seen += 1;
        }
    }

    // How much earlier each part of the sequence starts, and later it ends.
    let mut earlier = vec![0u32; parts.len()];
    let mut later = vec![0u32; parts.len()];
    let room_after = |index: usize| {
        lengths
            .get(index)
            .copied()
            .flatten()
            .map_or(u32::MAX, |length| length.saturating_sub(parts[index].to_ms))
    };
    let count = sequence.len();
    // What the going down takes before the liners, and the coming back up
    // after them: the hooks' spans stay clear of both.
    let ducking = recipe.ducking();
    let (pre, post) = (ducking.before(), ducking.after());
    for (gap, liners) in gaps.iter().enumerate() {
        if liners.is_empty() || count == 0 {
            continue;
        }
        let voice: u32 = liners.iter().map(|&i| parts[i].length_ms()).sum();
        if gap == 0 {
            let first = sequence[0];
            // The span starts once the music is back up.
            earlier[first] = (voice + post).min(parts[first].from_ms);
        } else if gap == count {
            let last = sequence[count - 1];
            later[last] = (voice + pre).min(room_after(last));
        } else {
            let (before, after) = (sequence[gap - 1], sequence[gap]);
            let total = voice + crossfade + pre + post;
            let shift = i64::from(parts[liners[0]].shift_ms);
            // Centred on the liners, whatever the ducking takes before and after them.
            let mut on = ((i64::from(total) + i64::from(pre) - i64::from(post)) / 2 + shift)
                .clamp(0, i64::from(total)) as u32;
            let mut ahead = total - on;
            let (room_on, room_ahead) = (room_after(before), parts[after].from_ms);
            if on > room_on {
                ahead += on - room_on;
                on = room_on;
            }
            if ahead > room_ahead {
                on = (on + (ahead - room_ahead)).min(room_on);
                ahead = room_ahead;
            }
            later[before] = on;
            earlier[after] = ahead;
        }
    }

    let mut placed: Vec<Option<Placed>> = vec![None; parts.len()];
    let mut cursor = 0u32;
    let mut previous: Option<usize> = None;
    for &index in &sequence {
        let part = &parts[index];
        let from_ms = part.from_ms - earlier[index];
        let to_ms = part.to_ms + later[index];
        // The medley's own start and end only fade over the edge.
        let mut item = Placed {
            kind: part.kind,
            at_ms: 0,
            from_ms,
            to_ms,
            fade_in_ms: EDGE_FADE_MS,
            fade_out_ms: EDGE_FADE_MS,
            over: false,
        };
        if let Some(before) = previous.and_then(|before| placed[before].as_mut()) {
            let longest = (to_ms - from_ms).min(before.to_ms - before.from_ms);
            let overlap = (i64::from(crossfade) - i64::from(part.move_ms))
                .clamp(0, i64::from(longest)) as u32;
            item.at_ms = cursor - overlap;
            let blend = overlap.max(EDGE_FADE_MS);
            item.fade_in_ms = blend;
            before.fade_out_ms = blend;
        }
        cursor = item.end_ms();
        placed[index] = Some(item);
        previous = Some(index);
    }
    // Each group of liners starts as the music before it is lowered: after
    // the span of the hook before (and its ramp), at 0 before the first.
    for (gap, liners) in gaps.iter().enumerate() {
        let mut at = if gap == 0 {
            0
        } else {
            let index = sequence[gap - 1];
            placed[index].map_or(0, |hook| {
                hook.at_ms + (parts[index].to_ms - hook.from_ms) + pre
            })
        };
        for &index in liners {
            let part = &parts[index];
            let item = Placed {
                kind: part.kind,
                at_ms: (i64::from(at) + i64::from(part.move_ms)).max(0) as u32,
                from_ms: part.from_ms,
                to_ms: part.to_ms,
                fade_in_ms: EDGE_FADE_MS,
                fade_out_ms: EDGE_FADE_MS,
                over: true,
            };
            at += part.length_ms();
            placed[index] = Some(item);
        }
    }
    let parts: Vec<Placed> = placed.into_iter().flatten().collect();
    Layout {
        length_ms: parts.iter().map(Placed::end_ms).max().unwrap_or(0),
        music_end_ms: parts
            .iter()
            .filter(|part| part.kind == PartKind::Hook)
            .map(Placed::end_ms)
            .max()
            .unwrap_or(0),
        music_start_ms: parts
            .iter()
            .filter(|part| part.kind == PartKind::Hook)
            .map(|part| part.at_ms)
            .min()
            .unwrap_or(0),
        fade_in_ms: recipe.fade_in_ms,
        fade_in_curve: recipe.fade_in_curve,
        fade_out_ms: recipe.fade_out_ms,
        fade_out_curve: recipe.fade_out_curve,
        duck_db: recipe.duck_db,
        duck_attack_ms: recipe.duck_attack_ms,
        duck_attack_at: recipe.duck_attack_at,
        duck_release_ms: recipe.duck_release_ms,
        duck_release_at: recipe.duck_release_at,
        parts,
    }
}

/// The shortest crossfade between the two spans of a shortened medley.
pub const MIN_SHORTENED_CROSSFADE_MS: u32 = 100;

/// A medley played shorter, from its rendered file: its last liner + hook
/// pairs left out, the closing liner kept (`docs/medleys.md#shortened-plays`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortened {
    pub hooks_left_out: u32,
    /// The parts left out, by their position in the medley from 0.
    pub parts_left_out: Vec<u32>,
    /// The first span ends here (ms in the file): where the first part left
    /// out starts.
    pub first_to_ms: u32,
    /// The second span starts here: the crossfade before the closing liner
    /// (and the music's going down before it, over the music).
    pub second_from_ms: u32,
    /// How long it plays from `cue_in_ms` to `cue_out_ms`: both spans, less
    /// the crossfade between them.
    pub length_ms: u32,
    pub crossfade_ms: u32,
}

/// The ways of playing a rendered medley shorter, from its rendered layout,
/// the fewest hooks left out first: never below 2 hooks, and only with a
/// closing liner (a liner after the last hook). `cue_in_ms` and
/// `cue_out_ms` are the track's, on the file's timeline.
pub fn shortened(
    layout: &Layout,
    crossfade_ms: u32,
    cue_in_ms: u32,
    cue_out_ms: u32,
) -> Vec<Shortened> {
    let parts = &layout.parts;
    let hooks: Vec<usize> = (0..parts.len())
        .filter(|&i| parts[i].kind == PartKind::Hook)
        .collect();
    let (Some(&last_hook), true) = (hooks.last(), hooks.len() >= 3) else {
        return Vec::new();
    };
    let closing = last_hook + 1;
    if closing >= parts.len() {
        return Vec::new();
    }
    let crossfade = crossfade_ms.max(MIN_SHORTENED_CROSSFADE_MS);
    let closing_at = parts[closing..]
        .iter()
        .map(|part| part.at_ms)
        .min()
        .unwrap_or(0);
    let pre = if parts[closing].over {
        layout.ducking().before()
    } else {
        0
    };
    let last = &parts[last_hook];
    let second_from_ms = closing_at
        .saturating_sub(crossfade + pre)
        .max(last.at_ms + last.fade_in_ms);
    if second_from_ms + crossfade >= cue_out_ms {
        return Vec::new();
    }
    let mut cuts = Vec::new();
    // Keeping hooks 1 to k, k from the last but one down to 2.
    for kept in (2..hooks.len()).rev() {
        let left_out: Vec<usize> = (hooks[kept - 1] + 1..closing).collect();
        let Some(first_to_ms) = left_out.iter().map(|&i| parts[i].at_ms).min() else {
            continue;
        };
        if first_to_ms <= cue_in_ms + crossfade || first_to_ms > second_from_ms {
            continue;
        }
        let length_ms = (first_to_ms - cue_in_ms) + (cue_out_ms - second_from_ms) - crossfade;
        cuts.push(Shortened {
            hooks_left_out: u32::try_from(hooks.len() - kept).unwrap_or(u32::MAX),
            parts_left_out: left_out
                .iter()
                .map(|&i| u32::try_from(i).unwrap_or(u32::MAX))
                .collect(),
            first_to_ms,
            second_from_ms,
            length_ms,
            crossfade_ms: crossfade,
        });
    }
    cuts
}

/// When the music is lowered: the liners over it, those in a row (their
/// ramps would meet) as one, from the first's start to the last's end.
pub fn duck_windows(layout: &Layout) -> Vec<(u32, u32)> {
    let ducking = layout.ducking();
    // A gap shorter than going down and back up takes: the music stays down.
    let reach = ducking.attack_ms + ducking.release_ms;
    let mut liners: Vec<(u32, u32)> = layout
        .parts
        .iter()
        .filter(|placed| placed.over)
        .map(|placed| (placed.at_ms, placed.end_ms()))
        .collect();
    liners.sort_unstable();
    let mut windows: Vec<(u32, u32)> = Vec::new();
    for (start, end) in liners {
        match windows.last_mut() {
            Some(last) if start < last.1 + reach => last.1 = last.1.max(end),
            _ => windows.push((start, end)),
        }
    }
    windows
}

/// Mixes the parts where `layout` puts them into one interleaved stereo
/// stretch, `audio[i]` the span `layout.parts[i]` reads. The music (the
/// hooks) is lowered under the liners over it and fades in and out as the
/// recipe says; the liners are never faded. The result is scaled down if it
/// would clip.
pub fn mix(recipe: &Recipe, layout: &Layout, audio: &[PartAudio]) -> Vec<f32> {
    let total = ms_to_frames(layout.length_ms);
    let mut music = vec![0.0f32; total * CHANNELS];
    let mut voice = vec![0.0f32; total * CHANNELS];
    for (placed, part) in layout.parts.iter().zip(audio) {
        let out = if part.kind == PartKind::Hook {
            &mut music
        } else {
            &mut voice
        };
        add(
            out,
            ms_to_frames(placed.at_ms),
            part,
            ms_to_frames(placed.fade_in_ms),
            ms_to_frames(placed.fade_out_ms),
        );
    }
    let total = music.len().max(voice.len()) / CHANNELS;
    music.resize(total * CHANNELS, 0.0);
    voice.resize(total * CHANNELS, 0.0);

    let mut envelope = vec![1.0f32; total];
    let duck = 10f32.powf(recipe.duck_db as f32 / 20.0);
    let ducking = layout.ducking();
    let frames_of = |ms: f64| (ms.max(0.0) * f64::from(SAMPLE_RATE) / 1_000.0) as usize;
    for (start_ms, end_ms) in duck_windows(layout) {
        let [down_from, down_to, up_from, up_to] = ducking.ramps(start_ms, end_ms);
        let (down_from, down_to) = (frames_of(down_from), frames_of(down_to));
        let (up_from, up_to) = (frames_of(up_from), frames_of(up_to).min(total));
        for (frame, level) in envelope.iter_mut().enumerate().take(up_to).skip(down_from) {
            let gain = if frame < down_to {
                let t = (frame - down_from) as f32 / (down_to - down_from).max(1) as f32;
                1.0 + (duck - 1.0) * t
            } else if frame >= up_from {
                let t = (frame - up_from) as f32 / (up_to - up_from).max(1) as f32;
                duck + (1.0 - duck) * t
            } else {
                duck
            };
            *level = level.min(gain);
        }
    }
    if recipe.fade_in_ms > 0 {
        let start = ms_to_frames(layout.music_start_ms).min(total);
        let length = ms_to_frames(recipe.fade_in_ms).max(1);
        for (frame, level) in envelope.iter_mut().enumerate().skip(start).take(length) {
            let t = (frame - start) as f32 / length as f32;
            *level *= recipe.fade_in_curve.rise(t);
        }
    }
    if recipe.fade_out_ms > 0 {
        let end = ms_to_frames(layout.music_end_ms).min(total);
        let length = ms_to_frames(recipe.fade_out_ms).min(end).max(1);
        let start = end - length;
        for (frame, level) in envelope.iter_mut().enumerate().skip(start) {
            let t = ((frame - start) as f32 / length as f32).min(1.0);
            *level *= recipe.fade_out_curve.gain(t);
        }
    }
    let mut out = voice;
    for (frame, level) in envelope.iter().enumerate() {
        for channel in 0..CHANNELS {
            out[frame * CHANNELS + channel] += music[frame * CHANNELS + channel] * level;
        }
    }

    let peak = out
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    if peak > PEAK_LIMIT {
        let scale = PEAK_LIMIT / peak;
        for sample in &mut out {
            *sample *= scale;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(track_id: u64, from_ms: u32, to_ms: u32) -> Candidate {
        Candidate {
            track_id,
            from_ms,
            to_ms,
            year: None,
            bpm: None,
        }
    }

    fn recipe() -> Recipe {
        Recipe {
            category_ids: vec![1],
            ..Recipe::default()
        }
    }

    /// Going down before the liners over 150 ms, coming back up during them:
    /// how medleys were mixed before the placement could be chosen.
    fn before_and_during() -> Recipe {
        Recipe {
            duck_attack_ms: 150,
            duck_attack_at: DuckPlace::Outside,
            duck_release_at: DuckPlace::Inside,
            ..recipe()
        }
    }

    #[test]
    fn recipes_are_checked() {
        assert!(recipe().check().is_ok());
        // No category: every category of the medley's stations.
        assert!(Recipe::default().check().is_ok());
        for wrong in [
            Recipe {
                hooks: 1,
                ..recipe()
            },
            Recipe {
                hook_min_ms: 9_000,
                ..recipe()
            },
            Recipe {
                crossfade_ms: 4_000,
                ..recipe()
            },
            Recipe {
                liner_every: 0,
                ..recipe()
            },
            Recipe {
                duck_db: 3.0,
                ..recipe()
            },
        ] {
            assert!(wrong.check().is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn hooks_fit_are_never_repeated_and_new_songs_come_first() {
        // 10's hook is too short (2 s), 11's too long (12 s): left out, never cut.
        let songs: Vec<Candidate> = (1..=11)
            .map(|id| {
                song(
                    id,
                    1_000,
                    match id {
                        10 => 3_000,
                        11 => 13_000,
                        _ => 7_000,
                    },
                )
            })
            .collect();
        let avoid: HashSet<u64> = [1, 2, 3, 4].into();
        let recipe = Recipe {
            hooks: 5,
            ..recipe()
        };
        let parts = plan(&recipe, &songs, &Liners::default(), &avoid, 42);
        assert_eq!(parts.len(), 5);
        let ids: HashSet<u64> = parts.iter().map(|part| part.track_id).collect();
        assert_eq!(ids.len(), 5, "never the same song twice");
        // 5, 6, 7, 8, 9 qualify and were not in the last version.
        assert_eq!(ids, [5, 6, 7, 8, 9].into());
        assert!(
            parts
                .iter()
                .all(|part| part.length_ms() == 6_000 && part.from_ms == 1_000)
        );
        assert_eq!(qualified(&recipe, &songs), 9);
        // Only long hooks: nothing qualifies, nothing is cut.
        let long = [song(20, 0, 30_000)];
        assert!(plan(&recipe, &long, &Liners::default(), &HashSet::new(), 1).is_empty());
    }

    #[test]
    fn hooks_follow_the_order_asked() {
        let songs = [
            Candidate {
                year: Some(1999),
                ..song(1, 0, 5_000)
            },
            Candidate {
                year: None,
                ..song(2, 0, 5_000)
            },
            Candidate {
                year: Some(1984),
                ..song(3, 0, 5_000)
            },
        ];
        let recipe = Recipe {
            hooks: 3,
            order: HookOrder::Year,
            ..recipe()
        };
        let ids: Vec<u64> = plan(&recipe, &songs, &Liners::default(), &HashSet::new(), 7)
            .iter()
            .map(|part| part.track_id)
            .collect();
        assert_eq!(ids, [3, 1, 2]);
    }

    #[test]
    fn liners_go_where_the_recipe_says_never_twice_in_a_row() {
        let songs: Vec<Candidate> = (1..=6).map(|id| song(id, 0, 6_000)).collect();
        let liners = [song(100, 0, 2_000), song(101, 0, 2_000)];
        let liners = Liners {
            between: liners.to_vec(),
            ..Liners::default()
        };
        let recipe = Recipe {
            hooks: 6,
            liner_every: 2,
            liner_at_start: true,
            liner_at_end: true,
            ..recipe()
        };
        for seed in 1..20 {
            let parts = plan(&recipe, &songs, &liners, &HashSet::new(), seed);
            let kinds: String = parts
                .iter()
                .map(|part| {
                    if part.kind == PartKind::Hook {
                        'H'
                    } else {
                        'L'
                    }
                })
                .collect();
            assert_eq!(kinds, "LHHLHHLHHL");
            let liners: Vec<u64> = parts
                .iter()
                .filter(|part| part.kind == PartKind::Liner)
                .map(|part| part.track_id)
                .collect();
            assert!(
                liners.windows(2).all(|pair| pair[0] != pair[1]),
                "{liners:?}"
            );
        }
    }

    #[test]
    fn opening_and_closing_liners_come_from_their_own_category() {
        let songs: Vec<Candidate> = (1..=3).map(|id| song(id, 0, 6_000)).collect();
        let liners = Liners {
            between: vec![song(100, 0, 2_000), song(101, 0, 2_000)],
            start: vec![song(200, 0, 2_000)],
            end: vec![song(300, 0, 2_000)],
        };
        let recipe = recipe();
        for seed in 1..10 {
            let parts = plan(&recipe, &songs, &liners, &HashSet::new(), seed);
            let ids: Vec<u64> = parts.iter().map(|part| part.track_id).collect();
            assert_eq!(parts.len(), 7, "{ids:?}");
            assert_eq!(ids[0], 200);
            assert_eq!(ids[6], 300);
            assert!([100, 101].contains(&ids[2]) && [100, 101].contains(&ids[4]));
        }
        // Without their own, the start and the end take the others.
        let shared = Liners {
            between: vec![song(100, 0, 2_000)],
            ..Liners::default()
        };
        let parts = plan(&recipe, &songs, &shared, &HashSet::new(), 3);
        assert_eq!(parts.first().map(|part| part.track_id), Some(100));
        assert_eq!(parts.last().map(|part| part.track_id), Some(100));
        // Liners off: hooks only.
        let off = Recipe {
            liners: false,
            ..recipe
        };
        let parts = plan(&off, &songs, &liners, &HashSet::new(), 3);
        assert!(parts.iter().all(|part| part.kind == PartKind::Hook));
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn names_list_the_hooks_once_and_fit_a_title() {
        assert_eq!(
            name("2026-10-29", &["Helena", "Panic! At The Disco", "Mabel"]),
            "Medley 2026-10-29 (Helena - Panic! At The Disco - Mabel)"
        );
        assert_eq!(
            name("2026-10-29", &["Mabel", " ", "mabel", "Helena"]),
            "Medley 2026-10-29 (Mabel - Helena)"
        );
        assert_eq!(name("2026-11-02", &[]), "Medley 2026-11-02");
        let many: Vec<String> = (1..=30).map(|i| format!("Artist number {i:02}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let long = name("2026-10-29", &many);
        assert!(long.chars().count() <= NAME_CHARS, "{long}");
        assert!(long.ends_with(" - …)"), "{long}");
        let huge = "x".repeat(400);
        let cut = name("2026-10-29", &[&huge]);
        assert_eq!(cut.chars().count(), NAME_CHARS);
        assert!(cut.ends_with("x…)"));
    }

    #[test]
    fn choices_are_the_hooks_that_fit() {
        let songs = [song(1, 0, 6_500), song(2, 0, 3_000), song(3, 0, 20_000)];
        let parts = choices(&recipe(), PartKind::Hook, &songs);
        assert_eq!(parts.len(), 1, "too short and too long left out");
        assert_eq!((parts[0].track_id, parts[0].to_ms), (1, 6_500));
        let liners = choices(&recipe(), PartKind::Liner, &[song(3, 100, 2_000)]);
        assert_eq!((liners[0].kind, liners[0].to_ms), (PartKind::Liner, 2_000));
    }

    #[test]
    fn a_replacement_is_none_of_the_excluded() {
        let songs: Vec<Candidate> = (1..=3).map(|id| song(id, 0, 5_000)).collect();
        let exclude: HashSet<u64> = [1, 2].into();
        let part = pick_one(
            &recipe(),
            PartKind::Hook,
            &songs,
            &exclude,
            &HashSet::new(),
            3,
        );
        assert_eq!(part.map(|part| part.track_id), Some(3));
        let all: HashSet<u64> = [1, 2, 3].into();
        assert_eq!(
            pick_one(&recipe(), PartKind::Hook, &songs, &all, &HashSet::new(), 3),
            None
        );
    }

    fn constant(kind: PartKind, frames: usize, value: f32) -> PartAudio {
        PartAudio {
            kind,
            samples: vec![value; frames * CHANNELS],
            gain: 1.0,
        }
    }

    fn part(kind: PartKind, from_ms: u32, to_ms: u32) -> Part {
        Part {
            kind,
            track_id: 1,
            from_ms,
            to_ms,
            shift_ms: 0,
            move_ms: 0,
        }
    }

    /// Lays the parts out and mixes them, each hook a constant `hook`, each
    /// liner a constant `liner`.
    fn mixed(
        recipe: &Recipe,
        parts: &[Part],
        lengths: &[Option<u32>],
        hook: f32,
        liner: f32,
    ) -> (Layout, Vec<f32>) {
        let layout = layout(recipe, parts, lengths);
        let audio: Vec<PartAudio> = layout
            .parts
            .iter()
            .map(|placed| {
                let frames = ms_to_frames(placed.to_ms - placed.from_ms);
                match placed.kind {
                    PartKind::Hook => constant(PartKind::Hook, frames, hook),
                    PartKind::Liner => constant(PartKind::Liner, frames, liner),
                }
            })
            .collect();
        let out = mix(recipe, &layout, &audio);
        (layout, out)
    }

    fn at(out: &[f32], ms: u32) -> f32 {
        out[ms_to_frames(ms) * CHANNELS]
    }

    #[test]
    fn parts_overlap_by_the_crossfade() {
        let recipe = Recipe {
            crossfade_ms: 500,
            ..recipe()
        };
        let parts = [
            part(PartKind::Hook, 0, 1_000),
            part(PartKind::Liner, 0, 1_000),
            part(PartKind::Hook, 0, 1_000),
        ];
        let (layout, out) = mixed(&recipe, &parts, &[], 0.25, 0.25);
        // Three parts of 1 s, minus two crossfades of 0.5 s.
        assert_eq!(layout.length_ms, 2_000);
        assert_eq!(out.len() / CHANNELS, ms_to_frames(2_000));
        // In the middle of the first part: its level; at the very start: faded in.
        assert!((at(&out, 250) - 0.25).abs() < 1e-6);
        assert!(out[0].abs() < 1e-6);
        // Linear crossfades add up to the same level.
        assert!((at(&out, 750) - 0.25).abs() < 1e-3, "{}", at(&out, 750));
    }

    #[test]
    fn liners_over_the_music_fall_between_whole_hooks() {
        let recipe = Recipe {
            crossfade_ms: 200,
            liner_mode: LinerMode::Over,
            duck_db: -20.0,
            ..before_and_during()
        };
        // An 8 s hook, a 2 s liner, a 6 s hook.
        let parts = [
            part(PartKind::Hook, 10_000, 18_000),
            part(PartKind::Liner, 0, 2_000),
            part(PartKind::Hook, 30_000, 36_000),
        ];
        let (layout, out) = mixed(&recipe, &parts, &[], 0.5, 0.0);
        // The gap: the liner, the crossfade and the ramp down before it,
        // 2.35 s, the music passing from one hook to the other under the
        // middle of the liner.
        let [first, liner, second] = layout.parts[..] else {
            panic!("three parts");
        };
        assert_eq!((first.from_ms, first.to_ms), (10_000, 19_250));
        assert_eq!((second.from_ms, second.to_ms), (28_900, 36_000));
        assert_eq!(second.at_ms, 9_250 - 200);
        // The liner starts after the first hook's span and its ramp, and
        // the second hook's span starts as it ends: the music is back up.
        assert_eq!(liner.at_ms, 8_000 + 150);
        assert_eq!(liner.end_ms(), second.at_ms + 1_100);
        assert_eq!(
            second.at_ms + 100,
            liner.at_ms + 1_000,
            "the middle of the liner"
        );
        assert_eq!(layout.length_ms, 9_050 + 7_100);
        // Both spans whole and not lowered; the extensions under the liner
        // at -20 dB, coming back up over its last 0.3 s.
        assert!((at(&out, 7_900) - 0.5).abs() < 1e-6);
        assert!((at(&out, 8_700) - 0.05).abs() < 1e-4, "{}", at(&out, 8_700));
        assert!((at(&out, 9_800) - 0.05).abs() < 1e-4, "{}", at(&out, 9_800));
        let halfway = 0.05 + (0.5 - 0.05) * 0.5;
        assert!(
            (at(&out, 10_000) - halfway).abs() < 1e-3,
            "{}",
            at(&out, 10_000)
        );
        assert!((at(&out, 10_160) - 0.5).abs() < 1e-6);

        // More of the liner over the hook before.
        let mut shifted = parts;
        shifted[1].shift_ms = 500;
        let moved = layout_of(&recipe, &shifted, &[]);
        assert_eq!(moved.parts[0].to_ms, 18_000 + 1_750);
        assert_eq!(moved.parts[2].from_ms, 30_000 - 600);
        assert_eq!(
            moved.parts[1].at_ms, 8_150,
            "the liner stays after the span"
        );
        // The first file ends 0.5 s after its hook: the second takes the rest.
        let short = layout_of(&recipe, &parts, &[Some(18_500), None, None]);
        assert_eq!(short.parts[0].to_ms, 18_500);
        assert_eq!(short.parts[2].from_ms, 30_000 - 1_850);
    }

    fn layout_of(recipe: &Recipe, parts: &[Part], lengths: &[Option<u32>]) -> Layout {
        layout(recipe, parts, lengths)
    }

    #[test]
    fn moved_parts_overlap_more_or_less() {
        let between = Recipe {
            crossfade_ms: 500,
            ..recipe()
        };
        let moved = |by: i32| {
            let mut parts = [
                part(PartKind::Hook, 0, 1_000),
                part(PartKind::Liner, 0, 1_000),
                part(PartKind::Hook, 0, 1_000),
            ];
            parts[1].move_ms = by;
            let layout = layout_of(&between, &parts, &[]);
            (layout.parts[1], layout.parts[2].at_ms)
        };
        // 0.2 s earlier: a 0.7 s crossfade, the next part follows.
        let (liner, next) = moved(-200);
        assert_eq!((liner.at_ms, liner.fade_in_ms, next), (300, 700, 800));
        // Never apart, never more than the shorter part.
        assert_eq!(moved(600).0.at_ms, 1_000);
        assert_eq!(moved(-5_000).0.at_ms, 0);
        // A liner over the music moves over it.
        let over = Recipe {
            crossfade_ms: 200,
            liner_mode: LinerMode::Over,
            ..before_and_during()
        };
        let mut parts = [
            part(PartKind::Hook, 10_000, 18_000),
            part(PartKind::Liner, 0, 2_000),
            part(PartKind::Hook, 30_000, 36_000),
        ];
        parts[1].move_ms = 300;
        assert_eq!(layout_of(&over, &parts, &[]).parts[1].at_ms, 8_450);
    }

    #[test]
    fn liners_over_the_start_and_the_end_extend_one_hook() {
        let recipe = Recipe {
            crossfade_ms: 200,
            liner_mode: LinerMode::Over,
            ..before_and_during()
        };
        let parts = [
            part(PartKind::Liner, 0, 2_000),
            part(PartKind::Hook, 5_000, 9_000),
            part(PartKind::Liner, 0, 1_000),
        ];
        let layout = layout(&recipe, &parts, &[]);
        let [opening, hook, closing] = layout.parts[..] else {
            panic!("three parts");
        };
        // The span starts as the opening liner ends.
        assert_eq!((hook.at_ms, hook.from_ms, hook.to_ms), (0, 3_000, 10_150));
        assert_eq!(opening.at_ms, 0);
        assert_eq!(closing.at_ms, (9_000 - 3_000) + 150);
        assert_eq!(closing.end_ms(), hook.end_ms());
        // Not before the file's start.
        let early = [
            part(PartKind::Liner, 0, 2_000),
            part(PartKind::Hook, 1_000, 5_000),
        ];
        assert_eq!(layout_of(&recipe, &early, &[]).parts[1].from_ms, 0);
    }

    #[test]
    fn the_music_fades_out_but_not_a_closing_liner() {
        let recipe = Recipe {
            crossfade_ms: 0,
            fade_out_ms: 1_000,
            ..recipe()
        };
        let parts = [
            part(PartKind::Hook, 0, 2_000),
            part(PartKind::Liner, 0, 1_000),
        ];
        let (layout, out) = mixed(&recipe, &parts, &[], 0.5, 0.25);
        assert_eq!(layout.music_end_ms, 2_000);
        assert!((at(&out, 900) - 0.5).abs() < 1e-6);
        assert!((at(&out, 1_500) - 0.25).abs() < 1e-3, "{}", at(&out, 1_500));
        assert!((at(&out, 2_500) - 0.25).abs() < 1e-6, "the liner is whole");
        // The curves.
        for curve in [
            FadeCurve::Linear,
            FadeCurve::FastThenSlow,
            FadeCurve::SlowThenFast,
            FadeCurve::SCurve,
        ] {
            assert!((curve.gain(0.0) - 1.0).abs() < 1e-6);
            assert!(curve.gain(1.0).abs() < 1e-6);
        }
        assert!(FadeCurve::FastThenSlow.gain(0.5) < FadeCurve::Linear.gain(0.5));
        assert!(FadeCurve::SlowThenFast.gain(0.5) > FadeCurve::Linear.gain(0.5));
        assert!((FadeCurve::SCurve.gain(0.5) - 0.5).abs() < 1e-6);
        assert!(FadeCurve::SCurve.gain(0.1) > FadeCurve::Linear.gain(0.1));
    }

    #[test]
    fn the_music_fades_in_but_not_an_opening_liner() {
        let recipe = Recipe {
            crossfade_ms: 0,
            fade_in_ms: 1_000,
            fade_in_curve: FadeCurve::SlowThenFast,
            ..recipe()
        };
        let parts = [
            part(PartKind::Liner, 0, 1_000),
            part(PartKind::Hook, 0, 2_000),
        ];
        let (layout, out) = mixed(&recipe, &parts, &[], 0.5, 0.25);
        assert_eq!((layout.music_start_ms, layout.fade_in_ms), (1_000, 1_000));
        assert!((at(&out, 500) - 0.25).abs() < 1e-6, "the liner is whole");
        // Halfway through the fade-in: slow then fast is at a quarter.
        assert!(
            (at(&out, 1_500) - 0.5 * 0.25).abs() < 1e-3,
            "{}",
            at(&out, 1_500)
        );
        assert!((at(&out, 2_500) - 0.5).abs() < 1e-6);
        assert!((FadeCurve::FastThenSlow.rise(0.5) - 0.75).abs() < 1e-6);
        assert!(FadeCurve::Linear.rise(0.0).abs() < 1e-6);
    }

    #[test]
    fn the_music_goes_down_and_comes_back_up_where_chosen() {
        // Across the liner's start and end: a crossfade.
        let recipe = Recipe {
            duck_attack_at: DuckPlace::Across,
            crossfade_ms: 200,
            liner_mode: LinerMode::Over,
            duck_db: -20.0,
            ..recipe()
        };
        let parts = [
            part(PartKind::Hook, 10_000, 18_000),
            part(PartKind::Liner, 0, 2_000),
            part(PartKind::Hook, 30_000, 36_000),
        ];
        let (layout, out) = mixed(&recipe, &parts, &[], 0.5, 0.0);
        let [_, liner, second] = layout.parts[..] else {
            panic!("three parts");
        };
        // Half of each ramp is outside the liner: the spans stay clear of it.
        assert_eq!(liner.at_ms, 8_000 + 150);
        assert_eq!(
            second.at_ms + (30_000 - second.from_ms),
            liner.end_ms() + 150
        );
        // Halfway down as the liner starts, halfway up as it ends.
        let halfway = 0.5 * (1.0 + 0.1) / 2.0;
        for ms in [liner.at_ms, liner.end_ms()] {
            assert!(
                (at(&out, ms) - halfway).abs() < 1e-2,
                "{ms}: {}",
                at(&out, ms)
            );
        }
        assert!((at(&out, 7_990) - 0.5).abs() < 1e-6);
        assert!((at(&out, liner.end_ms() + 160) - 0.5).abs() < 1e-6);

        // Outside: before it starts and after it ends.
        let outside = Ducking {
            attack_ms: 300,
            attack_at: DuckPlace::Outside,
            release_ms: 300,
            release_at: DuckPlace::Outside,
        };
        assert_eq!(
            outside.ramps(1_000, 2_000),
            [700.0, 1_000.0, 2_000.0, 2_300.0]
        );
        // Inside: during it, and sharing a liner too short for both.
        let inside = Ducking {
            attack_at: DuckPlace::Inside,
            release_at: DuckPlace::Inside,
            ..outside
        };
        assert_eq!(
            inside.ramps(1_000, 2_000),
            [1_000.0, 1_300.0, 1_700.0, 2_000.0]
        );
        assert_eq!(
            inside.ramps(1_000, 1_400),
            [1_000.0, 1_200.0, 1_200.0, 1_400.0]
        );
        assert_eq!((inside.before(), inside.after()), (0, 0));
        let inside_layout = layout_of(
            &Recipe {
                duck_attack_at: DuckPlace::Inside,
                duck_release_at: DuckPlace::Inside,
                ..recipe
            },
            &parts,
            &[],
        );
        assert_eq!(inside_layout.parts[1].at_ms, 8_000);
    }

    #[test]
    fn layouts_saved_before_the_placement_keep_theirs() {
        let saved: Layout = serde_json::from_str(
            r#"{"parts":[],"length_ms":0,"music_end_ms":0,"duck_release_ms":300}"#,
        )
        .expect("an old layout");
        assert_eq!(
            saved.ducking(),
            Ducking {
                attack_ms: 150,
                attack_at: DuckPlace::Outside,
                release_ms: 300,
                release_at: DuckPlace::Inside,
            }
        );
    }

    #[test]
    fn liners_in_a_row_keep_the_music_down() {
        let recipe = Recipe {
            crossfade_ms: 0,
            liner_mode: LinerMode::Over,
            duck_db: -20.0,
            ..recipe()
        };
        let parts = [
            part(PartKind::Hook, 0, 10_000),
            part(PartKind::Liner, 0, 1_000),
            part(PartKind::Liner, 0, 1_000),
            part(PartKind::Hook, 0, 10_000),
        ];
        let (layout, out) = mixed(&recipe, &parts, &[], 0.5, 0.0);
        let windows = duck_windows(&layout);
        assert_eq!(windows.len(), 1, "{windows:?}");
        // Where the first liner ends and the second starts: still down.
        let between = layout.parts[1].end_ms();
        assert!(
            (at(&out, between) - 0.05).abs() < 1e-4,
            "{}",
            at(&out, between)
        );
    }

    #[test]
    fn loud_mixes_are_scaled_down() {
        let parts = [part(PartKind::Hook, 0, 1_000)];
        let layout = layout(&recipe(), &parts, &[]);
        let audio = [PartAudio {
            gain: 2.0,
            ..constant(PartKind::Hook, 48_000, 0.9)
        }];
        let out = mix(&recipe(), &layout, &audio);
        let peak = out
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        assert!((peak - PEAK_LIMIT).abs() < 1e-6, "{peak}");
    }

    /// `[opening liner] hook1 liner1 hook2 liner2 hook3 [closing liner]`.
    fn medley_of(hooks: usize, liner_mode: LinerMode) -> (Recipe, Vec<Part>) {
        let recipe = Recipe {
            crossfade_ms: 500,
            liners: true,
            liner_every: 1,
            liner_at_start: true,
            liner_at_end: true,
            liner_mode,
            ..recipe()
        };
        let mut parts = vec![part(PartKind::Liner, 0, 2_000)];
        for hook in 0..hooks {
            if hook > 0 {
                parts.push(part(PartKind::Liner, 0, 4_000));
            }
            parts.push(part(PartKind::Hook, 20_000, 32_000));
        }
        parts.push(part(PartKind::Liner, 0, 3_000));
        (recipe, parts)
    }

    #[test]
    fn a_medley_is_shortened_by_its_last_liner_and_hook() {
        let (recipe, parts) = medley_of(3, LinerMode::Between);
        let layout = layout(&recipe, &parts, &[]);
        // 2 + 3 × 12 + 2 × 4 + 3 s, less six crossfades of 0.5 s.
        assert_eq!(layout.length_ms, 46_000);
        let cuts = shortened(&layout, recipe.crossfade_ms, 0, layout.length_ms);
        assert_eq!(cuts.len(), 1, "never below 2 hooks");
        let cut = &cuts[0];
        assert_eq!(
            (cut.hooks_left_out, cut.parts_left_out.clone()),
            (1, vec![4, 5])
        );
        assert_eq!(cut.length_ms, 31_000);
        // The first span ends where liner 2 starts; the second starts the
        // crossfade before the closing liner.
        assert_eq!(cut.first_to_ms, layout.parts[4].at_ms);
        assert_eq!(cut.second_from_ms, layout.parts[6].at_ms - 500);
    }

    #[test]
    fn several_pairs_can_be_left_out_the_fewest_first() {
        let (recipe, parts) = medley_of(5, LinerMode::Between);
        let laid = layout(&recipe, &parts, &[]);
        let cuts = shortened(&laid, recipe.crossfade_ms, 0, laid.length_ms);
        let left_out: Vec<u32> = cuts.iter().map(|cut| cut.hooks_left_out).collect();
        assert_eq!(left_out, [1, 2, 3]);
        // Each pair left out takes a liner and a hook, less their crossfades.
        assert_eq!(cuts[0].length_ms - cuts[1].length_ms, 15_000);
        // No closing liner, or 2 hooks: never shortened.
        let mut open = parts.clone();
        open.pop();
        assert!(shortened(&layout(&recipe, &open, &[]), 500, 0, 60_000).is_empty());
        let (recipe, two) = medley_of(2, LinerMode::Between);
        let two = layout(&recipe, &two, &[]);
        assert!(shortened(&two, 500, 0, two.length_ms).is_empty());
    }

    #[test]
    fn over_the_music_the_cut_is_never_in_a_liner() {
        let (recipe, parts) = medley_of(3, LinerMode::Over);
        let layout = layout(&recipe, &parts, &[]);
        let cuts = shortened(&layout, recipe.crossfade_ms, 0, layout.length_ms);
        let cut = &cuts[0];
        // The first span ends before liner 2 and hook 3 start.
        assert!(cut.first_to_ms <= layout.parts[4].at_ms);
        assert!(cut.first_to_ms <= layout.parts[5].at_ms);
        // and after liner 1 ends.
        assert!(cut.first_to_ms >= layout.parts[2].end_ms());
        // The second span starts on the last hook, before the closing liner.
        assert!(cut.second_from_ms < layout.parts[6].at_ms);
        assert!(cut.second_from_ms >= layout.parts[5].at_ms);
    }

    #[test]
    fn each_part_lands_in_the_rendered_file_where_the_layout_says() {
        // Each part silent but for one sample in its middle, at its own level.
        for mode in [LinerMode::Between, LinerMode::Over] {
            let (recipe, parts) = medley_of(3, mode);
            let recipe = Recipe {
                duck_db: 0.0,
                ..recipe
            };
            let layout = layout(&recipe, &parts, &[]);
            let audio: Vec<PartAudio> = layout
                .parts
                .iter()
                .enumerate()
                .map(|(index, placed)| {
                    let frames = ms_to_frames(placed.to_ms - placed.from_ms);
                    let mut samples = vec![0.0; frames * CHANNELS];
                    samples[(frames / 2) * CHANNELS] = 0.01 * (index + 1) as f32;
                    PartAudio {
                        kind: placed.kind,
                        samples,
                        gain: 1.0,
                    }
                })
                .collect();
            let out = mix(&recipe, &layout, &audio);
            for (index, placed) in layout.parts.iter().enumerate() {
                let frames = ms_to_frames(placed.to_ms - placed.from_ms);
                let frame = ms_to_frames(placed.at_ms) + frames / 2;
                let level = 0.01 * (index + 1) as f32;
                assert!(
                    (out[frame * CHANNELS] - level).abs() < 1e-6,
                    "{mode:?}: part {index} is not at {} ms",
                    placed.at_ms
                );
            }
        }
    }
}
