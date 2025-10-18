#![deny(clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]

use core::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail, format_err};
use common::broken_fed_key_pair;
use db::{DbKeyPrefix, DummyClientFundsKeyV1, DummyClientNameKey, migrate_to_v1};
use fedimint_api_client::api::{FederationApiExt, SerdeOutputOutcome, deserialize_outcome};
use fedimint_client_module::db::{ClientModuleMigrationFn, migrate_state};
use fedimint_client_module::module::init::{ClientModuleInit, ClientModuleInitArgs};
use fedimint_client_module::module::recovery::NoModuleBackup;
use fedimint_client_module::module::{
    ClientContext, ClientModule, IClientModule, OutPointRange, PrimaryModulePriority,
    PrimaryModuleSupport,
};
use fedimint_client_module::sm::{Context, ModuleNotifier};
use fedimint_client_module::transaction::{
    ClientInput, ClientInputBundle, ClientInputSM, ClientOutput, ClientOutputBundle,
    ClientOutputSM, TransactionBuilder,
};
use fedimint_core::core::{Decoder, ModuleKind, OperationId};
use fedimint_core::db::{
    Database, DatabaseTransaction, DatabaseVersion, IDatabaseTransactionOpsCoreTyped,
};
#[allow(deprecated)]
use fedimint_core::endpoint_constants::AWAIT_OUTPUT_OUTCOME_ENDPOINT;
use fedimint_core::module::{
    AmountUnit, Amounts, ApiRequestErased, ApiVersion, CommonModuleInit, ModuleCommon, ModuleInit,
    MultiApiVersion,
};
use fedimint_core::secp256k1::{Keypair, PublicKey, Secp256k1};
use fedimint_core::util::{BoxStream, NextOrPending};
use fedimint_core::{Amount, OutPoint, apply, async_trait_maybe_send};
pub use fedimint_dummy_common as common;
use fedimint_dummy_common::config::DummyClientConfig;
use fedimint_dummy_common::{
    DummyCommonInit, DummyInput, DummyModuleTypes, DummyOutput, DummyOutputOutcome, KIND,
    fed_key_pair,
};
use futures::{StreamExt, pin_mut};
use states::DummyStateMachine;
use strum::IntoEnumIterator;

pub mod api;
pub mod db;
pub mod states;

#[derive(Debug)]
pub struct DummyClientModule {
    cfg: DummyClientConfig,
    key: Keypair,
    notifier: ModuleNotifier<DummyStateMachine>,
    client_ctx: ClientContext<Self>,
    db: Database,
}

/// Data needed by the state machine
#[derive(Debug, Clone)]
pub struct DummyClientContext {
    pub dummy_decoder: Decoder,
}

// TODO: Boiler-plate
impl Context for DummyClientContext {
    const KIND: Option<ModuleKind> = None;
}

#[apply(async_trait_maybe_send!)]
impl ClientModule for DummyClientModule {
    type Init = DummyClientInit;
    type Common = DummyModuleTypes;
    type Backup = NoModuleBackup;
    type ModuleStateMachineContext = DummyClientContext;
    type States = DummyStateMachine;

    fn context(&self) -> Self::ModuleStateMachineContext {
        DummyClientContext {
            dummy_decoder: self.decoder(),
        }
    }

    fn input_fee(
        &self,
        _amount: &Amounts,
        _input: &<Self::Common as ModuleCommon>::Input,
    ) -> Option<Amounts> {
        Some(Amounts::new_bitcoin(self.cfg.tx_fee))
    }

    fn output_fee(
        &self,
        _amount: &Amounts,
        _output: &<Self::Common as ModuleCommon>::Output,
    ) -> Option<Amounts> {
        Some(Amounts::new_bitcoin(self.cfg.tx_fee))
    }

    fn supports_being_primary(&self) -> PrimaryModuleSupport {
        PrimaryModuleSupport::selected(PrimaryModulePriority::LOW, [AmountUnit::BITCOIN])
    }

