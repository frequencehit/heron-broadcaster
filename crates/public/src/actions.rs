// SPDX-License-Identifier: AGPL-3.0-or-later

//! The writes listeners can send, by show: what each show adds to take
//! dedications or votes (`docs/listener-interaction.md#building-a-show-on-the-base`).
//!
//! `POST /api/v1/public/stations/{code}/{module}[/{action}]` finds its
//! [`Action`] here; the gateway checks it and sends it on to
//! `/api/v1/gateway/stations/{code}/{module}[/{action}]`.

use heron_core::Module;
use serde_json::{Map, Value};

/// A field of a write.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub name: &'static str,
    /// Characters at most (of its text; numbers and booleans pass).
    pub max_chars: usize,
    pub required: bool,
    /// The values it may take; empty: any.
    pub choices: &'static [&'static str],
    /// It must be what the show's live state has under the same name (the
    /// play being voted on): else `409`, the write came too late.
    pub current: bool,
}

impl Field {
    /// A field of any value, at most `max_chars` characters.
    pub const fn text(name: &'static str, max_chars: usize, required: bool) -> Self {
        Self {
            name,
            max_chars,
            required,
            choices: &[],
            current: false,
        }
    }
}

/// How a write reaches the API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// At once, one by one; the listener gets the API's answer (a
    /// dedication, and its link to follow it).
    Forward,
    /// Added up in the gateway and sent every second, as
    /// `{"counts": [{"fields": {…}, "count": n}]}` (votes); the listener
    /// gets `202`.
    Batch,
    /// Kept per device, its last write only, and sent every second as
    /// `{"writes": [{"device": "…", "fields": {…}}]}` (likes, which a
    /// device can change); the listener gets `202`.
    Latest,
}

/// A write a show takes.
#[derive(Clone, Copy, Debug)]
pub struct Action {
    pub module: Module,
    /// The last part of its path; empty for `POST …/{module}`.
    pub name: &'static str,
    pub fields: &'static [Field],
    /// Writes per device until the show's opening range ends; 0: no limit.
    pub per_device: u32,
    pub delivery: Delivery,
}

/// The writes of every show. Each show adds its own (#132).
pub const ACTIONS: &[Action] = &[DEDICATION, LIKE];

/// A request for a track with a message (#131), sent on to the API, which
/// answers its follow link. The API counts each device's requests in the
/// range as the station sets it (1 to 10) and allows one waiting at a
/// time; the gateway stops a device at 10, the most a station allows.
pub const DEDICATION: Action = Action {
    module: Module::Dedications,
    name: "",
    fields: &[
        Field::text("track_id", 20, true),
        Field::text("from", 40, true),
        Field::text("to", 60, true),
        Field::text("message", 500, false),
    ],
    per_device: 10,
    delivery: Delivery::Forward,
};

/// A read a show adds, under `GET …/stations/{code}/{module}/{name}`: sent
/// on to the API's `/gateway/stations/{code}/{module}/{name}`, with the
/// gateway token, and cached per query.
#[derive(Clone, Copy, Debug)]
pub struct Read {
    pub module: Module,
    pub name: &'static str,
    /// Its query's parameters: name, characters at most, required.
    pub params: &'static [(&'static str, usize, bool)],
    /// Seconds an answer is kept.
    pub ttl: u32,
    /// Only while the show is open (else `409`, without asking the API).
    pub open: bool,
}

/// The reads of every show.
pub const READS: &[Read] = &[
    // The requestable tracks, while the dedications are open.
    Read {
        module: Module::Dedications,
        name: "search",
        params: &[("q", 60, true)],
        ttl: 30,
        open: true,
    },
    // A request, by the key of its follow link.
    Read {
        module: Module::Dedications,
        name: "follow",
        params: &[("id", 32, true)],
        ttl: 5,
        open: false,
    },
];

