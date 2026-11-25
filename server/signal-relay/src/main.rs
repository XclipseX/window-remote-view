use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use rv_signal_relay::server::{serve, serve_tcp, Config, State};
use tokio::net::{TcpListener, UdpSocket};

#[derive(Parser, Debug)]
#[command(version, about = "Signal + relay server (UDP)")]
struct Args {
    /// UDP bind address.
    #[arg(long, env = "RV_BIND", default_value = "0.0.0.0")]
    bind: std::net::IpAddr,
    /// UDP port.
    #[arg(long, short, env = "RV_PORT", default_value_t = 21116)]
    port: u16,
    /// Seconds before an un-refreshed host registration expires.
    #[arg(long, env = "RV_REGISTER_TTL", default_value_t = 60)]
    register_ttl: u64,
    /// Seconds before an idle relay session is dropped.
    #[arg(long, env = "RV_SESSION_IDLE", default_value_t = 60)]
    session_idle: u64,
    #[arg(long, env = "RV_MAX_REGISTRATIONS", default_value_t = 100_000)]
    max_registrations: usize,
    #[arg(long, env = "RV_MAX_SESSIONS", default_value_t = 10_000)]
    max_sessions: usize,
    /// Per-session relay bandwidth cap in bits/s (0 = unlimited). Default 25 Mbps.
    #[arg(long, env = "RV_MAX_BPS", default_value_t = rv_signal_relay::server::DEFAULT_MAX_BPS)]
    max_bps: u64,
    /// TCP fallback port (framed datagrams for UDP-blocked networks); use 443 behind
    /// restrictive firewalls. 0 disables the TCP listener.
    #[arg(long, env = "RV_TCP_PORT", default_value_t = 21117)]
    tcp_port: u16,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = Args::parse();
    let addr = SocketAddr::new(args.bind, args.port);
    let sock = Arc::new(UdpSocket::bind(addr).await?);
    tracing::info!("rv-signal-relay listening on udp://{}", sock.local_addr()?);
    let state = Arc::new(State::new(Config {
        register_ttl: Duration::from_secs(args.register_ttl),
        session_idle: Duration::from_secs(args.session_idle),
        max_registrations: args.max_registrations,
        max_sessions: args.max_sessions,
        max_bps: args.max_bps,
    }));
    let tcp = if args.tcp_port != 0 {
        let l = TcpListener::bind(SocketAddr::new(args.bind, args.tcp_port)).await?;
        tracing::info!("tcp fallback listening on tcp://{}", l.local_addr()?);
        Some(tokio::spawn(serve_tcp(l, state.clone(), sock.clone())))
    } else {
        None
    };
    let _tcp_guard = tcp;
    tokio::select! {
        r = serve(sock, state) => r,
        _ = tokio::signal::ctrl_c() => { tracing::info!("shutting down"); Ok(()) }
    }
}
