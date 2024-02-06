use crate::{Body, Response};

use std::borrow::Borrow;

use bytes::Bytes;
use headers::{ContentType, Header, HeaderMapExt};
use http::StatusCode;

pub mod deferred;

#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "json")]
pub use json::{json, Json};

// #[cfg(feature = "msgpack")]
// pub mod msgpack;

#[cfg(feature = "cbor")]
pub mod cbor;

pub trait Reply: Sized {
    fn into_response(self) -> Response;

    #[inline]
    fn with_status(self, status: StatusCode) -> WithStatus<Self> {
        with_status(self, status)
    }

    #[inline]
    fn with_header<H>(self, header: H) -> WithHeader<Self, H>
    where
        H: Header,
    {
        WithHeader { reply: self, header }
    }
}

#[cfg(feature = "either")]
impl<L, R> Reply for either::Either<L, R>
where
    L: Reply,
    R: Reply,
{
    fn into_response(self) -> Response {
        match self {
            either::Either::Left(l) => l.into_response(),
            either::Either::Right(r) => r.into_response(),
        }
    }
}

impl<L, R> Reply for futures::future::Either<L, R>
where
    L: Reply,
    R: Reply,
{
    fn into_response(self) -> Response {
        match self {
            futures::future::Either::Left(l) => l.into_response(),
            futures::future::Either::Right(r) => r.into_response(),
        }
    }
}

pub fn reply() -> impl Reply {
    StatusCode::OK
}

impl Reply for () {
    fn into_response(self) -> Response {
        StatusCode::OK.into_response()
    }
}

#[derive(Clone)]
pub struct WithStatus<R: Reply> {
    reply: R,
    status: StatusCode,
}

pub fn with_status<R: Reply>(reply: R, status: StatusCode) -> WithStatus<R> {
    WithStatus { reply, status }
}

impl<R: Reply> Reply for WithStatus<R> {
    #[inline]
    fn into_response(self) -> Response {
        let mut res = self.reply.into_response();

        // Don't override server errors with non-server errors
        if !res.status().is_server_error() || self.status.is_server_error() {
            *res.status_mut() = self.status;
        }

        res
    }
}

pub struct WithHeader<R: Reply, H: Header> {
    reply: R,
    header: H,
}

impl<R: Reply, H: Header> Reply for WithHeader<R, H> {
    #[inline]
    fn into_response(self) -> Response {
        let mut res = self.reply.into_response();
        res.headers_mut().typed_insert(self.header);
        res
    }
}

impl Reply for &'static str {
    #[inline]
    fn into_response(self) -> Response {
        Response::new(Bytes::from(self).into())
    }
}

impl Reply for String {
    #[inline]
    fn into_response(self) -> Response {
        Response::new(self.into())
    }
}

impl Reply for Response {
    #[inline]
    fn into_response(self) -> Response {
        self
    }
}

impl Reply for StatusCode {
    #[inline]
    fn into_response(self) -> Response {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = self;
        res
    }
}
