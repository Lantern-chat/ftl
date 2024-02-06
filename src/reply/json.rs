use crate::{body::BodySender, error::Error};

use super::*;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use hyper::body::Frame;

#[derive(Clone)]
pub struct Json {
    inner: Result<Bytes, ()>,
}

pub fn try_json<T: serde::Serialize>(value: T) -> Result<Json, serde_json::Error> {
    Ok(Json {
        inner: match serde_json::to_vec(&value) {
            Ok(v) => Ok(Bytes::from(v)),
            Err(e) => return Err(e),
        },
    })
}

pub fn json<T: serde::Serialize>(value: T) -> Json {
    match try_json(value) {
        Ok(resp) => resp,
        Err(e) => {
            log::error!("JSON Reply error: {e}");
            Json { inner: Err(()) }
        }
    }
}

impl Reply for Json {
    fn into_response(self) -> Response {
        match self.inner {
            Ok(body) => Response::new(Body::from(body))
                .with_header(ContentType::json())
                .into_response(),

            Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

use std::{
    mem,
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Clone, Copy)]
enum State {
    New,
    First,
    Running,
    Done,
}

#[pin_project::pin_project]
struct JsonArrayBody<S> {
    state: State,

    buffer: Vec<u8>,

    #[pin]
    stream: S,
}

#[pin_project::pin_project]
struct JsonMapBody<S> {
    state: State,

    buffer: String,

    #[pin]
    stream: S,
}

#[must_use]
#[allow(clippy::single_char_add_str)]
pub fn map_stream<S, K, T, E>(stream: S) -> impl Reply
where
    S: Stream<Item = Result<(K, T), E>> + Send + 'static,
    K: Borrow<str>,
    T: serde::Serialize + Send + Sync + 'static,
    E: std::error::Error,
{
    return Body::wrap(JsonMapBody {
        state: State::New,
        buffer: String::new(),
        stream,
    })
    .with_header(ContentType::json());

    impl<S, K, T, E> hyper::body::Body for JsonMapBody<S>
    where
        S: Stream<Item = Result<(K, T), E>> + Send + 'static,
        K: Borrow<str>,
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

            match this.state {
                State::New => {
                    this.buffer.reserve(128);
                    this.buffer.push_str("{");
                    *this.state = State::First
                }
                State::Done => return Poll::Ready(None),
                _ => {}
            }

            while let Some(item) = futures::ready!(this.stream.as_mut().poll_next(cx)) {
                let (key, value) = match item {
                    Ok(item) => item,
                    Err(e) => {
                        log::error!("Error sending JSON map stream: {e}");
                        break;
                    }
                };

                let pos = this.buffer.len();
                let key = key.borrow();

                // most keys will be well-behaved and not need escaping, so `,"key":`
                // extra byte won't hurt anything when the value is serialized
                this.buffer.reserve(key.len() + 4);

                if let State::First = *this.state {
                    this.buffer.push_str("\"");
                    *this.state = State::Running;
                } else {
                    this.buffer.push_str(",\"");
                }

                use std::fmt::Write;
                write!(this.buffer, "{}", v_jsonescape::escape(key)).expect("Unable to write to buffer");

                this.buffer.push_str("\":");

                if let Err(e) = serde_json::to_writer(unsafe { this.buffer.as_mut_vec() }, &value) {
                    this.buffer.truncate(pos); // revert back to previous element
                    log::error!("Error encoding JSON map stream: {e}");
                    break;
                }

                if this.buffer.len() >= (1024 * 8) {
                    return Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))));
                }
            }

            this.buffer.push_str("}");
            *this.state = State::Done;

            Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))))
        }
    }
}

#[must_use]
pub fn array_stream<S, T, E>(stream: S) -> impl Reply
where
    S: Stream<Item = Result<T, E>> + Send + 'static,
    T: serde::Serialize + Send + Sync + 'static,
    E: std::error::Error,
{
    return Body::wrap(JsonArrayBody {
        state: State::New,
        buffer: Vec::new(),
        stream,
    })
    .with_header(ContentType::json());

    impl<S, T, E> hyper::body::Body for JsonArrayBody<S>
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

            match this.state {
                State::New => {
                    this.buffer.reserve(128);
                    this.buffer.push(b'[');
                    *this.state = State::First;
                }
                State::Done => return Poll::Ready(None),
                _ => {}
            }

            while let Some(item) = futures::ready!(this.stream.as_mut().poll_next(cx)) {
                let item = match item {
                    Ok(item) => item,
                    Err(e) => {
                        log::error!("Error sending JSON array stream: {e}");
                        break;
                    }
                };

                let pos = this.buffer.len();

                if let State::First = *this.state {
                    this.buffer.push(b',');
                    *this.state = State::Running;
                }

                if let Err(e) = serde_json::to_writer(&mut this.buffer, &item) {
                    this.buffer.truncate(pos); // revert back to previous element
                    log::error!("Error encoding JSON array stream: {e}");
                    break;
                }

                if this.buffer.len() >= (1024 * 8) {
                    return Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))));
                }
            }

            this.buffer.push(b']');
            *this.state = State::Done;

            Poll::Ready(Some(Ok(Frame::data(Bytes::from(mem::take(this.buffer))))))
        }
    }
}