    async fn create_final_inputs_and_outputs(
        &self,
        dbtx: &mut DatabaseTransaction<'_>,
        operation_id: OperationId,
        unit: AmountUnit,
        input_amount: Amount,
        output_amount: Amount,
    ) -> anyhow::Result<(
        ClientInputBundle<DummyInput, DummyStateMachine>,
        ClientOutputBundle<DummyOutput, DummyStateMachine>,
    )> {
        dbtx.ensure_isolated().expect("must be isolated");

        if unit != AmountUnit::BITCOIN {
            bail!("Module can only handle Bitcoin");
        }

        match input_amount.cmp(&output_amount) {
            Ordering::Less => {
                let missing_input_amount = output_amount.saturating_sub(input_amount);

                // Check and subtract from our funds
                let our_funds = get_funds(dbtx).await;

                if our_funds < missing_input_amount {
                    return Err(format_err!("Insufficient funds"));
                }

                let updated = our_funds.saturating_sub(missing_input_amount);

                dbtx.insert_entry(&DummyClientFundsKeyV1, &updated).await;

                let input = ClientInput {
                    input: DummyInput {
                        amount: missing_input_amount,
                        account: self.key.public_key(),
                    },
                    amounts: Amounts::new_bitcoin(missing_input_amount),
                    keys: vec![self.key],
                };
                let input_sm = ClientInputSM {
                    state_machines: Arc::new(move |out_point_range| {
                        vec![DummyStateMachine::Input(
                            missing_input_amount,
                            out_point_range.txid(),
                            operation_id,
                        )]
                    }),
                };

                Ok((
                    ClientInputBundle::new(vec![input], vec![input_sm]),
                    ClientOutputBundle::new(vec![], vec![]),
                ))
            }
            Ordering::Equal => Ok((
                ClientInputBundle::new(vec![], vec![]),
                ClientOutputBundle::new(vec![], vec![]),
            )),
            Ordering::Greater => {
                let missing_output_amount = input_amount.saturating_sub(output_amount);
                let output = ClientOutput {
                    output: DummyOutput {
                        amount: missing_output_amount,
                        account: self.key.public_key(),
                    },
                    amounts: Amounts::new_bitcoin(missing_output_amount),
                };

                let output_sm = ClientOutputSM {
                    state_machines: Arc::new(move |out_point_range| {
                        vec![DummyStateMachine::Output(
                            missing_output_amount,
                            out_point_range.txid(),
                            operation_id,
                        )]
                    }),
                };

                Ok((
                    ClientInputBundle::new(vec![], vec![]),
                    ClientOutputBundle::new(vec![output], vec![output_sm]),
                ))
            }
        }
    }

    async fn await_primary_module_output(
        &self,
        operation_id: OperationId,
        out_point: OutPoint,
    ) -> anyhow::Result<()> {
        let stream = self
            .notifier
            .subscribe(operation_id)
            .await
            .filter_map(|state| async move {
                match state {
                    DummyStateMachine::OutputDone(_, txid, _) => {
                        if txid != out_point.txid {
                            return None;
                        }
                        Some(Ok(()))
                    }
                    DummyStateMachine::Refund(_) => Some(Err(anyhow::anyhow!(
                        "Error occurred processing the dummy transaction"
                    ))),
                    _ => None,
                }
            });

        pin_mut!(stream);

        stream.next_or_pending().await
    }

    async fn get_balance(&self, dbtc: &mut DatabaseTransaction<'_>) -> Amount {
        get_funds(dbtc).await
    }

    async fn subscribe_balance_changes(&self) -> BoxStream<'static, ()> {
        Box::pin(
            self.notifier
                .subscribe_all_operations()
                .filter_map(|state| async move {
                    match state {
                        DummyStateMachine::OutputDone(_, _, _)
                        | DummyStateMachine::Input { .. }
                        | DummyStateMachine::Refund(_) => Some(()),
                        _ => None,
                    }
                }),
        )
    }
}

impl DummyClientModule {
    pub async fn print_using_account(
        &self,
        amount: Amount,
        account_kp: Keypair,
    ) -> anyhow::Result<(OperationId, OutPoint)> {
        let op_id = OperationId(rand::random());

        // TODO: Building a tx could be easier
        // Create input using the fed's account
        let input = ClientInput {
            input: DummyInput {
                amount,
                account: account_kp.public_key(),
            },
            amounts: Amounts::new_bitcoin(amount),
            keys: vec![account_kp],
        };

        // Build and send tx to the fed
        // Will output to our primary client module
        let tx = TransactionBuilder::new().with_inputs(
            self.client_ctx
                .make_client_inputs(ClientInputBundle::new_no_sm(vec![input])),
        );
        let meta_gen = |change_range: OutPointRange| OutPoint {
            txid: change_range.txid(),
            out_idx: 0,
        };
        let change_range = self
            .client_ctx
            .finalize_and_submit_transaction(op_id, KIND.as_str(), meta_gen, tx)
            .await?;

        // Wait for the output of the primary module
        self.client_ctx
            .await_primary_module_outputs(op_id, change_range.into_iter().collect())
            .await
            .context("Waiting for the output of print_using_account")?;

        Ok((
            op_id,
            change_range
                .into_iter()
                .next()
                .expect("At least one output"),
        ))
    }

    /// Request the federation prints money for us
    pub async fn print_money(&self, amount: Amount) -> anyhow::Result<(OperationId, OutPoint)> {
        self.print_using_account(amount, fed_key_pair()).await
    }

    /// Use a broken printer to print a liability instead of money
    /// If the federation is honest, should always fail
    pub async fn print_liability(&self, amount: Amount) -> anyhow::Result<(OperationId, OutPoint)> {
        self.print_using_account(amount, broken_fed_key_pair())
            .await
    }

