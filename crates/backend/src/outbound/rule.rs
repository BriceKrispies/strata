//! The outbound rule: which addresses a tenant may make this process connect to, judged in their
//! canonical numeric form (WFL-252), and which addresses (URLs) are well formed enough to be judged
//! at all (WFL-1579).
//!
//! **Addresses.** An address is judged as a number, never as text: the URL parser turns every
//! spelling of an IPv4 address (decimal `2130706433`, octal `0177.0.0.1`, hexadecimal `0x7f.1`,
//! shortened `127.1`) into the dotted form before this module sees it, and an IPv6 address that
//! carries an IPv4 one (mapped `::ffff:a.b.c.d`, the NAT64 prefix `64:ff9b::/96`) is judged by the
//! IPv4 address it carries. Refused: loopback, private, link-local, shared (CGNAT), multicast,
//! unspecified, broadcast and reserved space, documentation and benchmarking space, the IETF
//! protocol blocks (Teredo, ORCHID, 6to4 among them), unique-local and site-local IPv6, every IPv6
//! address outside global unicast (`2000::/3`) that is not one of the carrying forms above, and the
//! cloud metadata addresses, including the one that is on a public address (Azure's
//! `168.63.129.16`).
//!
//! **URLs.** HTTPS on port 443 only, with no user name, no password and no fragment; a host that is a
//! name must have a dot and must not be one of the names that only ever mean the platform's own
//! network (`localhost`, `*.local`, `*.internal`, `*.arpa`). The development exception
//! (FED-2644) allows plain HTTP, any port and any host; the address is still resolved once and
//! connected to as resolved.
//!
//! Implements: WFL-008, WFL-252, WFL-1579, WFL-2645, H-026

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The longest address that is judged at all.
pub const MAX_URL_LEN: usize = 2048;

/// Why an IPv4 address is refused, or `None` when it is a public address.
pub fn v4_refusal(address: Ipv4Addr) -> Option<&'static str> {
    // Cloud metadata services, named first so that the reason says so.
    const METADATA: [[u8; 4]; 5] = [
        [169, 254, 169, 254], // AWS, GCP, Azure, OpenStack and others
        [169, 254, 170, 2],   // AWS ECS task metadata
        [100, 100, 100, 200], // Alibaba Cloud
        [192, 0, 0, 192],     // Oracle Cloud
        [168, 63, 129, 16],   // Azure wire server, on a public address
    ];
    if METADATA.contains(&address.octets()) {
        return Some("cloud metadata");
    }
    const RANGES: [([u8; 4], u32, &str); 15] = [
        ([0, 0, 0, 0], 8, "unspecified"),
        ([10, 0, 0, 0], 8, "private"),
        ([100, 64, 0, 0], 10, "shared (CGNAT)"),
        ([127, 0, 0, 0], 8, "loopback"),
        ([169, 254, 0, 0], 16, "link-local"),
        ([172, 16, 0, 0], 12, "private"),
        ([192, 0, 0, 0], 24, "protocol assignment"),
        ([192, 0, 2, 0], 24, "documentation"),
        ([192, 88, 99, 0], 24, "6to4 relay"),
        ([192, 168, 0, 0], 16, "private"),
        ([198, 18, 0, 0], 15, "benchmarking"),
        ([198, 51, 100, 0], 24, "documentation"),
        ([203, 0, 113, 0], 24, "documentation"),
        ([224, 0, 0, 0], 4, "multicast"),
        ([240, 0, 0, 0], 4, "reserved or broadcast"),
    ];
    let number = u32::from(address);
    RANGES.iter().find_map(|(base, bits, why)| {
        let mask = u32::MAX << (32 - bits);
        (number & mask == u32::from(Ipv4Addr::from(*base)) & mask).then_some(*why)
    })
}

/// The IPv4 address in the last 32 bits of an IPv6 address.
fn embedded_v4(segments: &[u16; 8]) -> Ipv4Addr {
    Ipv4Addr::new(
        (segments[6] >> 8) as u8,
        (segments[6] & 0xff) as u8,
        (segments[7] >> 8) as u8,
        (segments[7] & 0xff) as u8,
    )
}

