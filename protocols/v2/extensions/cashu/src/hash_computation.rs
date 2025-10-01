//! Hash computation for share verification and mint quote identification
//!
//! This module computes block hashes from share submission data, eliminating
//! the need to transmit hash values as TLV fields. The hash is computed from
//! the standard share fields (job_id, nonce, ntime, version, extranonce).

extern crate alloc;
use alloc::vec::Vec;

/// Compute the share hash from submission fields
/// 
/// This function reconstructs the block header hash that would be produced
/// by the mining operation described in the share submission. This hash
/// is used for:
/// 1. Mint quote identification
/// 2. Future verification proofs
/// 
/// # Arguments
/// * `job_id` - Job identifier referencing the block template
/// * `nonce` - The nonce value that was tried
/// * `ntime` - The timestamp used in the block header
/// * `version` - The block version field
/// * `extranonce` - Additional nonce bytes for the coinbase
/// 
/// # Returns
/// 32-byte SHA256d hash of the block header
/// 
/// # Note
/// This is a placeholder implementation. The actual implementation should:
/// 1. Look up the block template using job_id
/// 2. Reconstruct the complete block header
/// 3. Compute SHA256d(block_header)
pub fn compute_share_hash(
    job_id: u32,
    nonce: u32,
    ntime: u32,
    version: u32,
    extranonce: &[u8],
) -> [u8; 32] {
    // PLACEHOLDER IMPLEMENTATION
    // TODO: Replace with actual block header reconstruction and hashing
    
    // For now, create a deterministic hash from the input fields
    // This ensures consistent behavior during development
    let mut data = Vec::new();
    data.extend_from_slice(&job_id.to_le_bytes());
    data.extend_from_slice(&nonce.to_le_bytes());
    data.extend_from_slice(&ntime.to_le_bytes());
    data.extend_from_slice(&version.to_le_bytes());
    data.extend_from_slice(extranonce);
    
    // Simple deterministic hash for placeholder (no_std compatible)
    let mut result = [0u8; 32];
    
    // Create a simple hash by combining input bytes
    let mut hash_state: u64 = 0x1234567890ABCDEF; // Initial state
    
    for (i, &byte) in data.iter().enumerate() {
        hash_state = hash_state.wrapping_mul(31).wrapping_add(byte as u64);
        hash_state = hash_state.rotate_left(7);
        
        // Write to result array
        result[i % 32] ^= (hash_state >> (i % 64)) as u8;
    }
    
    // Second pass for better mixing
    for i in 0..32 {
        let pos = (i * 7) % 32;
        result[i] ^= result[pos];
    }
    
    result
}

/// Extract share fields from SubmitSharesExtended message
/// 
/// Helper function to extract the fields needed for hash computation
/// from a SubmitSharesExtended message.
pub fn extract_share_fields(
    _channel_id: u32,
    _sequence_number: u32, 
    job_id: u32,
    nonce: u32,
    ntime: u32,
    version: u32,
    extranonce: &[u8],
) -> (u32, u32, u32, u32, Vec<u8>) {
    (job_id, nonce, ntime, version, extranonce.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_hash_computation_deterministic() {
        let job_id = 12345;
        let nonce = 67890;
        let ntime = 1640995200; // 2022-01-01 00:00:00 UTC
        let version = 0x20000000;
        let extranonce = vec![1, 2, 3, 4];

        let hash1 = compute_share_hash(job_id, nonce, ntime, version, &extranonce);
        let hash2 = compute_share_hash(job_id, nonce, ntime, version, &extranonce);

        // Hash should be deterministic
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 32);
    }

    #[test]
    fn test_hash_computation_different_inputs() {
        let extranonce = vec![1, 2, 3, 4];

        let hash1 = compute_share_hash(1, 100, 1000, 0x20000000, &extranonce);
        let hash2 = compute_share_hash(2, 100, 1000, 0x20000000, &extranonce); // Different job_id
        let hash3 = compute_share_hash(1, 200, 1000, 0x20000000, &extranonce); // Different nonce

        // Different inputs should produce different hashes
        assert_ne!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_ne!(hash2, hash3);
    }

    #[test]
    fn test_extract_share_fields() {
        let extranonce = vec![5, 6, 7, 8];
        let (job_id, nonce, ntime, version, extracted_extranonce) = 
            extract_share_fields(1, 2, 3, 4, 5, 6, &extranonce);

        assert_eq!(job_id, 3);
        assert_eq!(nonce, 4);
        assert_eq!(ntime, 5);
        assert_eq!(version, 6);
        assert_eq!(extracted_extranonce, extranonce);
    }
}