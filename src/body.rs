use std::marker::PhantomData;

use bytes::{Buf, Bytes};
use headers::ContentType;
use http::StatusCode;
use serde::de::{Deserialize, DeserializeOwned};

use super::{BodyError, Reply, ReplyError, Response, Route};

lazy_static::lazy_static! {
    pub(crate) static ref APPLICATION_CBOR: ContentType = ContentType::from("application/cbor".parse::<mime::Mime>().unwrap());
}

pub async fn any<T, S>(route: &mut Route<S>) -> Result<T, BodyDeserializeError>
where
    T: DeserializeOwned,
{
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BodyType {
        Json,
        FormUrlEncoded,
        MsgPack,
        Cbor,
    }

    let kind = if let Some(ct) = route.header::<ContentType>() {
        if ct == ContentType::json() {
            BodyType::Json
        } else if ct == ContentType::form_url_encoded() {
            BodyType::FormUrlEncoded
        } else if ct == ContentType::from(mime::APPLICATION_MSGPACK) {
            BodyType::MsgPack
        } else if ct == *APPLICATION_CBOR {
            BodyType::Cbor
        } else {
            return Err(BodyDeserializeError::IncorrectContentType);
        }
    } else {
        return Err(BodyDeserializeError::IncorrectContentType);
    };

    let reader = route.aggregate().await?.reader();

    Ok(match kind {
        #[cfg(feature = "json")]
        BodyType::Json => serde_json::from_reader(reader)?,

        BodyType::FormUrlEncoded => serde_urlencoded::from_reader(reader)?,

        #[cfg(feature = "msgpack")]
        BodyType::MsgPack => rmp_serde::from_read(reader)?,

        #[cfg(feature = "cbor")]
        BodyType::Cbor => ciborium::de::from_reader(reader)?,

        #[allow(unreachable_patterns)]
        _ => return Err(BodyDeserializeError::IncorrectContentType),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum BodyDeserializeError {
    #[error("{0}")]
    BodyError(#[from] BodyError),

    #[cfg(feature = "json")]
    #[error("JSON Parse Error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Form Parse Error: {0}")]
    Form(#[from] serde_urlencoded::de::Error),

    #[cfg(feature = "msgpack")]
    #[error("MsgPack Parse Error: {0}")]
    MsgPack(#[from] rmp_serde::decode::Error),

    #[cfg(feature = "cbor")]
    #[error("CBOR Parse Error: {0}")]
    Cbor(#[from] ciborium::de::Error<std::io::Error>),

    #[error("Content Type Error")]
    IncorrectContentType,
}

#[cfg(feature = "json")]
pub async fn json<T, S>(route: &mut Route<S>) -> Result<T, BodyDeserializeError>
where
    T: DeserializeOwned,
{
    if route.header::<ContentType>() != Some(ContentType::json()) {
        return Err(BodyDeserializeError::IncorrectContentType);
    }

    let body = route.aggregate().await?;

    Ok(serde_json::from_reader(body.reader())?)
}

// pub struct OwnedBodyObject<'a, T: Deserialize<'a>> {
//     body: Bytes,
//     object: T,
//     _lt: PhantomData<&'a T>,
// }

// pub async fn json_ref<'a, T: 'a, S>(
//     route: &mut Route<S>,
// ) -> Result<OwnedBodyObject<'a, T>, BodyDeserializeError>
// where
//     T: Deserialize<'a>,
// {
//     if route.header::<ContentType>() != Some(ContentType::json()) {
//         return Err(BodyDeserializeError::IncorrectContentType);
//     }

//     let body = route.bytes().await?;

//     let object = serde_json::from_slice(unsafe { std::mem::transmute::<&[u8], &'static [u8]>(&*body) })?;

//     Ok(OwnedBodyObject {
//         body,
//         object,
//         _lt: PhantomData,
//     })
// }

pub async fn form<T, S>(route: &mut Route<S>) -> Result<T, BodyDeserializeError>
where
    T: DeserializeOwned,
{
    match route.header::<ContentType>() {
        Some(ct) if ct == ContentType::form_url_encoded() => {}
        _ => return Err(BodyDeserializeError::IncorrectContentType),
    }

    let body = route.aggregate().await?;

    Ok(serde_urlencoded::from_reader(body.reader())?)
}

#[cfg(feature = "msgpack")]
pub async fn msgpack<T, S>(route: &mut Route<S>) -> Result<T, BodyDeserializeError>
where
    T: DeserializeOwned,
{
    match route.header::<ContentType>() {
        Some(ct) if ct == ContentType::from(mime::APPLICATION_MSGPACK) => {}
        _ => return Err(BodyDeserializeError::IncorrectContentType),
    }

    let body = route.aggregate().await?;

    Ok(rmp_serde::from_read(body.reader())?)
}

#[cfg(feature = "cbor")]
pub async fn cbor<T, S>(route: &mut Route<S>) -> Result<T, BodyDeserializeError>
where
    T: DeserializeOwned,
{
    match route.header::<ContentType>() {
        Some(ct) if ct == *APPLICATION_CBOR => {}
        _ => return Err(BodyDeserializeError::IncorrectContentType),
    }

    let body = route.aggregate().await?;

    Ok(ciborium::de::from_reader(body.reader())?)
}

impl Reply for BodyDeserializeError {
    fn into_response(self) -> Response {
        match self {
            BodyDeserializeError::IncorrectContentType => "Incorrect Content-Type"
                .with_status(StatusCode::BAD_REQUEST)
                .into_response(),
            _ => self.status().into_response(),
        }
    }
}

impl ReplyError for BodyDeserializeError {
    fn status(&self) -> StatusCode {
        match self {
            BodyDeserializeError::BodyError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

pub fn content_length_limit<S>(route: &Route<S>, limit: u64) -> Option<impl Reply> {
    match route.content_length() {
        Some(len) if len > limit => Some("Content length is too long"),
        None => Some("Content-length is missing!"),
        _ => None,
    }
}
