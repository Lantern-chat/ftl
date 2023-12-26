//! FTL is the internal web framework derived from parts of warp,
//! but designed for a more imperative workflow.

#![allow(unused_imports)]

extern crate tracing as log;

pub mod body;
//pub mod compression;
pub mod error;
pub mod fs;
pub mod rate_limit;
pub mod real_ip;
pub mod reply;
pub mod route;
pub mod ws;

#[cfg(feature = "multipart")]
pub mod multipart;

pub use http::{Method, Request, StatusCode};

pub use self::body::{Body, Response};
pub use self::error::Error;
pub use self::reply::Reply;
pub use self::route::{
    BodyError, Route,
    Segment::{self, End, Exact},
};

pub use body::APPLICATION_CBOR;
