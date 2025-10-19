use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::ensure;
use bitcoin::key::Keypair;
use bitcoin::secp256k1;
use fedimint_core::core::{DynInput, DynOutput};
use fedimint_core::db::{
    Database, DatabaseVersion, DatabaseVersionKeyV0, IDatabaseTransactionOpsCoreTyped,
};
use fedimint_core::epoch::ConsensusItem;
use fedimint_core::module::registry::ModuleDecoderRegistry;
use fedimint_core::module::{AmountUnit, CommonModuleInit};
use fedimint_core::net::api_announcement::{ApiAnnouncement, SignedApiAnnouncement};
use fedimint_core::secp256k1::Message;
use fedimint_core::secp256k1::rand::rngs::OsRng;
use fedimint_core::secp256k1::rand::thread_rng;
use fedimint_core::session_outcome::{AcceptedItem, SessionOutcome, SignedSessionOutcome};
use fedimint_core::transaction::{Transaction, TransactionSignature};
use fedimint_core::{Amount, BitcoinHash, PeerId, TransactionId, anyhow};
use fedimint_dummy_common::{
    DummyCommonInit, DummyInput, DummyInputV1, DummyOutput, DummyOutputV1,
};
use fedimint_dummy_server::Dummy;
use fedimint_logging::{LOG_DB, TracingSetup};
use fedimint_server::consensus::db::{
    AcceptedItemKey, AcceptedItemPrefix, AcceptedTransactionKey, AcceptedTransactionKeyPrefix,
    AlephUnitsKey, AlephUnitsPrefix, ServerDbMigrationContext, SignedSessionOutcomeKey,
    SignedSessionOutcomePrefix, get_global_database_migrations,
};
use fedimint_server::core::ServerModule;
use fedimint_server::db::DbKeyPrefix;
use fedimint_server::net::api::announcement::{ApiAnnouncementKey, ApiAnnouncementPrefix};
use fedimint_testing_core::db::{
    BYTE_32, TEST_MODULE_INSTANCE_ID, snapshot_db_migrations_with_decoders,
    validate_migrations_global,
};
use futures::StreamExt;
use strum::IntoEnumIterator as _;
use tracing::info;

