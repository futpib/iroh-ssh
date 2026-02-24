use std::str::FromStr;

use tokio::net::TcpListener;
use tokio::sync::Mutex;

use iroh::{EndpointId, RelayUrl};

use crate::IrohSsh;

static INSTANCE: Mutex<Option<ProxyState>> = Mutex::const_new(None);

struct ProxyState {
    #[allow(dead_code)]
    iroh_ssh: IrohSsh,
    #[allow(dead_code)]
    local_port: u16,
    shutdown: tokio::sync::watch::Sender<bool>,
}

fn parse_relay_urls(urls: &[String]) -> anyhow::Result<Vec<RelayUrl>> {
    urls.iter()
        .map(|s| RelayUrl::from_str(s).map_err(|e| anyhow::anyhow!("invalid relay URL '{s}': {e}")))
        .collect()
}

/// Connect to a remote iroh-ssh endpoint.
/// Returns the local TCP port to connect an SSH client to.
///
/// `relay_urls` replaces the default relay servers; `extra_relay_urls` adds alongside them.
/// Pass empty vectors to use defaults.
pub async fn connect_iroh(
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

    let inner = iroh_ssh.inner.as_ref().unwrap().clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                accept = listener.accept() => {
                    match accept {
                        Ok((tcp_stream, _)) => {
                            let inner = inner.clone();
                            let eid = parsed_id;
                            tokio::spawn(async move {
                                if let Err(e) = proxy_connection(
                                    tcp_stream, &inner.endpoint, eid
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

    *INSTANCE.lock().await = Some(ProxyState {
        iroh_ssh,
        local_port,
        shutdown: shutdown_tx,
    });

    Ok(local_port)
}

/// Disconnect and clean up.
pub async fn disconnect_iroh() -> anyhow::Result<()> {
    if let Some(state) = INSTANCE.lock().await.take() {
        let _ = state.shutdown.send(true);
    }
    Ok(())
}

/// Check if currently connected.
pub async fn is_connected() -> bool {
    INSTANCE.lock().await.is_some()
}

async fn proxy_connection(
    mut tcp_stream: tokio::net::TcpStream,
    endpoint: &iroh::Endpoint,
    endpoint_id: EndpointId,
) -> anyhow::Result<()> {
    let conn = endpoint.connect(endpoint_id, &IrohSsh::ALPN()).await?;
    let (mut iroh_send, mut iroh_recv) = conn.open_bi().await?;
    let (mut tcp_read, mut tcp_write) = tcp_stream.split();

    tokio::select! {
        r = tokio::io::copy(&mut tcp_read, &mut iroh_send) => { r?; }
        r = tokio::io::copy(&mut iroh_recv, &mut tcp_write) => { r?; }
    };

    Ok(())
}
