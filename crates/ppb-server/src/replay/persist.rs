//! PMP `persist.touches/judges` paginated client (design §12.5).
//!
//! PMP returns a bare array of batches:
//! `[{sequence, round_uuid, player_id, count, first_game_time, last_game_time,
//!    payload, created_at}]`, ordered by `sequence` ASC.

use serde_json::{json, Value};

use crate::pmp::openuds::client::{OpenUdsClient, OpenUdsError};

pub const MAX_PAGE: i64 = 500;

/// Fetch durable round metadata. PMP owns the round/chart/room relationship;
/// PPB only relays it and never persists a duplicate Replay index.
pub async fn fetch_rounds(
    openuds: &OpenUdsClient,
    round_uuid: Option<&str>,
    player_id: Option<i32>,
    limit: i64,
) -> Result<Vec<Value>, OpenUdsError> {
    let mut params = json!({ "limit": limit.clamp(1, 1000) });
    if let Some(round) = round_uuid {
        params["round_uuid"] = json!(round);
    }
    if let Some(player) = player_id {
        params["player_id"] = json!(player);
    }
    let value = openuds
        .command_with_timeout("persist.rounds", params, Some(60_000))
        .await?;
    Ok(value
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| value.as_array().cloned())
        .unwrap_or_default())
}

/// Traverse every `persist.rounds` page. PMP page size remains bounded at 1000;
/// callers that need the complete inventory must not confuse that page size with
/// a global history limit.
pub async fn fetch_all_rounds(
    openuds: &OpenUdsClient,
    round_uuid: Option<&str>,
    player_id: Option<i32>,
) -> Result<Vec<Value>, OpenUdsError> {
    const PAGE: i64 = 1000;
    let mut all = Vec::new();
    let mut offset = 0i64;
    loop {
        let mut params = json!({ "limit": PAGE, "offset": offset });
        if let Some(round) = round_uuid { params["round_uuid"] = json!(round); }
        if let Some(player) = player_id { params["player_id"] = json!(player); }
        let value = openuds.command_with_timeout("persist.rounds", params, Some(60_000)).await?;
        let items = value.get("items").and_then(Value::as_array).cloned().or_else(|| value.as_array().cloned()).unwrap_or_default();
        let count = items.len() as i64;
        all.extend(items);
        let next = value.get("next_offset").and_then(Value::as_i64);
        match next {
            Some(next) if next > offset => offset = next,
            _ if count == PAGE => offset += count, // legacy-compatible fallback
            _ => break,
        }
    }
    Ok(all)
}

/// Fetch a page of persist batches.
pub async fn fetch_batches(
    openuds: &OpenUdsClient,
    stream: &str, // "touches" | "judges"
    since: i64,
    limit: i64,
    round_uuid: Option<&str>,
    player_id: Option<i32>,
) -> Result<Value, OpenUdsError> {
    let mut params = json!({ "since": since.max(0), "limit": limit.clamp(1, MAX_PAGE) });
    if let Some(r) = round_uuid {
        params["round_uuid"] = json!(r);
    }
    if let Some(p) = player_id {
        params["player_id"] = json!(p);
    }
    // Persist queries page large batches (up to MAX_PAGE) and can exceed the
    // 10s default; give them a generous per-command budget.
    openuds
        .command_with_timeout(&format!("persist.{stream}"), params, Some(60_000))
        .await
}

/// Fetch every page for one replay stream. The sequence cursor must advance;
/// otherwise the loop stops to avoid repeating a malformed upstream page.
pub async fn fetch_all_batches(
    openuds: &OpenUdsClient,
    stream: &str,
    round_uuid: &str,
    player_id: i32,
) -> Result<Vec<Value>, OpenUdsError> {
    let mut all = Vec::new();
    let mut since = 0i64;
    loop {
        let page = fetch_batches(
            openuds,
            stream,
            since,
            MAX_PAGE,
            Some(round_uuid),
            Some(player_id),
        )
        .await?;
        let batches = batches_of(&page);
        if batches.is_empty() {
            break;
        }
        let next = batches
            .iter()
            .filter_map(|batch| batch.get("sequence").and_then(Value::as_i64))
            .max()
            .unwrap_or(since);
        all.extend(batches);
        if next <= since || all.len() % MAX_PAGE as usize != 0 {
            break;
        }
        since = next;
    }
    Ok(all)
}

/// Normalize the persist response to a `Vec<Value>` of batch objects
/// (handles both a bare array and a wrapped `{"data":[...]}`).
pub fn batches_of(value: &Value) -> Vec<Value> {
    if let Some(arr) = value.as_array() {
        arr.clone()
    } else if let Some(arr) = value.get("data").and_then(Value::as_array) {
        arr.clone()
    } else {
        Vec::new()
    }
}

/// Summarize a page of batches: batch count, total frames, players, time range,
/// and the highest sequence (for continued pagination).
pub fn summarize_batches(batches: &[Value]) -> Value {
    let mut players = std::collections::BTreeSet::new();
    let mut total_frames = 0i64;
    let mut first_time: Option<f64> = None;
    let mut last_time: Option<f64> = None;
    let mut max_seq: i64 = 0;
    for b in batches {
        if let Some(seq) = b.get("sequence").and_then(Value::as_i64) {
            max_seq = max_seq.max(seq);
        }
        if let Some(p) = b.get("player_id").and_then(Value::as_i64) {
            players.insert(p);
        }
        if let Some(c) = b.get("count").and_then(Value::as_i64) {
            total_frames += c;
        }
        if let Some(t) = b.get("first_game_time").and_then(Value::as_f64) {
            first_time = Some(first_time.map_or(t, |f: f64| f.min(t)));
        }
        if let Some(t) = b.get("last_game_time").and_then(Value::as_f64) {
            last_time = Some(last_time.map_or(t, |f: f64| f.max(t)));
        }
    }
    json!({
        "batches": batches.len(),
        "frames": total_frames,
        "players": players.into_iter().collect::<Vec<i64>>(),
        "first_game_time": first_time,
        "last_game_time": last_time,
        "last_sequence": max_seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_bare_and_wrapped() {
        let bare = json!([{"sequence": 1}]);
        assert_eq!(batches_of(&bare).len(), 1);
        let wrapped = json!({"data": [{"sequence": 2}]});
        assert_eq!(batches_of(&wrapped).len(), 1);
        assert_eq!(batches_of(&json!({"x": 1})).len(), 0);
    }

    #[test]
    fn summary_counts() {
        let batches = vec![
            json!({"sequence": 1, "player_id": 7, "count": 10, "first_game_time": 0.5, "last_game_time": 5.0}),
            json!({"sequence": 2, "player_id": 9, "count": 20, "first_game_time": 1.0, "last_game_time": 9.0}),
        ];
        let s = summarize_batches(&batches);
        assert_eq!(s["batches"], 2);
        assert_eq!(s["frames"], 30);
        assert_eq!(s["players"], json!([7, 9]));
        assert_eq!(s["first_game_time"], 0.5);
        assert_eq!(s["last_game_time"], 9.0);
        assert_eq!(s["last_sequence"], 2);
    }
}
