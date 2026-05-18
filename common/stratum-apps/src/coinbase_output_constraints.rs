use stratum_core::{
    bitcoin::{
        absolute::LockTime,
        script::ScriptBuf,
        transaction::{OutPoint, Sequence, Transaction, TxIn, TxOut, Version},
        witness::Witness,
    },
    template_distribution_sv2::CoinbaseOutputConstraints,
};

/// Offset added to the calculated max size to account for the additional space required by
/// Pay-to-Taproot (p2tr) addresses in solo mining scenarios.
///
/// Rationale: p2tr (Taproot) addresses produce the largest coinbase outputs among standard
/// address types. When the actual mining address is different from what was used to calculate
/// the constraints, we need extra space to accommodate the largest possible address type.
/// The value 43 bytes accounts for the difference between typical outputs and p2tr outputs.
const OFFSET_ADDITIONAL_SIZE: u32 = 43;

/// Offset added to the calculated max sigops to account for the additional signature operations
/// required by Pay-to-Public-Key-Hash (p2pkh) addresses in solo mining scenarios.
///
/// Rationale: p2pkh addresses have higher sigop counts compared to modern address types.
/// When the actual mining address differs from what was used to calculate constraints,
/// we need extra sigops budget to handle the worst-case scenario.
/// The value 4 sigops accounts for p2pkh's higher signature operation count.
const OFFSET_MAX_SIGOPS: u16 = 4;

/// Creates a CoinbaseOutputConstraints message from a list of coinbase outputs.
///
/// This function calculates the exact maximum additional size and sigops required
/// by the given coinbase outputs. No safety margins are added - the values reflect
/// precisely what the provided outputs require.
///
/// # Arguments
/// * `coinbase_outputs` - List of transaction outputs that will be included in the coinbase
///
/// # Returns
/// CoinbaseOutputConstraints with exact size and sigops values based on the provided outputs
pub fn coinbase_output_constraints_message(
    coinbase_outputs: Vec<TxOut>,
) -> CoinbaseOutputConstraints {
    // calculate the max coinbase output size for CoinbaseOutputConstraints
    let max_size: u32 = coinbase_outputs.iter().map(|o| o.size() as u32).sum();
    tracing::debug!(
        max_size,
        outputs_count = coinbase_outputs.len(),
        "Calculated max coinbase output size"
    );

    // this is used to calculate the sigops of the coinbase outputs
    // for CoinbaseOutputConstraints
    let dummy_coinbase = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::from(vec![vec![0; 32]]),
        }],
        output: coinbase_outputs,
    };

    let max_sigops = dummy_coinbase.total_sigop_cost(|_| None) as u16;
    tracing::debug!(max_sigops, "Calculated max sigops for coinbase");

    CoinbaseOutputConstraints {
        coinbase_output_max_additional_size: max_size,
        coinbase_output_max_additional_sigops: max_sigops,
    }
}

