//! `zdesk-relay`: reach a Mac the gateway cannot reach itself, through a machine
//! that reaches both. See the crate's README.

use clap::Parser;
use zdesk_relay::server::{self, Network};

/// Relay a zDesk gateway to a Mac it cannot reach, such as a virtual Mac behind
/// its host's NAT, High Performance's media stream included, on one TCP port.
/// Nothing is authenticated: anyone who reaches the port can connect through it
/// to the networks it allows. For QA only.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// The address gateways connect to.
    #[arg(long, default_value = "[::]:15900")]
    listen: std::net::SocketAddr,
    /// A network the relay may connect to, as an address or address/prefix
    /// (192.168.160.0/24); repeat it for more.
    #[arg(long, required = true)]
    allow: Vec<Network>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    let allowed = args.allow.iter().map(Network::to_string).collect::<Vec<_>>().join(", ");
    log::warn!(
        "listening on {} with no authentication: anyone who reaches it can connect through it to {allowed}",
        listener.local_addr()?
    );
    server::serve(listener, args.allow).await
}
