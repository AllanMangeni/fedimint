use std::hash::Hash;

pub use common::{BackupRequest, SignedBackupRequest};
use config::MintClientConfig;
use fedimint_core::core::{Decoder, ModuleInstanceId, ModuleKind};
use fedimint_core::encoding::{Decodable, Encodable};
use fedimint_core::module::{CommonModuleInit, ModuleCommon, ModuleConsensusVersion};
use fedimint_core::{extensible_associated_module_type, plugin_types_trait_impl_common, Amount};
use serde::{Deserialize, Serialize};
use tbs::BlindedSignatureShare;
use thiserror::Error;
use tracing::error;

pub mod common;
pub mod config;
pub mod endpoint_constants;

pub const KIND: ModuleKind = ModuleKind::from_static_str("mint");
pub const MODULE_CONSENSUS_VERSION: ModuleConsensusVersion = ModuleConsensusVersion::new(2, 0);

/// By default, the maximum notes per denomination when change-making for users
pub const DEFAULT_MAX_NOTES_PER_DENOMINATION: u16 = 3;

/// The mint module currently doesn't define any consensus items and generally
/// throws an error on encountering one. To allow old clients to still decode
/// blocks in the future, should we decide to add consensus items, this has to
/// be an enum with only a default variant.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encodable, Decodable)]
pub enum MintConsensusItem {
    #[encodable_default]
    Default { variant: u64, bytes: Vec<u8> },
}

impl std::fmt::Display for MintConsensusItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MintConsensusItem")
    }
}

/// Result of Federation members confirming [`MintOutput`] by contributing
/// partial signatures via [`MintConsensusItem`]
///
/// A set of full blinded signatures.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize, Encodable, Decodable)]
pub struct MintOutputBlindSignature(pub tbs::BlindedSignature);

/// An verifiable one time use IOU from the mint.
///
/// Digital version of a "note of deposit" in a free-banking era.
///
/// Consist of a user-generated nonce and a threshold signature over it
/// generated by the federated mint (while in a [`BlindNonce`] form).
///
/// As things are right now the denomination of each note is determined by the
/// federation keys that signed over it, and needs to be tracked outside of this
/// type.
///
/// In this form it can only be validated, not spent since for that the
/// corresponding secret spend key is required.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Deserialize, Serialize, Encodable, Decodable)]
pub struct Note {
    pub nonce: Nonce,
    pub signature: tbs::Signature,
}

/// Unique ID of a mint note.
///
/// User-generated, random or otherwise unpredictably generated
/// (deterministically derived).
///
/// Internally a MuSig pub key so that transactions can be signed when being
/// spent.
#[derive(
    Debug,
    Copy,
    Clone,
    Eq,
    PartialEq,
    PartialOrd,
    Ord,
    Hash,
    Deserialize,
    Serialize,
    Encodable,
    Decodable,
)]
pub struct Nonce(pub secp256k1_zkp::PublicKey);

/// [`Nonce`] but blinded by the user key
///
/// Blinding prevents the Mint from being able to link the transaction spending
/// [`Note`]s as an `Input`s of `Transaction` with new [`Note`]s being created
/// in its `Output`s.
///
/// By signing it, the mint commits to the underlying (unblinded) [`Nonce`] as
/// valid (until eventually spent).
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Deserialize, Serialize, Encodable, Decodable)]
pub struct BlindNonce(pub tbs::BlindedMessage);

#[derive(Debug)]
pub struct MintCommonInit;

impl CommonModuleInit for MintCommonInit {
    const CONSENSUS_VERSION: ModuleConsensusVersion = MODULE_CONSENSUS_VERSION;
    const KIND: ModuleKind = KIND;

    type ClientConfig = MintClientConfig;

    fn decoder() -> Decoder {
        MintModuleTypes::decoder_builder().build()
    }
}

extensible_associated_module_type!(MintInput, MintInputV0, UnknownMintInputVariantError);

impl MintInput {
    pub fn new_v0(amount: Amount, note: Note) -> MintInput {
        MintInput::V0(MintInputV0 { amount, note })
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize, Encodable, Decodable)]
pub struct MintInputV0 {
    pub amount: Amount,
    pub note: Note,
}

impl std::fmt::Display for MintInputV0 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mint Note {}", self.amount)
    }
}

extensible_associated_module_type!(MintOutput, MintOutputV0, UnknownMintOutputVariantError);

impl MintOutput {
    pub fn new_v0(amount: Amount, blind_nonce: BlindNonce) -> MintOutput {
        MintOutput::V0(MintOutputV0 {
            amount,
            blind_nonce,
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize, Encodable, Decodable)]
pub struct MintOutputV0 {
    pub amount: Amount,
    pub blind_nonce: BlindNonce,
}

impl std::fmt::Display for MintOutputV0 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mint Note {}", self.amount)
    }
}

extensible_associated_module_type!(
    MintOutputOutcome,
    MintOutputOutcomeV0,
    UnknownMintOutputOutcomeVariantError
);

impl MintOutputOutcome {
    pub fn new_v0(blind_signature_share: BlindedSignatureShare) -> MintOutputOutcome {
        MintOutputOutcome::V0(MintOutputOutcomeV0(blind_signature_share))
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize, Serialize, Encodable, Decodable)]
pub struct MintOutputOutcomeV0(pub tbs::BlindedSignatureShare);

impl std::fmt::Display for MintOutputOutcomeV0 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MintOutputOutcome")
    }
}

pub struct MintModuleTypes;

impl Note {
    /// Verify the note's validity under a mit key `pk`
    pub fn verify(&self, pk: tbs::AggregatePublicKey) -> bool {
        tbs::verify(self.nonce.to_message(), self.signature, pk)
    }

    /// Access the nonce as the public key to the spend key
    pub fn spend_key(&self) -> &secp256k1_zkp::PublicKey {
        &self.nonce.0
    }
}

impl Nonce {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = vec![];
        bincode::serialize_into(&mut bytes, &self.0).unwrap();
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        // FIXME: handle errors or the client can be crashed
        bincode::deserialize(bytes).unwrap()
    }

    pub fn to_message(&self) -> tbs::Message {
        tbs::Message::from_bytes(&self.0.serialize()[..])
    }
}

plugin_types_trait_impl_common!(
    MintModuleTypes,
    MintClientConfig,
    MintInput,
    MintOutput,
    MintOutputOutcome,
    MintConsensusItem,
    MintInputError,
    MintOutputError
);

#[derive(Debug, Clone, Eq, PartialEq, Hash, Error, Encodable, Decodable)]
pub enum MintInputError {
    #[error("The note is already spent")]
    SpentCoin,
    #[error("The note has an invalid amount not issued by the mint: {0:?}")]
    InvalidAmountTier(Amount),
    #[error("The note has an invalid signature")]
    InvalidSignature,
    #[error("The mint input version is not supported by this federation")]
    UnknownInputVariant(#[from] UnknownMintInputVariantError),
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Error, Encodable, Decodable)]
pub enum MintOutputError {
    #[error("The note has an invalid amount not issued by the mint: {0:?}")]
    InvalidAmountTier(Amount),
    #[error("The mint output version is not supported by this federation")]
    UnknownOutputVariant(#[from] UnknownMintOutputVariantError),
}
