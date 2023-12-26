use crate::{body::BodySender, error::DynError};

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

pub struct JsonStream {
    body: Body,
}

pub fn map_stream<K, T, E>(stream: impl Stream<Item = Result<(K, T), E>> + Send + 'static) -> impl Reply
where
    K: Borrow<str>,
    T: serde::Serialize + Send + Sync + 'static,
    E: Into<DynError> + Send + Sync + 'static,
{
    let (body, sender) = Body::channel(16);

    tokio::spawn(async move {
        let mut stream = std::pin::pin!(stream);

        let mut first = true;
        let mut buffer = String::with_capacity(128);
        buffer.push('{');

        let error: Result<(), DynError> = loop {
            match stream.next().await {
                Some(Ok((key, ref value))) => {
                    let pos = buffer.len();
                    let key = key.borrow();

                    // most keys will be well-behaved and not need escaping, so `,"key":`
                    // extra byte won't hurt anything when the value is serialized
                    buffer.reserve(key.len() + 4);

                    if !first {
                        buffer.push_str(",\"");
                    } else {
                        buffer.push('"');
                    }

                    use std::fmt::Write;
                    if let Err(e) = write!(buffer, "{}", v_jsonescape::escape(key)) {
                        buffer.truncate(pos); // revert back to previous element
                        break Err(e.into());
                    }

                    buffer.push_str("\":");

                    if let Err(e) = serde_json::to_writer(unsafe { buffer.as_mut_vec() }, value) {
                        buffer.truncate(pos); // revert back to previous element
                        break Err(e.into());
                    }

                    first = false;
                }
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            }

            // Flush buffer at 8KiB
            if buffer.len() >= (1024 * 8) {
                let chunk = Bytes::from(std::mem::take(&mut buffer));
                if let Err(e) = sender.send(Ok(Frame::data(chunk))).await {
                    log::error!("Error sending JSON map chunk: {e}");
                    if !sender.abort().await {
                        log::error!("Error aborting JSON stream");
                    }
                    return;
                }
            }
        };

        buffer.push('}');
        if let Err(e) = sender.send(Ok(Frame::data(buffer.into()))).await {
            log::error!("Error sending JSON map chunk: {e}");
            if !sender.abort().await {
                log::error!("Error aborting JSON stream");
            }
            return;
        }

        if let Err(e) = error {
            log::error!("Error serializing json map: {e}");
        }
    });

    JsonStream { body }
}

pub fn array_stream<T, E>(stream: impl Stream<Item = Result<T, E>> + Send + 'static) -> impl Reply
where
    T: serde::Serialize + Send + Sync + 'static,
    E: Into<DynError> + Send + Sync + 'static,
{
    let (body, sender) = Body::channel(16);

    tokio::spawn(async move {
        let mut stream = std::pin::pin!(stream);

        let mut first = true;
        let mut buffer = Vec::with_capacity(128);
        buffer.push(b'[');

        let error: Result<(), DynError> = loop {
            match stream.next().await {
                Some(Ok(ref value)) => {
                    let pos = buffer.len();

                    if !first {
                        buffer.push(b',');
                    }

                    if let Err(e) = serde_json::to_writer(&mut buffer, value) {
                        buffer.truncate(pos); // revert back to previous element
                        break Err(e.into());
                    }

                    first = false;
                }
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            }

            // Flush buffer at 8KiB
            if buffer.len() >= (1024 * 8) {
                let chunk = Bytes::from(std::mem::take(&mut buffer));
                if let Err(e) = sender.send(Ok(Frame::data(chunk))).await {
                    log::error!("Error sending JSON array chunk: {e}");
                    if !sender.abort().await {
                        log::error!("Error aborting JSON stream");
                    }
                    return;
                }
            }
        };

        buffer.push(b']');
        if let Err(e) = sender.send(Ok(Frame::data(buffer.into()))).await {
            log::error!("Error sending JSON array chunk: {e}");
            if !sender.abort().await {
                log::error!("Error aborting JSON stream");
            }
            return;
        }

        if let Err(e) = error {
            log::error!("Error serializing json array: {e}");
        }
    });

    JsonStream { body }
}

impl Reply for JsonStream {
    #[inline]
    fn into_response(self) -> Response {
        Response::new(self.body)
            .with_header(ContentType::json())
            .into_response()
    }
}
