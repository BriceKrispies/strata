//! Who a request comes from, and how many sockets one session or source address may hold.
//!
//! The source of a request is the address of the TCP peer, unless the peer is a configured
//! trusted proxy: then it is the client the proxy reports in `X-Forwarded-For`, found by walking
//! the chain from the right and skipping every hop that is itself a trusted proxy. The header is
//! never honoured from a peer that is not trusted, so a visitor cannot choose its own source.

use std::net::IpAddr;
use strata_runtime::limits::{Cap, Permit};

/// What the BFF allows one session or source to hold open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Open sockets one session may hold (`STRATA_BFF_SOCKETS_PER_SESSION`).
    pub sockets_per_session: usize,
    /// Open sockets one source address may hold, signed in or not (`STRATA_BFF_SOCKETS_PER_SOURCE`).
    pub sockets_per_source: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            sockets_per_session: 8,
            sockets_per_source: 32,
        }
    }
}

/// The places a browser socket holds while it is open; dropping it frees them.
pub struct Places {
    _session: Option<Permit>,
    _source: Option<Permit>,
}

pub struct Admission {
    by_session: Cap,
    by_source: Cap,
}

impl Admission {
    pub fn new(limits: &Limits) -> Self {
        Admission {
            by_session: Cap::new(limits.sockets_per_session),
            by_source: Cap::new(limits.sockets_per_source),
        }
    }

    /// Places for a new socket of `session` (when there is one) from `source`; `Err` carries what
    /// to tell the browser.
    pub fn socket(
        &self,
        session: Option<&str>,
        source: Option<&str>,
    ) -> Result<Places, &'static str> {
        let by_session = match session {
            Some(session) => Some(
                self.by_session
                    .acquire(session)
                    .ok_or("This session already has as many connections open as it may.")?,
            ),
            None => None,
        };
        let by_source = match source {
            Some(source) => Some(
                self.by_source
                    .acquire(source)
                    .ok_or("Too many connections are open from this address.")?,
            ),
            None => None,
        };
        Ok(Places {
            _session: by_session,
            _source: by_source,
        })
    }
}

/// The address a request is attributed to.
///
/// `peer` is the TCP peer. `forwarded` is every `X-Forwarded-For` value of the request, joined
/// with commas. `trusted` is the configured list of proxy addresses (exact addresses; an
/// IPv4-mapped IPv6 peer is compared as its IPv4 address).
///
/// - A peer that is not trusted is the source, and `forwarded` is ignored.
/// - A trusted peer is a proxy that appended the address it received the connection from, so the
///   chain is read from the right. Each hop that is itself trusted is another proxy and is
///   skipped; the first hop that is not is the client.
/// - A chain that cannot be used (a hop that is not an address, or only trusted hops) attributes
///   the request to the proxy itself: a shared bucket, never one the visitor chose.
pub fn client_address(peer: IpAddr, forwarded: Option<&str>, trusted: &[IpAddr]) -> IpAddr {
    let peer = peer.to_canonical();
    let is_trusted = |address: &IpAddr| trusted.iter().any(|t| t.to_canonical() == *address);
    if !is_trusted(&peer) {
        return peer;
    }
    let Some(forwarded) = forwarded else {
        return peer;
    };
    for hop in forwarded.rsplit(',') {
        let Ok(hop) = hop.trim().parse::<IpAddr>() else {
            return peer;
        };
        let hop = hop.to_canonical();
        if !is_trusted(&hop) {
            return hop;
        }
    }
    peer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn the_header_is_never_honoured_from_a_peer_that_is_not_trusted() {
        let trusted = [ip("10.0.0.1")];
        assert_eq!(
            client_address(ip("198.51.100.7"), Some("203.0.113.9"), &trusted),
            ip("198.51.100.7")
        );
        assert_eq!(
            client_address(ip("198.51.100.7"), Some("10.0.0.1, 203.0.113.9"), &[]),
            ip("198.51.100.7"),
            "with no trusted proxies configured"
        );
    }

    #[test]
    fn a_trusted_proxy_reports_the_client_and_a_chain_is_read_from_the_right() {
        let trusted = [ip("10.0.0.1"), ip("10.0.0.2")];
        assert_eq!(
            client_address(ip("10.0.0.1"), Some("203.0.113.9"), &trusted),
            ip("203.0.113.9")
        );
        // The client wrote the leftmost hop itself; the proxies appended the rest.
        assert_eq!(
            client_address(
                ip("10.0.0.1"),
                Some("1.2.3.4, 203.0.113.9, 10.0.0.2"),
                &trusted
            ),
            ip("203.0.113.9"),
            "the rightmost hop that is not a trusted proxy"
        );
        assert_eq!(
            client_address(ip("::ffff:10.0.0.1"), Some("203.0.113.9"), &trusted),
            ip("203.0.113.9"),
            "an IPv4-mapped peer"
        );
    }

    #[test]
    fn a_chain_that_cannot_be_used_is_attributed_to_the_proxy() {
        let trusted = [ip("10.0.0.1")];
        for chain in [None, Some(""), Some("not-an-address"), Some("10.0.0.1")] {
            assert_eq!(
                client_address(ip("10.0.0.1"), chain, &trusted),
                ip("10.0.0.1")
            );
        }
        assert_eq!(
            client_address(ip("10.0.0.1"), Some("garbage, 10.0.0.1"), &trusted),
            ip("10.0.0.1"),
            "a hop left of the proxies' own that is not an address ends the walk"
        );
    }

    #[test]
    fn sockets_are_capped_per_session_and_per_source() {
        let admission = Admission::new(&Limits {
            sockets_per_session: 1,
            sockets_per_source: 2,
        });
        let first = admission.socket(Some("s1"), Some("a")).unwrap();
        assert!(admission.socket(Some("s1"), Some("b")).is_err());
        let _second = admission.socket(Some("s2"), Some("a")).unwrap();
        assert!(admission.socket(None, Some("a")).is_err());
        drop(first);
        assert!(admission.socket(Some("s1"), Some("a")).is_ok());
    }
}
