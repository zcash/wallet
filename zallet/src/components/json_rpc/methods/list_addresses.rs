use jsonrpsee::{core::RpcResult, types::ErrorCode as RpcErrorCode};
use serde::Serialize;
use transparent::keys::TransparentKeyScope;
use zcash_address::unified;
use zcash_client_backend::{
    data_api::{Account as _, AccountPurpose, AccountSource, WalletRead},
    encoding::AddressCodec,
    keys::UnifiedAddressRequest,
};
use zcash_primitives::block::BlockHash;
use zip32::fingerprint::SeedFingerprint;

use crate::components::{json_rpc::server::LegacyCode, wallet::WalletConnection};

/// Response to a `z_listaccounts` RPC request.
pub(crate) type Response = RpcResult<Vec<AddressSource>>;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AddressSource {
    source: &'static str,

    #[serde(skip_serializing_if = "Option::is_none")]
    transparent: Option<TransparentAddresses>,

    /// Each element in this list represents a set of diversified addresses derived from a
    /// single IVK.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sapling: Vec<SaplingAddresses>,

    /// Each element in this list represents a set of diversified Unified Addresses
    /// derived from a single UFVK.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unified: Vec<UnifiedAddresses>,
}

impl AddressSource {
    fn empty(source: &'static str) -> Self {
        Self {
            source,
            transparent: None,
            sapling: vec![],
            unified: vec![],
        }
    }

    fn has_data(&self) -> bool {
        self.transparent.is_some() || !self.sapling.is_empty() || !self.unified.is_empty()
    }
}

#[derive(Clone, Debug, Serialize)]
struct TransparentAddresses {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    addresses: Vec<String>,

    #[serde(rename = "changeAddresses")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    change_addresses: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SaplingAddresses {
    #[serde(rename = "zip32KeyPath")]
    #[serde(skip_serializing_if = "Option::is_none")]
    zip32_key_path: Option<String>,

    addresses: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct UnifiedAddresses {
    #[serde(skip_serializing_if = "Option::is_none")]
    seedfp: Option<String>,

    /// The ZIP 32 account index.
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<u32>,

    addresses: Vec<UnifiedAddress>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct UnifiedAddress {
    /// The diversifier index that the UA was derived at.
    diversifier_index: u128,

    /// The receiver types that the UA contains (valid values are "p2pkh", "sapling", "orchard").
    receiver_types: Vec<String>,

    /// The unified address corresponding to the diversifier.
    address: String,
}

pub(crate) fn call(wallet: &WalletConnection) -> Response {
    let mut imported_watchonly = AddressSource::empty("imported_watchonly");
    let mut mnemonic_seed = AddressSource::empty("mnemonic_seed");

    for account_id in wallet
        .get_account_ids()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
    {
        let account = wallet
            .get_account(account_id)
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
            // This would be a race condition between this and account deletion.
            .ok_or(RpcErrorCode::InternalError)?;

        let mut change_addresses = vec![];
        for (taddr, metadata) in wallet
            .get_transparent_receivers(account.id(), true)
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        {
            if let Some(metadata) = metadata {
                if metadata.scope() == TransparentKeyScope::INTERNAL {
                    // Change addresses are never used in a UA receiver, so we need to
                    // list them separately.
                    change_addresses.push(taddr.encode(wallet.params()));
                }
            } else {
                // TODO: Provide some way of determining whether this address is a UA receiver.
            }
        }

        // TODO: Expose all addresses, not just the last generated.
        let addresses = wallet
            .get_last_generated_address_matching(
                account.id(),
                UnifiedAddressRequest::AllAvailableKeys,
            )
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
            .into_iter()
            .map(|addr| UnifiedAddress {
                diversifier_index: 0, // TODO: Get real diversifier index.
                receiver_types: addr
                    .receiver_types()
                    .into_iter()
                    .map(|r| match r {
                        unified::Typecode::P2pkh => "p2pkh".into(),
                        unified::Typecode::P2sh => "p2sh".into(),
                        unified::Typecode::Sapling => "sapling".into(),
                        unified::Typecode::Orchard => "orchard".into(),
                        unified::Typecode::Unknown(typecode) => format!("unknown({typecode})"),
                    })
                    .collect(),
                address: addr.encode(wallet.params()),
            })
            .collect();

        // `zcashd` used `uint256::GetHex()` for rendering this, which byte-reverses the
        // data just like for block hashes.
        let seedfp_to_hex = |seedfp: &SeedFingerprint| BlockHash(seedfp.to_bytes()).to_string();

        match account.source() {
            AccountSource::Derived { derivation, .. } => {
                let transparent = mnemonic_seed
                    .transparent
                    .get_or_insert(TransparentAddresses {
                        addresses: vec![],
                        change_addresses: vec![],
                    });
                transparent.change_addresses.append(&mut change_addresses);

                mnemonic_seed.unified.push(UnifiedAddresses {
                    seedfp: Some(seedfp_to_hex(derivation.seed_fingerprint())),
                    account: Some(derivation.account_index().into()),
                    addresses,
                });
            }
            AccountSource::Imported { purpose, .. } => {
                let (seedfp, account) = match purpose {
                    // Imported UFVKs marked for spending are still counted as watch-only
                    // because their corresponding spending key has never been observed by the
                    // wallet; the distinction only affects whether Zallet tracks additional
                    // metadata about the UFVK's notes. The `imported` category was used by
                    // `zcashd` where individual spending keys were imported into the wallet.
                    AccountPurpose::Spending { derivation } => (
                        derivation
                            .as_ref()
                            .map(|d| seedfp_to_hex(d.seed_fingerprint())),
                        derivation.as_ref().map(|d| d.account_index().into()),
                    ),
                    AccountPurpose::ViewOnly => (None, None),
                };

                let transparent =
                    imported_watchonly
                        .transparent
                        .get_or_insert(TransparentAddresses {
                            addresses: vec![],
                            change_addresses: vec![],
                        });
                transparent.change_addresses.append(&mut change_addresses);

                imported_watchonly.unified.push(UnifiedAddresses {
                    seedfp,
                    account,
                    addresses,
                });
            }
        }
    }

    Ok([
        imported_watchonly.has_data().then_some(imported_watchonly),
        mnemonic_seed.has_data().then_some(mnemonic_seed),
    ]
    .into_iter()
    .flatten()
    .collect())
}
