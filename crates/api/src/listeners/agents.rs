// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reads a player's user agent into a device and a player family, without
//! its version, so that the versions of one app are one line. The user
//! agent itself never leaves the API. See
//! `docs/statistics.md#devices-and-players`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Device {
    Mobile,
    Desktop,
    SmartSpeaker,
    Car,
    /// A TV or a set-top box.
    Tv,
    Other,
}

impl Device {
    pub fn key(self) -> &'static str {
        match self {
            Self::Mobile => "mobile",
            Self::Desktop => "desktop",
            Self::SmartSpeaker => "smart_speaker",
            Self::Car => "car",
            Self::Tv => "tv",
            Self::Other => "other",
        }
    }
}

/// Words of a user agent that tell the device, the first found wins.
const DEVICES: &[(&str, Device)] = &[
    ("carplay", Device::Car),
    ("android auto", Device::Car),
    ("androidauto", Device::Car),
    ("tesla", Device::Car),
    ("ford sync", Device::Car),
    ("alexa", Device::SmartSpeaker),
    ("echo", Device::SmartSpeaker),
    ("sonos", Device::SmartSpeaker),
    ("homepod", Device::SmartSpeaker),
    ("google home", Device::SmartSpeaker),
    ("google nest", Device::SmartSpeaker),
    ("bose", Device::SmartSpeaker),
    ("heos", Device::SmartSpeaker),
    ("bluesound", Device::SmartSpeaker),
    ("smarttv", Device::Tv),
    ("smart-tv", Device::Tv),
    ("tizen", Device::Tv),
    ("web0s", Device::Tv),
    ("webos", Device::Tv),
    ("appletv", Device::Tv),
    ("apple tv", Device::Tv),
    ("crkey", Device::Tv),
    ("roku", Device::Tv),
    ("bravia", Device::Tv),
    ("fire tv", Device::Tv),
    ("aftb", Device::Tv),
    ("aftm", Device::Tv),
    ("afts", Device::Tv),
    ("kodi", Device::Tv),
    ("hbbtv", Device::Tv),
    ("iphone", Device::Mobile),
    ("ipad", Device::Mobile),
    ("ipod", Device::Mobile),
    ("android", Device::Mobile),
    ("mobile", Device::Mobile),
    ("exoplayer", Device::Mobile),
    ("stagefright", Device::Mobile),
    ("dalvik", Device::Mobile),
    ("okhttp", Device::Mobile),
    ("windows", Device::Desktop),
    ("macintosh", Device::Desktop),
    ("mac os x", Device::Desktop),
    ("x11", Device::Desktop),
    ("linux", Device::Desktop),
    ("vlc", Device::Desktop),
    ("winamp", Device::Desktop),
    ("foobar2000", Device::Desktop),
    ("itunes", Device::Desktop),
    ("music player daemon", Device::Desktop),
    ("mpv", Device::Desktop),
    ("mplayer", Device::Desktop),
    ("nsplayer", Device::Desktop),
    ("windows-media-player", Device::Desktop),
];

/// Words that tell the player family, the first found wins; browsers last,
/// since apps embedding one often say `Mozilla/5.0 … Chrome/…` too.
const PLAYERS: &[(&str, &str)] = &[
    ("alexa", "Alexa"),
    ("sonos", "Sonos"),
    ("vlc", "VLC"),
    ("winamp", "Winamp"),
    ("foobar2000", "foobar2000"),
    ("itunes", "iTunes"),
    ("applecoremedia", "Apple media player"),
    ("music player daemon", "Music Player Daemon"),
    ("mpv", "mpv"),
    ("mplayer", "MPlayer"),
    ("kodi", "Kodi"),
    ("nsplayer", "Windows Media Player"),
    ("windows-media-player", "Windows Media Player"),
    ("exoplayer", "Android app (ExoPlayer)"),
    ("stagefright", "Android media player"),
    ("okhttp", "Android app (OkHttp)"),
    ("dalvik", "Android app"),
    ("lavf", "FFmpeg"),
    ("gstreamer", "GStreamer"),
    ("streamripper", "Streamripper"),
    ("curl", "curl"),
    ("wget", "Wget"),
    ("python", "Python"),
    ("edg/", "Edge"),
    ("edga/", "Edge"),
    ("edgios/", "Edge"),
    ("opr/", "Opera"),
    ("samsungbrowser", "Samsung Internet"),
    ("firefox", "Firefox"),
    ("fxios", "Firefox"),
    ("crios", "Chrome"),
    ("chromium", "Chromium"),
    ("chrome", "Chrome"),
    ("safari", "Safari"),
];

