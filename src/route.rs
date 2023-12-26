use std::{
    convert::Infallible,
    net::{AddrParseError, IpAddr, SocketAddr},
    str::FromStr,
    time::{Duration, Instant},
};

use bytes::{Buf, Bytes};
use futures::Stream;
use headers::{Header, HeaderMapExt, HeaderValue};
use http::{
    header::{AsHeaderName, HeaderName, ToStrError},
    method::InvalidMethod,
    request::Parts,
    uri::Authority,
    HeaderMap, Method,
};
use http_body_util::{BodyStream, Collected};
use hyper::{
    body::{Body, Frame, Incoming},
    Request, Response,
};

pub struct Route<S> {
    pub addr: SocketAddr,
    pub real_addr: SocketAddr,
    pub head: Parts,
    pub body: Option<Incoming>,
    pub state: S,
    pub segment_index: usize,
    pub next_segment_index: usize,
    pub start: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Segment<'a> {
    Exact(&'a str),
    End,
}

impl<S> Route<S> {
    #[inline]
    pub fn new(addr: SocketAddr, req: Request<Incoming>, state: S) -> Route<S> {
        let (head, body) = req.into_parts();

        let mut route = Route {
            start: Instant::now(),
            addr,
            real_addr: addr,
            head,
            body: Some(body), // once told me
            state,
            segment_index: 0,
            next_segment_index: 0,
        };

        if let Ok(addr) = crate::real_ip::get_real_ip(&route) {
            route.real_addr.set_ip(addr);
        }

        route
    }

    #[inline]
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Use this at the start of a Route to override the provided HTTP Method with the value present in
    /// the `x-http-method-override` HTTP header.
    ///
    /// This is sometimes used in old browsers without support for PATCH or OPTIONS methods.
    pub fn apply_method_override(&mut self) -> Result<(), InvalidMethod> {
        if let Some(method_override) = self.raw_header(HeaderName::from_static("x-http-method-override")) {
            self.head.method = Method::from_bytes(method_override.as_bytes())?;
        }

        Ok(())
    }

    /// Attempt to discern the current host authority either by
    /// looking at the input URI or the `HOST` HTTP Header.
    ///
    /// If no consistent host authority can be found, `None` is returned.
    pub fn host(&self) -> Option<Authority> {
        let from_uri = self.head.uri.authority();

        let from_header = match self.parse_raw_header::<Authority, _>(HeaderName::from_static("host")) {
            Some(Ok(Ok(host))) => Some(host),
            _ => None,
        };

        match (from_uri, from_header) {
            (None, None) => None,
            (Some(a), None) => Some(a.clone()),
            (None, Some(b)) => Some(b),
            (Some(a), Some(b)) if *a == b => Some(b),
            _ => None,
        }
    }

    /// Parse the URI query
    pub fn query<T: serde::de::DeserializeOwned>(&self) -> Option<Result<T, serde_urlencoded::de::Error>> {
        self.head.uri.query().map(serde_urlencoded::de::from_str)
    }

    #[inline]
    pub fn raw_query(&self) -> Option<&str> {
        self.head.uri.query()
    }

    #[inline]
    pub fn path(&self) -> &str {
        self.head.uri.path()
    }

    /// Returns the remaining parts of the URI path **After** the current segment.
    #[inline]
    pub fn tail(&self) -> &str {
        &self.path()[self.next_segment_index..]
    }

    /// Parses a URI as an arbitrary parameter using `FromStr`
    pub fn param<P: FromStr>(&self) -> Option<Result<P, P::Err>> {
        match self.segment() {
            Segment::Exact(segment) => Some(segment.parse()),
            Segment::End => None,
        }
    }

    /// Parses a URI as an arbitrary parameter using `FromStr`, but with a length limit
    pub fn param_n<P: FromStr>(&self, limit: usize) -> Option<Result<P, P::Err>> {
        match self.segment() {
            Segment::Exact(segment) if segment.len() <= limit => Some(segment.parse()),
            _ => None,
        }
    }

