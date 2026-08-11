//! Chart viewer blob (contract §19): `GET /api/v1/charts/{id}/viewer` →
//! bincode `(ChartInfo, Chart)` with varint encoding.
//!
//! The chart file (zip: `info.yml` + chart JSON, or a bare chart JSON) is
//! parsed into PPB-defined `ChartInfo`/`Chart` and serialized. The field set
//! is the PPB contract (P-84); freeze it with PPF before the viewer consumes.

use std::io::Read;

use bincode::Options as _;
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ErrorCode};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChartInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub composer: String,
    #[serde(default)]
    pub illustrator: String,
    #[serde(default)]
    pub charter: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub difficulty: String,
    #[serde(default)]
    pub rating: Option<f64>,
    #[serde(default)]
    pub ranked: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub uploader: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Chart {
    #[serde(default)]
    pub lines: Vec<ChartLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartLine {
    pub id: i32,
    #[serde(default)]
    pub notes: Vec<ChartNote>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartNote {
    pub time: f64,
    pub lane: i32,
    #[serde(default)]
    pub width: f64,
    #[serde(rename = "type", default)]
    pub note_type: String,
    #[serde(default)]
    pub hold: Option<f64>,
    #[serde(default)]
    pub visible_time: Option<f64>,
}

/// Build the bincode `(ChartInfo, Chart)` blob from a chart file. Tries zip
/// (`info.yml` + chart JSON) first, then a bare chart JSON.
pub fn build_chart_blob(bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
    if let Ok(blob) = build_from_zip(bytes) {
        return Ok(blob);
    }
    let chart: Chart = serde_json::from_slice(bytes)
        .map_err(|e| ApiError::new(ErrorCode::PhiraApiUnavailable, format!("chart parse: {e}")))?;
    if chart.lines.is_empty() {
        return Err(ApiError::new(ErrorCode::PhiraApiUnavailable, "chart has no notes"));
    }
    encode_blob(ChartInfo::default(), chart)
}

fn build_from_zip(bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| ApiError::new(ErrorCode::PhiraApiUnavailable, format!("zip open: {e}")))?;
    let mut info = ChartInfo::default();
    let mut chart = Chart::default();
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    for name in names {
        let lower = name.to_lowercase();
        let mut file = archive
            .by_name(&name)
            .map_err(|e| ApiError::new(ErrorCode::PhiraApiUnavailable, format!("zip entry {name}: {e}")))?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|e| ApiError::new(ErrorCode::PhiraApiUnavailable, format!("zip read {name}: {e}")))?;
        if lower.ends_with(".yml") || lower.ends_with(".yaml") {
            if let Ok(i) = serde_yaml::from_str::<ChartInfo>(&text) {
                info = i;
            }
        } else if lower.ends_with(".json") {
            if let Ok(c) = serde_json::from_str::<Chart>(&text) {
                chart = c;
            }
        }
    }
    if chart.lines.is_empty() {
        return Err(ApiError::new(ErrorCode::PhiraApiUnavailable, "chart zip has no notes"));
    }
    encode_blob(info, chart)
}

fn encode_blob(info: ChartInfo, chart: Chart) -> Result<Vec<u8>, ApiError> {
    bincode::options()
        .with_varint_encoding()
        .serialize(&(info, chart))
        .map_err(|e| ApiError::new(ErrorCode::Internal, format!("bincode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_bare_chart_json() {
        let chart = r#"{"lines":[{"id":1,"notes":[{"time":1.0,"lane":2,"type":"tap"}]}]}"#;
        let blob = build_chart_blob(chart.as_bytes()).unwrap();
        let (info, parsed): (ChartInfo, Chart) =
            bincode::options().with_varint_encoding().deserialize(&blob).unwrap();
        assert!(info.name.is_empty());
        assert_eq!(parsed.lines.len(), 1);
        assert_eq!(parsed.lines[0].notes[0].lane, 2);
    }

    #[test]
    fn rejects_garbage() {
        assert!(build_chart_blob(b"not a chart").is_err());
    }

    #[test]
    fn parses_zip_with_info_and_chart() {
        // Minimal zip: info.yml + chart.json.
        let mut buf = Vec::new();
        {
            use zip::write::FileOptions;
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file("info.yml", opts).unwrap();
            writer.write_all(b"name: Test Song\ncomposer: Composer\n").unwrap();
            writer.start_file("chart.json", opts).unwrap();
            writer.write_all(br#"{"lines":[{"id":1,"notes":[]}]}"#).unwrap();
            writer.finish().unwrap();
        }
        let blob = build_chart_blob(&buf).unwrap();
        let (info, chart): (ChartInfo, Chart) =
            bincode::options().with_varint_encoding().deserialize(&blob).unwrap();
        assert_eq!(info.name, "Test Song");
        assert_eq!(info.composer, "Composer");
        assert_eq!(chart.lines.len(), 1);
    }
}
