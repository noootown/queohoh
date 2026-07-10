use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::event::Event;
use crate::ipc::types::StateSnapshot;

/// Short-lived NDJSON JSON-RPC client: one Unix-socket connection, sequential
/// calls (mirror of `ApiClient` in packages/daemon/src/client.ts as used by
/// actions.ts's `withClient` — connect, call, drop).
pub struct RpcClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
}

impl RpcClient {
    pub async fn connect(sock: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(sock).await?;
        let (r, w) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(r),
            writer: w,
            next_id: 1,
        })
    }

    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        // The daemon reads `req.params ?? {}` (api.ts), so `null` params are safe.
        let frame = format!(
            "{}\n",
            serde_json::json!({ "id": id, "method": method, "params": params })
        );
        let fut = async {
            self.writer
                .write_all(frame.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            let mut line = String::new();
            loop {
                line.clear();
                let n = self
                    .reader
                    .read_line(&mut line)
                    .await
                    .map_err(|e| e.to_string())?;
                if n == 0 {
                    return Err("connection closed".to_string());
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                    continue; // mirror handleFrame: unparseable lines are dropped
                };
                // Push frames and replies to other ids are not ours — skip
                // (correlate by id, sequential single-caller client).
                if v.get("event").is_some() {
                    continue;
                }
                if v.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
                    continue;
                }
                // TS checks `frame.error !== undefined` — key presence is an error.
                if let Some(err) = v.get("error") {
                    return Err(err
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| err.to_string()));
                }
                return Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null));
            }
        };
        match tokio::time::timeout(timeout, fut).await {
            Ok(res) => res,
            Err(_) => Err(format!("call timed out: {method}")),
        }
    }
}

