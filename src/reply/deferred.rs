use crate::error::DynError;

use super::*;

pub struct DeferredValue(Box<dyn IndirectSerialize + Send + 'static>);
pub struct DeferredStream(Box<dyn IndirectStream + Send + 'static>);

impl DeferredValue {
    #[inline]
    pub fn new<T>(value: T) -> Self
    where
        T: serde::Serialize + Send + 'static,
    {
        DeferredValue(Box::new(value))
    }
}

impl DeferredStream {
    pub fn new<T, E>(stream: impl futures::Stream<Item = Result<T, E>> + Send + 'static) -> Self
    where
        T: serde::Serialize + Send + Sync + 'static,
        E: Into<DynError> + Send + Sync + 'static,
    {
        DeferredStream(Box::new(Some(stream)))
    }
}

impl DeferredValue {
    #[cfg(feature = "json")]
    #[inline]
    pub fn as_json(&self) -> Result<String, serde_json::Error> {
        self.0.as_json()
    }

    #[cfg(feature = "cbor")]
    #[inline]
    pub fn as_cbor(&self) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
        self.0.as_cbor()
    }
}

impl DeferredStream {
    #[cfg(feature = "json")]
    #[inline]
    pub fn as_json(mut self) -> Response {
        self.0.as_json()
    }

    #[cfg(feature = "cbor")]
    #[inline]
    pub fn as_cbor(mut self) -> Response {
        self.0.as_cbor()
    }
}

trait IndirectSerialize {
    #[cfg(feature = "json")]
    fn as_json(&self) -> Result<String, serde_json::Error>;

    #[cfg(feature = "cbor")]
    fn as_cbor(&self) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>>;
}

trait IndirectStream {
    #[cfg(feature = "json")]
    fn as_json(&mut self) -> Response;

    #[cfg(feature = "cbor")]
    fn as_cbor(&mut self) -> Response;
}

const _: Option<&dyn IndirectSerialize> = None;

impl<T> IndirectSerialize for T
where
    T: serde::Serialize,
{
    #[cfg(feature = "json")]
    fn as_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    #[cfg(feature = "cbor")]
    fn as_cbor(&self) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
        let mut res = Vec::with_capacity(128);
        ciborium::ser::into_writer(self, &mut res)?;
        Ok(res)
    }
}

impl<S, T, E> IndirectStream for Option<S>
where
    S: futures::Stream<Item = Result<T, E>> + Send + 'static,
    T: serde::Serialize + Send + Sync + 'static,
    E: Into<DynError> + Send + Sync + 'static,
{
    #[cfg(feature = "json")]
    fn as_json(&mut self) -> Response {
        crate::reply::json::array_stream(unsafe { self.take().unwrap_unchecked() }).into_response()
    }

    #[cfg(feature = "cbor")]
    fn as_cbor(&mut self) -> Response {
        crate::reply::cbor::array_stream(unsafe { self.take().unwrap_unchecked() }).into_response()
    }
}
