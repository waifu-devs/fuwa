use tonic::{Code, Status};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything a request can fail with. Internal errors are logged and reach the
/// client without their details.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    InvalidArgument(String),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    AlreadyExists(String),
    #[error("sign in first")]
    Unauthenticated,
    #[error("{0}")]
    PermissionDenied(String),
    #[error("{0}")]
    FailedPrecondition(String),
    #[error("{0}")]
    ResourceExhausted(String),
    #[error("the server is busy; try again")]
    Busy,
    #[error("database: {0}")]
    Database(#[from] turso::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Internal(String),
}

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidArgument(message.into())
    }

    pub fn denied(message: impl Into<String>) -> Self {
        Self::PermissionDenied(message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

impl From<prost::DecodeError> for Error {
    fn from(err: prost::DecodeError) -> Self {
        Self::Internal(format!("decoding stored protobuf: {err}"))
    }
}

impl From<Error> for Status {
    fn from(err: Error) -> Self {
        let code = match &err {
            Error::InvalidArgument(_) => Code::InvalidArgument,
            Error::NotFound(_) => Code::NotFound,
            Error::AlreadyExists(_) => Code::AlreadyExists,
            Error::Unauthenticated => Code::Unauthenticated,
            Error::PermissionDenied(_) => Code::PermissionDenied,
            Error::FailedPrecondition(_) => Code::FailedPrecondition,
            Error::ResourceExhausted(_) => Code::ResourceExhausted,
            Error::Busy => Code::Unavailable,
            Error::Database(_) | Error::Io(_) | Error::Internal(_) => {
                tracing::error!(error = %err, "request failed");
                return Status::internal("something went wrong on the server");
            }
        };
        Status::new(code, err.to_string())
    }
}
