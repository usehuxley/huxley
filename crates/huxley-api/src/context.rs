//! Request metadata: client IP, user agent, request id.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use http::request::Parts;
use huxley_core::{Actor, RequestContext, Via};

use crate::AppState;

/// Request metadata for audit records, before authentication.
#[derive(Debug, Clone)]
pub struct ClientInfo(pub RequestContext);

impl FromRequestParts<AppState> for ClientInfo {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(Ok(Self(extract(parts, &state.config))))
    }
}

fn header<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts.headers.get(name).and_then(|v| v.to_str().ok())
}

fn extract(parts: &Parts, config: &crate::ApiConfig) -> RequestContext {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    let forwarded = forwarded_client(parts, config);
    RequestContext {
        actor: Actor::Anonymous,
        // Tool calls of the MCP server come through the API, marked.
        via: if parts.extensions.get::<crate::mcp::McpCall>().is_some() {
            Via::Mcp
        } else {
            Via::Api
        },
        ip: forwarded.or(peer),
        user_agent: header(parts, "user-agent").map(str::to_owned),
        request_id: header(parts, "x-request-id").map(str::to_owned),
    }
}

/// The client's address as the proxies in front of Huxley report it
/// (ADR-090), or `None` when they aren't trusted or say nothing usable.
fn forwarded_client(parts: &Parts, config: &crate::ApiConfig) -> Option<IpAddr> {
    if !config.trust_forwarded_for {
        return None;
    }
    if let Some(ip) = config
        .client_ip_header
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .and_then(|name| header(parts, name.trim()))
        .and_then(|v| v.trim().parse().ok())
    {
        return Some(ip);
    }
    nth_from_right(header(parts, "x-forwarded-for")?, config.proxy_hops)
}

/// The entry `hops` from the right of an `X-Forwarded-For` value: each
/// trusted proxy appends the address it got the request from, so earlier
/// entries are whatever the client claimed. With fewer entries than hops
/// (the web apps send just the client), the leftmost.
fn nth_from_right(value: &str, hops: usize) -> Option<IpAddr> {
    let entries: Vec<&str> = value
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect();
    let index = entries.len().saturating_sub(hops.max(1));
    entries.get(index)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApiConfig;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn request(headers: &[(&str, &str)]) -> Parts {
        let mut builder = http::Request::builder();
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        let (mut parts, ()) = builder.body(()).unwrap().into_parts();
        parts
            .extensions
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9))));
        parts
    }

    fn trusting(hops: usize, header: Option<&str>) -> ApiConfig {
        ApiConfig {
            trust_forwarded_for: true,
            proxy_hops: hops,
            client_ip_header: header.map(str::to_owned),
            ..ApiConfig::default()
        }
    }

    #[test]
    fn takes_the_entry_as_many_hops_from_the_right() {
        assert_eq!(
            nth_from_right("203.0.113.9, 10.0.0.1", 1),
            Some(ip("10.0.0.1"))
        );
        // Client, CDN edge (added by the ingress): the client is two from the right.
        assert_eq!(
            nth_from_right("forged, 203.0.113.9, 198.51.100.4", 2),
            Some(ip("203.0.113.9"))
        );
        // The web apps send just the client.
        assert_eq!(nth_from_right("203.0.113.9", 2), Some(ip("203.0.113.9")));
        assert_eq!(nth_from_right(" 2001:db8::1 ", 1), Some(ip("2001:db8::1")));
        assert_eq!(nth_from_right("not-an-ip", 1), None);
    }

    #[test]
    fn ignores_forwarded_header_unless_trusted() {
        let parts = request(&[("x-forwarded-for", "198.51.100.7")]);
        assert_eq!(
            extract(&parts, &ApiConfig::default()).ip,
            Some(ip("127.0.0.1"))
        );
        assert_eq!(
            extract(&parts, &trusting(1, None)).ip,
            Some(ip("198.51.100.7"))
        );
    }

    #[test]
    fn a_cdn_header_comes_first_and_falls_back_to_forwarded_for() {
        let config = trusting(1, Some("CF-Connecting-IP"));
        let parts = request(&[
            ("cf-connecting-ip", "203.0.113.9"),
            ("x-forwarded-for", "203.0.113.9, 198.51.100.4"),
        ]);
        assert_eq!(extract(&parts, &config).ip, Some(ip("203.0.113.9")));
        // From the web app: no CDN header, the client it resolved.
        let parts = request(&[("x-forwarded-for", "203.0.113.9")]);
        assert_eq!(extract(&parts, &config).ip, Some(ip("203.0.113.9")));
        // Untrusted, the header is ignored.
        let parts = request(&[("cf-connecting-ip", "203.0.113.9")]);
        let untrusted = ApiConfig {
            client_ip_header: Some("CF-Connecting-IP".into()),
            ..ApiConfig::default()
        };
        assert_eq!(extract(&parts, &untrusted).ip, Some(ip("127.0.0.1")));
    }
}
