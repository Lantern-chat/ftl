use std::mem::size_of_val;

use super::*;

use bytes::Bytes;
use futures::{Stream, StreamExt};

#[derive(Clone)]
pub struct Cbor {
    inner: Result<Bytes, ()>,
}

pub fn try_cbor<T: serde::Serialize>(value: T) -> Result<Cbor, ciborium::ser::Error<std::io::Error>> {
    let mut buf = Vec::with_capacity(size_of_val(&value));

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
