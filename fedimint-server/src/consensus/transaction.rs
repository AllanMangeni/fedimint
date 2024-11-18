use fedimint_core::db::DatabaseTransaction;
use fedimint_core::module::registry::ServerModuleRegistry;
use fedimint_core::module::{CoreConsensusVersion, TransactionItemAmount};
use fedimint_core::transaction::{Transaction, TransactionError, TRANSACTION_OVERFLOW_ERROR};
use fedimint_core::{Amount, OutPoint};
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::metrics::{CONSENSUS_TX_PROCESSED_INPUTS, CONSENSUS_TX_PROCESSED_OUTPUTS};

pub async fn process_transaction_with_dbtx(
    modules: ServerModuleRegistry,
    dbtx: &mut DatabaseTransaction<'_>,
    transaction: &Transaction,
    version: CoreConsensusVersion,
) -> Result<(), TransactionError> {
    let in_count = transaction.inputs.len();
    let out_count = transaction.outputs.len();

    dbtx.on_commit(move || {
        CONSENSUS_TX_PROCESSED_INPUTS.observe(in_count as f64);
        CONSENSUS_TX_PROCESSED_OUTPUTS.observe(out_count as f64);
    });

    // We can not return the error here as errors are not returned in a specified
    // order and the client still expects consensus on the error. Since the
    // error is not extensible at the moment we need to incorrectly return the
    // InvalidWitnessLength variant.
    transaction
        .inputs
        .clone()
        .into_par_iter()
        .try_for_each(|input| {
            modules
                .get_expect(input.module_instance_id())
                .verify_input(&input)
        })
        .map_err(|_| TransactionError::InvalidWitnessLength)?;

    let mut funding_verifier = FundingVerifier::default();
    let mut public_keys = Vec::new();

    for input in &transaction.inputs {
        let meta = modules
            .get_expect(input.module_instance_id())
            .process_input(
                &mut dbtx
                    .to_ref_with_prefix_module_id(input.module_instance_id())
                    .0,
                input,
            )
            .await
            .map_err(TransactionError::Input)?;

        funding_verifier.add_input(meta.amount)?;
        public_keys.push(meta.pub_key);
    }

    transaction.validate_signatures(&public_keys)?;

    let txid = transaction.tx_hash();

    for (output, out_idx) in transaction.outputs.iter().zip(0u64..) {
        let amount = modules
            .get_expect(output.module_instance_id())
            .process_output(
                &mut dbtx
                    .to_ref_with_prefix_module_id(output.module_instance_id())
                    .0,
                output,
                OutPoint { txid, out_idx },
            )
            .await
            .map_err(TransactionError::Output)?;

        funding_verifier.add_output(amount)?;
    }

    funding_verifier.verify_funding(version)?;

    Ok(())
}

pub struct FundingVerifier {
    input_amount: Amount,
    output_amount: Amount,
    fee_amount: Amount,
}

impl FundingVerifier {
    pub fn add_input(
        &mut self,
        input_amount: TransactionItemAmount,
    ) -> Result<(), TransactionError> {
        self.input_amount = self
            .input_amount
            .checked_add(input_amount.amount)
            .ok_or(TRANSACTION_OVERFLOW_ERROR)?;

        self.fee_amount = self
            .fee_amount
            .checked_add(input_amount.fee)
            .ok_or(TRANSACTION_OVERFLOW_ERROR)?;

        Ok(())
    }

    pub fn add_output(
        &mut self,
        output_amount: TransactionItemAmount,
    ) -> Result<(), TransactionError> {
        self.output_amount = self
            .output_amount
            .checked_add(output_amount.amount)
            .ok_or(TRANSACTION_OVERFLOW_ERROR)?;

        self.fee_amount = self
            .fee_amount
            .checked_add(output_amount.fee)
            .ok_or(TRANSACTION_OVERFLOW_ERROR)?;

        Ok(())
    }

    pub fn verify_funding(self, version: CoreConsensusVersion) -> Result<(), TransactionError> {
        let outputs_and_fees = self
            .output_amount
            .checked_add(self.fee_amount)
            .ok_or(TRANSACTION_OVERFLOW_ERROR)?;

        if self.input_amount == outputs_and_fees {
            return Ok(());
        }

        if self.input_amount > outputs_and_fees && version >= CoreConsensusVersion::new(2, 1) {
            return Ok(());
        }

        Err(TransactionError::UnbalancedTransaction {
            inputs: self.input_amount,
            outputs: self.output_amount,
            fee: self.fee_amount,
        })
    }
}

impl Default for FundingVerifier {
    fn default() -> Self {
        FundingVerifier {
            input_amount: Amount::ZERO,
            output_amount: Amount::ZERO,
            fee_amount: Amount::ZERO,
        }
    }
}
