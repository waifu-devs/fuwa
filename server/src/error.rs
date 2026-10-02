use tonic::{Code, Status};

/// Set on the answer to a request that reached the wrong shard.
pub const MISROUTED: &str = "fuwa-misrouted";
/// Set on the answer to a request a part turned away without doing anything,
/// because it isn't ready yet: the gateway tries again shortly.
pub const NOT_READY: &str = "fuwa-not-ready";

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
    /// A part of a split instance this needs is down.
    #[error("{0}")]
    Unavailable(String),
    /// A request for a server this shard doesn't hold; the gateway looks up
    /// where it is now and tries again.
    #[error("that server is on another shard")]
    Misrouted,
    /// What another part of a split instance answered, passed on as it is.
    #[error("{}", .0.message())]
    Remote(Status),
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

    /// An answer from another part, read as [`From<Status>`] does but without
    /// its warning: for a call that's tried again while that part restarts,
    /// where only giving up is worth a warning.
    pub fn retried(status: Status) -> Self {
        if std::error::Error::source(&status).is_some() {
            return Self::Unavailable(UNREACHABLE.into());
        }
        Self::Remote(status)
    }
}

/// What a call gets when another part of the instance didn't answer it.
pub const UNREACHABLE: &str = "part of this instance is unreachable right now; try again soon";

impl From<Status> for Error {
    /// An answer from another part of a split instance. Failing to reach it at
    /// all (a transport error, which carries its cause) reads as that part being down.
    fn from(status: Status) -> Self {
        if std::error::Error::source(&status).is_some() {
            tracing::warn!(error = %status, "a part of this instance didn't answer");
        }
        Self::retried(status)
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
            Error::Busy | Error::Unavailable(_) => Code::Unavailable,
            Error::Misrouted => {
                let mut status = Status::unavailable(err.to_string());
                status.metadata_mut().insert(MISROUTED, "1".parse().expect("a valid header value"));
                return status;
            }
            Error::Remote(status) => return status.clone(),
            Error::Database(_) | Error::Io(_) | Error::Internal(_) => {
                tracing::error!(error = %err, "request failed");
                return Status::internal("something went wrong on the server");
            }
        };
        Status::new(code, err.to_string())
    }
}
