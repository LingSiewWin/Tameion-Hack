//! Which algorithm signed what (docs/117 §2): one registry, one dispatcher.
//!
//! A signed object names its algorithm in an `alg` field beside the signature,
//! never inside the hashed body, so moving an object to another algorithm adds
//! a registry string and changes no digest. `alg` is parsed into [`Alg`] before
//! anything is verified: a string never reaches the dispatcher, and a missing or
//! unknown one is a typed refusal ([`SigRefusal`]), never a panic.
//!
//! Every algorithm here signs the object's 32-byte identity (an EIP-712 digest,
//! or the hash a WebAuthn assertion signs): hash first, then sign.

use alloy::primitives::{Address, B256, Signature};
use ml_dsa::{EncodedVerifyingKey, MlDsa65, VerifyingKey};
use std::fmt;
use std::str::FromStr;

/// The algorithms this service verifies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alg {
    /// The owner's passkey: ECDSA P-256 over a WebAuthn assertion.
    P256Webauthn,
    /// An Ethereum key: ECDSA secp256k1 over an EIP-712 digest, recovered to
    /// an address.
    Secp256k1Eip712,
    /// FIPS 204 ML-DSA-65 (security category 3), pure, empty context.
    MlDsa65,
}

/// The registry, in the order a reader lists it.
pub const REGISTRY: [Alg; 3] = [Alg::P256Webauthn, Alg::Secp256k1Eip712, Alg::MlDsa65];

impl Alg {
    /// The registry string, as a signed object carries it.
    pub fn as_str(self) -> &'static str {
        match self {
            Alg::P256Webauthn => "p256-webauthn",
            Alg::Secp256k1Eip712 => "secp256k1-eip712",
            Alg::MlDsa65 => "ml-dsa-65",
        }
    }

    /// `alg` as a body carries it: absent or blank is [`SigRefusal::Missing`].
    pub fn parse(v: Option<&str>) -> Result<Alg, SigRefusal> {
        match v.map(str::trim) {
            None | Some("") => Err(SigRefusal::Missing),
            Some(s) => s.parse(),
        }
    }
}

impl FromStr for Alg {
    type Err = SigRefusal;

    /// Exactly a registry string; anything else is [`SigRefusal::Unknown`].
    fn from_str(s: &str) -> Result<Alg, SigRefusal> {
        REGISTRY
            .into_iter()
            .find(|a| a.as_str() == s)
            .ok_or_else(|| SigRefusal::Unknown(s.chars().take(40).collect()))
    }
}

impl fmt::Display for Alg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The key a signature is checked against.
#[derive(Clone, Copy, Debug)]
pub enum KeyRef<'a> {
    /// secp256k1: the address the signature must recover to.
    Address(Address),
    /// P-256: the public point.
    P256 { x: B256, y: B256 },
    /// ML-DSA-65: the encoded public key (1,952 bytes).
    MlDsa65(&'a [u8]),
}

/// Why a signature was not accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SigRefusal {
    /// The signed object names no algorithm.
    Missing,
    /// `alg` is not a registry string (the first 40 characters of it).
    Unknown(String),
    /// The key is not a key of this algorithm.
    BadKey(&'static str),
    /// The signature is malformed or does not verify.
    BadSignature(&'static str),
    /// secp256k1: a valid signature, by this other address.
    WrongSigner(Address),
}

impl SigRefusal {
    /// The refusal code an agent reads (docs/115 §2).
    pub fn code(&self) -> &'static str {
        match self {
            SigRefusal::Missing => "sig_alg_missing",
            SigRefusal::Unknown(_) => "sig_alg_unknown",
            _ => "invalid_body",
        }
    }
}

