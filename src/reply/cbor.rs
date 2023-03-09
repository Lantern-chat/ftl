use std::mem::size_of_val;

use crate::APPLICATION_CBOR;

use super::*;

use bytes::Bytes;
use futures::{Stream, StreamExt};

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
            Ok(body) => Body::from(body)
                .with_header(crate::body::APPLICATION_CBOR.clone())
                .into_response(),

            Err(()) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

pub struct CborStream {
    body: Body,
}

pub fn array_stream<T, E>(stream: impl Stream<Item = Result<T, E>> + Send + 'static) -> impl Reply
where
    T: serde::Serialize + Send + Sync + 'static,
    E: Into<Box<dyn std::error::Error + Send + Sync>> + Send + Sync + 'static,
{
    let (mut sender, body) = Body::channel();

    tokio::spawn(async move {
        let mut stream = std::pin::pin!(stream);
        let mut buffer = Vec::with_capacity(128);

        let error: Result<(), Box<dyn std::error::Error + Send + Sync>> = loop {
            match stream.next().await {
                Some(Ok(ref value)) => {
                    let pos = buffer.len();

                    if let Err(e) = ciborium::ser::into_writer(value, &mut buffer) {
                        buffer.truncate(pos); // revert back to previous item
                        break Err(e.into());
                    }
                }
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            }

            // Flush buffer at 8KiB
            if buffer.len() >= (1024 * 8) {
                let chunk = Bytes::from(std::mem::take(&mut buffer));
                if let Err(e) = sender.send_data(chunk).await {
                    log::error!("Error sending CBOR chunk: {e}");
                    return sender.abort();
                }
            }
        };

        if let Err(e) = sender.send_data(buffer.into()).await {
            log::error!("Error sending CBOR chunk: {e}");
            return sender.abort();
        }

        if let Err(e) = error {
            log::error!("Error serializing CBOR stream: {e}");
        }
    });

    CborStream { body }
}

impl Reply for CborStream {
    fn into_response(self) -> Response {
        self.body.with_header(APPLICATION_CBOR.clone()).into_response()
    }
}
