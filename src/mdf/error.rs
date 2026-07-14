//! Error types for MDF reading and writing operations.

use std::io;

use thiserror::Error;

/// Error returned when opening (reading) an MDF file fails.
#[derive(Debug, Error)]
pub enum MdfOpenError {
    /// The stream does not begin with the expected MDF signature bytes.
    #[error("invalid mdf signature")]
    InvalidSignature,

    /// The caller requested MT19937 MDF handling with an invalid key length.
    #[error("invalid MT19937 MDF key length {0}; expected a non-zero length")]
    InvalidKeyLength(usize),

    /// The stream looks like an encrypted/obfuscated MDF, but no key/seed was supplied.
    #[error("MDF appears to be MT19937 encrypted; pass a seed or key plus original file name")]
    MissingMt19937Key,

    /// The MDF payload could not be inflated.
    #[error("failed to decompress MDF")]
    Decompress(#[source] io::Error),

    /// The MDF payload could not be decrypted with the supplied seed/key.
    #[error("failed to decrypt MT19937 MDF with supplied seed/key")]
    Decrypt,

    /// An I/O error occurred while reading the stream.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Error returned when creating (writing) an MDF file fails.
#[derive(Debug, Error)]
pub enum MdfCreateError {
    /// An I/O error occurred while writing the MDF header.
    #[error("failed to write header")]
    Header(
        #[from]
        #[source]
        io::Error,
    ),
}