impl Read {
    /// The query, checked: known parameters within their size, the
    /// required ones there; in a fixed order, as the cache's key.
    pub fn query(&self, given: &[(String, String)]) -> Result<String, String> {
        if let Some((name, _)) = given
            .iter()
            .find(|(name, _)| !self.params.iter().any(|(known, _, _)| known == name))
        {
            return Err(format!("unknown parameter {name}"));
        }
        let mut query = Vec::new();
        for (name, max, required) in self.params {
            match given.iter().find(|(given, _)| given == name) {
                Some((_, value)) if value.chars().count() > *max => {
                    return Err(format!("{name} is {max} characters at most"));
                }
                Some((_, value)) => query.push(format!("{name}={}", encode(value))),
                None if *required => return Err(format!("{name} is required")),
                None => {}
            }
        }
        Ok(query.join("&"))
    }
}

/// The read of a path, among `reads`.
pub fn find_read<'a>(reads: &'a [Read], module: &str, name: &str) -> Option<&'a Read> {
    reads
        .iter()
        .find(|read| read.module.key() == module && read.name == name)
}

/// `value` percent-encoded for a query.
fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// A like or a dislike of the track playing (#133): one per device per
/// play, the last one counting, so no limit per device but the address's.
pub const LIKE: Action = Action {
    module: Module::Likes,
    name: "vote",
    fields: &[
        Field {
            name: "play_id",
            max_chars: 20,
            required: true,
            choices: &[],
            current: true,
        },
        Field {
            name: "vote",
            max_chars: 7,
            required: true,
            choices: &["like", "dislike"],
            current: false,
        },
    ],
    per_device: 0,
    delivery: Delivery::Latest,
};

impl Action {
    /// Its path after `/stations/{code}/`.
    pub fn path(&self) -> String {
        if self.name.is_empty() {
            self.module.key().to_owned()
        } else {
            format!("{}/{}", self.module.key(), self.name)
        }
    }

    /// The body's fields, checked: an object of known fields, each within
    /// its size, the required ones there.
    pub fn check(&self, body: &Value) -> Result<Map<String, Value>, String> {
        let Value::Object(fields) = body else {
            return Err("the body is a JSON object".to_owned());
        };
        for (name, value) in fields {
            let Some(field) = self.fields.iter().find(|field| field.name == name) else {
                return Err(format!("unknown field {name}"));
            };
            match value {
                Value::String(text) if text.chars().count() > field.max_chars => {
                    return Err(format!("{name} is {} characters at most", field.max_chars));
                }
                Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => {}
                _ => return Err(format!("{name} is a text, a number or a boolean")),
            }
            if !field.choices.is_empty()
                && !value.is_null()
                && !value
                    .as_str()
                    .is_some_and(|text| field.choices.contains(&text))
            {
                return Err(format!("{name} is one of {}", field.choices.join(", ")));
            }
        }
        if let Some(missing) = self
            .fields
            .iter()
            .find(|field| field.required && fields.get(field.name).is_none_or(Value::is_null))
        {
            return Err(format!("{} is required", missing.name));
        }
        Ok(fields.clone())
    }

    /// The first field that is not what the show's live `state` has now.
    pub fn stale(&self, fields: &Map<String, Value>, state: &Value) -> Option<&'static str> {
        self.fields
            .iter()
            .filter(|field| field.current)
            .find(|field| fields.get(field.name) != state.get(field.name).filter(|v| !v.is_null()))
            .map(|field| field.name)
    }
}

