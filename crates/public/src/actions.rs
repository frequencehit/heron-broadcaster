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

/// The writes of every show. Each show adds its own (#131, #132, #133).
pub const ACTIONS: &[Action] = &[];

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
            Field {
                name: "first_name",
                max_chars: 5,
                required: true,
            },
            Field {
                name: "message",
                max_chars: 10,
                required: false,
            },
        ],
        per_device: 1,
        delivery: Delivery::Forward,
    };

    pub const VOTE: Action = Action {
        module: Module::Match,
        name: "vote",
        fields: &[Field {
            name: "choice",
            max_chars: 1,
            required: true,
        }],
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
    fn paths_name_the_show_and_the_action() {
        assert_eq!(MESSAGE.path(), "dedications");
        assert_eq!(VOTE.path(), "match/vote");
        let actions = [MESSAGE, VOTE];
        assert!(find(&actions, "match", "vote").is_some());
        assert!(find(&actions, "match", "").is_none());
    }
}
