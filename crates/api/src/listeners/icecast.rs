// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reads an Icecast server's statistics: listeners per mount from
//! `/status-json.xsl` (public), connected listeners from
//! `/admin/listclients` (admin credentials).

use std::collections::HashMap;
use std::time::Duration;

use quick_xml::events::Event;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A listener connected to a mount, as the server lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    /// The server's ID for the connection.
    pub id: u64,
    /// Used for the listener key, never stored.
    pub address: String,
    pub user_agent: String,
    /// Seconds since the listener connected.
    pub connected_s: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{0}")]
    Request(String),
    #[error("the server answered {0}")]
    Status(reqwest::StatusCode),
    #[error("wrong admin credentials")]
    Credentials,
    #[error("unexpected answer: {0}")]
    Format(String),
}

impl From<reqwest::Error> for ReadError {
    fn from(error: reqwest::Error) -> Self {
        // Without the URL: it can carry nothing secret, but it is noise.
        Self::Request(error.without_url().to_string())
    }
}

/// `http://host:port` (`https` with TLS), where the statistics are read.
pub fn base_url(host: &str, port: u16, tls: bool) -> String {
    let scheme = if tls { "https" } else { "http" };
    if host.contains(':') {
        format!("{scheme}://[{host}]:{port}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}

/// Listeners per mount.
pub async fn mount_listeners(
    client: &reqwest::Client,
    base: &str,
) -> Result<HashMap<String, u32>, ReadError> {
    let response = client
        .get(format!("{base}/status-json.xsl"))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(ReadError::Status(response.status()));
    }
    parse_status(&response.text().await?)
}

/// The listeners of a mount; empty when no source is connected to it.
pub async fn list_clients(
    client: &reqwest::Client,
    base: &str,
    mount: &str,
    username: &str,
    password: &str,
) -> Result<Vec<Client>, ReadError> {
    let response = client
        .get(format!("{base}/admin/listclients"))
        .query(&[("mount", mount)])
        .basic_auth(username, Some(password))
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;
    match response.status() {
        status if status.is_success() => parse_clients(&response.text().await?),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            Err(ReadError::Credentials)
        }
        // "Source does not exist": nobody can listen to it.
        reqwest::StatusCode::BAD_REQUEST => Ok(Vec::new()),
        status => Err(ReadError::Status(status)),
    }
}

/// Parses `/status-json.xsl`: `icestats.source` is missing without a
/// source, an object with one and an array with several.
pub fn parse_status(json: &str) -> Result<HashMap<String, u32>, ReadError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| ReadError::Format(error.to_string()))?;
    let stats = value
        .get("icestats")
        .ok_or_else(|| ReadError::Format("no icestats".to_owned()))?;
    let sources = match stats.get("source") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(sources)) => sources.iter().collect(),
        Some(source) => vec![source],
    };
    let mut mounts = HashMap::new();
    for source in sources {
        let Some(mount) = source
            .get("listenurl")
            .and_then(serde_json::Value::as_str)
            .and_then(mount_of)
        else {
            continue;
        };
        let listeners = source
            .get("listeners")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        mounts.insert(mount, u32::try_from(listeners).unwrap_or(u32::MAX));
    }
    Ok(mounts)
}

/// The mount of a listen URL (`http://host:8000/live` → `/live`).
fn mount_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.find('/').map(|slash| rest[slash..].to_owned())
}

