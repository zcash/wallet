use std::{collections::HashSet, fmt};

use abscissa_core::Application;
use jsonrpsee::{core::RpcResult, types::ErrorObjectOwned};
use serde::Serialize;
use zaino_state::{FetchServiceSubscriber, ZcashIndexer};
use zcash_client_backend::{data_api::WalletRead, proposal::Proposal};
use zcash_client_sqlite::wallet::Account;
use zcash_keys::address::Address;
use zcash_protocol::{PoolType, ShieldedProtocol, TxId, memo::MemoBytes};

use crate::{components::database::DbConnection, fl, prelude::APP};

use super::server::LegacyCode;

/// A strategy to use for managing privacy when constructing a transaction.
///
/// Policy for what information leakage is acceptable in a transaction created via a
/// JSON-RPC method.
///
/// This should only be used with existing JSON-RPC methods; it was introduced in `zcashd`
/// because shoe-horning cross-pool controls into existing methods was hard. A better
/// approach for new JSON-RPC methods is to design the interaction pattern such that the
/// caller receives a "transaction proposal", and they can consider the privacy
/// implications of a proposal before committing to it.
//
// Note: This intentionally does not implement `PartialOrd`. See `Self::meet` for a
// correct comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PrivacyPolicy {
    /// Only allow fully-shielded transactions (involving a single shielded value pool).
    FullPrivacy,

    /// Allow funds to cross between shielded value pools, revealing the amount that
    /// crosses pools.
    AllowRevealedAmounts,

    /// Allow transparent recipients.
    ///
    /// This also implies revealing information described under
    /// [`PrivacyPolicy::AllowRevealedAmounts`].
    AllowRevealedRecipients,

    /// Allow transparent funds to be spent, revealing the sending addresses and amounts.
    ///
    /// This implies revealing information described under
    /// [`PrivacyPolicy::AllowRevealedAmounts`].
    AllowRevealedSenders,

    /// Allow transaction to both spend transparent funds and have transparent recipients.
    ///
    /// This implies revealing information described under
    /// [`PrivacyPolicy::AllowRevealedSenders`] and
    /// [`PrivacyPolicy::AllowRevealedRecipients`].
    AllowFullyTransparent,

    /// Allow selecting transparent coins from the full account, rather than just the
    /// funds sent to the transparent receiver in the provided Unified Address.
    ///
    /// This implies revealing information described under
    /// [`PrivacyPolicy::AllowRevealedSenders`].
    AllowLinkingAccountAddresses,

    /// Allow the transaction to reveal any information necessary to create it.
    ///
    /// This implies revealing information described under
    /// [`PrivacyPolicy::AllowFullyTransparent`] and
    /// [`PrivacyPolicy::AllowLinkingAccountAddresses`].
    NoPrivacy,
}

impl From<PrivacyPolicy> for &'static str {
    fn from(value: PrivacyPolicy) -> Self {
        match value {
            PrivacyPolicy::FullPrivacy => "FullPrivacy",
            PrivacyPolicy::AllowRevealedAmounts => "AllowRevealedAmounts",
            PrivacyPolicy::AllowRevealedRecipients => "AllowRevealedRecipients",
            PrivacyPolicy::AllowRevealedSenders => "AllowRevealedSenders",
            PrivacyPolicy::AllowFullyTransparent => "AllowFullyTransparent",
            PrivacyPolicy::AllowLinkingAccountAddresses => "AllowLinkingAccountAddresses",
            PrivacyPolicy::NoPrivacy => "NoPrivacy",
        }
    }
}

impl fmt::Display for PrivacyPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", <&'static str>::from(*self))
    }
}

impl PrivacyPolicy {
    pub(super) fn from_str(s: &str) -> Option<Self> {
        match s {
            "FullPrivacy" => Some(Self::FullPrivacy),
            "AllowRevealedAmounts" => Some(Self::AllowRevealedAmounts),
            "AllowRevealedRecipients" => Some(Self::AllowRevealedRecipients),
            "AllowRevealedSenders" => Some(Self::AllowRevealedSenders),
            "AllowFullyTransparent" => Some(Self::AllowFullyTransparent),
            "AllowLinkingAccountAddresses" => Some(Self::AllowLinkingAccountAddresses),
            "NoPrivacy" => Some(Self::NoPrivacy),
            // Unknown privacy policy.
            _ => None,
        }
    }