/// Persistent push-subscription task: connect → `subscribe` → `state` → forward
/// every snapshot; on any error/EOF send `Disconnected`, sleep 2s, reconnect —
/// forever (mirror of use-daemon.ts's attempt/scheduleRetry loop).
pub fn spawn_subscription(sock: PathBuf, tx: UnboundedSender<Event>) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Persisted across reconnects: dedup key of the last committed snapshot
        // (mirror of lastPushedJson). The per-session `first` flag inside
        // subscription_session mirrors connectedRef: the first snapshot after a
        // (re)connect always commits, even when byte-identical.
        let mut last_committed: Option<String> = None;
        loop {
            let _ = subscription_session(&sock, &tx, &mut last_committed).await;
            if tx.send(Event::Disconnected).is_err() {
                return; // receiver dropped — the app exited
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
}

async fn subscription_session(
    sock: &Path,
    tx: &UnboundedSender<Event>,
    last_committed: &mut Option<String>,
) -> Result<(), String> {
    let stream = UnixStream::connect(sock).await.map_err(|e| e.to_string())?;
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r);
    w.write_all(b"{\"id\":1,\"method\":\"subscribe\"}\n")
        .await
        .map_err(|e| e.to_string())?;
    w.write_all(b"{\"id\":2,\"method\":\"state\"}\n")
        .await
        .map_err(|e| e.to_string())?;

    let mut first = true;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed".to_string());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        // Snapshot ingress has two envelopes — the `state` reply ({id:2,result})
        // and push frames ({event:"state",data}). One choke point, mirroring
        // use-daemon's applySnapshot.
        let data = if v.get("event").and_then(serde_json::Value::as_str) == Some("state") {
            v.get("data").cloned()
        } else if v.get("id").and_then(serde_json::Value::as_u64) == Some(2) {
            v.get("result").cloned()
        } else {
            None // subscribe ack (id 1) or anything else
        };
        let Some(data) = data else { continue };
        // Dedup on the serialized snapshot payload (envelope-independent, like
        // JSON.stringify(pushed)) — except the first snapshot of this session.
        let raw = data.to_string();
        if !first && last_committed.as_deref() == Some(raw.as_str()) {
            continue;
        }
        first = false;
        *last_committed = Some(raw);
        // Lenient decode: a malformed payload becomes the empty default rather
        // than killing the subscription task (types.rs handles missing fields).
        let snapshot: StateSnapshot = serde_json::from_value(data).unwrap_or_default();
        if tx.send(Event::Snapshot(snapshot)).is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::net::UnixListener;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    const SNAP_A: &str = r#"{"tasks":[],"archivedRecent":[],"sessions":[],"running":[]}"#;
    const SNAP_B: &str = r#"{"tasks":[],"archivedRecent":[],"sessions":[],"running":["t1"]}"#;

    fn sock_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        (dir, sock)
    }

    /// Read one NDJSON request line; return (raw id value, method).
    async fn read_req(reader: &mut BufReader<OwnedReadHalf>) -> (serde_json::Value, String) {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        let method = v["method"].as_str().unwrap().to_string();
        (v["id"].clone(), method)
    }

    /// One scripted subscription session: reply to `subscribe` then `state`
    /// (serving `state_json` as the result), write each push frame, then either
    /// close the connection or hold it open past the test's horizon.
    async fn serve_session(
        listener: &UnixListener,
        state_json: &str,
        pushes: &[&str],
        close_after: bool,
    ) {
        let (stream, _) = listener.accept().await.unwrap();
        let (r, mut w) = stream.into_split();
        let mut reader = BufReader::new(r);
        let (id, method) = read_req(&mut reader).await;
        assert_eq!(method, "subscribe");
        w.write_all(format!("{}\n", json!({"id": id, "result": null})).as_bytes())
            .await
            .unwrap();
        let (id, method) = read_req(&mut reader).await;
        assert_eq!(method, "state");
        let state: serde_json::Value = serde_json::from_str(state_json).unwrap();
        w.write_all(format!("{}\n", json!({"id": id, "result": state})).as_bytes())
            .await
            .unwrap();
        for push in pushes {
            let data: serde_json::Value = serde_json::from_str(push).unwrap();
            w.write_all(format!("{}\n", json!({"event": "state", "data": data})).as_bytes())
                .await
                .unwrap();
        }
        if !close_after {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        // returning drops both halves → connection closes
    }

    #[tokio::test]
    async fn call_returns_result_and_maps_error_frames() {
        let (_dir, sock) = sock_dir();
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, mut w) = stream.into_split();
            let mut reader = BufReader::new(r);
            let (id, method) = read_req(&mut reader).await;
            assert_eq!(method, "ping");
            w.write_all(format!("{}\n", json!({"id": id, "result": "pong"})).as_bytes())
                .await
                .unwrap();
            let (id, _) = read_req(&mut reader).await;
            w.write_all(format!("{}\n", json!({"id": id, "error": "boom"})).as_bytes())
                .await
                .unwrap();
        });

        let mut client = RpcClient::connect(&sock).await.unwrap();
        let ok = client
            .call("ping", serde_json::Value::Null, Duration::from_secs(1))
            .await;
        assert_eq!(ok.unwrap(), json!("pong"));
        let err = client
            .call("retry", json!({"id": "x"}), Duration::from_secs(1))
            .await;
        assert_eq!(err.unwrap_err(), "boom");
    }

    #[tokio::test]
    async fn call_times_out_with_method_in_message() {
        let (_dir, sock) = sock_dir();
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, _w) = stream.into_split();
            let mut reader = BufReader::new(r);
            let mut line = String::new();
            let _ = reader.read_line(&mut line).await; // read, never reply
            tokio::time::sleep(Duration::from_secs(5)).await; // hold open
        });
        let mut client = RpcClient::connect(&sock).await.unwrap();
        let err = client
            .call("ping", serde_json::Value::Null, Duration::from_millis(100))
            .await;
        assert_eq!(err.unwrap_err(), "call timed out: ping");
    }

    #[tokio::test]
    async fn subscription_delivers_state_then_pushes_and_dedups_identical_payloads() {
        let (_dir, sock) = sock_dir();
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            // initial state A; pushes: A (dup — skipped), A (dup — skipped), B
            serve_session(&listener, SNAP_A, &[SNAP_A, SNAP_A, SNAP_B], false).await;
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _handle = spawn_subscription(sock, tx);

        let first = timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        let Event::Snapshot(s) = first else {
            panic!("expected snapshot, got {first:?}")
        };
        assert!(s.running.is_empty());
        let second = timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        let Event::Snapshot(s) = second else {
            panic!("expected snapshot, got {second:?}")
        };
        assert_eq!(s.running, vec!["t1".to_string()]);
        // Nothing else arrives — the two byte-identical pushes were skipped.
        assert!(timeout(Duration::from_millis(300), rx.recv()).await.is_err());
    }

    #[tokio::test]
    async fn reconnect_sends_disconnected_and_recommits_identical_snapshot() {
        let (_dir, sock) = sock_dir();
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            // Session 1 serves A then closes; session 2 serves the SAME A.
            serve_session(&listener, SNAP_A, &[], true).await;
            serve_session(&listener, SNAP_A, &[], false).await;
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _handle = spawn_subscription(sock, tx);

        let first = timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(first, Event::Snapshot(_)));
        let second = timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        assert_eq!(second, Event::Disconnected);
        // After the 2s retry, the byte-identical snapshot IS delivered again —
        // the first snapshot after a (re)connect always commits.
        let third = timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(third, Event::Snapshot(_)));
    }
}
