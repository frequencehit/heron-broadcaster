// SPDX-License-Identifier: AGPL-3.0-or-later

//! Listeners' countries, from a local GeoIP country database in the MaxMind
//! DB format (`HERON_GEOIP_DB`), read from a file and never from an outside
//! service. Only the country code is kept, with the session, when it
//! starts. Off without the file. See `docs/statistics.md#countries`.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::SystemTime;

struct Loaded {
    reader: maxminddb::Reader<Vec<u8>>,
    modified: Option<SystemTime>,
}

pub struct GeoIp {
    path: Option<PathBuf>,
    loaded: RwLock<Option<Loaded>>,
}

/// An ISO 3166-1 alpha-2 code, as the database gives it.
fn country_code(code: &str) -> Option<String> {
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()))
        .then(|| code.to_ascii_uppercase())
}

impl GeoIp {
    /// Countries from the database at `path`, when it can be read.
    pub fn new(path: Option<PathBuf>) -> Self {
        let geoip = Self {
            path,
            loaded: RwLock::new(None),
        };
        geoip.refresh();
        geoip
    }

    /// No countries.
    #[cfg(test)]
    pub fn off() -> Self {
        Self::new(None)
    }

    /// Reads the file again when it changed, appeared or went away.
    pub fn refresh(&self) {
        let Some(path) = &self.path else { return };
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let present = std::fs::metadata(path).is_ok();
        let current = self
            .loaded
            .read()
            .ok()
            .map(|loaded| loaded.as_ref().map(|l| l.modified));
        match (present, current) {
            (false, Some(Some(_))) => {
                tracing::warn!(path = %path.display(), "GeoIP country database gone: countries are off");
                if let Ok(mut loaded) = self.loaded.write() {
                    *loaded = None;
                }
            }
            (true, Some(known)) if known != Some(modified) => {
                match maxminddb::Reader::open_readfile(path) {
                    Ok(reader) => {
                        let metadata = reader.metadata();
                        tracing::info!(
                            path = %path.display(),
                            database = %metadata.database_type,
                            built = metadata.build_epoch,
                            "GeoIP country database loaded"
                        );
                        if let Ok(mut loaded) = self.loaded.write() {
                            *loaded = Some(Loaded { reader, modified });
                        }
                    }
                    Err(error) => {
                        tracing::warn!(path = %path.display(), %error, "cannot read the GeoIP country database");
                    }
                }
            }
            _ => {}
        }
    }

    /// The database's type (`DBIP-Country-Lite`), when countries are on.
    pub fn database(&self) -> Option<String> {
        let loaded = self.loaded.read().ok()?;
        loaded
            .as_ref()
            .map(|l| l.reader.metadata().database_type.clone())
    }

    /// The country of an address, `None` when it is not found (or
    /// countries are off).
    pub fn country(&self, address: &str) -> Option<String> {
        let address = address.parse::<IpAddr>().ok()?.to_canonical();
        let loaded = self.loaded.read().ok()?;
        let reader = &loaded.as_ref()?.reader;
        if address.is_ipv6() && reader.metadata().ip_version == 4 {
            return None;
        }
        let found = reader.lookup(address).ok()?;
        ["country", "registered_country"].iter().find_map(|key| {
            let code: Option<String> = found
                .decode_path(&[
                    maxminddb::PathElement::Key(key),
                    maxminddb::PathElement::Key("iso_code"),
                ])
                .ok()
                .flatten();
            code.as_deref().and_then(country_code)
        })
    }
}

