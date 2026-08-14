# Phira+ V3 Golden Deployment

此目录是 **PPB + PPF + Panel + PostgreSQL + TLS reverse proxy** 的 clean-source 部署入口。PMP 仍是 multiplayer runtime 与 room/round/chat/touch/judge 的事实源，需在宿主机先运行并开放 OpenUDS。

`site.docs_url` 指向独立文档站；Golden Compose 不内置 docs 服务。没有独立 docs 站时必须改成实际可用地址，不能让公开导航指向不存在的域名。

## 1. 前置条件

- Linux + Docker Engine / Docker Compose v2。
- PMP 已启动，且可修改 `server_config.yml`。
- 三个 DNS 名指向本机：PPF、PPB API、Panel。
- 80/443 TCP（使用 HTTP/3 时另需 443 UDP）可入站。
- 从本目录执行 compose；build context 会引用同一 bundle 中的 PPB/PPF/Panel 源码。

## 2. PMP OpenUDS 最小配置

PMP `server_config.yml`：

```yaml
openuds:
  enabled: true
  socket_path: "/var/run/pmp-openuds.sock"
  auth_token: ""
  max_connections: 4
  event_buffer_size: 1024
  heartbeat_interval_secs: 60
```

认证方式二选一：

1. **Direct / 文件权限模式**：PMP `auth_token: ""`，`.env` 中 `PPB_PMP_OPENUDS_TOKEN=` 也保持空；依赖 Unix socket 文件权限隔离。
2. **Token 模式**：PMP `auth_token` 使用高熵随机值，`.env` 的 `PPB_PMP_OPENUDS_TOKEN` 必须完全一致。

PMP 重启后先验证源路径真的是 socket 文件：

```bash
sudo test -S /var/run/pmp-openuds.sock
sudo ls -la /var/run/pmp-openuds.sock
sudo stat -c 'type=%F mode=%a owner=%U group=%G path=%n' /var/run/pmp-openuds.sock
```

`test -S` 失败时不要启动 PPB。Docker 在 source 不存在时可能留下错误类型的 bind source；应先停止 compose、移除误建目录、让 PMP 创建真实 socket 后再启动。

## 3. 生成部署配置与 Secret

```bash
cd repos/ppb/Phira-plus-Backend-main/deploy
cp .env.example .env
cp ppb.toml.example ppb.toml
```

生成生产 Secret：

```bash
# PostgreSQL 密码：hex 可直接放 DATABASE_URL
openssl rand -hex 24

# JWT：至少 32 bytes
openssl rand -base64 32

# Phira refresh-token AES-256-GCM key：必须正好解码为 32 bytes
openssl rand -base64 32

# OpenUDS token 模式可另生成
openssl rand -hex 32
```

编辑 `.env`，替换所有 `CHANGE_ME`、三个域名、邮箱及必要凭据。编辑 `ppb.toml`，同步：

- `server.public_url`
- `site.ppf_url`
- `site.panel_url`
- `site.docs_url`
- `cors.allowed_origins`
- `security.return_to_allowlist`
- GitHub callback（启用 GitHub OAuth 时）

检查没有遗留示例值：

```bash
! grep -R 'CHANGE_ME\|example\.com' .env ppb.toml
```

Secret 只通过环境/secret manager 注入，不写入源码、审计日志或 Panel 响应。

## 4. 验证 Compose 并 no-cache 构建

先展开配置：

```bash
docker compose --env-file .env -f docker-compose.yml config >/tmp/phiraplus-compose.yml
```

Golden Deployment 的构建证据必须来自 clean source：

```bash
docker compose --env-file .env -f docker-compose.yml \
  build --no-cache ppb ppf panel
```

这会实际覆盖：

- PPB Rust workspace + migrations；
- PPF Viewer Rust path dependencies + wasm-pack + Nuxt static build；
- Panel Nuxt static build。

任一镜像失败就停止，不允许用宿主机旧 artifact 补齐缺失文件后继续宣称 clean build 通过。

## 5. 启动

```bash
docker compose --env-file .env -f docker-compose.yml up -d
docker compose --env-file .env -f docker-compose.yml ps
```

预期：PostgreSQL、PPB healthy；PPF、Panel、proxy running。

PPF/Panel 的公开 API/站点 URL 在 **image build time** 注入；域名变化后必须重建对应镜像。

## 6. `ppctl doctor`

```bash
docker compose --env-file .env -f docker-compose.yml \
  exec ppb ppctl doctor --config /etc/ppb/ppb.toml --report /tmp/ppb-doctor.json
```

核心期望项：

```text
[OK ] config schema
[OK ] runtime config file
[OK ] postgresql
[OK ] openuds handshake
[OK ] pmp capabilities
```

`openuds handshake` 现在会进行真实 authenticate，而不是只测试“能 connect socket”。常见失败：

- `missing`：宿主机路径错、PMP 未启用 OpenUDS、compose 启动前 socket 不存在；
- `connect: Permission denied`：检查 socket/父目录权限以及容器 bind；
- authentication failed：PMP token 与 `PPB_PMP_OPENUDS_TOKEN` 不一致，或 Direct/Token 模式配错；
- unexpected response：检查 PMP/PPB 协议版本与 PMP 日志。