impl fmt::Display for SigRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SigRefusal::Missing => {
                f.write_str("the signed object names no algorithm: add alg (p256-webauthn, secp256k1-eip712 or ml-dsa-65)")
            }
            SigRefusal::Unknown(a) => write!(
                f,
                "alg {a:?} is not one of p256-webauthn, secp256k1-eip712, ml-dsa-65"
            ),
            SigRefusal::BadKey(w) | SigRefusal::BadSignature(w) => f.write_str(w),
            SigRefusal::WrongSigner(a) => write!(f, "the signature is {a:#x}'s"),
        }
    }
}

/// Check `sig` over `msg` (the object's 32-byte identity) with `key` under
/// `alg`. A key of another algorithm is refused, never coerced. Never panics.
pub fn verify(alg: Alg, key: &KeyRef, msg: &[u8], sig: &[u8]) -> Result<(), SigRefusal> {
    match (alg, key) {
        (Alg::Secp256k1Eip712, KeyRef::Address(want)) => {
            let digest = B256::try_from(msg)
                .map_err(|_| SigRefusal::BadSignature("secp256k1-eip712 signs a 32-byte digest"))?;
            let s = Signature::from_raw(sig)
                .map_err(|_| SigRefusal::BadSignature("not a 65-byte secp256k1 signature"))?;
            let got = s
                .recover_address_from_prehash(&digest)
                .map_err(|_| SigRefusal::BadSignature("the signature recovers no key"))?;
            if got == *want {
                Ok(())
            } else {
                Err(SigRefusal::WrongSigner(got))
            }
        }
        (Alg::P256Webauthn, KeyRef::P256 { x, y }) => {
            use p256::ecdsa::signature::hazmat::PrehashVerifier;
            let point =
                p256::EncodedPoint::from_affine_coordinates(&x.0.into(), &y.0.into(), false);
            let key = p256::ecdsa::VerifyingKey::from_encoded_point(&point)
                .map_err(|_| SigRefusal::BadKey("the key is not a point on P-256"))?;
            let s = p256::ecdsa::Signature::from_slice(sig).map_err(|_| {
                SigRefusal::BadSignature("the signature is not a pair of P-256 scalars")
            })?;
            key.verify_prehash(msg, &s)
                .map_err(|_| SigRefusal::BadSignature("the signature does not verify"))
        }
        (Alg::MlDsa65, KeyRef::MlDsa65(pk)) => {
            let enc = EncodedVerifyingKey::<MlDsa65>::try_from(*pk)
                .map_err(|_| SigRefusal::BadKey("an ML-DSA-65 public key is 1,952 bytes"))?;
            let s = ml_dsa::Signature::<MlDsa65>::try_from(sig).map_err(|_| {
                SigRefusal::BadSignature("not an ML-DSA-65 signature (3,309 bytes)")
            })?;
            if VerifyingKey::<MlDsa65>::decode(&enc).verify_with_context(msg, &[], &s) {
                Ok(())
            } else {
                Err(SigRefusal::BadSignature("the signature does not verify"))
            }
        }
        _ => Err(SigRefusal::BadKey("the key is not a key of this algorithm")),
    }
}