/// Parses `/admin/listclients`:
/// `<icestats><source mount="/live"><listener><IP>…</IP><UserAgent>…</UserAgent>
/// <Connected>12</Connected><ID>3</ID></listener>…</source></icestats>`.
pub fn parse_clients(xml: &str) -> Result<Vec<Client>, ReadError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut clients = Vec::new();
    let mut current: Option<Fields> = None;
    let mut field: Option<String> = None;
    let mut seen_icestats = false;
    loop {
        match reader
            .read_event()
            .map_err(|error| ReadError::Format(error.to_string()))?
        {
            Event::Start(start) => {
                let name = start.local_name().as_ref().to_ascii_lowercase();
                match name.as_str() {
                    "icestats" => seen_icestats = true,
                    "listener" => current = Some(Fields::default()),
                    _ if current.is_some() => field = Some(name),
                    _ => {}
                }
            }
            Event::Text(text) => {
                if let (Some(fields), Some(name)) = (current.as_mut(), field.as_deref()) {
                    fields.set(name, &text.xml10_content());
                }
            }
            Event::GeneralRef(reference) => {
                // Entities split the text: `AT&amp;T` is `AT`, `&amp;`, `T`.
                if let (Some(fields), Some(name)) = (current.as_mut(), field.as_deref()) {
                    let resolved = reference
                        .resolve_char_ref()
                        .ok()
                        .flatten()
                        .map(String::from)
                        .or_else(|| {
                            quick_xml::escape::resolve_xml_entity(&reference).map(str::to_owned)
                        })
                        .unwrap_or_default();
                    fields.set(name, &resolved);
                }
            }
            Event::End(end) => {
                let name = end.local_name().as_ref().to_ascii_lowercase();
                if name == "listener" {
                    if let Some(client) = current.take().and_then(Fields::client) {
                        clients.push(client);
                    }
                } else {
                    field = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_icestats {
        return Err(ReadError::Format("no icestats".to_owned()));
    }
    Ok(clients)
}

#[derive(Default)]
struct Fields {
    id: String,
    address: String,
    user_agent: String,
    connected: String,
}

impl Fields {
    fn set(&mut self, name: &str, text: &str) {
        let target = match name {
            "id" => &mut self.id,
            "ip" => &mut self.address,
            "useragent" => &mut self.user_agent,
            "connected" => &mut self.connected,
            _ => return,
        };
        target.push_str(text);
    }

    fn client(self) -> Option<Client> {
        Some(Client {
            id: self.id.trim().parse().ok()?,
            address: self.address.trim().to_owned(),
            user_agent: self.user_agent.trim().to_owned(),
            connected_s: self.connected.trim().parse().unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_status_page() {
        // Icecast 2.4.4, one source: an object.
        let one = r#"{"icestats":{"admin":"icemaster@localhost","host":"localhost",
            "server_id":"Icecast 2.4.4","source":{"genre":"various","listener_peak":2,
            "listeners":2,"listenurl":"http://localhost:8000/live","server_type":"audio/mpeg",
            "dummy":null}}}"#;
        assert_eq!(
            parse_status(one).unwrap(),
            HashMap::from([("/live".to_owned(), 2)])
        );
        let several = r#"{"icestats":{"source":[
            {"listeners":3,"listenurl":"http://radio:8000/live"},
            {"listeners":0,"listenurl":"http://radio:8000/fallback"}]}}"#;
        assert_eq!(
            parse_status(several).unwrap(),
            HashMap::from([("/live".to_owned(), 3), ("/fallback".to_owned(), 0)])
        );
        let none = r#"{"icestats":{"admin":"x","host":"localhost"}}"#;
        assert!(parse_status(none).unwrap().is_empty());
        assert!(parse_status("<html>").is_err());
        assert!(parse_status("{}").is_err());
    }

    #[test]
    fn reads_the_listener_list() {
        let xml = r#"<?xml version="1.0"?>
<icestats><source mount="/live"><Listeners>2</Listeners><listener><IP>172.19.0.1</IP><UserAgent>Mozilla/5.0 Probe</UserAgent><Connected>3</Connected><ID>1</ID></listener><listener><IP>2001:db8::1</IP><UserAgent>AT&amp;T &#233;</UserAgent><Connected>125</Connected><ID>2</ID></listener></source></icestats>"#;
        assert_eq!(
            parse_clients(xml).unwrap(),
            [
                Client {
                    id: 1,
                    address: "172.19.0.1".to_owned(),
                    user_agent: "Mozilla/5.0 Probe".to_owned(),
                    connected_s: 3,
                },
                Client {
                    id: 2,
                    address: "2001:db8::1".to_owned(),
                    user_agent: "AT&T é".to_owned(),
                    connected_s: 125,
                },
            ]
        );
        let empty = r#"<?xml version="1.0"?><icestats><source mount="/live"><Listeners>0</Listeners></source></icestats>"#;
        assert!(parse_clients(empty).unwrap().is_empty());
        assert!(parse_clients("<html><body>Error</body></html>").is_err());
    }

    #[test]
    fn base_urls() {
        assert_eq!(base_url("icecast", 8000, false), "http://icecast:8000");
        assert_eq!(base_url("::1", 8000, false), "http://[::1]:8000");
        assert_eq!(base_url("relay", 443, true), "https://relay:443");
    }
}
