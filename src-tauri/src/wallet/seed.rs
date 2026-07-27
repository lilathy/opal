use bip39::{Language, Mnemonic};
use zeroize::Zeroize;

use crate::error::OpalError;

#[derive(Clone)]
pub struct SeedPhrase {
    pub mnemonic: String,
    pub word_count: u8,
}

impl Drop for SeedPhrase {
    fn drop(&mut self) {
        self.mnemonic.zeroize();
    }
}

pub fn generate_mnemonic(word_count: u8) -> Result<SeedPhrase, OpalError> {
    let mnemonic = match word_count {
        12 => Mnemonic::generate_in(Language::English, 12)
            .map_err(|e| OpalError::Crypto(format!("mnemonic generate: {e}")))?,
        24 => Mnemonic::generate_in(Language::English, 24)
            .map_err(|e| OpalError::Crypto(format!("mnemonic generate: {e}")))?,
        _ => {
            return Err(OpalError::InvalidInput(
                "word_count must be 12 or 24".into(),
            ))
        }
    };
    Ok(SeedPhrase {
        mnemonic: mnemonic.to_string(),
        word_count,
    })
}

pub fn parse_mnemonic(phrase: &str) -> Result<Mnemonic, OpalError> {
    let cleaned = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
    Mnemonic::parse_in(Language::English, &cleaned)
        .map_err(|e| OpalError::InvalidInput(format!("invalid mnemonic: {e}")))
}

pub fn seed_bytes(mnemonic: &Mnemonic, passphrase: &str) -> [u8; 64] {
    mnemonic.to_seed(passphrase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_12_and_24() {
        let a = generate_mnemonic(12).unwrap();
        assert_eq!(a.mnemonic.split_whitespace().count(), 12);
        let b = generate_mnemonic(24).unwrap();
        assert_eq!(b.mnemonic.split_whitespace().count(), 24);
        parse_mnemonic(&a.mnemonic).unwrap();
        parse_mnemonic(&b.mnemonic).unwrap();
    }

    #[test]
    fn reject_bad_phrase() {
        assert!(parse_mnemonic("not a real seed phrase at all here").is_err());
    }
}
