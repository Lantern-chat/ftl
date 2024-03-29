use std::net::{AddrParseError, IpAddr};
use std::str::FromStr;

use http::header::{HeaderName, ToStrError};

use super::*;

#[derive(Debug, thiserror::Error)]
pub enum GetRealIpError {
    #[error("Missing IP headers or information")]
    MissingAddress,

    #[error(transparent)]
    ToStrError(#[from] ToStrError),

    #[error(transparent)]
    AddrParseError(#[from] AddrParseError),
}

/// Attempts to get the real IP address of the client using a variety of headers.
pub fn get_real_ip<S>(route: &Route<S>) -> Result<IpAddr, GetRealIpError> {
    let headers = route.headers();

    if let Some(Ok(mut proxies)) = route.forwarded_for() {
        // NOTE: If it cannot be parsed, it will be ignored.
        if let Some(Ok(first)) = proxies.next() {
            return Ok(first);
        }
    }

    static HEADERS: [HeaderName; 4] = [
        HeaderName::from_static("x-real-ip"),
        HeaderName::from_static("client-ip"), // used by some load balancers
        HeaderName::from_static("x-cluster-client-ip"), // used by AWS sometimes
        HeaderName::from_static("cf-connecting-ip"), // used by Cloudflare sometimes
    ];

    for header in &HEADERS {
        if let Some(x_forwarded_for) = headers.get(header) {
            return Ok(IpAddr::from_str(x_forwarded_for.to_str()?.trim())?);
        }
    }

    Ok(route.addr.ip())
}
