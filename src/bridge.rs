use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, LazyLock};

use tokio::net::TcpListener;
use tokio::sync::Mutex;

use iroh::{EndpointId, RelayUrl};

use crate::IrohSsh;

pub struct IrohConnectionInfo {
    pub is_direct: bool,
    pub is_relay: bool,
    pub relay_url: Option<String>,
    pub latency_ms: Option<f64>,
}

static CONNECTIONS: LazyLock<Mutex<HashMap<u16, ConnectionState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct ConnectionState {
    iroh_ssh: IrohSsh,
    shutdown: tokio::sync::watch::Sender<bool>,
    connection: Arc<std::sync::Mutex<Option<iroh::endpoint::Connection>>>,
}

fn parse_relay_urls(urls: &[String]) -> anyhow::Result<Vec<RelayUrl>> {
    urls.iter()
        .map(|s| RelayUrl::from_str(s).map_err(|e| anyhow::anyhow!("invalid relay URL '{s}': {e}")))
        .collect()
}

/// Connect to a remote iroh-ssh endpoint.
/// Returns the local TCP port to connect an SSH client to.
/// The port also serves as the connection identifier for `disconnect`.
///
/// `relay_urls` replaces the default relay servers; `extra_relay_urls` adds alongside them.
/// Pass empty vectors to use defaults.
pub async fn connect(
    endpoint_id: String,
    relay_urls: Vec<String>,
    extra_relay_urls: Vec<String>,
) -> anyhow::Result<u16> {
    let iroh_ssh = IrohSsh::builder()
        .accept_incoming(false)
        .relay_urls(parse_relay_urls(&relay_urls)?)
        .extra_relay_urls(parse_relay_urls(&extra_relay_urls)?)
        .build()
        .await?;

    let parsed_id = EndpointId::from_str(&endpoint_id)?;

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local_port = listener.local_addr()?.port();

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

    let connection: Arc<std::sync::Mutex<Option<iroh::endpoint::Connection>>> =
        Arc::new(std::sync::Mutex::new(None));

    let inner = iroh_ssh.inner.as_ref().unwrap().clone();
    let connection_slot = connection.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                accept = listener.accept() => {
                    match accept {
                        Ok((tcp_stream, _)) => {
                            let inner = inner.clone();
                            let eid = parsed_id;
                            let conn_slot = connection_slot.clone();
                            tokio::spawn(async move {
                                if let Err(e) = proxy_connection(
                                    tcp_stream, &inner.endpoint, eid, conn_slot
                                ).await {
                                    tracing::error!("proxy error: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!("accept error: {e}");
                            break;
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    break;
                }
            }
        }
    });

    CONNECTIONS.lock().await.insert(local_port, ConnectionState {
        iroh_ssh,
        shutdown: shutdown_tx,
        connection,
    });

    Ok(local_port)
}

async fn shutdown_connection(state: ConnectionState) {
    let _ = state.shutdown.send(true);
    if let Some(inner) = state.iroh_ssh.inner {
        inner.router.shutdown().await.ok();
    }
}

/// Disconnect a connection by its port.
pub async fn disconnect(port: u16) -> anyhow::Result<()> {
    if let Some(state) = CONNECTIONS.lock().await.remove(&port) {
        shutdown_connection(state).await;
    }
    Ok(())
}

/// Disconnect all active connections.
pub async fn disconnect_all() -> anyhow::Result<()> {
    let connections: Vec<_> = CONNECTIONS.lock().await.drain().collect();
    for (_, state) in connections {
        shutdown_connection(state).await;
    }
    Ok(())
}

/// Get the ports of all active connections.
pub async fn connections() -> Vec<u16> {
    CONNECTIONS.lock().await.keys().copied().collect()
}

/// Query connection info for an active connection by its port.
/// Returns `None` if no iroh connection has been established yet.
pub async fn connection_info(port: u16) -> anyhow::Result<Option<IrohConnectionInfo>> {
    let connections = CONNECTIONS.lock().await;
    let state = connections
        .get(&port)
        .ok_or_else(|| anyhow::anyhow!("no connection on port {port}"))?;

    let conn = state.connection.lock().unwrap().clone();
    let Some(conn) = conn else {
        return Ok(None);
    };

    let info = conn.remote_info();
    let Some(path) = info.path else {
        return Ok(None);
    };

    let is_direct = path.is_direct();
    let is_relay = !path.is_direct();
    let relay_url = path.relay_url.as_ref().map(|u| u.to_string());
    let latency_ms = path.latency.map(|d| d.as_secs_f64() * 1000.0);

    Ok(Some(IrohConnectionInfo {
        is_direct,
        is_relay,
        relay_url,
        latency_ms,
    }))
}

async fn proxy_connection(
    mut tcp_stream: tokio::net::TcpStream,
    endpoint: &iroh::Endpoint,
    endpoint_id: EndpointId,
    connection_slot: Arc<std::sync::Mutex<Option<iroh::endpoint::Connection>>>,
) -> anyhow::Result<()> {
    let conn = endpoint.connect(endpoint_id, &IrohSsh::ALPN()).await?;

    {
        let mut slot = connection_slot.lock().unwrap();
        if slot.is_none() {
            *slot = Some(conn.clone());
        }
    }

    let (mut iroh_send, mut iroh_recv) = conn.open_bi().await?;
    let (mut tcp_read, mut tcp_write) = tcp_stream.split();

    tokio::select! {
        r = tokio::io::copy(&mut tcp_read, &mut iroh_send) => { r?; }
        r = tokio::io::copy(&mut iroh_recv, &mut tcp_write) => { r?; }
    };

    Ok(())
}
