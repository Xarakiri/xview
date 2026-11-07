use std::time::Duration;

use futures::stream::{BoxStream, StreamExt};
use tokio::sync::mpsc;
use tokio::time::error::Elapsed;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use xv_core::Candle;
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

/// A healthy Binance stream carries pings roughly every 20 s, so silence this long means the
/// connection is dead (sleep/wake, Wi-Fi switch) even though no FIN/RST arrived.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// How long to wait for the close handshake of a connection being dropped.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

type ReadResult = Result<Option<Result<Message, WsError>>, Elapsed>;

#[derive(Debug, PartialEq)]
enum ReadOutcome {
    Kline(Candle),
    Ignore,
    Disconnect,
}

/// Decides what a (timed) websocket read means. Timeout, close, error and stream end are all
/// treated as a disconnect.
fn classify_read(r: ReadResult) -> ReadOutcome {
    match r {
        Ok(Some(Ok(Message::Text(text)))) => match parse_ws_kline(&text) {
            Ok(c) => ReadOutcome::Kline(c),
            // Subscription acks and other non-kline payloads are ignored.
            Err(_) => ReadOutcome::Ignore,
        },
        // Pings are answered by tungstenite while we keep reading.
        Ok(Some(Ok(Message::Close(_)))) | Ok(Some(Err(_))) | Ok(None) | Err(_) => ReadOutcome::Disconnect,
        Ok(Some(Ok(_))) => ReadOutcome::Ignore,
    }
}

/// Runs until the consumer drops the receiver.
///
/// Backpressure: candle updates use `try_send` and are dropped when the channel is full, so a
/// slow consumer can never stall the read loop (and with it pong replies); the next kline
/// supersedes a dropped one. Control events (Disconnected/Reconnected) are rare and must not be
/// lost, so they use `send().await`; that only waits when the channel is full and fails
/// immediately once the receiver is gone.
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
                let read = tokio::select! {
                    _ = tx.closed() => return,
                    r = tokio::time::timeout(READ_TIMEOUT, ws.next()) => r,
                };
                match classify_read(read) {
                    ReadOutcome::Kline(c) => {
                        attempt = 0;
                        match tx.try_send(CandleUpdate::Candle(c)) {
                            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                            Err(mpsc::error::TrySendError::Closed(_)) => return,
                        }
                    }
                    ReadOutcome::Ignore => {}
                    ReadOutcome::Disconnect => break,
                }
            }
            // A dead peer never answers the close handshake: don't hang on it.
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, ws.close(None)).await;
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

    const WS: &str = include_str!("../tests/fixtures/ws_kline.json");

    async fn elapsed() -> Elapsed {
        tokio::time::timeout(Duration::from_millis(1), std::future::pending::<()>()).await.unwrap_err()
    }

    #[tokio::test]
    async fn timeout_and_stream_end_are_disconnects() {
        assert_eq!(classify_read(Err(elapsed().await)), ReadOutcome::Disconnect);
        assert_eq!(classify_read(Ok(None)), ReadOutcome::Disconnect);
        assert_eq!(classify_read(Ok(Some(Ok(Message::Close(None))))), ReadOutcome::Disconnect);
        assert_eq!(classify_read(Ok(Some(Err(WsError::ConnectionClosed)))), ReadOutcome::Disconnect);
    }

    #[test]
    fn kline_and_noise_classification() {
        let expected = parse_ws_kline(WS).unwrap();
        assert_eq!(classify_read(Ok(Some(Ok(Message::Text(WS.into()))))), ReadOutcome::Kline(expected));
        assert_eq!(classify_read(Ok(Some(Ok(Message::Ping(vec![1]))))), ReadOutcome::Ignore);
        assert_eq!(classify_read(Ok(Some(Ok(Message::Text("{\"result\":null,\"id\":1}".into()))))), ReadOutcome::Ignore);
    }
}
