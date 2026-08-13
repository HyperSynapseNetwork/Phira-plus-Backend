//! Permission Manifest — the single source of permission definitions.
//!
//! Format `<resource>:<action>`; `room:*` allowed; `*:*` is Root-only. Panel
//! renders grouped from this manifest; frontends never hardcode the full set.

use serde::Serialize;

/// Risk level attached to a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

/// One permission definition (matches contract §5 / design §8.2).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PermissionDef {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub root_only: bool,
    pub risk: Risk,
}

macro_rules! perm {
    ($id:literal, $group:literal, $label:literal, $desc:literal, $root_only:expr, $risk:expr) => {
        PermissionDef {
            id: $id,
            group: $group,
            label: $label,
            description: $desc,
            root_only: $root_only,
            risk: $risk,
        }
    };
}

/// Static permission manifest. Kept in sync with contract §5.
pub fn permission_manifest() -> &'static [PermissionDef] {
    &[
        // room
        perm!("room:view", "room", "查看房间", "查看房间信息/玩家/状态", false, Risk::Low),
        perm!("room:kick", "room", "踢出房间玩家", "从房间移除玩家", false, Risk::Medium),
        perm!("room:move", "room", "移动玩家", "将玩家移入目标房间", false, Risk::Medium),
        perm!("room:start", "room", "开始/取消开始", "控制房间开始或取消", false, Risk::Medium),
        perm!("room:config", "room", "配置房间", "锁/循环/隐藏/持久化/换谱/房主等", false, Risk::Medium),
        perm!("room:whitelist", "room", "管理白名单", "房间白名单增删查", false, Risk::Medium),
        perm!("room:blacklist", "room", "管理黑名单", "房间黑名单增删查", false, Risk::Medium),
        perm!("room:manage", "room", "管理房间", "房间级管理操作全集", false, Risk::High),
        // user
        perm!("user:view", "user", "查看用户", "查看用户信息", false, Risk::Low),
        perm!("user:kick", "user", "踢出玩家", "从服务器断开玩家", false, Risk::Medium),
        perm!("user:ban", "user", "封禁玩家", "封禁用户 ID", false, Risk::High),
        perm!("user:ban_ip", "user", "封禁 IP", "封禁玩家 IP", false, Risk::High),
        perm!("user:view_ip_history", "user", "查看 IP 历史", "查看玩家 IP 历史", false, Risk::High),
        // server
        perm!("server:view", "server", "查看服务器", "查看服务器状态/统计", false, Risk::Low),
        perm!("server:manage", "server", "管理服务器", "常规服务器管理", false, Risk::High),
        perm!("server:update", "server", "更新服务器", "PMP 更新 apply", true, Risk::Critical),
        perm!("server:shutdown", "server", "关闭服务器", "关闭 PMP", true, Risk::Critical),
        perm!("server:start", "server", "启动服务器", "启动 PMP（受控 Supervisor）", true, Risk::Critical),
        // config
        perm!("config:view", "config", "查看配置", "查看 PMP/PPB 配置", false, Risk::Low),
        perm!("config:reload", "config", "重载配置", "热重载配置", false, Risk::High),
        perm!("config:rollback", "config", "回滚配置", "回滚到历史快照", true, Risk::Critical),
        // plugin
        perm!("plugin:view", "plugin", "查看插件", "列出/查看插件", false, Risk::Low),
        perm!("plugin:manage", "plugin", "管理插件", "启用/禁用/重载/移除", true, Risk::High),
        perm!("plugin:call", "plugin", "调用插件 API", "plugin.call", false, Risk::High),
        // audit
        perm!("audit:view", "audit", "查看审计", "查看审计日志", false, Risk::Medium),
        perm!("audit:export", "audit", "导出审计", "导出审计 CSV", false, Risk::Medium),
        // broadcast
        perm!("broadcast:all", "broadcast", "全服广播", "broadcast.all 系统广播", false, Risk::High),
        perm!("broadcast:room", "broadcast", "房间广播", "broadcast.room 系统广播", false, Risk::Medium),
        perm!("broadcast:user", "broadcast", "用户广播", "broadcast.user 系统消息", false, Risk::Medium),
        // pmp
        perm!("pmp:cli", "pmp", "PMP 控制台", "原始 cli.execute 控制台", false, Risk::High),
        // logs
        perm!("logs:view", "logs", "查看日志", "查看 PMP 日志与控制台输出", false, Risk::Low),
        // notification
        perm!("notification:send_system", "notification", "系统通知", "向用户发送系统通知", false, Risk::Medium),
        // coupon
        perm!("coupon:view", "coupon", "查看兑换码", "查看兑换码", false, Risk::Low),
        perm!("coupon:create", "coupon", "创建兑换码", "创建兑换码", false, Risk::Medium),
        perm!("coupon:manage", "coupon", "管理兑换码", "管理兑换码状态", true, Risk::High),
        perm!("coupon:revoke", "coupon", "撤销兑换码", "撤销已发放兑换码", true, Risk::High),
        // group
        perm!("group:view", "group", "查看用户组", "查看用户组与成员", false, Risk::Low),
        perm!("group:create", "group", "创建用户组", "创建自定义用户组", false, Risk::Medium),
        perm!("group:edit", "group", "编辑用户组", "改名/描述/权限", false, Risk::Medium),
        perm!("group:delete", "group", "删除用户组", "删除用户组", false, Risk::High),
        perm!("group:assign_user", "group", "分配成员", "增删用户组成员", false, Risk::Medium),
        // automation
        perm!("automation:view", "automation", "查看自动化", "查看 Runbook 与运行记录", false, Risk::Low),
        perm!("automation:edit", "automation", "编辑自动化", "创建/修改/删除 Runbook", false, Risk::Medium),
        perm!("automation:execute", "automation", "执行自动化", "运行 Runbook（逐步骤重鉴权）", false, Risk::High),
        // dashboard
        perm!("dashboard:view", "dashboard", "查看仪表盘", "查看管理仪表盘", false, Risk::Low),
        // preference
        perm!("preference:manage", "preference", "管理偏好", "管理用户偏好", false, Risk::Low),
    ]
}

/// The marker permission for Root only.
pub const ROOT_WILDCARD: &str = "*:*";

impl PermissionDef {
    pub fn root_only_ids() -> Vec<&'static str> {
        permission_manifest()
            .iter()
            .filter(|p| p.root_only)
            .map(|p| p.id)
            .collect()
    }

    /// All non-root-only ids (auto-granted to `admin_scope` groups).
    pub fn non_root_only_ids() -> Vec<&'static str> {
        permission_manifest()
            .iter()
            .filter(|p| !p.root_only)
            .map(|p| p.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_unique_and_wellformed() {
        let manifest = permission_manifest();
        let mut seen = std::collections::HashSet::new();
        for p in manifest {
            assert!(seen.insert(p.id), "duplicate permission id {}", p.id);
            assert_eq!(p.id.split(':').count(), 2, "permission must be resource:action");
            assert!(!p.id.is_empty());
        }
    }

    #[test]
    fn no_manifest_entry_is_wildcard_star() {
        for p in permission_manifest() {
            assert_ne!(p.id, ROOT_WILDCARD);
        }
    }

    #[test]
    fn admin_scope_set_is_nonempty() {
        assert!(!PermissionDef::non_root_only_ids().is_empty());
    }
}
