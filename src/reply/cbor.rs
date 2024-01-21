use std::mem::size_of_val;

use crate::{body::BodySender, error::Error, APPLICATION_CBOR};

use super::*;

use bytes::Bytes;
use futures::{SinkExt, Stream, StreamExt};
use hyper::body::Frame;

#[derive(Clone)]
pub struct Cbor {
    inner: Result<Bytes, ()>,
}

pub fn try_cbor<T: serde::Serialize>(value: T) -> Result<Cbor, ciborium::ser::Error<std::io::Error>> {
    let mut buf = Vec::with_capacity(128);

    Ok(Cbor {
        inner: match ciborium::ser::into_writer(&value, &mut buf) {
            Ok(()) => Ok(Bytes::from(buf)),
            Err(e) => return Err(e),
        },
    })
}

pub fn cbor<T: serde::Serialize>(value: T) -> Cbor {
    match try_cbor(value) {
        Ok(resp) => resp,
        Err(e) => {
            log::error!("CBOR Reply error: {e}");
            Cbor { inner: Err(()) }
        }
    }
}

impl Reply for Cbor {
    fn into_response(self) -> Response {
        match self.inner {
            Ok(body) => Response::new(Body::from(body))
                .with_header(crate::body::APPLICATION_CBOR.clone())
                .into_response(),

            Err(()) => Response::new(Body::Empty)
                .with_status(StatusCode::INTERNAL_SERVER_ERROR)
                .into_response(),
        }
    }
}

#[pin_project::pin_project]
struct CborArrayBody<S> {
    buffer: Vec<u8>,

    #[pin]
    stream: S,
}

use std::{
    mem,
    pin::Pin,
    task::{Context, Poll},
};

pub fn array_stream<S, T, E>(stream: S) -> impl Reply
where
    S: Stream<Item = Result<T, E>> + Send + 'static,
    T: serde::Serialize + Send + Sync + 'static,
    E: std::error::Error,
{
    return Body::Dyn(Box::pin(CborArrayBody {
        buffer: Vec::new(),
        stream,
    }))
    .with_header(APPLICATION_CBOR.clone());

    impl<S, T, E> hyper::body::Body for CborArrayBody<S>
    where
        S: Stream<Item = Result<T, E>> + Send + 'static,
        T: serde::Serialize + Send + Sync + 'static,
        E: std::error::Error,
    {
        type Data = Bytes;
        type Error = Error;

        fn poll_frame(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            let mut this = self.project();

            while let Some(item) = futures::ready!(this.stream.as_mut().poll_next(cx)) {
                let item = match item {
                    Ok(item) => item,
                    Err(e) => {
                        log::error!("Error sending CBOR stream: {e}");
                        break;
                    }
                };

                let pos = this.buffer.len();

                if let Err(e) = ciborium::into_writer(&item, &mut this.buffer) {
                    this.buffer.truncate(pos);
                    log::error!("Error encoding CBOR stream: {e}");
                    break;
                }

                if this.buffer.len() >= (1024 * 8) {
                    return Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))));
                }
            }

            Poll::Ready(match this.buffer.is_empty() {
                false => Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))),
                true => None,
            })
        }
    }
}