    /// Returns the meet (greatest lower bound) of `self` and `other`.
    ///
    /// Privacy policies form a lattice where the relation is "strictness". I.e., `x ≤ y`
    /// means "Policy `x` allows at least everything that policy `y` allows."
    ///
    /// This function returns the strictest policy that allows everything allowed by
    /// `self` and also everything allowed by `other`.
    ///
    /// See [zcash/zcash#6240] for the graph that this models.
    ///
    /// [zcash/zcash#6240]: https://github.com/zcash/zcash/issues/6240
    pub(super) fn meet(self, other: Self) -> Self {
        match self {
            PrivacyPolicy::FullPrivacy => other,
            PrivacyPolicy::AllowRevealedAmounts => match other {
                PrivacyPolicy::FullPrivacy => self,
                _ => other,
            },
            PrivacyPolicy::AllowRevealedRecipients => match other {
                PrivacyPolicy::FullPrivacy | PrivacyPolicy::AllowRevealedAmounts => self,
                PrivacyPolicy::AllowRevealedSenders => PrivacyPolicy::AllowFullyTransparent,
                PrivacyPolicy::AllowLinkingAccountAddresses => PrivacyPolicy::NoPrivacy,
                _ => other,
            },
            PrivacyPolicy::AllowRevealedSenders => match other {
                PrivacyPolicy::FullPrivacy | PrivacyPolicy::AllowRevealedAmounts => self,
                PrivacyPolicy::AllowRevealedRecipients => PrivacyPolicy::AllowFullyTransparent,
                _ => other,
            },
            PrivacyPolicy::AllowFullyTransparent => match other {
                PrivacyPolicy::FullPrivacy
                | PrivacyPolicy::AllowRevealedAmounts
                | PrivacyPolicy::AllowRevealedRecipients
                | PrivacyPolicy::AllowRevealedSenders => self,
                PrivacyPolicy::AllowLinkingAccountAddresses => PrivacyPolicy::NoPrivacy,
                _ => other,
            },
            PrivacyPolicy::AllowLinkingAccountAddresses => match other {
                PrivacyPolicy::FullPrivacy
                | PrivacyPolicy::AllowRevealedAmounts
                | PrivacyPolicy::AllowRevealedSenders => self,
                PrivacyPolicy::AllowRevealedRecipients | PrivacyPolicy::AllowFullyTransparent => {
                    PrivacyPolicy::NoPrivacy
                }
                _ => other,
            },
            PrivacyPolicy::NoPrivacy => self,
        }
    }

    /// This policy is compatible with a given policy if it is identical to or less strict
    /// than the given policy.
    ///
    /// For example, if a transaction requires a policy no stricter than
    /// [`PrivacyPolicy::AllowRevealedSenders`], then that transaction can safely be
    /// constructed if the user specifies [`PrivacyPolicy::AllowLinkingAccountAddresses`],
    /// because `AllowLinkingAccountAddresses` is compatible with `AllowRevealedSenders`
    /// (the transaction will not link addresses anyway). However, if the transaction
    /// required [`PrivacyPolicy::AllowRevealedRecipients`], it could not be constructed,
    /// because `AllowLinkingAccountAddresses` is _not_ compatible with
    /// `AllowRevealedRecipients` (the transaction reveals recipients, which is not
    /// allowed by `AllowLinkingAccountAddresses`.
    pub(super) fn is_compatible_with(&self, other: Self) -> bool {
        self == &self.meet(other)
    }

    pub(super) fn allow_revealed_amounts(&self) -> bool {
        self.is_compatible_with(PrivacyPolicy::AllowRevealedAmounts)
    }

    pub(super) fn allow_revealed_recipients(&self) -> bool {
        self.is_compatible_with(PrivacyPolicy::AllowRevealedRecipients)
    }

    pub(super) fn allow_revealed_senders(&self) -> bool {
        self.is_compatible_with(PrivacyPolicy::AllowRevealedSenders)
    }

    pub(super) fn allow_fully_transparent(&self) -> bool {
        self.is_compatible_with(PrivacyPolicy::AllowFullyTransparent)
    }

