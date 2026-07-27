use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OpalError {
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Crypto(String),
    #[error("invalid password")]
    InvalidPassword,
    #[error("vault already exists")]
    VaultExists,
    #[error("vault not found")]
    VaultMissing,
    #[error("vault wiped after too many failed attempts")]
    VaultWiped,
    #[error("vault is locked")]
    Locked,
    #[error("{0}")]
    WeakPassword(String),
    #[error("{0}")]
    InvalidInput(String),
}

#[derive(Serialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

impl OpalError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::Crypto(_) => "crypto",
            Self::InvalidPassword => "invalid_password",
            Self::VaultExists => "vault_exists",
            Self::VaultMissing => "vault_missing",
            Self::VaultWiped => "vault_wiped",
            Self::Locked => "locked",
            Self::WeakPassword(_) => "weak_password",
            Self::InvalidInput(_) => "invalid_input",
        }
    }

    pub fn to_payload(&self) -> ErrorPayload {
        ErrorPayload {
            code: self.code().into(),
            message: self.to_string(),
        }
    }
}

impl From<OpalError> for String {
    fn from(value: OpalError) -> Self {
        serde_json::to_string(&value.to_payload()).unwrap_or_else(|_| {
            format!(
                r#"{{"code":"unknown","message":"{}"}}"#,
                value.to_string().replace('"', "'")
            )
        })
    }
}
