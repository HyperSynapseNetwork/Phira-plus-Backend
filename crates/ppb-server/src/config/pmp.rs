//! PMP config Form Descriptor + snapshot/rollback (design §20).
//!
//! PPB does NOT modify PMP's schema; it edits `server_config.yml` through a
//! versioned Form Descriptor, stores full YAML snapshots, and reloads PMP.

use std::path::Path;

use serde::Serialize;

use crate::error::{ApiError, ErrorCode};

/// A PMP config field descriptor (Panel renders grouped forms).
#[derive(Debug, Clone, Serialize)]
pub struct ConfigFieldDescriptor {
    pub path: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub group: &'static str,
    pub widget: &'static str, // switch | text | number | select
    pub risk: &'static str,   // low | medium | high | critical
    pub reload: &'static str, // hot | restart | rebuild
    pub sensitive: bool,
    pub order: u32,
}

macro_rules! field {
    ($path:literal, $label:literal, $desc:literal, $group:literal, $widget:literal, $risk:literal, $reload:literal, $sensitive:expr, $order:expr) => {
        ConfigFieldDescriptor {
            path: $path,
            label: $label,
            description: $desc,
            group: $group,
            widget: $widget,
            risk: $risk,
            reload: $reload,
            sensitive: $sensitive,
            order: $order,
        }
    };
}

/// Versioned descriptor for PMP `server_config.yml` (source: PMP server/config.rs).
pub fn pmp_config_descriptor() -> &'static [ConfigFieldDescriptor] {
    &[
        field!("server_name", "服务器名称", "服务器显示名称", "server", "text", "low", "hot", false, 1),
        field!("welcome", "欢迎语", "加入房间时的欢迎消息", "server", "text", "low", "hot", false, 2),
        field!("chat_enabled", "启用聊天", "是否允许房间聊天", "chat", "switch", "low", "hot", false, 10),
        field!("chat_history_limit", "聊天历史条数", "房间聊天历史上限", "chat", "number", "low", "hot", false, 11),
        field!("port", "TCP 端口", "多人游戏 TCP 端口", "network", "number", "high", "restart", false, 20),
        field!("http_port", "HTTP 端口", "内部 HTTP/SSE/WS 端口", "network", "number", "high", "restart", false, 21),
        field!("max_rooms", "最大房间数", "同时存在的房间上限", "room", "number", "medium", "restart", false, 30),
        field!("max_users_per_room", "每房最大人数", "每个房间玩家上限", "room", "number", "medium", "restart", false, 31),
        field!("room_creation_enabled", "允许建房", "是否允许创建房间", "room", "switch", "medium", "hot", false, 32),
        field!("ready_countdown_secs", "开始倒计时", "准备后开始倒计时秒数", "room", "number", "low", "hot", false, 33),
        field!("max_sessions", "最大会话数", "服务器会话上限", "limit", "number", "high", "restart", false, 40),
        field!("graceful_shutdown_timeout_secs", "优雅关闭超时", "关闭超时秒数", "server", "number", "low", "restart", false, 41),
        field!("log_retention_days", "日志保留天数", "日志保留周期", "log", "number", "low", "restart", false, 50),
        field!("phira_api_endpoint", "Phira API 地址", "PMP 使用的 Phira API 地址", "phira", "text", "high", "restart", false, 60),
        field!("database_url", "数据库连接串", "PMP PostgreSQL 连接串", "database", "text", "critical", "restart", true, 70),
        field!("admin_phira_ids", "管理员 Phira ID", "PMP 管理员列表", "admin", "text", "high", "hot", false, 80),
        field!("filtered_player_ids", "过滤玩家", "被过滤的玩家 ID 列表", "moderation", "text", "medium", "hot", false, 90),
        field!("openuds.enabled", "OpenUDS 启用", "是否启用 OpenUDS 管理接口", "openuds", "switch", "high", "restart", false, 100),
        field!("openuds.socket_path", "OpenUDS 套接字路径", "Unix Domain Socket 路径", "openuds", "text", "high", "restart", false, 101),
        field!("openuds.auth_token", "OpenUDS 令牌", "token 认证令牌（approve 模式留空）", "openuds", "text", "critical", "restart", true, 102),
    ]
}

/// Reads/writes the PMP config YAML and manages snapshots.
#[derive(Clone)]
pub struct PmpConfigManager {
    config_path: Option<std::path::PathBuf>,
}

impl PmpConfigManager {
    pub fn new(config_path: Option<std::path::PathBuf>) -> Self {
        Self { config_path }
    }

    pub fn configured(&self) -> bool {
        self.config_path.is_some()
    }

    pub fn path(&self) -> Result<&Path, ApiError> {
        self.config_path
            .as_deref()
            .ok_or_else(|| ApiError::new(ErrorCode::PmpUnavailable, "pmp.config_path not configured"))
    }

    /// Read the current PMP config YAML.
    pub fn read_yaml(&self) -> Result<String, ApiError> {
        std::fs::read_to_string(self.path()?)
            .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))
    }

    /// Atomic write: write to a temp file in the same directory, then rename.
    pub fn write_yaml_atomic(&self, content: &str) -> Result<(), ApiError> {
        let path = self.path()?;
        let dir = path
            .parent()
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "config path has no parent"))?;
        let tmp = dir.join(format!(".{}.ppb.tmp", path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default()));
        std::fs::write(&tmp, content).map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
        Ok(())
    }

    /// Extract a field value from the YAML (by dotted path) as a string.
    pub fn field_value(&self, yaml: &str, path: &str) -> Option<serde_yaml::Value> {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
        let mut current = &value;
        for part in path.split('.') {
            current = current.get(part)?;
        }
        Some(current.clone())
    }
}