/// A country database in the MaxMind DB format, written for the tests:
/// IPv4 networks (`192.0.2.0/24`) and their country codes.
#[cfg(test)]
pub fn test_database(networks: &[(&str, &str)]) -> Vec<u8> {
    fn control(kind: u8, size: usize, out: &mut Vec<u8>) {
        let size = u8::try_from(size).unwrap();
        assert!(size < 29);
        if kind > 7 {
            out.push(size);
            out.push(kind - 7);
        } else {
            out.push((kind << 5) | size);
        }
    }
    fn text(value: &str, out: &mut Vec<u8>) {
        control(2, value.len(), out);
        out.extend_from_slice(value.as_bytes());
    }
    fn unsigned(kind: u8, value: u64, out: &mut Vec<u8>) {
        let bytes = value.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        control(kind, 8 - skip, out);
        out.extend_from_slice(&bytes[skip..]);
    }

    // The data: one `{"country": {"iso_code": …}}` per network.
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    for (_, code) in networks {
        offsets.push(data.len());
        control(7, 1, &mut data);
        text("country", &mut data);
        control(7, 1, &mut data);
        text("iso_code", &mut data);
        text(code, &mut data);
    }
    // The search tree, IPv4: nodes of two records, `None` for no data.
    #[derive(Clone, Copy)]
    enum Record {
        Empty,
        Node(usize),
        Data(usize),
    }
    let mut nodes: Vec<[Record; 2]> = vec![[Record::Empty; 2]];
    for (n, (network, _)) in networks.iter().enumerate() {
        let (address, bits) = network.split_once('/').unwrap();
        let address = u32::from(address.parse::<std::net::Ipv4Addr>().unwrap());
        let bits: usize = bits.parse().unwrap();
        let mut node = 0;
        for depth in 0..bits {
            let bit = usize::from((address >> (31 - depth)) & 1 == 1);
            if depth + 1 == bits {
                nodes[node][bit] = Record::Data(offsets[n]);
            } else {
                node = match nodes[node][bit] {
                    Record::Node(next) => next,
                    _ => {
                        nodes.push([Record::Empty; 2]);
                        let next = nodes.len() - 1;
                        nodes[node][bit] = Record::Node(next);
                        next
                    }
                };
            }
        }
    }
    let count = nodes.len();
    let mut out = Vec::new();
    for node in &nodes {
        for record in node {
            let value = match *record {
                Record::Empty => count,
                Record::Node(next) => next,
                Record::Data(offset) => count + 16 + offset,
            };
            out.extend_from_slice(&u32::try_from(value).unwrap().to_be_bytes()[1..]);
        }
    }
    out.extend_from_slice(&[0; 16]);
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\xAB\xCD\xEFMaxMind.com");
    control(7, 9, &mut out);
    text("binary_format_major_version", &mut out);
    unsigned(5, 2, &mut out);
    text("binary_format_minor_version", &mut out);
    unsigned(5, 0, &mut out);
    text("build_epoch", &mut out);
    unsigned(9, 1_792_454_400, &mut out);
    text("database_type", &mut out);
    text("Test-Country", &mut out);
    text("description", &mut out);
    control(7, 0, &mut out);
    text("ip_version", &mut out);
    unsigned(5, 4, &mut out);
    text("languages", &mut out);
    control(11, 0, &mut out);
    text("node_count", &mut out);
    unsigned(6, u64::try_from(count).unwrap(), &mut out);
    text("record_size", &mut out);
    unsigned(5, 24, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_country_of_an_address() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("country.mmdb");
        let off = GeoIp::new(Some(path.clone()));
        assert_eq!((off.database(), off.country("192.0.2.1")), (None, None));

        std::fs::write(
            &path,
            test_database(&[("192.0.2.0/24", "FR"), ("198.51.100.0/25", "ca")]),
        )
        .unwrap();
        off.refresh();
        assert_eq!(off.database().as_deref(), Some("Test-Country"));
        assert_eq!(off.country("192.0.2.200").as_deref(), Some("FR"));
        assert_eq!(off.country("::ffff:198.51.100.7").as_deref(), Some("CA"));
        // Not in the database, IPv6 in an IPv4 database, not an address.
        assert_eq!(off.country("198.51.100.200"), None);
        assert_eq!(off.country("2001:db8::1"), None);
        assert_eq!(off.country("unknown"), None);

        // Replaced: read again; gone: off.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, test_database(&[("192.0.2.0/24", "BE")])).unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        off.refresh();
        assert_eq!(off.country("192.0.2.1").as_deref(), Some("BE"));
        std::fs::remove_file(&path).unwrap();
        off.refresh();
        assert_eq!(off.database(), None);
        assert_eq!(GeoIp::off().country("192.0.2.1"), None);
    }

    #[test]
    fn a_broken_file_leaves_countries_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("country.mmdb");
        std::fs::write(&path, b"not a database").unwrap();
        assert_eq!(GeoIp::new(Some(path)).database(), None);
    }
}
