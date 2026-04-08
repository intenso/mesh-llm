use crate::network::openai::transport;
use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug)]
pub(crate) struct BackendProxyHandle {
    port: u16,
    stop_tx: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl BackendProxyHandle {
    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    pub(crate) async fn shutdown(self) {
        let _ = self.stop_tx.send(true);
        self.task.abort();
        let _ = self.task.await;
    }
}

pub(crate) async fn start_backend_proxy(llama_port: u16) -> Result<BackendProxyHandle> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);

    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                accept = listener.accept() => {
                    let (stream, _) = match accept {
                        Ok(v) => v,
                        Err(err) => {
                            tracing::warn!("backend proxy accept error: {err}");
                            continue;
                        }
                    };
                    tokio::spawn(async move {
                        if let Err(err) = handle_connection(stream, llama_port).await {
                            tracing::debug!("backend proxy request failed: {err}");
                        }
                    });
                }
                changed = stop_rx.changed() => {
                    if changed.is_err() || *stop_rx.borrow() {
                        break;
                    }
                }
            }
        }
    });

    Ok(BackendProxyHandle {
        port,
        stop_tx,
        task,
    })
}

async fn handle_connection(mut stream: TcpStream, llama_port: u16) -> Result<()> {
    let _ = stream.set_nodelay(true);
    let request = match transport::read_http_request(&mut stream).await {
        Ok(request) => request,
        Err(err) => {
            let _ = transport::send_400(stream, &err.to_string()).await;
            return Ok(());
        }
    };

    let mut upstream = match TcpStream::connect(format!("127.0.0.1:{llama_port}")).await {
        Ok(stream) => stream,
        Err(err) => {
            let _ = transport::send_503(stream, &format!("llama backend unavailable: {err}")).await;
            return Ok(());
        }
    };
    let _ = upstream.set_nodelay(true);

    if let Err(err) = upstream.write_all(&request.raw).await {
        let _ = transport::send_503(stream, &format!("llama backend write failed: {err}")).await;
        return Ok(());
    }

    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    let _ = stream.shutdown().await;
    Ok(())
}