    /// Send money to another user
    pub async fn send_money(&self, account: PublicKey, amount: Amount) -> anyhow::Result<OutPoint> {
        self.db.ensure_isolated().expect("must be isolated");

        let op_id = OperationId(rand::random());

        // Create output using another account
        let output = ClientOutput {
            output: DummyOutput { amount, account },
            amounts: Amounts::new_bitcoin(amount),
        };

        // Build and send tx to the fed
        let tx = TransactionBuilder::new().with_outputs(
            self.client_ctx
                .make_client_outputs(ClientOutputBundle::new_no_sm(vec![output])),
        );

        let meta_gen = |change_range: OutPointRange| OutPoint {
            txid: change_range.txid(),
            out_idx: 0,
        };
        let change_range = self
            .client_ctx
            .finalize_and_submit_transaction(op_id, DummyCommonInit::KIND.as_str(), meta_gen, tx)
            .await?;

        let tx_subscription = self.client_ctx.transaction_updates(op_id).await;

        tx_subscription
            .await_tx_accepted(change_range.txid())
            .await
            .map_err(|e| anyhow!(e))?;

        Ok(OutPoint {
            txid: change_range.txid(),
            out_idx: 0,
        })
    }

    /// Wait to receive money at an outpoint
    pub async fn receive_money_hack(&self, outpoint: OutPoint) -> anyhow::Result<()> {
        let mut dbtx = self.db.begin_transaction().await;

        #[allow(deprecated)]
        let outcome = self
            .client_ctx
            .global_api()
            .request_current_consensus::<SerdeOutputOutcome>(
                AWAIT_OUTPUT_OUTCOME_ENDPOINT.to_owned(),
                ApiRequestErased::new(outpoint),
            )
            .await?;

        let outcome = deserialize_outcome::<DummyOutputOutcome>(&outcome, &self.decoder())?;

        if outcome.1 != self.key.public_key() {
            return Err(format_err!("Wrong account id"));
        }

        // HACK: This is a terrible hack. The balance is set
        // straight to the amount from the output, assuming that no funds were available
        // before the receive. The actual state machine is supposed to update the
        // balance, but `receive_money` is typically paired with `send_money`
        // which creates a state machine only on the sender's client.
        dbtx.insert_entry(&DummyClientFundsKeyV1, &outcome.0).await;
        dbtx.commit_tx().await;

        Ok(())
    }

    /// Return our account
    pub fn account(&self) -> PublicKey {
        self.key.public_key()
    }
}

async fn get_funds(dbtx: &mut DatabaseTransaction<'_>) -> Amount {
    let funds = dbtx.get_value(&DummyClientFundsKeyV1).await;
    funds.unwrap_or(Amount::ZERO)
}

#[derive(Debug, Clone)]
pub struct DummyClientInit;

// TODO: Boilerplate-code
impl ModuleInit for DummyClientInit {
    type Common = DummyCommonInit;

    async fn dump_database(
        &self,
        dbtx: &mut DatabaseTransaction<'_>,
        prefix_names: Vec<String>,
    ) -> Box<dyn Iterator<Item = (String, Box<dyn erased_serde::Serialize + Send>)> + '_> {
        let mut items: BTreeMap<String, Box<dyn erased_serde::Serialize + Send>> = BTreeMap::new();
        let filtered_prefixes = DbKeyPrefix::iter().filter(|f| {
            prefix_names.is_empty() || prefix_names.contains(&f.to_string().to_lowercase())
        });

        for table in filtered_prefixes {
            match table {
                DbKeyPrefix::ClientFunds => {
                    if let Some(funds) = dbtx.get_value(&DummyClientFundsKeyV1).await {
                        items.insert("Dummy Funds".to_string(), Box::new(funds));
                    }
                }
                DbKeyPrefix::ClientName => {
                    if let Some(name) = dbtx.get_value(&DummyClientNameKey).await {
                        items.insert("Dummy Name".to_string(), Box::new(name));
                    }
                }
                DbKeyPrefix::ExternalReservedStart
                | DbKeyPrefix::CoreInternalReservedStart
                | DbKeyPrefix::CoreInternalReservedEnd => {}
            }
        }

        Box::new(items.into_iter())
    }
}

/// Generates the client module
#[apply(async_trait_maybe_send!)]
impl ClientModuleInit for DummyClientInit {
    type Module = DummyClientModule;

    fn supported_api_versions(&self) -> MultiApiVersion {
        MultiApiVersion::try_from_iter([ApiVersion { major: 0, minor: 0 }])
            .expect("no version conflicts")
    }

    async fn init(&self, args: &ClientModuleInitArgs<Self>) -> anyhow::Result<Self::Module> {
        Ok(DummyClientModule {
            cfg: args.cfg().clone(),
            key: args
                .module_root_secret()
                .clone()
                .to_secp_key(&Secp256k1::new()),

            notifier: args.notifier().clone(),
            client_ctx: args.context(),
            db: args.db().clone(),
        })
    }

    fn get_database_migrations(&self) -> BTreeMap<DatabaseVersion, ClientModuleMigrationFn> {
        let mut migrations: BTreeMap<DatabaseVersion, ClientModuleMigrationFn> = BTreeMap::new();
        migrations.insert(DatabaseVersion(0), |dbtx, _, _| {
            Box::pin(migrate_to_v1(dbtx))
        });

        migrations.insert(DatabaseVersion(1), |_, active_states, inactive_states| {
            Box::pin(async {
                migrate_state(active_states, inactive_states, db::get_v1_migrated_state)
            })
        });

        migrations
    }
}
