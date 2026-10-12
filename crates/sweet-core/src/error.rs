use std::fmt;

#[derive(Debug)]
pub struct SweetError(pub String);

impl fmt::Display for SweetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SweetError {}

impl SweetError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