/// The longest family made of a user agent's first word.
const FAMILY_CHARS: usize = 40;

/// The device and player family of a user agent.
pub fn classify(user_agent: &str) -> (Device, String) {
    let lower = user_agent.to_ascii_lowercase();
    let device = DEVICES
        .iter()
        .find(|(word, _)| lower.contains(word))
        .map_or(Device::Other, |&(_, device)| device);
    let family = PLAYERS
        .iter()
        .find(|(word, _)| lower.contains(word))
        .map(|&(_, family)| family.to_owned())
        .unwrap_or_else(|| first_word(user_agent));
    (device, family)
}

/// The first word of a user agent, without its version: `MyApp/2.1 (…)`
/// is `MyApp`. `Mozilla/5.0` says nothing of the player.
fn first_word(user_agent: &str) -> String {
    let word = user_agent
        .split(|c: char| c == '/' || c == '(' || c.is_whitespace() || c == ';')
        .find(|w| !w.is_empty())
        .unwrap_or_default();
    let word: String = word
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
        .take(FAMILY_CHARS)
        .collect();
    if word.is_empty() || word.eq_ignore_ascii_case("mozilla") {
        "Other".to_owned()
    } else {
        word
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_and_families() {
        let cases = [
            ("VLC/3.0.20 LibVLC/3.0.20", Device::Desktop, "VLC"),
            ("VLC/3.0.21 LibVLC/3.0.21", Device::Desktop, "VLC"),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1",
                Device::Mobile,
                "Safari",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0",
                Device::Desktop,
                "Edge",
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
                Device::Desktop,
                "Firefox",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36",
                Device::Mobile,
                "Chrome",
            ),
            (
                "AppleCoreMedia/1.0.0.21E236 (iPhone; U; CPU OS 17_4 like Mac OS X; en_us)",
                Device::Mobile,
                "Apple media player",
            ),
            (
                "AppleCoreMedia/1.0.0.21E236 (HomePod; U; CPU OS 17_4 like Mac OS X; en_us)",
                Device::SmartSpeaker,
                "Apple media player",
            ),
            (
                "AlexaMediaPlayer/2.1.4676.0 (Linux;Android 5.1.1) ExoPlayerLib/1.5.9",
                Device::SmartSpeaker,
                "Alexa",
            ),
            (
                "Linux UPnP/1.0 Sonos/79.1-56030 (ZPS27)",
                Device::SmartSpeaker,
                "Sonos",
            ),
            (
                "ExoPlayerLib/2.18.1",
                Device::Mobile,
                "Android app (ExoPlayer)",
            ),
            (
                "stagefright/1.2 (Linux;Android 13)",
                Device::Mobile,
                "Android media player",
            ),
            (
                "Mozilla/5.0 (SMART-TV; Linux; Tizen 6.0) AppleWebKit/538.1",
                Device::Tv,
                "Other",
            ),
            (
                "Mozilla/5.0 (X11; Linux armv7l) AppleWebKit/537.36 CrKey/1.56.500000",
                Device::Tv,
                "Other",
            ),
            (
                "CarPlay/1.0 AppleCoreMedia/1.0",
                Device::Car,
                "Apple media player",
            ),
            ("Lavf/60.16.100", Device::Other, "FFmpeg"),
            ("MyRadioApp/4.2 (Linux)", Device::Desktop, "MyRadioApp"),
            ("SomePlayer", Device::Other, "SomePlayer"),
            ("", Device::Other, "Other"),
            ("Mozilla/5.0", Device::Other, "Other"),
        ];
        for (agent, device, family) in cases {
            assert_eq!(classify(agent), (device, family.to_owned()), "{agent}");
        }
    }
}
