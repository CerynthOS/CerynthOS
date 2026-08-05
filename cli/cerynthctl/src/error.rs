use std::fmt;

#[derive(Debug)]
pub enum ClientError {
    Connection(std::io::Error),
    InvalidResponse,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Connection(err) => {
                write!(f, "Connection error: {}", err)
            }

            ClientError::InvalidResponse => {
                write!(f, "Invalid response from daemon")
            }
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(err: std::io::Error) -> Self {
        ClientError::Connection(err)
    }
}
