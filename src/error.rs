pub(crate) type DynError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Hyper error: {0}")]
    HyperError(#[from] hyper::Error),

    #[error("Stream Aborted")]
    StreamAborted,

    #[error("Body Deserialize Error: {0}")]
    BodyDeserializeError(#[from] crate::body::BodyDeserializeError),

    #[error("Route Body Error: {0}")]
    BodyError(#[from] crate::BodyError),
}
