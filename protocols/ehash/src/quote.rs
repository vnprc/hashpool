use std::convert::TryInto;

use binary_sv2::{CompressedPubKey, Str0255, Sv2Option, U256};
use mint_quote_sv2::MintQuoteRequest;
use thiserror::Error;

/// Errors that can occur while constructing a mint quote request.
#[derive(Debug, Error)]
pub enum QuoteBuildError {
    #[error("invalid unit string: {0:?}")]
    InvalidUnit(binary_sv2::Error),
    #[error("invalid header hash: {0:?}")]
    InvalidHeaderHash(binary_sv2::Error),
    #[error("invalid header hash length: {0}")]
    InvalidHeaderHashLength(usize),
}

/// Build a `MintQuoteRequest` using the canonical "HASH" unit and the provided
/// share metadata.
pub fn build_mint_quote_request(
    amount: u64,
    header_hash: &[u8],
    locking_key: CompressedPubKey<'static>,
) -> Result<MintQuoteRequest<'static>, QuoteBuildError> {
    if header_hash.len() != 32 {
        return Err(QuoteBuildError::InvalidHeaderHashLength(header_hash.len()));
    }

    let unit: Str0255 = "HASH"
        .as_bytes()
        .to_vec()
        .try_into()
        .map_err(QuoteBuildError::InvalidUnit)?;

    let header_hash_vec = header_hash.to_vec();
    let header_hash: U256 = header_hash_vec
        .try_into()
        .map_err(QuoteBuildError::InvalidHeaderHash)?;

    Ok(MintQuoteRequest {
        amount,
        unit,
        header_hash,
        description: Sv2Option::new(None),
        locking_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_sv2::CompressedPubKey;

    fn dummy_pubkey() -> CompressedPubKey<'static> {
        // 33-byte compressed key with even parity.
        let mut bytes = [0u8; 33];
        bytes[0] = 0x02;
        // remaining bytes stay zero which is fine for serialization tests
        CompressedPubKey::from_bytes(&mut bytes).unwrap().into_static()
    }

    #[test]
    fn builds_request_successfully() {
        let hash = [0xAAu8; 32];
        let req = build_mint_quote_request(42, &hash, dummy_pubkey()).unwrap();
        assert_eq!(req.amount, 42);
        assert_eq!(req.unit.inner_as_ref(), b"HASH");
        assert_eq!(req.header_hash.inner_as_ref(), &hash);
    }

    #[test]
    fn rejects_header_hash_with_wrong_size() {
        let err = build_mint_quote_request(1, &[0u8; 31], dummy_pubkey()).unwrap_err();
        match err {
            QuoteBuildError::InvalidHeaderHashLength(31) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
