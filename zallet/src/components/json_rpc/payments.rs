use std::{
    collections::{HashSet, VecDeque},
    fmt,
    sync::{LazyLock, Mutex},
};

use abscissa_core::Application;
use documented::Documented;
use jsonrpsee::core::JsonValue;
use jsonrpsee::{core::RpcResult, types::ErrorObjectOwned};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zcash_client_backend::{data_api::WalletRead, proposal::Proposal};
use zcash_client_sqlite::wallet::Account;
use zcash_keys::address::Address;
use zcash_protocol::{PoolType, TxId, memo::MemoBytes};

use crate::{
    components::{chain::Chain, database::DbConnection},
    fl,
    prelude::APP,
};

use super::server::LegacyCode;

#[derive(Serialize, Deserialize, JsonSchema)]
pub(crate) struct AmountParameter {
    /// A taddr, zaddr, or Unified Address.
    address: String,

    /// The numeric amount in ZEC.
    amount: JsonValue,

    /// If the address is a zaddr, raw data represented in hexadecimal string format. If
    /// the output is being sent to a transparent address, it’s an error to include this
    /// field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memo: Option<String>,
}

impl AmountParameter {
    pub fn address(&self) -> &String {
        &self.address
    }

    pub fn amount(&self) -> &JsonValue {
        &self.amount
    }

    pub fn memo(&self) -> &Option<String> {
        &self.memo
    }
}

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

/// Parses the `privacy_policy` RPC argument, defaulting to [`PrivacyPolicy::FullPrivacy`]
/// when absent.
///
/// Shared by `z_sendmany` (where the argument is optional) and `z_sendfromaccount` (where it
/// is required and supplied as `Some`). The `"LegacyCompat"` policy that `zcashd` accepted is
/// explicitly rejected.
pub(super) fn parse_privacy_policy(privacy_policy: Option<&str>) -> RpcResult<PrivacyPolicy> {
    match privacy_policy {
        Some("LegacyCompat") => Err(LegacyCode::InvalidParameter
            .with_static("LegacyCompat privacy policy is unsupported in Zallet")),
        Some(s) => PrivacyPolicy::from_str(s).ok_or_else(|| {
            LegacyCode::InvalidParameter.with_message(format!("Unknown privacy policy {s}"))
        }),
        None => Ok(PrivacyPolicy::FullPrivacy),
    }
}

/// Maximum number of proposal policies retained by [`RequiredPolicyCache`].
const REQUIRED_POLICY_CACHE_CAPACITY: usize = 256;

/// A bounded, insertion-ordered cache mapping a PCZT (by content hash) to the
/// [`PrivacyPolicy`] required to execute it.
///
/// `z_proposetransaction` computes the required policy exactly from the proposal and records
/// it here; `z_finalizetransaction` looks it up to enforce that the caller acknowledged a
/// sufficient policy, without having to re-derive it from the (lossy) PCZT. Entries are
/// evicted in insertion order once the capacity is exceeded; a cache miss (eviction, restart,
/// or a PCZT proposed elsewhere) falls back to accepting the caller's policy.
struct RequiredPolicyCache {
    by_pczt: std::collections::HashMap<[u8; 32], PrivacyPolicy>,
    order: VecDeque<[u8; 32]>,
    capacity: usize,
}

impl RequiredPolicyCache {
    fn new(capacity: usize) -> Self {
        Self {
            by_pczt: std::collections::HashMap::new(),
            order: VecDeque::new(),
            capacity,
        }
    }

    fn insert(&mut self, key: [u8; 32], policy: PrivacyPolicy) {
        // Re-inserting an existing key just refreshes the policy without growing the cache.
        if self.by_pczt.insert(key, policy).is_none() {
            self.order.push_back(key);
            while self.order.len() > self.capacity {
                if let Some(evicted) = self.order.pop_front() {
                    self.by_pczt.remove(&evicted);
                }
            }
        }
    }

    fn get(&self, key: &[u8; 32]) -> Option<PrivacyPolicy> {
        self.by_pczt.get(key).copied()
    }
}

