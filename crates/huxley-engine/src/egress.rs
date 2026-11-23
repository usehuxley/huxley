//! Outbound HTTP for workflow steps, with an egress policy against SSRF.
//!
//! Workflows are written by users and run inside the network Huxley is
//! deployed in, so by default a step may only reach public addresses:
//!
//! - Host names are resolved by Huxley itself and every address is checked
//!   before connecting, and the connection goes to a checked address, so a
//!   name that later resolves elsewhere (DNS rebinding) gets nowhere.
//! - IP literals in the URL and in every redirect are checked the same way.
//! - Link-local addresses (cloud metadata endpoints such as 169.254.169.254)
//!   are never reachable, even when private addresses are allowed.
//! - Proxies from the environment are ignored (they would bypass the check).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde::Deserialize;

/// Egress settings (`[engine.egress]`, or `HUXLEY_ENGINE__EGRESS__*`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EgressConfig {
    /// Let steps reach private and loopback addresses (an internal network).
    /// Link-local addresses stay blocked.
    pub allow_private: bool,
    /// Host names that may resolve to private addresses even when
    /// `allow_private` is off: `api.internal` or `*.corp.example`.
    pub private_hosts: Vec<String>,
    /// Host names steps may never reach (same patterns).
    pub denied_hosts: Vec<String>,
    /// Largest response body a step reads.
    pub max_response_bytes: usize,
    pub max_redirects: usize,
}

impl Default for EgressConfig {
    fn default() -> Self {
        Self {
            allow_private: false,
            private_hosts: Vec::new(),
            denied_hosts: Vec::new(),
            max_response_bytes: 1024 * 1024,
            max_redirects: 5,
        }
    }
}

/// Why a request was refused.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Blocked(pub String);

fn matches(pattern: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let pattern = pattern.trim().to_ascii_lowercase();
    match pattern.strip_prefix("*.") {
        Some(suffix) => host.ends_with(&format!(".{suffix}")),
        None => host == pattern,
    }
}

/// What an address is, for the policy.
#[derive(Debug, PartialEq, Eq)]
enum Class {
    Public,
    /// Loopback, RFC 1918, unique-local, carrier-grade NAT.
    Private,
    /// Link-local (metadata endpoints), unspecified, multicast, broadcast,
    /// reserved: never reachable.
    Forbidden,
}

fn classify(ip: IpAddr) -> Class {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => {
            // IPv4-mapped/compatible and NAT64 addresses carry an IPv4 one.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return classify_v4(v4);
            }
            let segments = v6.segments();
            if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let [a, b] = segments[6].to_be_bytes();
                let [c, d] = segments[7].to_be_bytes();
                return classify_v4(Ipv4Addr::new(a, b, c, d));
            }
            classify_v6(v6)
        }
    }
}

fn classify_v4(ip: Ipv4Addr) -> Class {
    let [a, b, ..] = ip.octets();
    if ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
    {
        Class::Forbidden
    } else if ip.is_loopback() || ip.is_private() || (a == 100 && (64..128).contains(&b)) {
        Class::Private
    } else {
        Class::Public
    }
}

fn classify_v6(ip: Ipv6Addr) -> Class {
    let first = ip.segments()[0];
    if ip.is_unspecified() || ip.is_multicast() || (first & 0xffc0) == 0xfe80 {
        Class::Forbidden
    } else if ip.is_loopback() || (first & 0xfe00) == 0xfc00 {
        Class::Private
    } else if first == 0x2001 && ip.segments()[1] == 0x0db8 {
        Class::Forbidden // documentation
    } else {
        Class::Public
    }
}

#[derive(Debug)]
pub(crate) struct Policy {
    config: EgressConfig,
}

impl Policy {
    fn denied(&self, host: &str) -> bool {
        self.config.denied_hosts.iter().any(|p| matches(p, host))
    }

    fn allows(&self, host: &str, ip: IpAddr) -> bool {
        match classify(ip) {
            Class::Public => true,
            Class::Private => {
                self.config.allow_private
                    || self.config.private_hosts.iter().any(|p| matches(p, host))
            }
            Class::Forbidden => false,
        }
    }

    /// Checks a URL's scheme, host name and (for IP literals) address.
    pub(crate) fn check_url(&self, url: &reqwest::Url) -> Result<(), Blocked> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Blocked(format!(
                "{}: only http and https URLs",
                url.scheme()
            )));
        }
        let Some(host) = url.host_str() else {
            return Err(Blocked("the URL has no host".into()));
        };
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        if self.denied(bare) {
            return Err(Blocked(format!("{bare} is on the denied list")));
        }
        if let Ok(ip) = bare.parse::<IpAddr>()
            && !self.allows(bare, ip)
        {
            return Err(Blocked(format!("{ip} isn't a public address")));
        }
        Ok(())
    }
}