/// Why an IPv6 address is refused, or `None` when it is a public address.
pub fn v6_refusal(address: Ipv6Addr) -> Option<&'static str> {
    let s = address.segments();
    if address.is_unspecified() {
        return Some("unspecified");
    }
    if address.is_loopback() {
        return Some("loopback");
    }
    // ::ffff:a.b.c.d, IPv4-mapped: judged as the IPv4 address it carries.
    if s[..5] == [0; 5] && s[5] == 0xffff {
        return v4_refusal(embedded_v4(&s));
    }
    // ::a.b.c.d (IPv4-compatible, deprecated) and ::ffff:0:a.b.c.d (IPv4-translated).
    if s[..6] == [0; 6] || (s[..4] == [0; 4] && s[4] == 0xffff && s[5] == 0) {
        return Some("IPv4-compatible");
    }
    // 64:ff9b::/96, the NAT64 well-known prefix: judged as the IPv4 address it carries.
    if s[0] == 0x64 && s[1] == 0xff9b && s[2..6] == [0; 4] {
        return v4_refusal(embedded_v4(&s));
    }
    // 64:ff9b:1::/48, local-use NAT64: what it reaches is the operator's own network.
    if s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1 {
        return Some("local NAT64");
    }
    // fd00:ec2::254, AWS's metadata service over IPv6 (also inside unique-local below).
    if s == [0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x254] {
        return Some("cloud metadata");
    }
    let first = s[0];
    if first & 0xfe00 == 0xfc00 {
        return Some("unique-local");
    }
    if first & 0xffc0 == 0xfe80 {
        return Some("link-local");
    }
    if first & 0xffc0 == 0xfec0 {
        return Some("site-local");
    }
    if first & 0xff00 == 0xff00 {
        return Some("multicast");
    }
    // 2001::/23: IETF protocol assignments, with Teredo (2001::/32), benchmarking and ORCHID.
    if first == 0x2001 && s[1] < 0x0200 {
        return Some("protocol assignment");
    }
    if first == 0x2001 && s[1] == 0x0db8 {
        return Some("documentation");
    }
    if first == 0x2002 {
        return Some("6to4");
    }
    // 3fff::/20, documentation (RFC 9637): 3fff:0000:: to 3fff:0fff::.
    if first == 0x3fff && s[1] < 0x1000 {
        return Some("documentation");
    }
    if first & 0xe000 != 0x2000 {
        return Some("not global unicast");
    }
    None
}

/// Why an address is refused under the outbound rule, or `None` when a tenant may reach it.
pub fn refusal(address: IpAddr) -> Option<&'static str> {
    match address {
        IpAddr::V4(v4) => v4_refusal(v4),
        IpAddr::V6(v6) => v6_refusal(v6),
    }
}

/// The host of a judged URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Host {
    /// A numeric address: connected to as it is.
    Ip(IpAddr),
    /// A name, lower-case and in its ASCII (IDNA) form, as the URL parser left it: resolved by
    /// the gate, once.
    Name(String),
}

/// A URL that passed the rule's checks of its form, with its host classified.
#[derive(Clone, Debug)]
pub struct Target {
    pub url: reqwest::Url,
    pub host: Host,
}

/// Why a URL is refused before anything is resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlRefusal(pub String);

/// Whether a name only ever means a host of the platform's own network.
fn internal_name(name: &str) -> bool {
    let bare = name.trim_end_matches('.');
    !bare.contains('.')
        || bare == "localhost"
        || [
            ".localhost",
            ".local",
            ".internal",
            ".arpa",
            ".lan",
            ".home",
        ]
        .iter()
        .any(|suffix| bare.ends_with(suffix))
}

/// Parse and judge the form of an address (WFL-1579): HTTPS, port 443, no user name or password,
/// no fragment, and a host that is a public name or a public number. `development` is the
/// development exception: plain HTTP, any port and any host are allowed.
pub fn parse_target(text: &str, development: bool) -> Result<Target, UrlRefusal> {
    let refuse = |why: &str| Err(UrlRefusal(why.to_string()));
    if text.len() > MAX_URL_LEN {
        return refuse("the address is too long");
    }
    let Ok(url) = reqwest::Url::parse(text.trim()) else {
        return refuse("the address cannot be read");
    };
    let scheme_ok = url.scheme() == "https" || (development && url.scheme() == "http");
    if !scheme_ok {
        return refuse("only https is allowed");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return refuse("an address may not carry a user name or a password");
    }
    if url.fragment().is_some() {
        return refuse("an address may not carry a fragment");
    }
    if !development && url.port_or_known_default() != Some(443) {
        return refuse("only port 443 is allowed");
    }
    let Some(host) = url.host_str().map(str::to_string) else {
        return refuse("the address has no host");
    };
    // The parser has already written every numeric spelling in its canonical form: an IPv4
    // address as four decimal numbers, an IPv6 address in brackets.
    let classified = if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        match inner.parse::<Ipv6Addr>() {
            Ok(v6) => Host::Ip(IpAddr::V6(v6)),
            Err(_) => return refuse("the address cannot be read"),
        }
    } else if let Ok(v4) = host.parse::<Ipv4Addr>() {
        Host::Ip(IpAddr::V4(v4))
    } else {
        if host.is_empty() || host.len() > 253 {
            return refuse("the host name is not valid");
        }
        if !development && internal_name(&host) {
            return refuse("the host name is one of the platform's own network");
        }
        Host::Name(host)
    };
    if let (Host::Ip(address), false) = (&classified, development) {
        if let Some(why) = refusal(*address) {
            return Err(UrlRefusal(format!("a {why} address is not allowed")));
        }
    }
    Ok(Target {
        url,
        host: classified,
    })
}

/// The address as a log line may show it (WFL-254): scheme, host, port and path, with no query,
/// no user name and no password, the path cut at 200 characters.
pub fn redacted(url: &reqwest::Url) -> String {
    let mut path: String = url.path().chars().take(200).collect();
    if url.path().chars().count() > 200 {
        path.push('…');
    }
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    format!(
        "{}://{}{port}{path}",
        url.scheme(),
        url.host_str().unwrap_or("")
    )
}

/// The same, for an address given as text; one that cannot be read is shown as `(unreadable)`.
pub fn redacted_text(text: &str) -> String {
    reqwest::Url::parse(text)
        .map(|url| redacted(&url))
        .unwrap_or_else(|_| "(unreadable)".to_string())
}
