use thiserror::Error;

#[derive(Error, Debug)]
pub enum HalError {
    #[error("backend not implemented")]
    NotImplemented,

    #[error("device not found")]
    DeviceNotFound,

    #[error("device already open")]
    DeviceAlreadyOpen,

    #[error("device not open")]
    DeviceNotOpen,

    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("io error: {0}")]
    Io(String),
}

pub type HalResult<T> = Result<T, HalError>;
