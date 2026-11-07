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

/// The writes of every show. Each show adds its own (#131, #132).
pub const ACTIONS: &[Action] = &[LIKE];

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
    fn paths_name_the_show_and_the_action() {
        assert_eq!(MESSAGE.path(), "dedications");
        assert_eq!(VOTE.path(), "match/vote");
        let actions = [MESSAGE, VOTE];
        assert!(find(&actions, "match", "vote").is_some());
        assert!(find(&actions, "match", "").is_none());
    }
}
