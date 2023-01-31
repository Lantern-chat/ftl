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
use hyper::{
    header::{CONTENT_ENCODING, CONTENT_LENGTH},
    Body,
};
use tokio_util::io::{ReaderStream, StreamReader};

use crate::{Reply, Response, Route};

pin_project_lite::pin_project! {
    #[derive(Debug)]
    struct CompressableBody<S, E>
    where
        E: StdError,
        S: Stream<Item = Result<Bytes, E>>,
    {
        #[pin]
        body: S,
    }
}

use std::pin::Pin;
use std::task::{Context, Poll};

impl<S, E> Stream for CompressableBody<S, E>
where
    E: StdError,
    S: Stream<Item = Result<Bytes, E>>,
{
    type Item = std::io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        use std::io::{Error, ErrorKind};
        let pin = self.project();
        S::poll_next(pin.body, cx).map_err(|_| Error::from(ErrorKind::InvalidData))
    }
}

impl From<Body> for CompressableBody<Body, hyper::Error> {
    fn from(body: Body) -> Self {
        CompressableBody { body }
    }
}

#[derive(Debug)]
struct CompressionProps {
    body: CompressableBody<Body, hyper::Error>,
    head: http::response::Parts,
}

impl From<Response> for CompressionProps {
    fn from(resp: Response) -> Self {
        let (head, body) = resp.into_parts();
        CompressionProps {
            body: body.into(),
            head,
        }
    }
}

pub async fn wrap_route<S, T, F, R>(enable: bool, route: Route<S>, r: T) -> Response
where
    T: FnOnce(Route<S>) -> F,
    F: Future<Output = R>,
    R: Reply,
{
    use headers::{ContentCoding, ContentLength, HeaderMapExt};

    let encoding = route
        .header::<headers::AcceptEncoding>()
        .and_then(|h| h.prefered_encoding());

    let resp = r(route).await.into_response();

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
            let mut props = CompressionProps::from(resp);

            if let Some(cl) = props.head.headers.typed_get::<ContentLength>() {
                if cl.0 < 32 {
                    return Response::from_parts(props.head, props.body.body); // recombine
                }
            }

            let encoding_value = HeaderValue::from_static(encoding.to_static());
            props.head.headers.append(CONTENT_ENCODING, encoding_value);
            props.head.headers.remove(CONTENT_LENGTH);

            let reader = StreamReader::new(props.body);

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

            Response::from_parts(props.head, body)
        }
    }
}
