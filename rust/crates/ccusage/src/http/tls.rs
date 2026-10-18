//! The rustls crypto provider for the pricing refresh.
//!
//! rustls' own `ring` provider also carries everything a TLS server or a client
//! presenting a certificate needs, and its key loader keeps ring's RSA, ECDSA,
//! and Ed25519 signing code in the binary. The refresh only ever verifies a
//! server, so this provider is assembled from the same ring primitives without
//! a way to load private keys.

use std::sync::Arc;

use rustls::{
    SignatureScheme,
    crypto::{
        CryptoProvider, GetRandomFailed, KeyProvider, SecureRandom, WebPkiSupportedAlgorithms, ring,
    },
    pki_types::PrivateKeyDer,
    sign::SigningKey,
};
use webpki::ring as algs;

/// Builds the provider the pricing client hands to rustls.
pub(super) fn provider() -> CryptoProvider {
    CryptoProvider {
        cipher_suites: ring::ALL_CIPHER_SUITES.to_vec(),
        kx_groups: ring::ALL_KX_GROUPS.to_vec(),
        signature_verification_algorithms: SIGNATURE_ALGORITHMS,
        secure_random: &VerifyOnly,
        key_provider: &VerifyOnly,
    }
}

/// The signature algorithms rustls' ring provider verifies, in its order.
static SIGNATURE_ALGORITHMS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[
        algs::ECDSA_P256_SHA256,
        algs::ECDSA_P256_SHA384,
        algs::ECDSA_P384_SHA256,
        algs::ECDSA_P384_SHA384,
        algs::ED25519,
        algs::RSA_PSS_2048_8192_SHA256_LEGACY_KEY,
        algs::RSA_PSS_2048_8192_SHA384_LEGACY_KEY,
        algs::RSA_PSS_2048_8192_SHA512_LEGACY_KEY,
        algs::RSA_PKCS1_2048_8192_SHA256,
        algs::RSA_PKCS1_2048_8192_SHA384,
        algs::RSA_PKCS1_2048_8192_SHA512,
        algs::RSA_PKCS1_2048_8192_SHA256_ABSENT_PARAMS,
        algs::RSA_PKCS1_2048_8192_SHA384_ABSENT_PARAMS,
        algs::RSA_PKCS1_2048_8192_SHA512_ABSENT_PARAMS,
    ],
    mapping: &[
        // TLS 1.3 fixes the curve per scheme; TLS 1.2 does not, hence two entries.
        (
            SignatureScheme::ECDSA_NISTP384_SHA384,
            &[algs::ECDSA_P384_SHA384, algs::ECDSA_P256_SHA384],
        ),
        (
            SignatureScheme::ECDSA_NISTP256_SHA256,
            &[algs::ECDSA_P256_SHA256, algs::ECDSA_P384_SHA256],
        ),
        (SignatureScheme::ED25519, &[algs::ED25519]),
        (
            SignatureScheme::RSA_PSS_SHA512,
            &[algs::RSA_PSS_2048_8192_SHA512_LEGACY_KEY],
        ),
        (
            SignatureScheme::RSA_PSS_SHA384,
            &[algs::RSA_PSS_2048_8192_SHA384_LEGACY_KEY],
        ),
        (
            SignatureScheme::RSA_PSS_SHA256,
            &[algs::RSA_PSS_2048_8192_SHA256_LEGACY_KEY],
        ),
        (
            SignatureScheme::RSA_PKCS1_SHA512,
            &[algs::RSA_PKCS1_2048_8192_SHA512],
        ),
        (
            SignatureScheme::RSA_PKCS1_SHA384,
            &[algs::RSA_PKCS1_2048_8192_SHA384],
        ),
        (
            SignatureScheme::RSA_PKCS1_SHA256,
            &[algs::RSA_PKCS1_2048_8192_SHA256],
        ),
    ],
};

/// Supplies ring's system randomness and refuses to load private keys.
#[derive(Debug)]
struct VerifyOnly;

impl SecureRandom for VerifyOnly {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        use ::ring::rand::SecureRandom as _;

        ::ring::rand::SystemRandom::new()
            .fill(buf)
            .map_err(|_| GetRandomFailed)
    }
}

impl KeyProvider for VerifyOnly {
    fn load_private_key(
        &self,
        _key_der: PrivateKeyDer<'static>,
    ) -> Result<Arc<dyn SigningKey>, rustls::Error> {
        Err(rustls::Error::General(
            "the pricing client does not present certificates".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rustls::{ClientConfig, pki_types::PrivateKeyDer};

    use super::provider;

    #[test]
    fn provider_builds_a_client_config_but_loads_no_keys() {
        let provider = provider();
        assert!(
            provider
                .key_provider
                .load_private_key(PrivateKeyDer::Pkcs8(Vec::new().into()))
                .is_err()
        );
        assert!(
            ClientConfig::builder_with_provider(Arc::new(provider))
                .with_safe_default_protocol_versions()
                .is_ok(),
            "rustls must accept the provider"
        );
    }
}
