//! Zcash network parameters.

use serde::{Deserialize, Serialize};
use zcash_protocol::{
    consensus::{self, BlockHeight},
    local_consensus,
};

/// Chain parameters for a Zcash consensus network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Network {
    /// A public global consensus network.
    Consensus(consensus::Network),
    /// A local network used for integration testing.
    RegTest(local_consensus::LocalNetwork),
}

impl Network {
    pub(crate) fn from_type(
        network_type: consensus::NetworkType,
        nuparams: &[RegTestNuParam],
    ) -> Self {
        match network_type {
            consensus::NetworkType::Main => Self::Consensus(consensus::Network::MainNetwork),
            consensus::NetworkType::Test => Self::Consensus(consensus::Network::TestNetwork),
            consensus::NetworkType::Regtest => {
                let find_nu = |nu: consensus::BranchId| {
                    nuparams
                        .iter()
                        .find(|p| p.consensus_branch_id == nu)
                        .map(|p| p.activation_height)
                };

                // If a NU is omitted, ensure that it activates at the same height as the
                // subsequent specified NU (if any).
                let nu6 = find_nu(consensus::BranchId::Nu6);
                let nu5 = find_nu(consensus::BranchId::Nu5).or(nu6);
                let canopy = find_nu(consensus::BranchId::Canopy).or(nu5);
                let heartwood = find_nu(consensus::BranchId::Heartwood).or(canopy);
                let blossom = find_nu(consensus::BranchId::Blossom).or(heartwood);
                let sapling = find_nu(consensus::BranchId::Sapling).or(blossom);
                let overwinter = find_nu(consensus::BranchId::Overwinter).or(sapling);

                Self::RegTest(local_consensus::LocalNetwork {
                    overwinter,
                    sapling,
                    blossom,
                    heartwood,
                    canopy,
                    nu5,
                    nu6,
                })
            }
        }
    }

    pub(crate) fn to_zebra(self) -> zebra_chain::parameters::Network {
        match self {
            Network::Consensus(network) => match network {
                consensus::Network::MainNetwork => zebra_chain::parameters::Network::Mainnet,
                consensus::Network::TestNetwork => {
                    zebra_chain::parameters::Network::new_default_testnet()
                }
            },
            // TODO: This does not create a compatible regtest network because Zebra does
            // not have the necessary flexibility.
            Network::RegTest(local_network) => zebra_chain::parameters::Network::new_regtest(
                local_network.nu5.map(|h| h.into()),
                local_network.nu6.map(|h| h.into()),
            ),
        }
    }
}

impl consensus::Parameters for Network {
    fn network_type(&self) -> consensus::NetworkType {
        match self {
            Self::Consensus(params) => params.network_type(),
            Self::RegTest(params) => params.network_type(),
        }
    }

    fn activation_height(&self, nu: consensus::NetworkUpgrade) -> Option<BlockHeight> {
        match self {
            Self::Consensus(params) => params.activation_height(nu),
            Self::RegTest(params) => params.activation_height(nu),
        }
    }
}

/// A parameter for regtest mode.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(try_from = "String")]
#[serde(into = "String")]
pub struct RegTestNuParam {
    consensus_branch_id: consensus::BranchId,
    activation_height: BlockHeight,
}

impl TryFrom<String> for RegTestNuParam {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let (branch_id, height) = value.split_once(':').ok_or("Invalid `regtest_nuparam`")?;

        let consensus_branch_id = u32::from_str_radix(branch_id, 16)
            .ok()
            .and_then(|branch_id| consensus::BranchId::try_from(branch_id).ok())
            .ok_or("Invalid `regtest_nuparam`")?;

        let activation_height = height
            .parse()
            .map(BlockHeight::from_u32)
            .map_err(|_| "Invalid `regtest_nuparam`")?;

        Ok(Self {
            consensus_branch_id,
            activation_height,
        })
    }
}

impl From<RegTestNuParam> for String {
    fn from(nuparam: RegTestNuParam) -> Self {
        format!(
            "{:08x}:{}",
            u32::from(nuparam.consensus_branch_id),
            nuparam.activation_height
        )
    }
}

pub(crate) mod kind {
    use std::fmt;

    use serde::{Deserializer, Serializer, de::Visitor};
    use zcash_protocol::consensus::NetworkType;

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<NetworkType, D::Error> {
        struct NetworkTypeVisitor;
        impl Visitor<'_> for NetworkTypeVisitor {
            type Value = NetworkType;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "one of 'main', 'test', or 'regtest'")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v {
                    "main" => Ok(NetworkType::Main),
                    "test" => Ok(NetworkType::Test),
                    "regtest" => Ok(NetworkType::Regtest),
                    _ => Err(serde::de::Error::invalid_type(
                        serde::de::Unexpected::Str(v),
                        &self,
                    )),
                }
            }
        }

        deserializer.deserialize_str(NetworkTypeVisitor)
    }

    pub(crate) fn serialize<S: Serializer>(
        network_type: &NetworkType,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match network_type {
            NetworkType::Main => "main",
            NetworkType::Test => "test",
            NetworkType::Regtest => "regtest",
        })
    }

    #[derive(serde::Serialize)]
    pub(crate) struct Serializable(#[serde(with = "crate::network::kind")] pub(crate) NetworkType);
}