/// sha256 of an ML-DSA public key: how a binding names it without carrying it.
pub fn key_hash(pk: &[u8]) -> B256 {
    use sha2::{Digest, Sha256};
    B256::from_slice(&Sha256::digest(pk))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::SignerSync;
    use alloy::signers::local::PrivateKeySigner;
    use ml_dsa::{KeyExport, Keypair, SignatureEncoding as _, Signer as _};

    #[test]
    fn the_registry_parses_exactly_its_three_strings() {
        for a in REGISTRY {
            assert_eq!(a.as_str().parse::<Alg>(), Ok(a));
            assert_eq!(Alg::parse(Some(a.as_str())), Ok(a));
        }
        assert_eq!(Alg::parse(None), Err(SigRefusal::Missing));
        assert_eq!(Alg::parse(Some("  ")), Err(SigRefusal::Missing));
        for bad in [
            "none",
            "ML-DSA-65",
            "ml-dsa-65 ",
            "es256",
            "ml-dsa-87",
            "\u{0}",
        ] {
            let e = bad.parse::<Alg>().unwrap_err();
            assert_eq!(e.code(), "sig_alg_unknown", "{bad:?}");
        }
        assert_eq!(SigRefusal::Missing.code(), "sig_alg_missing");
        let long = "x".repeat(10_000);
        assert!(matches!(long.parse::<Alg>(), Err(SigRefusal::Unknown(s)) if s.len() == 40));
    }

    #[test]
    fn each_algorithm_verifies_its_own_key_and_refuses_the_others() {
        let msg = alloy::primitives::keccak256("tameion.sig.test");
        // secp256k1-eip712
        let k = PrivateKeySigner::random();
        let s = k.sign_hash_sync(&msg).unwrap().as_bytes();
        let who = KeyRef::Address(k.address());
        assert_eq!(
            verify(Alg::Secp256k1Eip712, &who, msg.as_slice(), &s),
            Ok(())
        );
        let other = KeyRef::Address(Address::repeat_byte(7));
        assert_eq!(
            verify(Alg::Secp256k1Eip712, &other, msg.as_slice(), &s),
            Err(SigRefusal::WrongSigner(k.address()))
        );
        // ml-dsa-65, from a fixed seed
        let sk = ml_dsa::SigningKey::<MlDsa65>::from_seed(&[5u8; 32].into());
        let pk = sk.verifying_key().to_bytes();
        let sig = sk.sign(msg.as_slice()).to_bytes();
        assert_eq!((pk.len(), sig.len()), (1952, 3309));
        let mk = KeyRef::MlDsa65(pk.as_slice());
        assert_eq!(verify(Alg::MlDsa65, &mk, msg.as_slice(), &sig), Ok(()));
        let mut bent = sig.to_vec();
        bent[100] ^= 1;
        assert!(verify(Alg::MlDsa65, &mk, msg.as_slice(), &bent).is_err());
        assert!(verify(Alg::MlDsa65, &mk, b"another message", &sig).is_err());
        // Wrong lengths, wrong key kinds and empty input are refusals, not panics.
        assert!(
            verify(
                Alg::MlDsa65,
                &KeyRef::MlDsa65(&pk[..100]),
                msg.as_slice(),
                &sig
            )
            .is_err()
        );
        assert!(verify(Alg::MlDsa65, &mk, msg.as_slice(), &sig[..3308]).is_err());
        assert!(verify(Alg::MlDsa65, &who, msg.as_slice(), &sig).is_err());
        assert!(verify(Alg::Secp256k1Eip712, &mk, msg.as_slice(), &s).is_err());
        assert!(verify(Alg::Secp256k1Eip712, &who, b"short", &s).is_err());
        assert!(verify(Alg::P256Webauthn, &who, msg.as_slice(), &[]).is_err());
        let zero = KeyRef::P256 {
            x: B256::ZERO,
            y: B256::ZERO,
        };
        assert!(verify(Alg::P256Webauthn, &zero, msg.as_slice(), &[0u8; 64]).is_err());
        assert_eq!(key_hash(&pk), key_hash(pk.as_slice()));
        assert_ne!(key_hash(&pk), key_hash(&pk[1..]));
    }

    /// Our call is FIPS 204 Algorithm 3 (ML-DSA.Verify, empty context), which is
    /// Algorithm 8 over `0x00 ‖ 0x00 ‖ M` — the interface NIST's ACVP sig-ver
    /// vectors exercise (tests/ml_dsa_acvp.rs). This ties the two together.
    #[test]
    fn our_ml_dsa_call_is_the_internal_verify_the_nist_vectors_check() {
        let sk = ml_dsa::SigningKey::<MlDsa65>::from_seed(&[9u8; 32].into());
        let vk = sk.verifying_key();
        let m = b"an invoice digest stands here";
        let sig = sk.sign(m);
        assert!(vk.verify_with_context(m, &[], &sig));
        assert!(vk.verify_internal(&[&[0u8, 0u8][..], m].concat(), &sig));
        assert!(!vk.verify_internal(m, &sig));
    }
}