/// Resolves names and drops addresses the policy doesn't allow.
struct GuardedResolver {
    policy: Arc<Policy>,
}

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            if policy.denied(&host) {
                return Err(Box::new(Blocked(format!("{host} is on the denied list"))) as _);
            }
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let allowed: Vec<SocketAddr> = addrs
                .iter()
                .copied()
                .filter(|a| policy.allows(&host, a.ip()))
                .collect();
            if allowed.is_empty() {
                let shown: Vec<String> = addrs.iter().map(|a| a.ip().to_string()).collect();
                return Err(Box::new(Blocked(format!(
                    "{host} resolves to {}, which isn't a public address",
                    shown.join(", ")
                ))) as _);
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

/// The HTTP client steps use, and the policy it enforces.
#[derive(Debug, Clone)]
pub(crate) struct Egress {
    pub(crate) client: reqwest::Client,
    pub(crate) policy: Arc<Policy>,
}

impl Egress {
    pub(crate) fn new(config: &EgressConfig) -> Self {
        let policy = Arc::new(Policy {
            config: config.clone(),
        });
        let redirect_policy = policy.clone();
        let max_redirects = config.max_redirects;
        let client = reqwest::Client::builder()
            .dns_resolver(Arc::new(GuardedResolver {
                policy: policy.clone(),
            }))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= max_redirects {
                    return attempt.error(Blocked(format!("more than {max_redirects} redirects")));
                }
                match redirect_policy.check_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(blocked) => attempt.error(blocked),
                }
            }))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("Huxley/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("the HTTP client builds");
        Self { client, policy }
    }

    pub(crate) fn max_response_bytes(&self) -> usize {
        self.policy.config.max_response_bytes
    }
}

/// Whether an error (or anything it wraps) was the egress policy.
pub(crate) fn blocked_reason(error: &(dyn std::error::Error + 'static)) -> Option<String> {
    let mut current = Some(error);
    while let Some(e) = current {
        if let Some(blocked) = e.downcast_ref::<Blocked>() {
            return Some(blocked.0.clone());
        }
        current = e.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(config: EgressConfig) -> Policy {
        Policy { config }
    }

    #[test]
    fn classifies_addresses() {
        for (ip, class) in [
            ("8.8.8.8", Class::Public),
            ("2606:4700::1111", Class::Public),
            ("10.1.2.3", Class::Private),
            ("172.16.0.1", Class::Private),
            ("192.168.1.1", Class::Private),
            ("127.0.0.1", Class::Private),
            ("100.64.0.1", Class::Private),
            ("::1", Class::Private),
            ("fd00::1", Class::Private),
            ("169.254.169.254", Class::Forbidden),
            ("fe80::1", Class::Forbidden),
            ("0.0.0.0", Class::Forbidden),
            ("255.255.255.255", Class::Forbidden),
            ("224.0.0.1", Class::Forbidden),
            ("::ffff:169.254.169.254", Class::Forbidden),
            ("::ffff:10.0.0.1", Class::Private),
            ("64:ff9b::a9fe:a9fe", Class::Forbidden),
        ] {
            assert_eq!(classify(ip.parse().unwrap()), class, "{ip}");
        }
    }

    #[test]
    fn private_addresses_need_permission_and_metadata_never_passes() {
        let strict = policy(EgressConfig::default());
        let private: IpAddr = "10.0.0.5".parse().unwrap();
        let metadata: IpAddr = "169.254.169.254".parse().unwrap();
        assert!(!strict.allows("x", private));
        let open = policy(EgressConfig {
            allow_private: true,
            ..EgressConfig::default()
        });
        assert!(open.allows("x", private));
        assert!(!open.allows("x", metadata));
        let listed = policy(EgressConfig {
            private_hosts: vec!["*.corp.example".into(), "api.internal".into()],
            ..EgressConfig::default()
        });
        assert!(listed.allows("hr.corp.example", private));
        assert!(listed.allows("API.internal.", private));
        assert!(!listed.allows("corp.example", private));
        assert!(!listed.allows("evil.example", private));
    }

    #[test]
    fn urls_are_checked() {
        let strict = policy(EgressConfig {
            denied_hosts: vec!["*.blocked.example".into()],
            ..EgressConfig::default()
        });
        let check = |u: &str| strict.check_url(&u.parse().unwrap());
        assert!(check("https://example.com/x").is_ok());
        assert!(check("ftp://example.com/").is_err());
        assert!(check("file:///etc/passwd").is_err());
        assert!(check("http://127.0.0.1:8080/").is_err());
        assert!(check("http://[::1]/").is_err());
        assert!(check("http://169.254.169.254/latest/meta-data").is_err());
        assert!(check("https://a.blocked.example/").is_err());
    }
}
