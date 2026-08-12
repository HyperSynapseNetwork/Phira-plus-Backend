# 配置参考（Configuration）

> 来源：`crates/ppb-server/src/config/mod.rs` 的 `RuntimeConfig` + [config/example.toml](../config/example.toml)。
> 分层（design §20.1 / §25.7）：
> - **Deployment / Secret**（环境变量 / secret file，Panel 不返回原值）；
> - **PPB Runtime**（TOML，Panel 可改）；
> - **PMP Config**（PPB Form Descriptor 维护）；
> - **PPF Build/SEO 与 Public Content**（DB，Panel 可改）。
>
> 标注 `[secret]` 的字段**绝不**写入日志 / 审计 / API 响应。

## 加载顺序

1. `PPB_RUNTIME_CONFIG` 环境变量指定的 TOML；
2. `./config/ppb.toml`；
3. 内置默认值。

DB 覆盖（`ppb_runtime_overrides`，存于 PostgreSQL）合并到启动 TOML 之上；未知键忽略。

## `[server]`

| 键 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `listen_addr` | SocketAddr | `0.0.0.0:8080` | 监听地址 |
| `public_url` | string | `https://api-phira.htadiy.com` | PPB 对外 URL |
| `graceful_shutdown_secs` | int | `15` | 优雅关闭秒数 |

## `[site]`

| 键 | 默认 | 说明 |
|---|---|---|
| `ppf_url` | `https://phira.htadiy.com` | PPF 站点 |
| `panel_url` | `https://panel-phira.htadiy.com` | Panel 站点 |
| `docs_url` | `https://docs.phira.htadiy.com` | Docs 站点 |
| `visit_count` | `0` | 隐私友好聚合访问数基线（P-86）；服务端计数叠加，缺失为 0 |

## `[cors]`

| 键 | 默认 | 说明 |
|---|---|---|
| `credentials` | `true` | 凭据 CORS；为 `true` 时禁止 `*` |
| `allowed_origins` | `[phira, panel]` | 精确来源白名单 |
| `dev_origins` | `[localhost:3000, localhost:5173]` | 本地开发来源 |

## `[session]`

| 键 | 默认 | 说明 |
|---|---|---|
| `access_ttl_secs` | `3600` | access JWT 有效期 |
| `refresh_ttl_secs` | `2592000`（30d） | refresh 有效期 |
| `cookie_domain` | `api-phira.htadiy.com` | host-only cookie 域 |
| `cookie_secure` | `true` | Secure cookie |
| `cookie_samesite` | `lax` | lax\|strict\|none |
| `csrf_cookie_name` | `ppb_csrf` | CSRF cookie |
| `csrf_header_name` | `X-CSRF-Token` | CSRF header |
| `reauth_ttl_secs` | `300`（5min） | reauth context JWT 有效期 |

## `[pmp]`

| 键 | 默认 | 说明 |
|---|---|---|
| `openuds_path` | `/var/run/pmp-openuds.sock` | OpenUDS Unix socket |
| `auth_mode` | `approve` | token\|approve |
| `client_name` | `ppb-server` | approve 模式客户端名 |
| `reconnect_base_ms` / `reconnect_max_ms` | `500` / `30000` | 重连退避 |
| `request_timeout_ms` | `10000` | OpenUDS 命令超时 |
| `capabilities` | `[persist.touches, persist.judges, room.chat_send, stream.touches, stream.judges]` | 能力集（与版本映射交集） |
| `config_path` | 无 | PMP `server_config.yml` 路径（Form Descriptor / 快照） |
| `http_url` | 无 | PMP HTTP 健康地址（如 `http://127.0.0.1:12347`） |

## `[phira]`

| 键 | 默认 | 说明 |
|---|---|---|
| `base_url` | `https://phira.5wyxi.com` | Phira API |
| `timeout_ms` | `15000` | HTTP 超时 |
| `access_token_ttl_secs` | `21600`（6h） | Phira access token 内存有效期 |
| `gateway_ttl_secs` | `120` | 数据网关缓存 TTL |
| `gateway_rate_per_minute` | `60` | 数据网关速率 |
| `aggregator_enabled` | `true` | TopChart 聚合 worker |
| `aggregator_interval_hours` | `1` | 快照间隔 |
| `aggregator_top_n` | `50` | 热门谱面 N |

## `[rate_limit]`

| 键 | 默认 |
|---|---|
| `login_per_minute` | `10` |
| `reauth_per_minute` | `10` |
| `github_callback_per_minute` | `20` |
| `chat_send_per_minute` | `60` |
| `raw_cli_per_minute` | `30` |

## `[audit]`

`retention_days=90`（自动清理任务每日运行）。

## `[notifications]`

| 键 | 默认 | 说明 |
|---|---|---|
| `default_chat_channel` | `only_when_companion_background` | chat 通知档位 |
| `vapid_private_key_pem` | 无 `[secret]` | Web Push VAPID P-256 PEM；空则 WebPush 报告 not_configured |
| `vapid_subject` | 无 | VAPID subject（如 `mailto:admin@...`） |

## `[metrics]`

`retention_days=30`。

## `[security]`

`return_to_allowlist`：OAuth return_to 白名单（防 open redirect），默认 `[phira, panel]`。

## `[github]`

`callback_url`：固定 `https://api-phira.htadiy.com/api/v1/auth/github/callback`。

## 环境变量（Deployment/Secret）

> 这些是密钥唯一允许存在的位置（环境变量或 secret file）。参考 [config/example.env](../config/example.env) 与 [deploy/systemd/ppb.env.example](../deploy/systemd/ppb.env.example)。

| 变量 | 说明 |
|---|---|
| `PPB_DATABASE_URL` | PostgreSQL 连接串 `[secret]` |
| `PPB_JWT_SECRET` | JWT 签名密钥（≥32 字节）`[secret]` |
| `PPB_PHIRA_CREDENTIAL_KEY` | Phira refresh token 加密密钥（32 字节）`[secret]` |
| `PPB_PMP_OPENUDS_TOKEN` | OpenUDS token 认证令牌 `[secret]` |
| `PPB_GITHUB_CLIENT_ID` / `PPB_GITHUB_CLIENT_SECRET` | GitHub OAuth `[secret]` |
| `PPB_RUNTIME_CONFIG` | 运行时 TOML 路径 |
| `PPB_VAPID_PUBLIC_KEY` / `PPB_VAPID_PRIVATE_KEY` | Web Push VAPID `[secret]`（Phase D；配置侧对应 `PPB_VAPID_PRIVATE_KEY_PEM` / `PPB_VAPID_SUBJECT`） |
| `PPB_FCM_SERVICE_ACCOUNT_JSON` / `PPB_WNS_PACKAGE_SID` / `PPB_WNS_CLIENT_SECRET` | Android / Windows 推送 `[secret]`（Phase D，待 Owner 凭据） |

> [!NOTE]
> `example.env` 与 `CONFIG_REFERENCE.md` 在 VAPID 变量名上存在历史命名差异（`PPB_VAPID_PRIVATE_KEY` vs `PPB_VAPID_PRIVATE_KEY_PEM`）。以 `config/mod.rs` 的 `NotificationConfig` 为准：配置侧键为 `vapid_private_key_pem` / `vapid_subject`。