/// The action of a path, among `actions`.
pub fn find<'a>(actions: &'a [Action], module: &str, name: &str) -> Option<&'a Action> {
    actions
        .iter()
        .find(|action| action.module.key() == module && action.name == name)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A show's write, as one would add it.
    pub const MESSAGE: Action = Action {
        module: Module::Dedications,
        name: "",
        fields: &[
            Field::text("first_name", 5, true),
            Field::text("message", 10, false),
        ],
        per_device: 1,
        delivery: Delivery::Forward,
    };

    pub const VOTE: Action = Action {
        module: Module::Match,
        name: "vote",
        fields: &[Field::text("choice", 1, true)],
        per_device: 0,
        delivery: Delivery::Batch,
    };

    #[test]
    fn fields_are_checked() {
        let ok = serde_json::json!({"first_name": "Marc", "message": "Hello"});
        assert_eq!(MESSAGE.check(&ok).unwrap().len(), 2);
        for (body, error) in [
            (serde_json::json!([]), "object"),
            (
                serde_json::json!({"first_name": "Marc", "email": "x"}),
                "unknown field email",
            ),
            (serde_json::json!({"first_name": "Marcel"}), "5 characters"),
            (
                serde_json::json!({"message": "Hi"}),
                "first_name is required",
            ),
            (serde_json::json!({"first_name": null}), "required"),
            (serde_json::json!({"first_name": {"a": 1}}), "a text"),
        ] {
            let refused = MESSAGE.check(&body).unwrap_err();
            assert!(refused.contains(error), "{body}: {refused}");
        }
        // Sizes are in characters, not bytes.
        assert!(
            MESSAGE
                .check(&serde_json::json!({"first_name": "Zoë"}))
                .is_ok()
        );
    }

    #[test]
    fn likes_name_the_play_and_like_or_dislike_it() {
        assert_eq!(LIKE.path(), "likes/vote");
        let vote = LIKE
            .check(&serde_json::json!({"play_id": 12, "vote": "like"}))
            .unwrap();
        for (body, error) in [
            (
                serde_json::json!({"play_id": 12, "vote": "love"}),
                "one of like, dislike",
            ),
            (serde_json::json!({"play_id": 12, "vote": 1}), "one of"),
            (serde_json::json!({"vote": "like"}), "play_id is required"),
        ] {
            let refused = LIKE.check(&body).unwrap_err();
            assert!(refused.contains(error), "{body}: {refused}");
        }
        // Only the play on air now.
        assert_eq!(LIKE.stale(&vote, &serde_json::json!({"play_id": 12})), None);
        assert_eq!(
            LIKE.stale(&vote, &serde_json::json!({"play_id": 13})),
            Some("play_id")
        );
        assert_eq!(LIKE.stale(&vote, &Value::Null), Some("play_id"));
        let no_play = LIKE
            .check(&serde_json::json!({"play_id": null, "vote": "like"}))
            .unwrap_err();
        assert!(no_play.contains("required"));
    }

    #[test]
    fn dedications_take_a_track_and_a_short_message() {
        assert_eq!(DEDICATION.path(), "dedications");
        let ok =
            serde_json::json!({"track_id": 42, "from": "Marc", "to": "Julie", "message": "Hi"});
        assert_eq!(DEDICATION.check(&ok).unwrap().len(), 4);
        let long = "x".repeat(501);
        for (body, error) in [
            (
                serde_json::json!({"from": "Marc", "to": "Julie"}),
                "track_id is required",
            ),
            (
                serde_json::json!({"track_id": 1, "from": "Marc", "to": "Julie", "email": "a@b"}),
                "unknown field email",
            ),
            (
                serde_json::json!({"track_id": 1, "from": "Marc", "to": "Julie", "message": long}),
                "500 characters",
            ),
        ] {
            let refused = DEDICATION.check(&body).unwrap_err();
            assert!(refused.contains(error), "{body}: {refused}");
        }
    }

    #[test]
    fn reads_check_their_query() {
        let search = find_read(READS, "dedications", "search").unwrap();
        let given = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert_eq!(search.query(&given(&[("q", "neon")])).unwrap(), "q=neon");
        assert_eq!(
            search.query(&given(&[("q", "sœur & co")])).unwrap(),
            "q=s%C5%93ur%20%26%20co"
        );
        assert!(search.query(&given(&[])).unwrap_err().contains("required"));
        assert!(
            search
                .query(&given(&[("q", "a"), ("x", "1")]))
                .unwrap_err()
                .contains("unknown")
        );
        assert!(search.query(&given(&[("q", &"a".repeat(61))])).is_err());
        assert!(find_read(READS, "dedications", "other").is_none());
        assert!(find_read(READS, "likes", "search").is_none());
    }

    #[test]
    fn paths_name_the_show_and_the_action() {
        assert_eq!(MESSAGE.path(), "dedications");
        assert_eq!(VOTE.path(), "match/vote");
        let actions = [MESSAGE, VOTE];
        assert!(find(&actions, "match", "vote").is_some());
        assert!(find(&actions, "match", "").is_none());
    }
}
