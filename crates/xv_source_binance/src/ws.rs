use std::time::Duration;

use futures::stream::{BoxStream, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use xv_data::CandleUpdate;

use crate::WS_HOSTS;
use crate::parse::parse_ws_kline;

/// Reconnect delay: 1, 2, 4, 8, 16, then 30 seconds forever.
pub fn backoff_secs(attempt: u32) -> u64 {
    match attempt {
        0..=4 => 1u64 << attempt,
        _ => 30,
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(path: &str) -> Option<Ws> {
    for host in WS_HOSTS {
        let url = format!("{host}{path}");
        let attempt = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(url));
        if let Ok(Ok((ws, _))) = attempt.await {
            return Some(ws);
        }
    }
    None
}

/// Runs until the consumer drops the receiver.
async fn run(path: String, tx: mpsc::Sender<CandleUpdate>) {
    let mut attempt: u32 = 0;
    let mut announced_down = false;
    loop {
        if let Some(mut ws) = connect(&path).await {
            if announced_down {
                announced_down = false;
                if tx.send(CandleUpdate::Reconnected).await.is_err() {
                    return;
                }
            }
            loop {
                let msg = tokio::select! {
                    _ = tx.closed() => return,
                    m = ws.next() => m,
                };
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Subscription acks and other non-kline payloads are ignored.
                        if let Ok(c) = parse_ws_kline(&text) {
                            attempt = 0;
                            if tx.send(CandleUpdate::Candle(c)).await.is_err() {
                                return;
                            }
                        }
                    }
                    // Pings are answered by tungstenite while we keep reading.
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                }
            }
            let _ = ws.close(None).await;
        }
        if !announced_down {
            announced_down = true;
            if tx.send(CandleUpdate::Disconnected).await.is_err() {
                return;
            }
        }
        let delay = Duration::from_secs(backoff_secs(attempt));
        attempt = attempt.saturating_add(1);
        tokio::select! {
            _ = tx.closed() => return,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

/// Starts the stream task on `handle`. The task ends when the returned stream is dropped.
pub(crate) fn subscribe(
    handle: &tokio::runtime::Handle,
    symbol_id: &str,
    interval: &str,
) -> BoxStream<'static, CandleUpdate> {
    let path = format!("/ws/{}@kline_{}", symbol_id.to_lowercase(), interval);
    let (tx, rx) = mpsc::channel(64);
    handle.spawn(run(path, tx));
    futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) }).boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_backoff_sequence() {
        let v: Vec<_> = (0..7).map(backoff_secs).collect();
        assert_eq!(v, [1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(backoff_secs(1000), 30);
    }
}
