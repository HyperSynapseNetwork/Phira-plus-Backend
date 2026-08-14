//! PMP config Form Descriptor + snapshot/rollback (design §20).
//!
//! PPB does NOT modify PMP's schema; it edits `server_config.yml` through a
//! versioned Form Descriptor, stores full YAML snapshots, and reloads PMP.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::error::{ApiError, ErrorCode};

/// A PMP config field descriptor (Panel renders grouped forms). Schema-freeze
/// (§22): descriptor carries type/widget/min/max/risk/permission/reload
/// semantics/sensitive/default.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigFieldDescriptor {
    pub path: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    /// Wire type: `string | number | boolean`.
    pub r#type: &'static str,
    pub widget: &'static str, // switch | text | number | select
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    pub risk: &'static str,   // low | medium | high | critical
    pub permission: &'static str,
    /// hot | restart | rebuild
    pub reload_semantics: &'static str,
    pub sensitive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    pub order: u32,
}

/// A named group of related config fields (Panel renders per group).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigFieldGroup {
    pub key: &'static str,
    pub label: &'static str,
    pub fields: Vec<ConfigFieldDescriptor>,
}

macro_rules! field {
    ($path:literal, $label:literal, $desc:literal, $group:literal, $widget:literal, $risk:literal, $reload:literal, $sensitive:expr, $order:expr) => {
        ConfigFieldDescriptor {
            path: $path,
            label: $label,
            description: $desc,
            r#type: match $widget {
                "switch" => "boolean",
                "number" => "number",
                _ => "string",
            },
            widget: $widget,
            min: None,
            max: None,
            risk: $risk,
            permission: if $reload == "hot" { "config:reload" } else { "config:rollback" },
            reload_semantics: $reload,
            sensitive: $sensitive,
            default: None,
            order: $order,
        }
    };
}

/// Versioned flat descriptor for PMP `server_config.yml` (source: PMP server/config.rs).
pub fn pmp_config_descriptor() -> Vec<ConfigFieldDescriptor> {
    vec![
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
        field!("openuds.auth_token", "OpenUDS 令牌", "token 认证令牌（留空则按 socket 文件权限直接放行）", "openuds", "text", "critical", "restart", true, 102),
    ]
}

/// Grouped descriptor (§22 model A): `{ key, label, fields }`.
pub fn pmp_config_groups() -> Vec<ConfigFieldGroup> {
    let mut groups: Vec<ConfigFieldGroup> = Vec::new();
    for f in pmp_config_descriptor() {
        let group_key = group_key_of(&f);
        match groups.iter_mut().find(|g| g.key == group_key) {
            Some(g) => g.fields.push(f.clone()),
            None => groups.push(ConfigFieldGroup {
                key: group_key,
                label: group_label(group_key),
                fields: vec![f.clone()],
            }),
        }
    }
    for g in &mut groups {
        g.fields.sort_by_key(|f| f.order);
    }
    groups
}

fn group_key_of(f: &ConfigFieldDescriptor) -> &'static str {
    match f.path {
        "server_name" | "welcome" | "graceful_shutdown_timeout_secs" => "server",
        "chat_enabled" | "chat_history_limit" => "chat",
        "port" | "http_port" => "network",
        "max_rooms" | "max_users_per_room" | "room_creation_enabled" | "ready_countdown_secs" => "room",
        "max_sessions" => "limit",
        "log_retention_days" => "log",
        "phira_api_endpoint" => "phira",
        "database_url" => "database",
        "admin_phira_ids" => "admin",
        "filtered_player_ids" => "moderation",
        _ => "openuds",
    }
}

fn group_label(key: &str) -> &'static str {
    match key {
        "server" => "服务器",
        "chat" => "聊天",
        "network" => "网络",
        "room" => "房间",
        "limit" => "限制",
        "log" => "日志",
        "phira" => "Phira",
        "database" => "数据库",
        "admin" => "管理员",
        "moderation" => "审核",
        _ => "OpenUDS",
    }
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
            .map_err(|error| { tracing::error!(%error, "PMP config operation failed"); ApiError::internal() })
    }

    /// Atomic write: write to a temp file in the same directory, then rename.
    pub fn write_yaml_atomic(&self, content: &str) -> Result<(), ApiError> {
        let path = self.path()?;
        let dir = path
            .parent()
            .ok_or_else(|| ApiError::new(ErrorCode::InternalError, "config path has no parent"))?;
        let tmp = dir.join(format!(".{}.ppb.tmp", path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default()));
        std::fs::write(&tmp, content).map_err(|error| { tracing::error!(%error, "PMP config operation failed"); ApiError::internal() })?;
        std::fs::rename(&tmp, path).map_err(|error| { tracing::error!(%error, "PMP config operation failed"); ApiError::internal() })?;
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
