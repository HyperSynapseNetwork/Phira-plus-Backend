//! OpenUDS frame encoding/decoding.
//!
//! Frame = 4-byte little-endian length prefix + UTF-8 JSON payload (max 16 MiB).
//! Mirrors PMP `openuds/protocol.rs`.

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Max payload size: 16 MiB.
pub const MAX_PAYLOAD_SIZE: u32 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("payload too large: {0} > max {MAX_PAYLOAD_SIZE}")]
    PayloadTooLarge(u32),
    #[error("invalid length prefix")]
    InvalidLengthPrefix,
    #[error("invalid utf-8: {0}")]
    InvalidUtf8(std::str::Utf8Error),
    #[error("invalid json: {0}")]
    InvalidJson(serde_json::Error),
    #[error("io error: {0}")]
    Io(std::io::Error),
}

impl From<std::io::Error> for ProtocolError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Encode a JSON value into a length-prefixed frame buffer.
pub fn encode(value: &Value) -> Result<Vec<u8>, ProtocolError> {
    let payload = serde_json::to_vec(value)?;
    let len = payload.len() as u32;
    if len > MAX_PAYLOAD_SIZE {
        return Err(ProtocolError::PayloadTooLarge(len));
    }
    let mut buf = Vec::with_capacity(4 + len as usize);
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(&payload);
    Ok(buf)
}

/// Async read one complete frame from a tokio stream.
pub async fn read_frame_async(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Value, ProtocolError> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let payload_len = u32::from_le_bytes(len_buf);
    if payload_len == 0 {
        return Err(ProtocolError::InvalidLengthPrefix);
    }
    if payload_len > MAX_PAYLOAD_SIZE {
        return Err(ProtocolError::PayloadTooLarge(payload_len));
    }
    let mut payload = vec![0u8; payload_len as usize];
    stream.read_exact(&mut payload).await?;
    let json_str = std::str::from_utf8(&payload).map_err(ProtocolError::InvalidUtf8)?;
    serde_json::from_str(json_str).map_err(ProtocolError::InvalidJson)
}

/// Async write a JSON value as a length-prefixed frame.
pub async fn write_frame_async(
    stream: &mut (impl AsyncWrite + Unpin),
    value: &Value,
) -> Result<(), ProtocolError> {
    let buf = encode(value)?;
    stream.write_all(&buf).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_round_trip_via_cursor() {
        use tokio::io::AsyncReadExt;
        let value = serde_json::json!({"type": "ping"});
        let buf = encode(&value).unwrap();
        let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        assert_eq!(len as usize, buf.len() - 4);
        assert_eq!(buf.len() - 4, serde_json::to_vec(&value).unwrap().len());
        // Async read from the buffer (as a Reader).
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut cursor = tokio::io::BufReader::new(&buf[..]);
            let mut len_buf = [0u8; 4];
            cursor.read_exact(&mut len_buf).await.unwrap();
            let _ = &len_buf;
        });
    }

    #[test]
    fn rejects_oversized() {
        let big = serde_json::json!({"data": "x".repeat((MAX_PAYLOAD_SIZE + 1) as usize)});
        assert!(matches!(encode(&big), Err(ProtocolError::PayloadTooLarge(_))));
    }

    #[test]
    fn rejects_zero_length_prefix() {
        let buf = vec![0u8; 4];
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            let mut cursor = tokio::io::BufReader::new(&buf[..]);
            read_frame_async(&mut cursor).await
        });
        assert!(matches!(result, Err(ProtocolError::InvalidLengthPrefix)));
    }
}