    /// Returns both the `Method` and URI `Segment` a the same time for convenient `match` statements.
    pub fn method_segment(&self) -> (&Method, Segment) {
        let path = self.path();
        let method = self.method();

        let segment = if self.segment_index == path.len() {
            Segment::End
        } else {
            Segment::Exact(&path[self.segment_index..self.next_segment_index])
        };

        (method, segment)
    }

    #[inline]
    pub fn segment(&self) -> Segment {
        self.method_segment().1
    }

    #[inline]
    pub fn method(&self) -> &Method {
        &self.head.method
    }

    pub fn headers(&self) -> &HeaderMap<HeaderValue> {
        &self.head.headers
    }

    #[inline]
    pub fn header<H: Header>(&self) -> Option<H> {
        self.head.headers.typed_get()
    }

    #[inline]
    pub fn raw_header<K: AsHeaderName>(&self, name: K) -> Option<&HeaderValue> {
        self.head.headers.get(name)
    }

    /// Parse a header value using `FromStr`
    #[inline]
    pub fn parse_raw_header<T: FromStr, K: AsHeaderName>(
        &self,
        name: K,
    ) -> Option<Result<Result<T, T::Err>, ToStrError>> {
        self.raw_header(name)
            .map(|header| header.to_str().map(FromStr::from_str))
    }

    /// Parses the `Content-Length` header and returns the value as a `u64`,
    /// or `None` if there was not a set content length
    #[inline]
    pub fn content_length(&self) -> Option<u64> {
        self.header::<headers::ContentLength>().map(|cl| cl.0)
    }

    /// Parses the proxy chain in the `x-forwarded-for` HTTP header.
    pub fn forwarded_for(
        &self,
    ) -> Option<Result<impl Iterator<Item = Result<IpAddr, AddrParseError>> + '_, ToStrError>> {
        self.raw_header(HeaderName::from_static("x-forwarded-for"))
            .map(|ff| {
                ff.to_str()
                    .map(|ff| ff.split(',').map(|segment| IpAddr::from_str(segment.trim())))
            })
    }

    /// Finds the next segment in the URI path, storing the result internally for further usage.
    ///
    /// Use [`.segment()`], [`.method_segment()`] or [`param`] to parse the segment found (if any)
    pub fn next_mut(&mut self) -> &mut Self {
        self.segment_index = self.next_segment_index;

        let path = self.head.uri.path(); // avoid whole self lifetime

        // already at end, nothing to do
        if self.segment_index == path.len() {
            return self;
        }

        // skip leading slash
        if path.as_bytes()[self.segment_index] == b'/' {
            self.segment_index += 1;
        }

        let segment = path[self.segment_index..]
            .split('/') // split the next segment
            .next() // only take the first
            .expect("split always has at least 1");

        self.next_segment_index = self.segment_index + segment.len();

        self
    }

    /// Same as [`.next_mut()`] but without the `mut` borrow of Self
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> &Self {
        self.next_mut()
    }

    pub fn body(&self) -> Option<&Incoming> {
        self.body.as_ref()
    }

    /// Takes ownership of the request body, returning `None` if it was already consumed.
    pub fn take_body(&mut self) -> Option<Incoming> {
        self.body.take()
    }

    /// Combines the body together into a buffered but chunked set of buffers
    ///
    /// Use `aggregate` or `to_bytes` on `Collected<Bytes>` for your use cases.
    pub async fn collect(&mut self) -> Result<Collected<Bytes>, BodyError> {
        use http_body_util::BodyExt;

        Ok(match self.take_body() {
            Some(body) => body.collect().await?,
            None => return Err(BodyError::DoubleUseError),
        })
    }

    pub fn stream(&mut self) -> Result<BodyStream<Incoming>, BodyError> {
        Ok(match self.take_body() {
            Some(body) => BodyStream::new(body),
            None => return Err(BodyError::DoubleUseError),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    #[error("Body cannot be used twice")]
    DoubleUseError,

    #[error("Error aggregating: {0}")]
    AggregateError(#[from] hyper::Error),
}
