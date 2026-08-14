# 开发指南（Development）

> 面向 PPB 贡献者：模块布局、架构、数据模型、测试。跨仓契约与设计基线见 `contracts/README.md` 与 `DESIGN/PP-B-F-P_V3_总体设计规范.md`。

## 架构概览

```
Browser (PPF/Panel)
   │  HTTPS / SSE / Live WS / Replay WS
   ▼
ppb-server (axum)                ── Phira API 客户端（数据网关 + 凭据加密）
   │                             ── OpenUDS 客户端（typed 命令 + 事件 + 高频流）
   │  sqlx migrate ─────────────► PostgreSQL（PPB 自有库）
   │
   ├─ auth        ：Phira 登录 → PPB JWT / Root / GitHub 绑定 / reauth
   ├─ permissions ：Manifest + 三级解析 + groups
   ├─ actions     ：Action Registry + Command Broker
   ├─ pmp/openuds ：协议 / 客户端 / 事件 / 高频流
   ├─ phira       ：数据网关 + TopChart 聚合
   ├─ rooms       ：房间命令门面（OpenUDS typed 包装）
   ├─ replay      ：Replay 策略（可见性 / ACL / share links，仅策略，不存内容）
   ├─ live/replay ：WebSocket 网关（JSON 信封）
   ├─ public      ：/api/v1/public/*（meta/site/announcements/downloads/nodes）
   └─ admin       ：/api/v1/admin/*（root 认证 + 管理超集）
```

**领域垂直规则（design §24.1）**：每个领域拥有自己的 `model/repo/service/routes`；无全局 `models/` + `services/`。

## 目录布局

```
ppb/
├── Cargo.toml                     # workspace 根 + workspace.dependencies
├── rust-toolchain.toml            # 1.96.0（rustfmt/clippy/rust-src）
├── config/
│   ├── example.toml               # 运行时配置示例（无密钥）
│   └── example.env                # 部署环境变量参考（无真实值）
├── migrations/                    # sqlx 迁移（0001_init / 0002_phase_b / 0003_coupons）
├── scripts/sync-workspace-version.py  # CI 版本一致性脚本
├── deploy/                        # docker-compose / nginx / caddy / systemd / update.sh
├── docs/                          # 本文档集
└── crates/ppb-server/
    ├── Cargo.toml                 # bin: ppb-server + ppctl
    └── src/
        ├── main.rs                # 进程入口（--version / root init / --check-config）
        ├── lib.rs                 # 模块图
        ├── app.rs                 # AppState + build_router + build_state + 后台任务
        ├── telemetry.rs
        ├── config/                # 部署 env + 运行时 TOML + DB overrides
        ├── error/                 # 统一错误契约 + 分页
        ├── middleware/            # request_id / csrf / cors / rate_limit / auth extractor
        ├── auth/                  # phira / root / github / reauth / jwt / session / gateway
        ├── users/ identities/     # 用户与身份绑定（phira|github）
        ├── permissions/           # manifest / resolver / groups
        ├── preferences/           # JSONB + revision 乐观并发
        ├── social/                # friends / blocks
        ├── rooms/                 # 房间命令门面
        ├── replay/                # replay 策略（visibility/acl/share_links/persist）
        ├── notifications/         # notification_events / user_notifications / push_endpoints
        ├── actions/ commands/     # Action Registry + Command Broker + command_runs
        ├── jobs/ automation/      # jobs + runbooks
        ├── audit/ logs/ metrics/  # 审计 / 日志 / 指标
        ├── phira/                 # Phira API 客户端 + 凭据加密 + 数据网关 + 聚合
        ├── pmp/                   # capabilities / openuds / events / cli
        ├── live/                  # Live WS 网关
        ├── public/                # /api/v1/public/*
        ├── admin/                 # /api/v1/admin/*（server/plugins/notifications/coupons/routes）
        └── join_intent/           # JoinIntent 存储 + user.online → room.force_move
```

## 数据模型（迁移概览）

PPB 只持久化**身份 / 策略 / 控制 / 社区 / 偏好 / 通知 / 审计**。Replay 表只存**策略**，绝不存 Replay 内容；`room.*` / online 状态是运行时缓存，绝不作为事实落盘。

关键表（`migrations/0001_init.sql` + `0002_phase_b.sql` + `0003_coupons.sql`）：

- `users` / `user_identities`（provider = phira|github）/ `phira_credentials`（加密 refresh token）
- `sessions`（principal_type user|root）/ `root_credentials`（Root 本地凭据，bcrypt）
- `groups` / `group_members` / `group_permissions`（`CHECK (permission <> '*:*')`）
- `user_profiles` / `friend_requests` / `friendships` / `user_blocks`
- `replay_overrides` / `replay_acl` / `replay_share_links`（策略）
- `user_preferences`（namespace common|ppf|panel|experiments，JSONB + revision）
- `notification_events` / `user_notifications` / `push_endpoints`
- `audit_events` / `command_runs` / `jobs`
- `coupons`（技术兼容命名；产品语义为“兑换码”）

## 认证与权限（要点）

- **Phira 登录**：PPB 调 Phira `/login`（access+refresh）→ `/me`（phira_id）→ upsert PPB user + phira identity → 加密存 refresh token → 签发 `ppb_access` JWT（`Secure+HttpOnly` cookie，host-only `api-phira.htadiy.com`）→ `ppb_csrf` cookie（double-submit CSRF）。
- **Root**：独立于 `users` 表，本地 bcrypt 凭据；首启随机口令 `ppb-server root init` 打印一次。
- **GitHub**：仅**绑定**到已登录用户（state 里携带 user），绝不创建裸账号。
- **权限解析**：User → Groups → Permissions（V1 无 direct per-user override）；`admin_scope` 自动映射全部 `root_only=false`；`*:*` 仅 Root。

## 测试

CI（`cargo test --locked --workspace --all-features`）覆盖：

- Phira 登录成功/失败（mock Phira API）、token 刷新、refresh 过期 → `PHIRA_REAUTH_REQUIRED`
- Root 与普通用户分离（Root 不在 `users` 表）
- `admin_scope` 自动权限（新权限自动可见）
- group 不能获得 `*:*`（API + DB CHECK）
- OpenUDS 信封解析 + 重连/退避（mock transport）
- 错误契约形状（request_id 存在、code 正确）
- JWT claims 往返
- config 解析（`example.toml`）/ 校验规则 / 默认值

本地跑测试（工具链可用时）：

```bash
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo check --locked --workspace --all-targets --all-features
```

> [!NOTE]
> sqlx 使用运行时查询（无编译期 `query!` 宏），因此编译不需要数据库连接。
