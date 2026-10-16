//! Ed25519 identity of a server instance, used to sign the bundles it exports
//! (`Dados → Exportar pacote`). Imports verify the signature when one is present;
//! it proves integrity between two instances, nothing about the content, which is
//! validated in quarantine either way. Watcher uploads are not signed.

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

pub struct Keys {
    signing: SigningKey,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys").field("public_key", &self.public_key()).finish()
    }
}

impl Keys {
    /// Load the seed from `path` or create a fresh key pair there.
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        if let Ok(text) = std::fs::read_to_string(path) {
            let bytes = B64.decode(text.trim()).map_err(|e| anyhow::anyhow!("chave de assinatura inválida em {}: {e}", path.display()))?;
            let seed: [u8; 32] = bytes.try_into().map_err(|_| anyhow::anyhow!("chave de assinatura inválida em {}", path.display()))?;
            return Ok(Self { signing: SigningKey::from_bytes(&seed) });
        }
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, B64.encode(seed))?;
        tracing::info!("nova chave de assinatura criada em {}", path.display());
        Ok(Self { signing: SigningKey::from_bytes(&seed) })
    }

    /// Deterministic key (tests and tooling).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { signing: SigningKey::from_bytes(&seed) }
    }

    /// Public key, base64 (32 bytes).
    pub fn public_key(&self) -> String {
        B64.encode(self.signing.verifying_key().to_bytes())
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public_key())
    }

    /// Detached signature over `msg`, base64.
    pub fn sign(&self, msg: &[u8]) -> String {
        B64.encode(self.signing.sign(msg).to_bytes())
    }
}

fn parse_key(public_key_b64: &str) -> Result<VerifyingKey, String> {
    let bytes = B64.decode(public_key_b64.trim()).map_err(|_| "chave pública inválida (base64)".to_string())?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| "chave pública inválida (tamanho)".to_string())?;
    VerifyingKey::from_bytes(&arr).map_err(|_| "chave pública inválida".to_string())
}

/// Normalized form of a public key as users paste it (trims, validates).
pub fn normalize_key(public_key_b64: &str) -> Result<String, String> {
    Ok(B64.encode(parse_key(public_key_b64)?.to_bytes()))
}

pub fn verify(public_key_b64: &str, msg: &[u8], sig_b64: &str) -> Result<(), String> {
    let key = parse_key(public_key_b64)?;
    let sig = B64.decode(sig_b64.trim()).map_err(|_| "assinatura inválida (base64)".to_string())?;
    let sig = Signature::from_slice(&sig).map_err(|_| "assinatura inválida (tamanho)".to_string())?;
    key.verify(msg, &sig).map_err(|_| "assinatura não confere com a chave".to_string())
}

/// Short, human-checkable id of a public key: `a1b2 c3d4 e5f6 0718`.
pub fn fingerprint(public_key_b64: &str) -> String {
    let bytes = B64.decode(public_key_b64.trim()).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    digest[..8].chunks(2).map(|c| format!("{:02x}{:02x}", c[0], c[1])).collect::<Vec<_>>().join(" ")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip() {
        let k = Keys::from_seed([7u8; 32]);
        let sig = k.sign(b"hello");
        assert!(verify(&k.public_key(), b"hello", &sig).is_ok());
        assert!(verify(&k.public_key(), b"hellp", &sig).is_err());
        let other = Keys::from_seed([8u8; 32]);
        assert!(verify(&other.public_key(), b"hello", &sig).is_err());
        assert_eq!(k.fingerprint().len(), 19);
        assert_eq!(normalize_key(&format!("  {}\n", k.public_key())).unwrap(), k.public_key());
        assert!(normalize_key("nope").is_err());
    }

    #[test]
    fn key_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("wikelo-sign-{}", std::process::id()));
        let path = dir.join("signing.key");
        let a = Keys::load_or_create(&path).unwrap();
        let b = Keys::load_or_create(&path).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