    pub(super) fn allow_linking_account_addresses(&self) -> bool {
        self.is_compatible_with(PrivacyPolicy::AllowLinkingAccountAddresses)
    }

    pub(super) fn allow_no_privacy(&self) -> bool {
        self.is_compatible_with(PrivacyPolicy::NoPrivacy)
    }
}

pub(super) fn enforce_privacy_policy<FeeRuleT, NoteRef>(
    proposal: &Proposal<FeeRuleT, NoteRef>,
    privacy_policy: PrivacyPolicy,
) -> Result<(), IncompatiblePrivacyPolicy> {
    for step in proposal.steps() {
        // TODO: Break out this granularity from `Step::involves`
        let input_in_pool = |pool_type: PoolType| match pool_type {
            PoolType::Transparent => step.is_shielding() || !step.transparent_inputs().is_empty(),
            PoolType::SAPLING => step.shielded_inputs().iter().any(|s_in| {
                s_in.notes()
                    .iter()
                    .any(|note| matches!(note.note().protocol(), ShieldedProtocol::Sapling))
            }),
            PoolType::ORCHARD => step.shielded_inputs().iter().any(|s_in| {
                s_in.notes()
                    .iter()
                    .any(|note| matches!(note.note().protocol(), ShieldedProtocol::Orchard))
            }),
        };
        let output_in_pool =
            |pool_type: PoolType| step.payment_pools().values().any(|pool| *pool == pool_type);
        let change_in_pool = |pool_type: PoolType| {
            step.balance()
                .proposed_change()
                .iter()
                .any(|c| c.output_pool() == pool_type)
        };

        let has_transparent_recipient = output_in_pool(PoolType::Transparent);
        let has_transparent_change = change_in_pool(PoolType::Transparent);
        let has_sapling_recipient =
            output_in_pool(PoolType::SAPLING) || change_in_pool(PoolType::SAPLING);
        let has_orchard_recipient =
            output_in_pool(PoolType::ORCHARD) || change_in_pool(PoolType::ORCHARD);

        if input_in_pool(PoolType::Transparent) {
            let received_addrs = step
                .transparent_inputs()
                .iter()
                .map(|input| input.recipient_address())
                .collect::<HashSet<_>>();

            if received_addrs.len() > 1 {
                if has_transparent_recipient || has_transparent_change {
                    if !privacy_policy.allow_no_privacy() {
                        return Err(IncompatiblePrivacyPolicy::NoPrivacy);
                    }
                } else if !privacy_policy.allow_linking_account_addresses() {
                    return Err(IncompatiblePrivacyPolicy::LinkingAccountAddresses);
                }
            } else if has_transparent_recipient || has_transparent_change {
                if !privacy_policy.allow_fully_transparent() {
                    return Err(IncompatiblePrivacyPolicy::FullyTransparent);
                }
            } else if !privacy_policy.allow_revealed_senders() {
                return Err(IncompatiblePrivacyPolicy::TransparentSender);
            }
        } else if has_transparent_recipient {
            if !privacy_policy.allow_revealed_recipients() {
                return Err(IncompatiblePrivacyPolicy::TransparentRecipient);
            }
        } else if has_transparent_change {
            if !privacy_policy.allow_revealed_recipients() {
                return Err(IncompatiblePrivacyPolicy::TransparentChange);
            }
        } else if input_in_pool(PoolType::ORCHARD) && has_sapling_recipient {
            // TODO: This should only trigger when there is a non-fee valueBalance.
            if !privacy_policy.allow_revealed_amounts() {
                // TODO: Determine whether this is due to the presence of an explicit
                // Sapling recipient address, or having insufficient funds to pay a UA
                // within a single pool.
                return Err(IncompatiblePrivacyPolicy::RevealingSaplingAmount);
            }
        } else if input_in_pool(PoolType::SAPLING) && has_orchard_recipient {
            // TODO: This should only trigger when there is a non-fee valueBalance.
            if !privacy_policy.allow_revealed_amounts() {
                return Err(IncompatiblePrivacyPolicy::RevealingOrchardAmount);
            }
        }
    }

    // If we reach here, no step revealed anything; this proposal satisfies any privacy
    // policy.
    assert!(privacy_policy.is_compatible_with(PrivacyPolicy::FullPrivacy));
    Ok(())
}

