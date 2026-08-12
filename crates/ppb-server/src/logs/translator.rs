//! Error code → human-readable translation registry (design §19.2).
//! Rule-based; no LLM. Raw log line always visible.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TranslatedError {
    pub title: String,
    pub description: String,
    pub module: String,
    pub severity: String,
}

/// Translate a known error code/pattern into a human explanation.
pub fn translate(code: &str) -> Option<TranslatedError> {
    match code {
        "PMP_OPENUDS_TIMEOUT" => Some(TranslatedError {
            title: "PMP 命令响应超时".into(),
            description: "PPB 已发送命令，但 PMP 未在预算内返回。".into(),
            module: "OpenUDS".into(),
            severity: "warning".into(),
        }),
        "PMP_OPENUDS_UNAVAILABLE" | "PMP_UNAVAILABLE" => Some(TranslatedError {
            title: "PMP 连接不可用".into(),
            description: "OpenUDS 连接断开或 PMP 未运行，PPB 将自动重连。".into(),
            module: "OpenUDS".into(),
            severity: "error".into(),
        }),
        "PMP_COMMAND_ERROR" => Some(TranslatedError {
            title: "PMP 命令执行失败".into(),
            description: "PMP 拒绝了该命令或命令执行出错。".into(),
            module: "PMP".into(),
            severity: "warning".into(),
        }),
        "PHIRA_API_UNAVAILABLE" => Some(TranslatedError {
            title: "Phira API 不可用".into(),
            description: "PPB 无法访问 Phira API，请稍后重试。".into(),
            module: "Phira API".into(),
            severity: "warning".into(),
        }),
        "PHIRA_REAUTH_REQUIRED" => Some(TranslatedError {
            title: "需要重新验证 Phira 身份".into(),
            description: "Phira 凭据已过期，请重新登录验证。".into(),
            module: "Auth".into(),
            severity: "info".into(),
        }),
        "RATE_LIMIT" => Some(TranslatedError {
            title: "请求过于频繁".into(),
            description: "超过速率限制，请稍后重试。".into(),
            module: "PPB".into(),
            severity: "warning".into(),
        }),
        "PERMISSION_DENIED" => Some(TranslatedError {
            title: "权限不足".into(),
            description: "当前身份没有执行该操作的权限。".into(),
            module: "RBAC".into(),
            severity: "warning".into(),
        }),
        "CAPABILITY_NOT_SUPPORTED" => Some(TranslatedError {
            title: "PMP 能力缺失".into(),
            description: "当前 PMP 版本不支持该能力，功能已禁用。".into(),
            module: "PMP".into(),
            severity: "info".into(),
        }),
        _ => None,
    }
}

/// Best-effort pattern fallback (e.g., `PHIRA_API_UNAVAILABLE: 502`).
pub fn translate_pattern(line: &str) -> Option<TranslatedError> {
    for known in [
        "timeout",
        "timed out",
        "connection refused",
        "broken pipe",
        "no route to host",
    ] {
        if line.to_ascii_lowercase().contains(known) {
            return Some(TranslatedError {
                title: "疑似网络/超时错误".into(),
                description: "日志行匹配常见网络错误模式。".into(),
                module: "Network".into(),
                severity: "warning".into(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_translate() {
        assert!(translate("PMP_OPENUDS_TIMEOUT").is_some());
        assert!(translate("PHIRA_REAUTH_REQUIRED").is_some());
        assert!(translate("NO_SUCH_CODE").is_none());
    }

    #[test]
    fn pattern_fallback() {
        assert!(translate_pattern("connection refused to upstream").is_some());
        assert!(translate_pattern("normal operation").is_none());
    }
}
