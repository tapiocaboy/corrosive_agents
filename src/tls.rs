//! TLS configuration helpers (feature `tls`).
//!
//! One [`TlsConfig`] drives both transports:
//! [`server::serve_tls`](crate::server::serve_tls) for REST/WebSocket and
//! [`grpc::serve_tls`](crate::grpc::serve_tls) for gRPC.
//!
//! ```no_run
//! use corrosive_agents::tls::TlsConfig;
//!
//! let tls = TlsConfig::from_pem_files("cert.pem", "key.pem");
//! ```

use std::path::PathBuf;

use crate::error::{Error, Result};

enum Source {
    Files { cert: PathBuf, key: PathBuf },
    Pem { cert: Vec<u8>, key: Vec<u8> },
}

/// A certificate/key pair for serving TLS.
pub struct TlsConfig {
    source: Source,
}

impl std::fmt::Debug for TlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.source {
            Source::Files { cert, key } => f
                .debug_struct("TlsConfig")
                .field("cert", cert)
                .field("key", key)
                .finish(),
            Source::Pem { .. } => f.debug_struct("TlsConfig").field("source", &"pem").finish(),
        }
    }
}

impl TlsConfig {
    /// Load the certificate chain and private key from PEM files at serve
    /// time.
    pub fn from_pem_files(cert: impl Into<PathBuf>, key: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::Files {
                cert: cert.into(),
                key: key.into(),
            },
        }
    }

    /// Use in-memory PEM data.
    pub fn from_pem(cert_pem: impl Into<Vec<u8>>, key_pem: impl Into<Vec<u8>>) -> Self {
        Self {
            source: Source::Pem {
                cert: cert_pem.into(),
                key: key_pem.into(),
            },
        }
    }

    /// The (certificate, key) PEM bytes.
    pub(crate) fn pem_pair(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        match &self.source {
            Source::Files { cert, key } => {
                let cert = std::fs::read(cert).map_err(|e| {
                    Error::Config(format!("cannot read TLS cert {}: {e}", cert.display()))
                })?;
                let key = std::fs::read(key).map_err(|e| {
                    Error::Config(format!("cannot read TLS key {}: {e}", key.display()))
                })?;
                Ok((cert, key))
            }
            Source::Pem { cert, key } => Ok((cert.clone(), key.clone())),
        }
    }
}
