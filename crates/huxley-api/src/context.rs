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
        std::future::ready(Ok(Self(extract(parts, state.config.trust_forwarded_for))))
    }
}

fn header<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts.headers.get(name).and_then(|v| v.to_str().ok())
}

fn extract(parts: &Parts, trust_forwarded_for: bool) -> RequestContext {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    let forwarded = trust_forwarded_for
        .then(|| header(parts, "x-forwarded-for"))
        .flatten()
        .and_then(rightmost_ip);
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

/// The address added by the nearest proxy. Earlier entries are supplied by the
/// client and can be forged, so only the rightmost one is trusted.
fn rightmost_ip(value: &str) -> Option<IpAddr> {
    value.rsplit(',').next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_rightmost_forwarded_address() {
        assert_eq!(
            rightmost_ip("203.0.113.9, 10.0.0.1"),
            Some("10.0.0.1".parse().unwrap())
        );
        assert_eq!(
            rightmost_ip(" 2001:db8::1 "),
            Some("2001:db8::1".parse().unwrap())
        );
        assert_eq!(rightmost_ip("not-an-ip"), None);
    }

    #[test]
    fn ignores_forwarded_header_unless_trusted() {
        let (mut parts, ()) = http::Request::builder()
            .header("x-forwarded-for", "198.51.100.7")
            .body(())
            .unwrap()
            .into_parts();
        parts
            .extensions
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9))));
        assert_eq!(
            extract(&parts, false).ip,
            Some("127.0.0.1".parse().unwrap())
        );
        assert_eq!(
            extract(&parts, true).ip,
            Some("198.51.100.7".parse().unwrap())
        );
    }
}
