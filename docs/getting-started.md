# 快速开始（Getting Started）

> PPB = Phira+ Backend（身份 / 社区 / 控制 / 集成平面）。本指南覆盖 Docker 与 Native Linux 两条官方路径（design §25.1）。

## 前置条件

- **PostgreSQL 16+**：PPB 不直连 PMP PostgreSQL，使用自己的库；首次启动自动执行 sqlx 迁移建表。
- **PMP（phira-mp-plus）已运行**：OpenUDS Unix socket 可用（默认 `/var/run/pmp-openuds.sock`，可通过配置 `pmp.openuds_path` 修改）。
- **三个固定域名**（或自部署时的替换域名，见 `config/example.toml` 的 `[site]`）：
  - `api-phira.htadiy.com`（PPB）
  - `phira.htadiy.com`（PPF，公开伴生站）
  - `panel-phira.htadiy.com`（Panel，管理控制台）

在 PMP `server_config.yml` 中启用 OpenUDS，并确保 socket 路径与 PPB `[pmp].openuds_path` 完全一致：

```yaml
openuds:
  enabled: true
  socket_path: "/var/run/pmp-openuds.sock"
  auth_token: ""
  max_connections: 4
  event_buffer_size: 1024
  heartbeat_interval_secs: 60
```

启动 PPB 前运行 `ppctl doctor --config /etc/ppb/ppb.toml`；它会检查配置、socket 存在性和连接权限。

## 方式 A：一键安装（推荐）

[`deploy/install.sh`](../deploy/install.sh) 是 Native Linux x86_64 的首选安装路径：幂等、自动下载并
校验 sha256、自动生成配置与密钥（`ppctl init`）、自动配置本地 PostgreSQL（或使用外部
`PPB_DATABASE_URL`）、安装 systemd 服务并轮询 `/healthz`。

```bash
# 在线安装（root；默认取最新 release，可用 PPB_VERSION 覆盖）
curl -fsSL https://raw.githubusercontent.com/HyperSynapseNetwork/Phira-plus-Backend/main/deploy/install.sh | sudo bash
```

非交互部署（环境变量覆盖；secrets 仍自动生成）：

```bash
sudo PPB_NONINTERACTIVE=1 \
  PPB_API_URL=https://api.example.com \
  PPB_PPF_URL=https://phira.example.com \
  PPB_PANEL_URL=https://panel.example.com \
  PPB_DATABASE_URL=postgres://ppb:CHANGE_ME@127.0.0.1:5432/ppb \
  bash deploy/install.sh
```

> [!WARNING]
> 需要传入环境变量时不要用 `PPB_DATABASE_URL=... curl ... | sudo bash`：`sudo` 默认
> `env_reset`，变量只传给 `curl`，不会传给 `sudo bash`，脚本会报「未提供 PPB_DATABASE_URL」。
> 改用下列任一种：
>
> ```bash
> # 1) sudo -E 保留当前环境，再在 root 下执行管道
> PPB_DATABASE_URL=postgres://... sudo -E bash -c \
>   'curl -fsSL https://raw.githubusercontent.com/HyperSynapseNetwork/Phira-plus-Backend/main/deploy/install.sh | bash'
>
> # 2) 先落盘，再 sudo env 显式传入
> curl -fsSL https://raw.githubusercontent.com/HyperSynapseNetwork/Phira-plus-Backend/main/deploy/install.sh -o /tmp/install.sh
> sudo env PPB_DATABASE_URL=postgres://... PPB_NONINTERACTIVE=1 bash /tmp/install.sh
> ```

脚本结尾会打印首次 Root 一次性口令查看方式、反代模板位置与 `ppctl root reset-password` 用法。

## 方式 B：Docker Compose

```bash
cp deploy/docker-compose.yml docker-compose.yml
# 填入强口令（自动生成见 `ppctl init`）：
export PPB_DB_PASSWORD=... PPB_JWT_SECRET=... PPB_PHIRA_CREDENTIAL_KEY=...
export PPB_IMAGE=ghcr.io/your-org/ppb-server:0.1.x
export PPF_IMAGE=ghcr.io/your-org/phira-plus-frontend:0.1.x
export PANEL_IMAGE=ghcr.io/your-org/phira-plus-panel:0.1.x
export PMP_OPENUDS_SOCKET=/var/run/pmp-openuds.sock
export API_DOMAIN=api.example.com PPF_DOMAIN=phira.example.com PANEL_DOMAIN=panel.example.com
export ACME_EMAIL=admin@example.com
docker compose up -d
```

