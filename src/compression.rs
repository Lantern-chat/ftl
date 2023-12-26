use std::error::Error as StdError;

use bytes::Bytes;
use futures::{Future, Stream};

#[cfg(feature = "brotli")]
use async_compression::tokio::bufread::BrotliEncoder;

use async_compression::{
    tokio::bufread::{DeflateEncoder, GzipEncoder},
    Level,
};
use http::{header::HeaderValue, StatusCode};
use http_body_util::{BodyStream, StreamBody};
use hyper::{
    body::Body as HttpBody,
    body::Frame,
    header::{CONTENT_ENCODING, CONTENT_LENGTH},
    Response,
};
use tokio_util::io::{ReaderStream, StreamReader};

use crate::{Body, BodyError, Reply, Route};

pub async fn wrap_route<S, T, F, B, E>(enable: bool, route: Route<S>, r: T) -> Response<Body>
where
    T: FnOnce(Route<S>) -> F,
    F: Future<Output = Response<Body>>,
{
    use headers::{ContentCoding, ContentLength, HeaderMapExt};

    let encoding = route
        .header::<headers::AcceptEncoding>()
        .and_then(|h| h.prefered_encoding());

    let resp = r(route).await;

    // skip compressing error responses, don't waste time on these
    if !enable || !resp.status().is_success() || resp.status() == StatusCode::NO_CONTENT {
        return resp;
    }

    match encoding {
        // COMPRESS method is unsupported (and never used in practice anyway)
        None | Some(ContentCoding::IDENTITY) | Some(ContentCoding::COMPRESS) => resp,

        #[cfg(not(feature = "brotli"))]
        Some(ContentCoding::BROTLI) => resp,

        Some(encoding) => {
            let (mut parts, body) = resp.into_parts();

            if let Some(cl) = parts.headers.typed_get::<ContentLength>() {
                if cl.0 <= 32 {
                    return Response::from_parts(parts, body); // recombine
                }
            }

            let encoding_value = HeaderValue::from_static(encoding.to_static());
            parts.headers.append(CONTENT_ENCODING, encoding_value);
            parts.headers.remove(CONTENT_LENGTH);

            let reader = StreamReader::new(BodyStream::new(body));

            let body = match encoding {
                #[cfg(feature = "brotli")]
                ContentCoding::BROTLI => Body::wrap_stream(ReaderStream::new(BrotliEncoder::with_quality(
                    reader,
                    if cfg!(debug_assertions) { Level::Fastest } else { Level::Precise(2) },
                ))),
                ContentCoding::GZIP => Body::wrap_stream(ReaderStream::new(GzipEncoder::with_quality(
                    reader,
                    if cfg!(debug_assertions) { Level::Fastest } else { Level::Default },
                ))),
                ContentCoding::DEFLATE => Body::wrap_stream(ReaderStream::new(DeflateEncoder::with_quality(
                    reader,
                    if cfg!(debug_assertions) { Level::Fastest } else { Level::Default },
                ))),
                ContentCoding::IDENTITY | ContentCoding::COMPRESS => unreachable!(),

                #[cfg(not(feature = "brotli"))]
                ContentCoding::BROTLI => unreachable!(),
            };

            Response::from_parts(parts, Body::Stream(()))
        }
    }
}
