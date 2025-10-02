use std::convert::{TryFrom, TryInto};

use binary_sv2::{self, CompressedPubKey, Str0255, Sv2Option, U256};
use cdk::{
    mint::MintQuote,
    nuts::{nutXX::MintQuoteMiningShareRequest, CurrencyUnit, PublicKey},
    secp256k1::hashes::Hash as CdkHash,
    Amount,
};
use mint_quote_sv2::{MintQuoteRequest, MintQuoteResponse};
use thiserror::Error;

use crate::share::{ShareHash, ShareHashError};

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

/// Errors that can occur while decoding or validating incoming mint quote requests.
#[derive(Debug, Error)]
pub enum QuoteParseError {
    #[error("failed to decode MintQuoteRequest: {0:?}")]
    Decode(binary_sv2::Error),
    #[error("invalid share hash: {0}")]
    ShareHash(#[from] ShareHashError),
    #[error("invalid unit string: {0:?}")]
    InvalidUnit(binary_sv2::Error),
    #[error("invalid description string: {0:?}")]
    InvalidDescription(binary_sv2::Error),
}

/// Errors returned while converting parsed quote data into downstream representations.
#[derive(Debug, Error)]
pub enum QuoteConversionError {
    #[error("invalid share hash encoding: {0}")]
    ShareHash(#[from] ShareHashError),
    #[error("failed to convert header hash for CDK request: {0}")]
    InvalidHeaderHash(String),
    #[error("failed to convert locking key for CDK request: {0}")]
    InvalidLockingKey(String),
    #[error("invalid quote identifier: {0:?}")]
    InvalidQuoteId(binary_sv2::Error),
}

/// Result of parsing a mint quote request payload.
#[derive(Debug, Clone)]
pub struct ParsedMintQuoteRequest {
    pub request: MintQuoteRequest<'static>,
    pub share_hash: ShareHash,
}

impl ParsedMintQuoteRequest {
    /// Convert the parsed SV2 request into a CDK quote request.
    pub fn to_cdk_request(&self) -> Result<MintQuoteMiningShareRequest, QuoteConversionError> {
        let amount = Amount::from(self.request.amount);
        let unit = CurrencyUnit::Hash;

        let header_hash = CdkHash::from_slice(self.share_hash.as_bytes())
            .map_err(|e| QuoteConversionError::InvalidHeaderHash(e.to_string()))?;

        let description = self
            .request
            .description
            .clone()
            .into_inner()
            .map(|s| String::from_utf8_lossy(s.inner_as_ref()).to_string());

        let pubkey = PublicKey::from_slice(self.request.locking_key.inner_as_ref())
            .map_err(|e| QuoteConversionError::InvalidLockingKey(e.to_string()))?;

        Ok(MintQuoteMiningShareRequest {
            amount,
            unit,
            header_hash,
            description,
            pubkey,
        })
    }
}

/// Parse an incoming SV2 mint quote request payload into a validated structure.
pub fn parse_mint_quote_request(payload: &[u8]) -> Result<ParsedMintQuoteRequest, QuoteParseError> {
    let mut payload_copy = payload.to_vec();

    let parsed_request: MintQuoteRequest =
        binary_sv2::from_bytes(&mut payload_copy).map_err(QuoteParseError::Decode)?;

    let share_hash = ShareHash::from_u256(&parsed_request.header_hash)?;
    let request = into_static_request(parsed_request, share_hash)?;

    Ok(ParsedMintQuoteRequest {
        request,
        share_hash,
    })
}

/// Convert a CDK mint quote response back into the SV2 wire format.
pub fn mint_quote_response_from_cdk(
    share_hash: ShareHash,
    quote: MintQuote,
) -> Result<MintQuoteResponse<'static>, QuoteConversionError> {
    let quote_id =
        Str0255::try_from(quote.id.to_string()).map_err(QuoteConversionError::InvalidQuoteId)?;

    let header_hash = share_hash.into_u256()?;

    Ok(MintQuoteResponse {
        quote_id,
        header_hash,
    })
}

fn into_static_request(
    request: MintQuoteRequest<'_>,
    share_hash: ShareHash,
) -> Result<MintQuoteRequest<'static>, QuoteParseError> {
    let unit = request
        .unit
        .inner_as_ref()
        .to_vec()
        .try_into()
        .map_err(QuoteParseError::InvalidUnit)?;

    let description = match request.description.into_inner() {
        Some(desc) => {
            let inner = desc
                .inner_as_ref()
                .to_vec()
                .try_into()
                .map_err(QuoteParseError::InvalidDescription)?;
            Sv2Option::new(Some(inner))
        }
        None => Sv2Option::new(None),
    };

    let locking_key = request.locking_key.into_static();

    let header_hash = share_hash.into_u256()?;

    Ok(MintQuoteRequest {
        amount: request.amount,
        unit,
        header_hash,
        description,
        locking_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_sv2::{to_bytes, CompressedPubKey};
    use secp256k1::{Secp256k1, SecretKey};

    fn sample_locking_key() -> (CompressedPubKey<'static>, PublicKey) {
        let secp = Secp256k1::new();
        let sk = SecretKey::from_slice(&[1u8; 32]).expect("valid secret key");
        let pk = secp256k1::PublicKey::from_secret_key(&secp, &sk);

        let mut bytes = pk.serialize();
        let compressed = CompressedPubKey::from_bytes(&mut bytes)
            .expect("compress pubkey")
            .into_static();
        let cdk_pub = PublicKey::from_slice(&bytes).expect("cdk public key");
        (compressed, cdk_pub)
    }

    #[test]
    fn builds_request_successfully() {
        let hash = [0xAAu8; 32];
        let (locking_key, _) = sample_locking_key();
        let req = build_mint_quote_request(42, &hash, locking_key).unwrap();
        assert_eq!(req.amount, 42);
        assert_eq!(req.unit.inner_as_ref(), b"HASH");
        assert_eq!(req.header_hash.inner_as_ref(), &hash);
    }

    #[test]
    fn rejects_header_hash_with_wrong_size() {
        let (locking_key, _) = sample_locking_key();
        let err = build_mint_quote_request(1, &[0u8; 31], locking_key).unwrap_err();
        match err {
            QuoteBuildError::InvalidHeaderHashLength(31) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn parses_and_converts_request_payload() {
        let hash = [0x11u8; 32];
        let (locking_key, expected_pubkey) = sample_locking_key();
        let request = build_mint_quote_request(10, &hash, locking_key).unwrap();

        let mut encoded = vec![];
        to_bytes(&request, &mut encoded).expect("encode quote request");

        let parsed = parse_mint_quote_request(&encoded).expect("parse payload");
        assert_eq!(parsed.share_hash.as_bytes(), &hash);
        assert_eq!(parsed.request.amount, 10);

        let cdk_request = parsed.to_cdk_request().expect("convert to cdk");
        assert_eq!(cdk_request.amount, Amount::from(10_u64));
        assert_eq!(cdk_request.unit, CurrencyUnit::Hash);
        assert_eq!(cdk_request.pubkey, expected_pubkey);
    }
}