健康检查：`curl http://127.0.0.1:8080/healthz` → `{"status":"ok"}`。

反向代理：将 `deploy/nginx/nginx.conf` 或 `deploy/caddy/Caddyfile` 的域名/证书替换后启用；WebSocket `/ws/v1/...` 已配置。

## 方式 C：Native Linux + systemd（手动）

```bash
# 1) 准备目录与配置（root 运行，无需 ppb 用户）
sudo install -d /opt/ppb /etc/ppb /var/run/pmp
sudo install -m 0644 config/example.toml /etc/ppb/ppb.toml
# 编辑 /etc/ppb/ppb.toml：listen_addr、public_url、pmp.openuds_path、phira.base_url 等

# 2) 环境变量（密钥；ppctl init 生成）
sudo install -m 0600 deploy/systemd/ppb.env.example /etc/ppb/ppb.env
sudo vi /etc/ppb/ppb.env   # 填入真实值

# 3) 安装二进制与 unit
sudo cp target/x86_64-unknown-linux-musl/release/ppb-server /usr/local/bin/ppb-server
sudo cp deploy/systemd/ppb.service /etc/systemd/system/ppb.service
sudo systemctl daemon-reload
sudo systemctl enable --now ppb

# 4) PMP OpenUDS 挂载：确保 /var/run/pmp 下能看到 PMP 的 socket
```

> [!NOTE]
> 配置加载顺序（`crates/ppb-server/src/config/mod.rs`）：`PPB_RUNTIME_CONFIG` 环境变量指定的 TOML → `./config/ppb.toml` → 内置默认值。systemd 部署时请用 `PPB_RUNTIME_CONFIG=/etc/ppb/ppb.toml` 显式指定配置文件（当前二进制未解析 `--config` 参数；`--check-config <path>` 仅用于配置校验）。

## 首次启动：Root-only Bootstrap 与 Public Auth Enable

Fresh install 默认 `legal.public_auth_enabled=false`。先完成 Root-only bootstrap，再启用普通账户身份，不要把两步倒置。

### 1. Root-only Bootstrap

PPB 首次启动会生成一次性 Root 口令并输出一次。使用 Panel 登录后立即改密，确认 PostgreSQL / OpenUDS / health/doctor 正常，并先创建所需管理员组。此阶段 Auth Gateway 的普通 Phira/GitHub 登录保持禁用。

```bash
ppctl root reset-password --env-file /etc/ppb/ppb.env   # 仅在需要重置时
```

> `ppb-server root init` 也是 CLI 路径；两者都要求 `PPB_DATABASE_URL` 已设置。

### 2. Enable Normal Identity

Owner-approved Terms / Privacy 有稳定版本和安全 URL 后，在 `ppb.toml` 配置 `[legal]`：

```toml
[legal]
public_auth_enabled = true
terms_version = "<approved-version>"
privacy_version = "<approved-version>"
terms_url = "https://phira.example.com/terms"
privacy_url = "https://phira.example.com/privacy"
```

校验并 reload/restart 后，完成第一个 Phira 登录，再由 Root 把该 PPB 用户加入普通管理员组；最后退出 Root，用普通管理员重新验证授权页面。已接受当前 version pair 的用户后续登录不重复要求同意，只有版本变化才重新要求。

## 更新

- **Docker**：切换镜像 tag，`docker compose up -d`。
- **Native**：`PPB_VERSION=<v> ./deploy/update.sh`（stage → 校验 → 原子激活，禁止半部署中间态）。

## 故障排查

```bash
ppctl doctor --report out.tar.gz   # 脱敏 support bundle（config/PostgreSQL/OpenUDS/能力集/URL 可达性）
journalctl -u ppb -f               # 服务日志
```

## 参考

- 配置项：[configuration.md](./configuration.md)
- PMP 集成：[pmp-integration.md](./pmp-integration.md)
- 部署/运维：[deployment.md](./deployment.md)
- 架构/设计：`DESIGN/PP-B-F-P_V3_总体设计规范.md`
