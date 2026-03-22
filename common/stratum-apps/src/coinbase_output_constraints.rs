use stratum_core::{
    bitcoin::{
        absolute::LockTime,
        script::ScriptBuf,
        transaction::{OutPoint, Sequence, Transaction, TxIn, TxOut, Version},
        witness::Witness,
    },
    template_distribution_sv2::CoinbaseOutputConstraints,
};

/// Creates a CoinbaseOutputConstraints message from a list of coinbase outputs
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
