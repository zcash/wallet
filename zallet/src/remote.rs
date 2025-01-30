use std::borrow::Cow;
use std::fmt;

use abscissa_core::tracing::info;
use tonic::transport::{Channel, ClientTlsConfig};
use zcash_client_backend::proto::service::compact_tx_streamer_client::CompactTxStreamerClient;
use zcash_protocol::consensus::{NetworkType, Parameters};

use crate::{
    error::{Error, ErrorKind},
    network::Network,
};

const ECC_TESTNET: &[Server<'_>] = &[Server::fixed("lightwalletd.testnet.electriccoin.co", 9067)];

const YWALLET_MAINNET: &[Server<'_>] = &[
    Server::fixed("lwd1.zcash-infra.com", 9067),
    Server::fixed("lwd2.zcash-infra.com", 9067),
    Server::fixed("lwd3.zcash-infra.com", 9067),
    Server::fixed("lwd4.zcash-infra.com", 9067),
    Server::fixed("lwd5.zcash-infra.com", 9067),
    Server::fixed("lwd6.zcash-infra.com", 9067),
    Server::fixed("lwd7.zcash-infra.com", 9067),
    Server::fixed("lwd8.zcash-infra.com", 9067),
];

const ZEC_ROCKS_MAINNET: &[Server<'_>] = &[
    Server::fixed("zec.rocks", 443),
    Server::fixed("ap.zec.rocks", 443),
    Server::fixed("eu.zec.rocks", 443),
    Server::fixed("na.zec.rocks", 443),
    Server::fixed("sa.zec.rocks", 443),
];
const ZEC_ROCKS_TESTNET: &[Server<'_>] = &[Server::fixed("testnet.zec.rocks", 443)];

#[derive(Clone, Debug)]
pub(crate) enum ServerOperator {
    Ecc,
    YWallet,
    ZecRocks,
}

impl ServerOperator {
    fn servers(&self, network: NetworkType) -> &[Server<'_>] {
        match (self, network) {
            (ServerOperator::Ecc, NetworkType::Main) => &[],
            (ServerOperator::Ecc, NetworkType::Test) => ECC_TESTNET,
            (ServerOperator::YWallet, NetworkType::Main) => YWALLET_MAINNET,
            (ServerOperator::YWallet, NetworkType::Test) => &[],
            (ServerOperator::ZecRocks, NetworkType::Main) => ZEC_ROCKS_MAINNET,
            (ServerOperator::ZecRocks, NetworkType::Test) => ZEC_ROCKS_TESTNET,
            (_, NetworkType::Regtest) => &[],
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Servers {
    Hosted(ServerOperator),
    Custom(Vec<Server<'static>>),
}

impl Servers {
    pub(crate) fn parse(s: &str) -> Result<Self, Error> {
        match s {
            "ecc" => Ok(Self::Hosted(ServerOperator::Ecc)),
            "ywallet" => Ok(Self::Hosted(ServerOperator::YWallet)),
            "zecrocks" => Ok(Self::Hosted(ServerOperator::ZecRocks)),
            _ => s
                .split(',')
                .map(|sub| {
                    sub.rsplit_once(':').and_then(|(host, port_str)| {
                        port_str
                            .parse()
                            .ok()
                            .map(|port| Server::custom(host.into(), port))
                    })
                })
                .collect::<Option<_>>()
                .map(Self::Custom)
                .ok_or(ErrorKind::Generic
                    .context(format!("'{}' must be one of ['ecc', 'ywallet', 'zecrocks'], or a comma-separated list of host:port", s))
                    .into()),
        }
    }

    pub(crate) fn pick(&self, network: Network) -> Result<&Server<'_>, Error> {
        // For now just use the first server in the list.
        match self {
            Servers::Hosted(server_operator) => server_operator
                .servers(network.network_type())
                .first()
                .ok_or(
                    ErrorKind::Generic
                        .context(format!("{:?} doesn't serve {:?}", server_operator, network))
                        .into(),
                ),
            Servers::Custom(servers) => Ok(servers.first().expect("not empty")),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Server<'a> {
    host: Cow<'a, str>,
    port: u16,
}

impl fmt::Display for Server<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

impl Server<'static> {
    const fn fixed(host: &'static str, port: u16) -> Self {
        Self {
            host: Cow::Borrowed(host),
            port,
        }
    }
}

impl<'a> Server<'a> {
    fn custom(host: String, port: u16) -> Self {
        Self {
            host: Cow::Owned(host),
            port,
        }
    }

    fn use_tls(&self) -> bool {
        // Assume that localhost will never have a cert, and require remotes to have one.
        !matches!(self.host.as_ref(), "localhost" | "127.0.0.1" | "::1")
    }

    fn endpoint(&self) -> String {
        format!(
            "{}://{}:{}",
            if self.use_tls() { "https" } else { "http" },
            self.host,
            self.port
        )
    }

    pub(crate) async fn connect_direct(&self) -> Result<CompactTxStreamerClient<Channel>, Error> {
        info!("Connecting to {}", self);

        let channel =
            Channel::from_shared(self.endpoint()).map_err(|e| ErrorKind::Generic.context(e))?;

        let channel = if self.use_tls() {
            let tls = ClientTlsConfig::new()
                .domain_name(self.host.to_string())
                .with_webpki_roots();
            channel
                .tls_config(tls)
                .map_err(|e| ErrorKind::Generic.context(e))?
        } else {
            channel
        };

        Ok(CompactTxStreamerClient::new(
            channel
                .connect()
                .await
                .map_err(|e| ErrorKind::Generic.context(e))?,
        ))
    }
}