pub(super) enum IncompatiblePrivacyPolicy {
    /// Requested [`PrivacyPolicy`] doesn’t include `NoPrivacy`.
    NoPrivacy,

    /// Requested [`PrivacyPolicy`] doesn’t include `AllowLinkingAccountAddresses`.
    LinkingAccountAddresses,

    /// Requested [`PrivacyPolicy`] doesn’t include `AllowFullyTransparent`.
    FullyTransparent,

    /// Requested [`PrivacyPolicy`] doesn’t include `AllowRevealedSenders`.
    TransparentSender,

    /// Requested [`PrivacyPolicy`] doesn’t include `AllowRevealedRecipients`.
    TransparentRecipient,

    /// Requested [`PrivacyPolicy`] doesn’t include `AllowRevealedRecipients`.
    TransparentChange,

    /// Requested [`PrivacyPolicy`] doesn’t include `AllowRevealedRecipients`, but we are
    /// trying to pay a UA where we can only select a transparent receiver.
    TransparentReceiver,

    /// Requested [`PrivacyPolicy`] doesn’t include `AllowRevealedAmounts`, but we don’t
    /// have enough Sapling funds to avoid revealing amounts.
    RevealingSaplingAmount,

    /// Requested [`PrivacyPolicy`] doesn’t include `AllowRevealedAmounts`, but we don’t
    /// have enough Orchard funds to avoid revealing amounts.
    RevealingOrchardAmount,

    /// Requested [`PrivacyPolicy`] doesn’t include `AllowRevealedAmounts`, but we are
    /// trying to pay a UA where we don’t have enough funds in any single pool that it has
    /// a receiver for.
    RevealingReceiverAmounts,
}

impl From<IncompatiblePrivacyPolicy> for ErrorObjectOwned {
    fn from(e: IncompatiblePrivacyPolicy) -> Self {
        LegacyCode::InvalidParameter.with_message(match e {
            IncompatiblePrivacyPolicy::NoPrivacy => fl!(
                "err-privpol-no-privacy-not-allowed",
                parameter = "privacyPolicy",
                policy = "NoPrivacy"
            ),
            IncompatiblePrivacyPolicy::LinkingAccountAddresses => format!(
                "{} {}",
                fl!("err-privpol-linking-addrs-not-allowed"),
                fl!(
                    "rec-privpol-privacy-weakening",
                    parameter = "privacyPolicy",
                    policy = "AllowLinkingAccountAddresses"
                )
            ),
            IncompatiblePrivacyPolicy::FullyTransparent => format!(
                "{} {}",
                fl!("err-privpol-fully-transparent-not-allowed"),
                fl!(
                    "rec-privpol-privacy-weakening",
                    parameter = "privacyPolicy",
                    policy = "AllowFullyTransparent"
                )
            ),
            IncompatiblePrivacyPolicy::TransparentSender => format!(
                "{} {}",
                fl!("err-privpol-transparent-sender-not-allowed"),
                fl!(
                    "rec-privpol-privacy-weakening",
                    parameter = "privacyPolicy",
                    policy = "AllowRevealedSenders"
                )
            ),
            IncompatiblePrivacyPolicy::TransparentRecipient => format!(
                "{} {}",
                fl!("err-privpol-transparent-recipient-not-allowed"),
                fl!(
                    "rec-privpol-privacy-weakening",
                    parameter = "privacyPolicy",
                    policy = "AllowRevealedRecipients"
                )
            ),
            IncompatiblePrivacyPolicy::TransparentChange => format!(
                "{} {}",
                fl!("err-privpol-transparent-change-not-allowed"),
                fl!(
                    "rec-privpol-privacy-weakening",
                    parameter = "privacyPolicy",
                    policy = "AllowRevealedRecipients"
                )
            ),
            IncompatiblePrivacyPolicy::TransparentReceiver => format!(
                "{} {}",
                fl!("err-privpol-transparent-receiver-not-allowed"),
                fl!(
                    "rec-privpol-privacy-weakening",
                    parameter = "privacyPolicy",
                    policy = "AllowRevealedRecipients"
                )
            ),
            IncompatiblePrivacyPolicy::RevealingSaplingAmount => format!(
                "{} {}",
                fl!("err-privpol-revealing-amount-not-allowed", pool = "Sapling"),
                fl!(
                    "rec-privpol-privacy-weakening",
                    parameter = "privacyPolicy",
                    policy = "AllowRevealedAmounts"
                )
            ),
            IncompatiblePrivacyPolicy::RevealingOrchardAmount => format!(
                "{} {}",
                fl!("err-privpol-revealing-amount-not-allowed", pool = "Orchard"),
                fl!(
                    "rec-privpol-privacy-weakening",
                    parameter = "privacyPolicy",
                    policy = "AllowRevealedAmounts"
                )
            ),
            IncompatiblePrivacyPolicy::RevealingReceiverAmounts => format!(
                "{} {}",
                fl!("err-privpol-revealing-receiver-amounts-not-allowed"),
                fl!(
                    "rec-privpol-privacy-weakening",
                    parameter = "privacyPolicy",
                    policy = "AllowRevealedAmounts"
                )
            ),
        })
    }
}