不要把 token 输出到 support bundle 或工单。

## 7. Root 首次登录与普通管理员

空数据库首次启动会自动迁移并生成一次性 Root 口令，写到 PPB 容器日志一次：

```bash
docker compose --env-file .env -f docker-compose.yml logs ppb \
  | grep 'Root first-boot password'
```

若数据库已初始化且原口令不可得：

```bash
docker compose --env-file .env -f docker-compose.yml \
  exec ppb ppctl root reset-password
```

随后：

1. 打开 `https://<PANEL_DOMAIN>/login` 以 Root 登录。
2. 若进入强制改密流程，必须完成，不能跳过。
3. 创建/调整普通管理员组，只授予工作所需权限；Root 的全局能力不应作为日常账户权限模板。
4. 让已有 Phira 身份的 PPB 用户加入该组。
5. 退出 Root，以普通管理员重新登录并验证 Users / Rooms / Logs / Config 等授权页面。

## 8. OpenUDS bind-mount 排查

宿主机：

```bash
SOCK=$(grep '^PMP_OPENUDS_SOCKET=' .env | cut -d= -f2-)
printf 'socket=%s\n' "$SOCK"
sudo test -S "$SOCK"
sudo ls -la "$SOCK"
sudo stat "$SOCK"
```

确认 compose 展开的 source/target：

```bash
docker compose --env-file .env -f docker-compose.yml config \
  | sed -n '/ppb:/,/^[^ ]/p'
```

容器内：

```bash
docker compose --env-file .env -f docker-compose.yml \
  exec ppb sh -lc 'ls -la /var/run/pmp-openuds.sock; test -S /var/run/pmp-openuds.sock'
```

只挂载 OpenUDS socket，不把整个 `/var/run`、PMP 数据目录或数据库目录交给 PPB。

## 9. 受控 Deployment Adapter

Panel 的 `server.start / supervisor stop / ppf.build / backup` 只有在部署者显式配置 capability 时显示/执行。配置形态是**固定 JSON argv**，不是 shell：

```dotenv
PPB_PMP_SUPERVISOR_START_JSON=["/usr/local/bin/phiraplus-pmp-start"]
PPB_PMP_SUPERVISOR_STOP_JSON=["/usr/local/bin/phiraplus-pmp-stop"]
PPB_PMP_SUPERVISOR_ARG_SCHEMA_JSON=[{"key":"port","flag":"--port","kind":"integer","min":1,"max":65535}]
PPB_PPF_BUILD_COMMAND_JSON=["/usr/local/bin/phiraplus-ppf-build"]
PPB_BACKUP_COMMAND_JSON=["/usr/local/bin/phiraplus-backup"]
```

适配器不经过 shell，启动参数只接受结构化 allowlist；child process 默认 `env_clear()`，额外 allowlist 也禁止透传 `PPB_*`。未配置 capability 时 API 明确返回不支持，不能生成假成功任务。

> 默认 Golden Compose 的 PPB 容器没有挂载宿主机部署脚本，因此这些 adapter 默认关闭。若需要启用，应使用经过审计的专用 runner/sidecar 或受控可执行文件挂载，不要把 Docker socket 或任意 shell 暴露给 PPB。

## 10. 最终 URL 与动态路由 smoke

```bash
curl -fsS "https://${API_DOMAIN}/healthz"
```

浏览器直接打开：

- `https://<PPF_DOMAIN>/`
- `https://<PANEL_DOMAIN>/login`
- `https://<API_DOMAIN>/healthz`

然后必须通过地址栏**直接打开并刷新**真实对象 URL：

```text
/room/<real-room-id>
/chart/<real-chart-id>
/user/<real-phira-id>
/replay/share/<real-valid-share-token>
```

PPF Nginx 已使用 `/index.html` SPA fallback；但只有真实浏览器 direct-route E2E 运行后才能把该项升级为 CI/Integration verified。

## 11. 最低 Golden Journey 签字条件

同一 Git commit / file manifest 至少留下以下证据：

- no-cache `ppb/ppf/panel` image build；
- PostgreSQL 16 空库 migration + API health；
- OpenUDS authenticate + capabilities；
- Root 首登强改密 + 普通管理员登录；
- PPF/Panel/API 三个公开 URL；
- PPF 动态实体 URL direct open + refresh；
- 普通玩家关键旅程 + 管理员关键旅程 Browser E2E；
- Rust check/test/clippy、前端 clean build、Error Contract/i18n/PPNotice gates；
- 对应 CI artifact、commit/status。

源码/静态检查只允许标 `SOURCE_IMPLEMENTED` / `STATIC_VALIDATED`；缺上述实际运行证据时不得写 `CI_VERIFIED`、`INTEGRATION_VERIFIED` 或 `PRODUCTION_VERIFIED`。

### Versioned legal acceptance

Public Phira/GitHub account sign-in is intentionally disabled by default. Enable `[legal].public_auth_enabled` only after Owner-approved Terms and Privacy documents have stable versions and configured HTTPS (or same-product relative) URLs. The Auth Gateway requires an explicit checkbox for the configured versions and PPB persists the acceptance separately from analytics/cookie consent. Root authentication remains an independent local principal and does not use this public-account consent flow.
