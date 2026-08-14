//! Live WebSocket route: `WSS /ws/v1/rooms/{room_id}/live`.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};

use super::JitterMode;
use crate::app::AppState;
use crate::error::ApiError;

#[derive(Debug, Deserialize)]
pub struct LiveWsParams {
    /// `low_latency` (1s) or `stable` (2s). Defaults to low_latency.
    #[serde(default)]
    pub mode: Option<String>,
}

/// WSS /ws/v1/rooms/{room_id}/live
pub async fn live_ws(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Query(params): Query<LiveWsParams>,
    ws: WebSocketUpgrade,
) -> Result<axum::response::Response, ApiError> {
    let mode = match params.mode.as_deref() {
        Some("stable") => JitterMode::Stable,
        _ => JitterMode::LowLatency,
    };
    Ok(ws.on_upgrade(move |socket| live_ws_task(socket, state, room_id, mode)))
}

async fn live_ws_task(socket: WebSocket, state: Arc<AppState>, room_id: String, mode: JitterMode) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = state.openuds.subscribe_stream_frames();
    let mut players: Option<BTreeSet<i64>> = None;
    let mut buffer: Vec<Value> = Vec::new();
    let mut last_seq: HashMap<String, u64> = HashMap::new();
    let mut last_round: Option<String> = None;
    let tick = Duration::from_millis(mode.default_delay_ms());
    let mut interval = tokio::time::interval(tick);
    interval.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            recv = rx.recv() => {
                match recv {
                    Ok(frame) => {
                        // The stream frame's `room` is the room id string; the WS
                        // path carries that identifier (see P-82 report note).
                        let room_match = frame.room.as_deref() == Some(&room_id);
                        if !room_match {
                            continue;
                        }
                        if let Some(seq) = frame.sequence {
                            let key = frame.stream.clone();
                            let prev = last_seq.get(&key).copied();
                            last_seq.insert(key.clone(), seq);
                            if let Some(p) = prev {
                                if seq > p + 1 {
                                    let _ = sink
                                        .send(Message::text(json!({
                                            "type": "resync",
                                            "stream": key,
                                            "expected": p + 1,
                                            "reason": "sequence_gap",
                                        }).to_string()))
                                        .await;
                                }
                            }
                        }
                        if let Some(round) = frame.round.clone() {
                            if last_round.as_deref() != Some(&round) {
                                last_round = Some(round.clone());
                                let _ = sink
                                    .send(Message::text(json!({
                                        "type": "round_switch",
                                        "round": round,
                                    }).to_string()))
                                    .await;
                            }
                        }
                        if let Some(set) = &players {
                            if !set.contains(&frame.user_id) {
                                continue;
                            }
                        }
                        let event_type = if frame.stream == "judges" { "judges" } else { "touches" };
                        let mut event = json!({
                            "type": event_type,
                            "player": frame.user_id,
                            "sequence": frame.sequence,
                            "round": frame.round,
                            "timestamp": frame.timestamp,
                        });
                        event[if event_type == "judges" { "judges" } else { "frames" }] = frame.frames;
                        buffer.push(event);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = sink
                            .send(Message::text(json!({"type":"resync","reason":"lagged"}).to_string()))
                            .await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = interval.tick() => {
                if !buffer.is_empty() {
                    let batch = std::mem::take(&mut buffer);
                    for event in batch {
                        if sink.send(Message::text(event.to_string())).await.is_err() {
                            return;
                        }
                    }
                }
                let _ = sink.send(Message::text(json!({"type":"heartbeat"}).to_string())).await;
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(text.as_str()) {
                            match v.get("type").and_then(Value::as_str) {
                                Some("set_players") => {
                                    players = v.get("players").and_then(Value::as_array)
                                        .map(|arr| arr.iter().filter_map(|x| x.as_i64()).collect());
                                }
                                Some("resync") => {
                                    last_seq.clear();
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }
}