/// Creates a CoinbaseOutputConstraints message with safety margins (offsets) applied.
///
/// This function first calculates the exact constraints using
/// [`coinbase_output_constraints_message`], then adds safety margins to account for address type
/// variation in solo mining scenarios.
///
/// The offsets ensure that the coinbase can accommodate the largest possible address types
/// (p2tr for size, p2pkh for sigops) even when the actual mining address differs from what
/// was used to generate the coinbase outputs. This prevents validation failures when the
/// pool assigns a different address type than what the miner expected.
///
/// # Arguments
/// * `coinbase_outputs` - List of transaction outputs that will be included in the coinbase
///
/// # Returns
/// CoinbaseOutputConstraints with added safety margins for address type variation
///
/// # Offsets Applied
/// - **Size**: +43 bytes to accommodate Pay-to-Taproot (p2tr) addresses
/// - **Sigops**: +4 to accommodate Pay-to-Public-Key-Hash (p2pkh) addresses
pub fn coinbase_output_constraints_message_with_offset(
    coinbase_outputs: Vec<TxOut>,
) -> CoinbaseOutputConstraints {
    let constraints = coinbase_output_constraints_message(coinbase_outputs);

    CoinbaseOutputConstraints {
        coinbase_output_max_additional_size: constraints.coinbase_output_max_additional_size
            + OFFSET_ADDITIONAL_SIZE,
        coinbase_output_max_additional_sigops: constraints.coinbase_output_max_additional_sigops
            + OFFSET_MAX_SIGOPS,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use stratum_core::bitcoin::{Address, Amount, Network, TxOut};

    /// Informational test to showcase how the offset values were calculated.
    ///
    /// This test iterates through all standard Bitcoin address types and prints their
    /// sizes and sigop counts. Run with `--nocapture` to see the output.
    ///
    /// The offsets were determined by finding the worst-case address types:
    /// - **Size**: p2tr (Taproot) produces the largest coinbase output (43 bytes larger than
    ///   others)
    /// - **Sigops**: p2pkh has higher sigop count than modern address types (4 sigops)
    ///
    /// Since the pool cannot know in advance which address type the miner will use,
    /// we add these safety margins to ensure the coinbase can accommodate any address type.
    #[test]
    fn test_offset_values_rationale() {
        let addresses = vec![
            ("p2pkh", "19drg6CgjcvqFZSW5FLWdmqTBBeFLS5iC7"),
            ("p2sh", "18tRWCdi2Fc9CM57fUfmFK3ZC6cpGQeBkV"),
            ("p2wpkh", "bc1qwq787dzgj2w8hh58t4clr594y0cjgjashr0fz5"),
            ("p2wsh", "bc1qn04san36d0j76j0xksz2tesmtww7uf24j2x6v3"),
            (
                "p2tr",
                "bc1p8fltq0npm605tzl22gqewhy9dt5l25m7x67832vyhh9aem24rgdsgqwtpu",
            ),
        ];

        let mut max_size = 0u32;
        let mut max_sigops = 0u16;
        let mut max_size_type = "";
        let mut max_sigops_type = "";

        for (name, addr_str) in addresses {
            let addr = Address::from_str(addr_str)
                .unwrap()
                .require_network(Network::Bitcoin)
                .unwrap();

            let script = addr.script_pubkey();

            let coinbase_outputs = vec![TxOut {
                value: Amount::from_sat(50_0000_0000),
                script_pubkey: script.clone(),
            }];

            let dummy_coinbase = Transaction {
                version: Version::TWO,
                lock_time: LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint::null(),
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Witness::from(vec![vec![0; 32]]),
                }],
                output: coinbase_outputs.clone(),
            };

            let size_of_current_address: u32 =
                coinbase_outputs.iter().map(|o| o.size() as u32).sum();
            let sigops_of_current_address = dummy_coinbase.total_sigop_cost(|_| None) as u16;

            if size_of_current_address > max_size {
                max_size = size_of_current_address;
                max_size_type = name;
            }

            if sigops_of_current_address > max_sigops {
                max_sigops = sigops_of_current_address;
                max_sigops_type = name;
            }

            println!("--- {}", name);
            println!("address: {}", addr);
            println!("script: {}", script.to_hex_string());
            println!(
                "max_size={} max_sigops={}",
                size_of_current_address, sigops_of_current_address
            );
        }

        println!("\n=== worst case summary ===");
        println!("largest output size: {} ({})", max_size, max_size_type);
        println!("largest sigops: {} ({})", max_sigops, max_sigops_type);
    }

    /// Tests that `coinbase_output_constraints_message` returns exact values without offsets.
    #[test]
    fn test_coinbase_output_constraints_message_exact() {
        let coinbase_outputs = vec![TxOut {
            value: Amount::from_sat(50_0000_0000),
            script_pubkey: ScriptBuf::from_hex(
                "0020aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
        }];

        let result = coinbase_output_constraints_message(coinbase_outputs.clone());

        let expected_size: u32 = coinbase_outputs.iter().map(|o| o.size() as u32).sum();
        let dummy_coinbase = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::from(vec![vec![0; 32]]),
            }],
            output: coinbase_outputs,
        };
        let expected_sigops = dummy_coinbase.total_sigop_cost(|_| None) as u16;

        assert_eq!(result.coinbase_output_max_additional_size, expected_size);
        assert_eq!(
            result.coinbase_output_max_additional_sigops,
            expected_sigops
        );
    }

    /// Tests that `coinbase_output_constraints_message_with_offset` adds the correct offsets.
    #[test]
    fn test_coinbase_output_constraints_message_with_offset() {
        let coinbase_outputs = vec![TxOut {
            value: Amount::from_sat(50_0000_0000),
            script_pubkey: ScriptBuf::from_hex(
                "0020aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
        }];

        let result_with_offset =
            coinbase_output_constraints_message_with_offset(coinbase_outputs.clone());
        let result_exact = coinbase_output_constraints_message(coinbase_outputs);

        assert_eq!(
            result_with_offset.coinbase_output_max_additional_size,
            result_exact.coinbase_output_max_additional_size + OFFSET_ADDITIONAL_SIZE
        );
        assert_eq!(
            result_with_offset.coinbase_output_max_additional_sigops,
            result_exact.coinbase_output_max_additional_sigops + OFFSET_MAX_SIGOPS
        );
    }

    /// Tests that offsets are applied correctly regardless of input.
    /// Uses a p2wpkh address to verify the offset logic.
    #[test]
    fn test_offset_values_are_applied_correctly() {
        let addr = Address::from_str("bc1qwq787dzgj2w8hh58t4clr594y0cjgjashr0fz5")
            .unwrap()
            .require_network(Network::Bitcoin)
            .unwrap();

        let coinbase_outputs = vec![TxOut {
            value: Amount::from_sat(50_0000_0000),
            script_pubkey: addr.script_pubkey(),
        }];

        let exact = coinbase_output_constraints_message(coinbase_outputs.clone());
        let with_offset = coinbase_output_constraints_message_with_offset(coinbase_outputs);

        assert_eq!(
            with_offset.coinbase_output_max_additional_size,
            exact.coinbase_output_max_additional_size + OFFSET_ADDITIONAL_SIZE
        );
        assert_eq!(
            with_offset.coinbase_output_max_additional_sigops,
            exact.coinbase_output_max_additional_sigops + OFFSET_MAX_SIGOPS
        );
    }
}