pub(super) fn parse_memo(memo_hex: &str) -> RpcResult<MemoBytes> {
    let memo_bytes = hex::decode(memo_hex).map_err(|_| {
        LegacyCode::InvalidParameter
            .with_static("Invalid parameter, expected memo data in hexadecimal format.")
    })?;

    MemoBytes::from_bytes(&memo_bytes).map_err(|_| {
        LegacyCode::InvalidParameter
            .with_static("Invalid parameter, memo is longer than the maximum allowed 512 bytes.")
    })
}

pub(super) fn get_account_for_address(
    wallet: &DbConnection,
    address: &Address,
) -> RpcResult<Account> {
    // TODO: Make this more efficient with a `WalletRead` method.
    //       https://github.com/zcash/librustzcash/issues/1944
    for account_id in wallet
        .get_account_ids()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
    {
        for address_info in wallet
            .list_addresses(account_id)
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        {
            if address_info.address() == address {
                return Ok(wallet
                    .get_account(account_id)
                    .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
                    .expect("present"));
            }
        }
    }

    Err(LegacyCode::InvalidAddressOrKey
        .with_static("Invalid from address, no payment source found for address."))
}

/// Broadcasts the specified transactions to the network, if configured to do so.
pub(super) async fn broadcast_transactions(
    wallet: &DbConnection,
    chain: FetchServiceSubscriber,
    txids: Vec<TxId>,
) -> RpcResult<SendResult> {
    if APP.config().external.broadcast() {
        for txid in &txids {
            let tx = wallet
                .get_transaction(*txid)
                .map_err(|e| {
                    LegacyCode::Database.with_message(format!("Failed to get transaction: {e}"))
                })?
                .ok_or_else(|| {
                    LegacyCode::Wallet
                        .with_message(format!("Wallet does not contain transaction {txid}"))
                })?;

            let mut tx_bytes = vec![];
            tx.write(&mut tx_bytes)
                .map_err(|e| LegacyCode::OutOfMemory.with_message(e.to_string()))?;
            let raw_transaction_hex = hex::encode(&tx_bytes);

            chain
                .send_raw_transaction(raw_transaction_hex)
                .await
                .map_err(|e| {
                    LegacyCode::Wallet
                        .with_message(format!("SendTransaction: Transaction commit failed:: {e}"))
                })?;
        }
    }

    Ok(SendResult::new(txids))
}

/// The result of sending a payment.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct SendResult {
    /// The ID of the resulting transaction, if the payment only produced one.
    ///
    /// Omitted if more than one transaction was sent; see [`SendResult::txids`] in that
    /// case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    txid: Option<String>,

    /// The IDs of the sent transactions resulting from the payment.
    txids: Vec<String>,
}

impl SendResult {
    fn new(txids: Vec<TxId>) -> Self {
        let txids = txids
            .into_iter()
            .map(|txid| txid.to_string())
            .collect::<Vec<_>>();

        Self {
            txid: (txids.len() == 1).then(|| txids.first().expect("present").clone()),
            txids,
        }
    }
}