/// Create a database with version 0 data. The database produced is not
/// intended to be real data or semantically correct. It is only
/// intended to provide coverage when reading the database
/// in future code versions. This function should not be updated when
/// database keys/values change - instead a new function should be added
/// that creates a new database backup that can be tested.
async fn create_server_db_with_v0_data(db: Database) {
    let mut dbtx = db.begin_transaction().await;

    // Will be migrated to `DatabaseVersionKey` during `apply_migrations`
    dbtx.insert_new_entry(&DatabaseVersionKeyV0, &DatabaseVersion(0))
        .await;

    let accepted_tx_id = AcceptedTransactionKey(TransactionId::from_slice(&BYTE_32).unwrap());

    let (sk, _) = secp256k1::generate_keypair(&mut OsRng);
    let secp = secp256k1::Secp256k1::new();
    let key_pair = Keypair::from_secret_key(&secp, &sk);
    let schnorr = secp.sign_schnorr_with_rng(
        &Message::from_digest_slice(&BYTE_32).unwrap(),
        &key_pair,
        &mut thread_rng(),
    );
    let transaction = Transaction {
        inputs: vec![DynInput::from_typed::<DummyInput>(
            0,
            DummyInputV1 {
                amount: Amount::ZERO,
                unit: AmountUnit::BITCOIN,
                account: key_pair.public_key(),
            }
            .into(),
        )],
        outputs: vec![DynOutput::from_typed::<DummyOutput>(
            0,
            DummyOutputV1 {
                amount: Amount::ZERO,
                unit: AmountUnit::BITCOIN,
                account: key_pair.public_key(),
            }
            .into(),
        )],
        nonce: [0x42; 8],
        signatures: TransactionSignature::NaiveMultisig(vec![schnorr]),
    };

    let module_ids = transaction
        .outputs
        .iter()
        .map(DynOutput::module_instance_id)
        .collect::<Vec<_>>();

    dbtx.insert_new_entry(&accepted_tx_id, &module_ids).await;

    dbtx.insert_new_entry(
        &AcceptedItemKey(0),
        &AcceptedItem {
            item: ConsensusItem::Transaction(transaction.clone()),
            peer: PeerId::from_str("0").unwrap(),
        },
    )
    .await;

    dbtx.insert_new_entry(
        &SignedSessionOutcomeKey(0),
        &SignedSessionOutcome {
            session_outcome: SessionOutcome { items: Vec::new() },
            signatures: BTreeMap::new(),
        },
    )
    .await;

    dbtx.insert_new_entry(&AlephUnitsKey(0), &vec![42, 42, 42])
        .await;

    dbtx.insert_new_entry(
        &ApiAnnouncementKey(PeerId::from(42)),
        &SignedApiAnnouncement {
            api_announcement: ApiAnnouncement {
                api_url: "wss://foo.bar".parse().expect("valid url"),
                nonce: 0,
            },
            signature: secp256k1::schnorr::Signature::from_slice(&[42; 64]).unwrap(),
        },
    )
    .await;

    dbtx.commit_tx().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_server_db_migrations() -> anyhow::Result<()> {
    snapshot_db_migrations_with_decoders(
        "fedimint-server",
        |db| {
            Box::pin(async {
                create_server_db_with_v0_data(db).await;
            })
        },
        ModuleDecoderRegistry::from_iter([(
            TEST_MODULE_INSTANCE_ID,
            DummyCommonInit::KIND,
            <Dummy as ServerModule>::decoder(),
        )]),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn test_server_db_migrations() -> anyhow::Result<()> {
    let _ = TracingSetup::default().init();

    validate_migrations_global(
        |db| async move {
            let mut dbtx = db.begin_transaction_nc().await;

            for prefix in DbKeyPrefix::iter() {
                match prefix {
                    DbKeyPrefix::AcceptedItem => {
                        let accepted_items = dbtx
                            .find_by_prefix(&AcceptedItemPrefix)
                            .await
                            .collect::<Vec<_>>()
                            .await;
                        let accepted_items = accepted_items.len();
                        ensure!(
                            accepted_items > 0,
                            "validate_migrations was not able to read any AcceptedItems"
                        );
                        info!(target: LOG_DB, "Validated AcceptedItems");
                    }
                    DbKeyPrefix::AcceptedTransaction => {
                        let accepted_transactions = dbtx
                            .find_by_prefix(&AcceptedTransactionKeyPrefix)
                            .await
                            .collect::<Vec<_>>()
                            .await;
                        let num_accepted_transactions = accepted_transactions.len();
                        ensure!(
                            num_accepted_transactions > 0,
                            "validate_migrations was not able to read any AcceptedTransactions"
                        );
                        info!(target: LOG_DB, "Validated AcceptedTransactions");
                    }
                    DbKeyPrefix::SignedSessionOutcome => {
                        let signed_session_outcomes = dbtx
                            .find_by_prefix(&SignedSessionOutcomePrefix)
                            .await
                            .collect::<Vec<_>>()
                            .await;
                        let num_signed_session_outcomes = signed_session_outcomes.len();
                        ensure!(
                            num_signed_session_outcomes > 0,
                            "validate_migrations was not able to read any SignedSessionOutcomes"
                        );
                        info!(target: LOG_DB, "Validated SignedSessionOutcome");
                    }
                    DbKeyPrefix::AlephUnits => {
                        let aleph_units = dbtx
                            .find_by_prefix(&AlephUnitsPrefix)
                            .await
                            .collect::<Vec<_>>()
                            .await;
                        let num_aleph_units = aleph_units.len();
                        ensure!(
                            num_aleph_units > 0,
                            "validate_migrations was not able to read any AlephUnits"
                        );
                        info!(target: LOG_DB, "Validated AlephUnits");
                    }
                    // Module prefix is reserved for modules, no migration testing is needed
                    DbKeyPrefix::Module
                    | DbKeyPrefix::ServerInfo
                    | DbKeyPrefix::DatabaseVersion
                    | DbKeyPrefix::ClientBackup => {}
                    DbKeyPrefix::ApiAnnouncements => {
                        let announcements = dbtx
                            .find_by_prefix(&ApiAnnouncementPrefix)
                            .await
                            .collect::<Vec<_>>()
                            .await;

                        assert_eq!(announcements.len(), 1);
                    }
                }
            }
            Ok(())
        },
        Arc::new(ServerDbMigrationContext) as Arc<_>,
        "fedimint-server",
        get_global_database_migrations(),
        ModuleDecoderRegistry::from_iter([(
            TEST_MODULE_INSTANCE_ID,
            DummyCommonInit::KIND,
            <Dummy as ServerModule>::decoder(),
        )]),
    )
    .await
}
