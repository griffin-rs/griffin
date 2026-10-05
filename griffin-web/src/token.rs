//! Signed tokens: data handed to the browser that it can read but not change.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The key Griffin signs tokens with.
///
/// It is built at run time from a secret the application supplies (configuration, an
/// environment variable), never from anything compiled into the binary. Everyone who
/// has the secret can forge tokens. `Debug` does not print it.
#[derive(Clone)]
pub struct SigningKey(Hmac<Sha256>);

impl SigningKey {
    /// A key from `secret`, which must be at least 32 bytes of random data.
    pub fn new(secret: impl AsRef<[u8]>) -> Result<SigningKey, SigningKeyTooShort> {
        let secret = secret.as_ref();
        if secret.len() < 32 {
            return Err(SigningKeyTooShort);
        }
        let mac = Hmac::new_from_slice(secret).expect("HMAC takes a key of any length");
        Ok(SigningKey(mac))
    }

    /// A token carrying `data` and the time it was signed. The browser can read it
    /// (it is signed, not encrypted) but cannot change it.
    ///
    /// The token is `payload.signature`, both base64url: the payload is JSON and the
    /// signature is HMAC-SHA256 over the encoded payload. `purpose` names what the
    /// token is for, and a token is only accepted for the purpose it was signed for.
    pub(crate) fn sign(&self, purpose: &str, data: &impl Serialize) -> String {
        self.sign_at(purpose, data, now_ms())
    }

    /// [`sign`](Self::sign) as of `issued_at`, in milliseconds since the Unix epoch:
    /// for a test of expiry to have a token of the past.
    pub(crate) fn sign_at(&self, purpose: &str, data: &impl Serialize, issued_at: u64) -> String {
        let envelope = Envelope {
            purpose: purpose.into(),
            issued_at,
            data,
        };
        let json = serde_json::to_vec(&envelope).expect("token data serializes to JSON");
        let payload = URL_SAFE_NO_PAD.encode(json);
        let signature = self.0.clone().chain_update(&payload).finalize();
        format!(
            "{payload}.{}",
            URL_SAFE_NO_PAD.encode(signature.into_bytes())
        )
    }

    /// The data of a token signed by this key for `purpose` less than `max_age` ago.
    pub(crate) fn verify<T: DeserializeOwned>(
        &self,
        purpose: &str,
        token: &str,
        max_age: Duration,
    ) -> Result<T, TokenError> {
        let (payload, signature) = token.split_once('.').ok_or(TokenError::Invalid)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| TokenError::Invalid)?;
        // Compares in constant time. Nothing of the payload is read before this passes.
        self.0
            .clone()
            .chain_update(payload)
            .verify_slice(&signature)
            .map_err(|_| TokenError::Invalid)?;

        let json = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| TokenError::Invalid)?;
        let envelope: Envelope<T> =
            serde_json::from_slice(&json).map_err(|_| TokenError::Invalid)?;
        if envelope.purpose != purpose {
            return Err(TokenError::Invalid);
        }
        let age = Duration::from_millis(now_ms().saturating_sub(envelope.issued_at));
        if age >= max_age {
            return Err(TokenError::Expired);
        }
        Ok(envelope.data)
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    purpose: String,
    /// Milliseconds since the Unix epoch.
    issued_at: u64,
    data: T,
}

pub(crate) fn now_ms() -> u64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH);
    since_epoch.map_or(0, |elapsed| elapsed.as_millis() as u64)
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SigningKey(REDACTED)")
    }
}

/// The secret given to [`SigningKey::new`] was shorter than 32 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigningKeyTooShort;

impl fmt::Display for SigningKeyTooShort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the signing secret must be at least 32 bytes long")
    }
}

impl std::error::Error for SigningKeyTooShort {}

/// Why a token was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// Not a token this key signed for this purpose: forged, changed, cut short, or
    /// signed for something else. Which of these is deliberately not told.
    Invalid,
    /// Genuine, but signed too long ago.
    Expired,
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TokenError::Invalid => "the token is not valid",
            TokenError::Expired => "the token has expired",
        })
    }
}

impl std::error::Error for TokenError {}