static REQUIRED_POLICY_CACHE: LazyLock<Mutex<RequiredPolicyCache>> =
    LazyLock::new(|| Mutex::new(RequiredPolicyCache::new(REQUIRED_POLICY_CACHE_CAPACITY)));

/// The cache key for a PCZT: the SHA-256 of its serialized bytes.
///
/// `z_proposetransaction` and `z_finalizetransaction` hash the same canonical serialization,
/// so the policy recorded at proposal time is found again at finalize time.
pub(super) fn pczt_policy_key(pczt_bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(pczt_bytes).into()
}

/// Records the privacy policy required to execute the PCZT identified by `key`.
pub(super) fn record_required_policy(key: [u8; 32], policy: PrivacyPolicy) {
    REQUIRED_POLICY_CACHE
        .lock()
        .expect("policy cache mutex is not poisoned")
        .insert(key, policy);
}

/// Returns the previously-recorded required policy for the PCZT identified by `key`, if it is
/// still cached.
pub(super) fn cached_required_policy(key: &[u8; 32]) -> Option<PrivacyPolicy> {
    REQUIRED_POLICY_CACHE
        .lock()
        .expect("policy cache mutex is not poisoned")
        .get(key)
}

pub(super) fn enforce_privacy_policy<FeeRuleT, NoteRef>(
    proposal: &Proposal<FeeRuleT, NoteRef>,
    privacy_policy: PrivacyPolicy,
) -> Result<(), IncompatiblePrivacyPolicy> {
    for step in proposal.steps() {
        let has_transparent_recipient = step.output_in_pool(PoolType::Transparent);
        let has_transparent_change = step.change_in_pool(PoolType::Transparent);
        let has_sapling_recipient =
            step.output_in_pool(PoolType::SAPLING) || step.change_in_pool(PoolType::SAPLING);
        let has_orchard_recipient =
            step.output_in_pool(PoolType::ORCHARD) || step.change_in_pool(PoolType::ORCHARD);

        if step.input_in_pool(PoolType::Transparent) {
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
        } else if step.input_in_pool(PoolType::ORCHARD) && has_sapling_recipient {
            // TODO: This should only trigger when there is a non-fee valueBalance.
            if !privacy_policy.allow_revealed_amounts() {
                // TODO: Determine whether this is due to the presence of an explicit
                // Sapling recipient address, or having insufficient funds to pay a UA
                // within a single pool.
                return Err(IncompatiblePrivacyPolicy::RevealingSaplingAmount);
            }
        } else if step.input_in_pool(PoolType::SAPLING) && has_orchard_recipient {
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

/// Returns the privacy policy required to execute the given proposal.
///
/// This is the inverse of [`enforce_privacy_policy`]: rather than checking a caller-
/// supplied policy against the information a proposal would leak, it computes the
/// strictest [`PrivacyPolicy`] that still permits the proposal. Any policy that
/// [`PrivacyPolicy::is_compatible_with`] the returned value is sufficient to execute the
/// transaction; the returned value is itself the strictest such policy.
///
/// Used by `z_proposetransaction` to report the privacy implications of a proposed
/// transaction without requiring the caller to commit to a policy up front.
pub(super) fn required_privacy_policy<FeeRuleT, NoteRef>(
    proposal: &Proposal<FeeRuleT, NoteRef>,
) -> PrivacyPolicy {
    // The required policy for the whole proposal is the meet (greatest lower bound, i.e.
    // most-permissive-needed) of the policies required by each step. We start from
    // `FullPrivacy` (the strictest policy, the lattice top); `meet` with each step's
    // requirement relaxes it exactly as much as that step's leakage demands.
    proposal
        .steps()
        .iter()
        .fold(PrivacyPolicy::FullPrivacy, |required, step| {
            // This mirrors the branch structure of `enforce_privacy_policy` exactly; keep
            // the two in sync. Each step fires exactly one branch, yielding the single
            // policy level that step requires.
            let has_transparent_recipient = step.output_in_pool(PoolType::Transparent);
            let has_transparent_change = step.change_in_pool(PoolType::Transparent);
            let has_sapling_recipient =
                step.output_in_pool(PoolType::SAPLING) || step.change_in_pool(PoolType::SAPLING);
            let has_orchard_recipient =
                step.output_in_pool(PoolType::ORCHARD) || step.change_in_pool(PoolType::ORCHARD);

            let step_required = if step.input_in_pool(PoolType::Transparent) {
                let received_addrs = step
                    .transparent_inputs()
                    .iter()
                    .map(|input| input.recipient_address())
                    .collect::<HashSet<_>>();

                if received_addrs.len() > 1 {
                    if has_transparent_recipient || has_transparent_change {
                        PrivacyPolicy::NoPrivacy
                    } else {
                        PrivacyPolicy::AllowLinkingAccountAddresses
                    }
                } else if has_transparent_recipient || has_transparent_change {
                    PrivacyPolicy::AllowFullyTransparent
                } else {
                    PrivacyPolicy::AllowRevealedSenders
                }
            } else if has_transparent_recipient || has_transparent_change {
                PrivacyPolicy::AllowRevealedRecipients
            } else if (step.input_in_pool(PoolType::ORCHARD) && has_sapling_recipient)
                || (step.input_in_pool(PoolType::SAPLING) && has_orchard_recipient)
            {
                // TODO: As in `enforce_privacy_policy`, this should only trigger when there
                // is a non-fee valueBalance.
                PrivacyPolicy::AllowRevealedAmounts
            } else {
                // Nothing is revealed by this step.
                PrivacyPolicy::FullPrivacy
            };

            required.meet(step_required)
        })
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

/// Maximum decoded memo size in bytes, matching [`MemoBytes::from_bytes`].
const MAX_MEMO_BYTES: usize = 512;

pub(super) fn parse_memo(memo_hex: &str) -> RpcResult<MemoBytes> {
    if memo_hex.len() > MAX_MEMO_BYTES * 2 {
        return Err(LegacyCode::InvalidParameter
            .with_static("Invalid parameter, memo is longer than the maximum allowed 512 bytes."));
    }

    let memo_bytes = hex::decode(memo_hex).map_err(|_| {
        LegacyCode::InvalidParameter
            .with_static("Invalid parameter, expected memo data in hexadecimal format.")
    })?;

    MemoBytes::from_bytes(&memo_bytes).map_err(|_| {
        LegacyCode::InvalidParameter
            .with_static("Invalid parameter, memo is longer than the maximum allowed 512 bytes.")
    })
}

#[cfg(test)]
mod parse_memo_tests {
    use super::*;
    use jsonrpsee::types::ErrorObject;

    fn invalid_parameter_message(err: ErrorObject<'_>) -> String {
        err.message().to_string()
    }

    #[test]
    fn parse_memo_accepts_max_length_hex() {
        let memo_hex = "00".repeat(MAX_MEMO_BYTES);
        assert!(parse_memo(&memo_hex).is_ok());
    }

    #[test]
    fn parse_memo_rejects_overlong_hex_before_decode() {
        let memo_hex = "00".repeat(MAX_MEMO_BYTES + 1);
        let err = parse_memo(&memo_hex).expect_err("overlong memo should be rejected");
        assert_eq!(
            invalid_parameter_message(err),
            "Invalid parameter, memo is longer than the maximum allowed 512 bytes."
        );
    }

    #[test]
    fn parse_memo_rejects_invalid_hex() {
        let err = parse_memo("not-hex").expect_err("invalid hex should be rejected");
        assert_eq!(
            invalid_parameter_message(err),
            "Invalid parameter, expected memo data in hexadecimal format."
        );
    }
}

#[cfg(test)]
mod privacy_policy_tests {
    use proptest::prelude::*;

    use super::*;

    const ALL_POLICIES: &[PrivacyPolicy] = &[
        PrivacyPolicy::FullPrivacy,
        PrivacyPolicy::AllowRevealedAmounts,
        PrivacyPolicy::AllowRevealedRecipients,
        PrivacyPolicy::AllowRevealedSenders,
        PrivacyPolicy::AllowFullyTransparent,
        PrivacyPolicy::AllowLinkingAccountAddresses,
        PrivacyPolicy::NoPrivacy,
    ];

    #[test]
    fn parse_privacy_policy_defaults_to_full_privacy_when_absent() {
        assert_eq!(
            parse_privacy_policy(None).unwrap(),
            PrivacyPolicy::FullPrivacy,
        );
    }

    #[test]
    fn parse_privacy_policy_accepts_every_known_policy() {
        // Every policy round-trips through its string name.
        for &policy in ALL_POLICIES {
            let name: &'static str = policy.into();
            assert_eq!(parse_privacy_policy(Some(name)).unwrap(), policy);
        }
    }

    #[test]
    fn parse_privacy_policy_rejects_legacy_compat() {
        let err = parse_privacy_policy(Some("LegacyCompat"))
            .expect_err("LegacyCompat should be rejected");
        assert_eq!(
            err.message(),
            "LegacyCompat privacy policy is unsupported in Zallet",
        );
    }

    #[test]
    fn parse_privacy_policy_rejects_unknown_policy() {
        let err =
            parse_privacy_policy(Some("Whatever")).expect_err("unknown policy should be rejected");
        assert_eq!(err.message(), "Unknown privacy policy Whatever");
    }

    #[test]
    fn meet_with_full_privacy_is_identity() {
        // `FullPrivacy` is the lattice top: meeting it with any policy yields that policy.
        for &policy in ALL_POLICIES {
            assert_eq!(PrivacyPolicy::FullPrivacy.meet(policy), policy);
            assert_eq!(policy.meet(PrivacyPolicy::FullPrivacy), policy);
        }
    }

    #[test]
    fn meet_with_no_privacy_is_no_privacy() {
        // `NoPrivacy` is the lattice bottom: meeting it with any policy yields `NoPrivacy`.
        for &policy in ALL_POLICIES {
            assert_eq!(
                PrivacyPolicy::NoPrivacy.meet(policy),
                PrivacyPolicy::NoPrivacy,
            );
            assert_eq!(
                policy.meet(PrivacyPolicy::NoPrivacy),
                PrivacyPolicy::NoPrivacy,
            );
        }
    }

    #[test]
    fn meet_is_commutative() {
        for &a in ALL_POLICIES {
            for &b in ALL_POLICIES {
                assert_eq!(
                    a.meet(b),
                    b.meet(a),
                    "meet should be commutative: {a} vs {b}"
                );
            }
        }
    }

    #[test]
    fn meet_combines_transparent_sender_and_recipient() {
        // Revealing both senders and recipients requires the fully-transparent policy.
        assert_eq!(
            PrivacyPolicy::AllowRevealedSenders.meet(PrivacyPolicy::AllowRevealedRecipients),
            PrivacyPolicy::AllowFullyTransparent,
        );
    }

    #[test]
    fn a_policy_is_compatible_with_itself_and_stricter_ones() {
        // A caller-supplied policy must permit everything a required policy needs. Any policy
        // satisfies `FullPrivacy`, and `NoPrivacy` satisfies any required policy.
        for &policy in ALL_POLICIES {
            assert!(policy.is_compatible_with(PrivacyPolicy::FullPrivacy));
            assert!(PrivacyPolicy::NoPrivacy.is_compatible_with(policy));
        }
    }

    /// A proptest strategy yielding an arbitrary [`PrivacyPolicy`].
    fn arb_policy() -> impl Strategy<Value = PrivacyPolicy> {
        prop::sample::select(ALL_POLICIES.to_vec())
    }

    proptest! {
        /// `meet` is the greatest-lower-bound of a lattice, so it must be idempotent,
        /// commutative, and associative.
        #[test]
        fn meet_is_idempotent(a in arb_policy()) {
            prop_assert_eq!(a.meet(a), a);
        }

        #[test]
        fn meet_is_commutative_prop(a in arb_policy(), b in arb_policy()) {
            prop_assert_eq!(a.meet(b), b.meet(a));
        }

        #[test]
        fn meet_is_associative(a in arb_policy(), b in arb_policy(), c in arb_policy()) {
            prop_assert_eq!(a.meet(b).meet(c), a.meet(b.meet(c)));
        }

        /// Any string that is neither a known policy name nor the rejected `"LegacyCompat"`
        /// is reported as an unknown policy.
        #[test]
        fn parse_privacy_policy_rejects_arbitrary_unknown_strings(s in "[A-Za-z]{0,24}") {
            prop_assume!(PrivacyPolicy::from_str(&s).is_none() && s != "LegacyCompat");
            let err = parse_privacy_policy(Some(&s))
                .expect_err("an unknown policy name should be rejected");
            let expected = format!("Unknown privacy policy {s}");
            prop_assert_eq!(err.message(), expected);
        }
    }
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
pub(super) async fn broadcast_transactions<C: Chain>(
    wallet: &DbConnection,
    chain: C,
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

            chain.broadcast_transaction(&tx).await.map_err(|e| {
                LegacyCode::Wallet
                    .with_message(format!("SendTransaction: Transaction commit failed:: {e}"))
            })?;
        }
    }

    Ok(SendResult::new(txids))
}

/// The result of sending a payment.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
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

#[cfg(test)]
mod required_policy_cache_tests {
    use proptest::prelude::*;

    use super::*;

    /// Builds a distinct 32-byte key from an index.
    fn key(i: usize) -> [u8; 32] {
        let mut k = [0u8; 32];
        k[..8].copy_from_slice(&(i as u64).to_le_bytes());
        k
    }

    #[test]
    fn insert_then_get_round_trips() {
        let mut cache = RequiredPolicyCache::new(4);
        cache.insert(key(1), PrivacyPolicy::AllowRevealedAmounts);
        assert_eq!(
            cache.get(&key(1)),
            Some(PrivacyPolicy::AllowRevealedAmounts)
        );
        assert_eq!(cache.get(&key(2)), None);
    }

    #[test]
    fn reinserting_a_key_updates_without_growing() {
        let mut cache = RequiredPolicyCache::new(4);
        cache.insert(key(1), PrivacyPolicy::FullPrivacy);
        cache.insert(key(1), PrivacyPolicy::NoPrivacy);
        assert_eq!(cache.by_pczt.len(), 1);
        assert_eq!(cache.get(&key(1)), Some(PrivacyPolicy::NoPrivacy));
    }

    #[test]
    fn pczt_policy_key_is_deterministic_and_collision_resistant() {
        assert_eq!(
            pczt_policy_key(b"pczt-bytes"),
            pczt_policy_key(b"pczt-bytes")
        );
        assert_ne!(
            pczt_policy_key(b"pczt-bytes"),
            pczt_policy_key(b"other-bytes")
        );
    }

    #[test]
    fn global_cache_round_trips() {
        // A key unique to this test so the shared global cache cannot interfere.
        let k = pczt_policy_key(b"required_policy_cache_tests::global_cache_round_trips");
        assert_eq!(cached_required_policy(&k), None);
        record_required_policy(k, PrivacyPolicy::AllowRevealedSenders);
        assert_eq!(
            cached_required_policy(&k),
            Some(PrivacyPolicy::AllowRevealedSenders),
        );
    }

    #[test]
    fn finalize_check_accepts_sufficient_and_rejects_insufficient() {
        // This is exactly the check `z_finalizetransaction` performs:
        // `supplied.is_compatible_with(required)`.
        let required = PrivacyPolicy::AllowRevealedSenders;
        assert!(PrivacyPolicy::NoPrivacy.is_compatible_with(required));
        assert!(PrivacyPolicy::AllowRevealedSenders.is_compatible_with(required));
        // FullPrivacy forbids revealing senders, so it is insufficient.
        assert!(!PrivacyPolicy::FullPrivacy.is_compatible_with(required));
    }

    proptest! {
        /// The cache never exceeds its capacity, and after inserting N distinct keys it
        /// retains exactly the most recent `capacity` of them, evicting the rest in insertion
        /// order.
        #[test]
        fn evicts_oldest_beyond_capacity(n in 0usize..64, capacity in 1usize..16) {
            let mut cache = RequiredPolicyCache::new(capacity);
            for i in 0..n {
                cache.insert(key(i), PrivacyPolicy::FullPrivacy);
            }

            prop_assert_eq!(cache.by_pczt.len(), n.min(capacity));

            let oldest_retained = n.saturating_sub(capacity);
            for i in 0..n {
                prop_assert_eq!(cache.get(&key(i)).is_some(), i >= oldest_retained);
            }
        }
    }
}
