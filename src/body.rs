#![allow(clippy::type_complexity)]

use std::marker::PhantomData;

use bytes::{Buf, Bytes};
use headers::ContentType;
use http::{Response as HttpResponse, StatusCode};
use hyper::body::{Body as HttpBody, Frame};
use serde::de::{Deserialize, DeserializeOwned};
use tokio::sync::mpsc;

use super::{BodyError, Error, Reply, Route};

lazy_static::lazy_static! {
    pub static ref APPLICATION_CBOR: ContentType = ContentType::from("application/cbor".parse::<mime::Mime>().unwrap());
}

use http_body_util::{Full, StreamBody};

use tokio_stream::wrappers::ReceiverStream;

pub type Response = HttpResponse<Body>;

impl Reply for Body {
    #[inline]
    fn into_response(self) -> Response {
        Response::new(self)
    }
}

#[derive(Default)]
pub struct Body(BodyInner);

#[derive(Default)]
#[pin_project::pin_project(project = BodyProj)]
enum BodyInner {
    #[default]
    Empty,
    Full(#[pin] Full<Bytes>),
    Channel(#[pin] StreamBody<ReceiverStream<Result<Frame<Bytes>, Error>>>),
    Stream(#[pin] StreamBody<futures::stream::BoxStream<'static, Result<Frame<Bytes>, Error>>>),
    //Buf(#[pin] Full<Pin<Box<dyn Buf + Send + 'static>>>),
    Dyn(#[pin] Pin<Box<dyn HttpBody<Data = Bytes, Error = Error> + Send + 'static>>),
}

// assert Send
const _: () = {
    const fn test_send<T: Send>() {}
    test_send::<Body>();
};

use std::{
    pin::Pin,
    task::{Context, Poll},
};

impl HttpBody for Body {
    type Data = Bytes;
    type Error = Error;

    #[inline]
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.get_mut().0).poll_frame(cx)
    }

    #[inline]
    fn is_end_stream(&self) -> bool {
        self.0.is_end_stream()
    }

    #[inline]
    fn size_hint(&self) -> hyper::body::SizeHint {
        self.0.size_hint()
    }
}

impl HttpBody for BodyInner {
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.project() {
            BodyProj::Empty => Poll::Ready(None),
            BodyProj::Full(full) => full.poll_frame(cx).map_err(|_| unreachable!()),
            //BodyProj::Buf(buf) => buf.poll_frame(cx).map_err(|_| unreachable!()),
            BodyProj::Channel(stream) => stream.poll_frame(cx),
            BodyProj::Stream(stream) => stream.poll_frame(cx),
            BodyProj::Dyn(body) => body.poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Full(inner) => inner.is_end_stream(),
            Self::Channel(inner) => inner.is_end_stream(),
            Self::Stream(inner) => inner.is_end_stream(),
            Self::Dyn(inner) => inner.is_end_stream(),
        }
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        match self {
            Self::Empty => hyper::body::SizeHint::new(),
            Self::Full(inner) => inner.size_hint(),
            Self::Channel(inner) => inner.size_hint(),
            Self::Stream(inner) => inner.size_hint(),
            Self::Dyn(inner) => inner.size_hint(),
        }
    }
}

impl From<Bytes> for Body {
    #[inline]
    fn from(value: Bytes) -> Self {
        Body(BodyInner::Full(Full::new(value)))
    }
}

impl From<Vec<u8>> for Body {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Bytes::from(value).into()
    }
}

impl From<String> for Body {
    #[inline]
    fn from(value: String) -> Self {
        Bytes::from(value).into()
    }
}

impl Body {
    pub const fn empty() -> Body {
        Body(BodyInner::Empty)
    }

    /// Create a new bounded channel with the given capacity where
    /// the receiver will forward given frames to the HTTP Body.
    pub fn channel(capacity: usize) -> (Self, BodySender) {
        let (tx, rx) = mpsc::channel::<Result<Frame<Bytes>, Error>>(capacity);

        (
            Body(BodyInner::Channel(StreamBody::new(ReceiverStream::new(rx)))),
            BodySender(tx),
        )
    }

    /// Creates an HTTP Body by wrapping a Stream of byte frames.
    pub fn stream<S>(stream: S) -> Body
    where
        S: futures::Stream<Item = Result<Frame<Bytes>, Error>> + Send + 'static,
    {
        Body(BodyInner::Stream(StreamBody::new(Box::pin(stream))))
    }

    pub fn wrap<B>(body: B) -> Body
    where
        B: HttpBody<Data = Bytes, Error = Error> + Send + 'static,
    {
        Body(BodyInner::Dyn(Box::pin(body)))
    }
}

pub struct BodySender(mpsc::Sender<Result<Frame<Bytes>, Error>>);

impl std::ops::Deref for BodySender {
    type Target = mpsc::Sender<Result<Frame<Bytes>, Error>>;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl BodySender {
    /// Aborts the body stream with an [`Error::StreamAborted`] error
    pub async fn abort(self) -> bool {
        self.send(Err(Error::StreamAborted)).await.is_ok()
    }
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

    let reader = route.collect().await?.aggregate().reader();

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

    let body = route.collect().await?.aggregate();

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

    let body = route.collect().await?.aggregate();

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

    let body = route.collect().await?.aggregate();

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

    let body = route.collect().await?.aggregate();

    Ok(ciborium::de::from_reader(body.reader())?)
}

pub fn content_length_limit<S>(route: &Route<S>, limit: u64) -> Option<impl Reply> {
    match route.content_length() {
        Some(len) if len > limit => Some("Content length is too long"),
        None => Some("Content-length is missing!"),
        _ => None,
    }
}
