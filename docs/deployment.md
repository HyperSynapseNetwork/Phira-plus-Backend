# 部署与运维（Deployment & Operations）

> 本页汇总 PPB 的部署路径、反代、更新与运维操作。模板文件位于 [`deploy/`](../deploy/)。

## 部署拓扑

```
Browser → TLS 反代 (nginx/caddy) ─→ api-phira.htadiy.com → ppb-server:8080
                                  └→ phira.htadiy.com       → PPF 静态站
                                  └→ panel-phira.htadiy.com → Panel SPA
ppb-server ── OpenUDS (Unix socket) ──> PMP (phira-mp-plus)
ppb-server ── PostgreSQL 16+ (独立库，sqlx 迁移)
```

## 方式 A：一键安装（推荐）

脚本：[`deploy/install.sh`](../deploy/install.sh)

```bash
# 在线安装（root；默认取最新 release）
curl -fsSL https://raw.githubusercontent.com/HyperSynapseNetwork/Phira-plus-Backend/main/deploy/install.sh | sudo bash

# 指定版本 / 非交互：
sudo PPB_VERSION=0.1.0 PPB_NONINTERACTIVE=1 \
  PPB_API_URL=https://api.example.com \
  PPB_DATABASE_URL=postgres://... bash deploy/install.sh
```

流程：前置检查（curl/sha256sum/systemctl/useradd/PostgreSQL 16+，仅 Linux x86_64）→ 下载
`ppb-server` / `ppctl` / `SHA256SUMS` 并校验 sha256 → 安装到 `/usr/local/bin`（0755）→ 创建
`ppb` 系统用户与 `/opt/ppb`、`/etc/ppb`、`/var/run/pmp` → `ppctl init --non-interactive` 生成
`ppb.toml` + `ppb.env`（secrets 自动生成）→ 追加 `PPB_RUNTIME_CONFIG=/etc/ppb/ppb.toml` →
数据库二选一（本地 PostgreSQL 自动建 `ppb` role+库，口令 `openssl rand` 生成；或外部
`PPB_DATABASE_URL`）→ 安装 systemd unit + `enable --now` → 轮询 `/healthz` 直到
`{"status":"ok"}`。

要点：

- **幂等**：重装保留既有 `/etc/ppb/ppb.toml` + `ppb.env`（不换密钥），只更新二进制与 unit。
- **secrets 自动生成**：脚本绝不接受密码入参（与 `ppctl init` 哲学一致）。
- 交互模式逐项提示 URL（回车用 `config/example.toml` 默认值）；非交互用 `PPB_NONINTERACTIVE=1`
  + `PPB_*` 环境变量覆盖。

## 方式 B：Docker Compose

模板：[`deploy/docker-compose.yml`](../deploy/docker-compose.yml)

```bash
cp deploy/docker-compose.yml docker-compose.yml
export PPB_DB_PASSWORD=... PPB_JWT_SECRET=... PPB_PHIRA_CREDENTIAL_KEY=...
export PPB_IMAGE=ghcr.io/your-org/ppb-server:0.1.x
# PMP OpenUDS socket 目录（PMP 同机运行）
export PMP_OPENUDS_SOCKET_DIR=/var/run/pmp
docker compose up -d
```

要点：

- 容器内 PostgreSQL 由 compose 自带（`postgres:16-alpine`，数据在 `pgdata` volume）；也可指向外部库（`PPB_DATABASE_URL`）。
- `config/` 目录只读挂载到 `/etc/ppb`（`PPB_RUNTIME_CONFIG=/etc/ppb/ppb.toml`）。
- PMP OpenUDS socket 从宿主机只读挂载到 `/var/run/pmp`（PMP 需与 PPB 同机运行）。
- 健康检查：`/healthz`。

## 方式 C：Native Linux + systemd（手动）

模板：[`deploy/systemd/ppb.service`](../deploy/systemd/ppb.service) · [`deploy/systemd/ppb.env.example`](../deploy/systemd/ppb.env.example)

```bash
sudo install -d -o ppb -g ppb /opt/ppb /etc/ppb /var/run/pmp
sudo install -m 0644 config/example.toml /etc/ppb/ppb.toml
sudo install -m 0600 deploy/systemd/ppb.env.example /etc/ppb/ppb.env   # 填入真实值
sudo cp target/x86_64-unknown-linux-musl/release/ppb-server /usr/local/bin/ppb-server
sudo cp deploy/systemd/ppb.service /etc/systemd/system/ppb.service
sudo systemctl daemon-reload && sudo systemctl enable --now ppb
```

要点：

- unit 以 `ppb` 用户运行，`ProtectSystem=strict` + `ReadWritePaths=/var/run/pmp`。
- 环境变量经 `EnvironmentFile=/etc/ppb/ppb.env` 注入，勿把密码写进 unit。
- **配置路径**：unit 的 `ExecStart` 带 `--config /etc/ppb/ppb.toml`，但当前二进制解析的是 `PPB_RUNTIME_CONFIG` 环境变量 / `config/ppb.toml`（见 [getting-started.md](./getting-started.md) 的 NOTE）。请在 `ppb.env` 中显式设置 `PPB_RUNTIME_CONFIG=/etc/ppb/ppb.toml`，否则会回退到内置默认值。

## 反向代理

### Nginx

模板：[`deploy/nginx/nginx.conf`](../deploy/nginx/nginx.conf)

- `api-phira.htadiy.com` → `127.0.0.1:8080`（TLS 终止在 Nginx）。
- WebSocket `/ws/v1/...` 必须配置 `Upgrade` / `Connection` 头（模板已含）。
- 同时给出 PPF 静态站（`/srv/ppf/dist`）与 Panel SPA（`/srv/panel/dist`，history fallback）的 server 块。

### Caddy

模板：[`deploy/caddy/Caddyfile`](../deploy/caddy/Caddyfile)

## 更新（升级）

### Docker

切换镜像 tag 后 `docker compose up -d`；无半部署中间态（image 标签原子切换）。

### Native

脚本：[`deploy/update.sh`](../deploy/update.sh)

```bash
PPB_VERSION=0.1.x ./deploy/update.sh
```

流程：`[1/5]` 下载/解压到 staging → `[2/5]` `ppb-server --version` + `--check-config /etc/ppb/ppb.toml`（失败即中止，旧版本保持运行）→ `[3/5]` 停旧服务 → `[4/5]` symlink 原子激活 → `[5/5]` 启动 + `/healthz` 健康检查，失败提示回滚。

`ppctl update [--check-config PATH]` 会校验暂存配置并打印同一套标准流程。

## 运维与故障排查

```bash
ppctl doctor --report out.tar.gz   # 脱敏 support bundle
                                   # 检查项：config schema / config/ / PostgreSQL / OpenUDS socket /
                                   #         pmp capabilities / push (VAPID) / github oauth / public urls
journalctl -u ppb -f               # systemd 服务日志
curl http://127.0.0.1:8080/healthz # 健康检查 → {"status":"ok"}
```

- **Root 口令重置**：`ppctl root reset-password`。
- **审计留存**：`audit.retention_days`（默认 90），服务启动时 + 每日自动清理。

## 版本与发布（CI）

| 流程 | 触发 | 说明 |
|---|---|---|
| [Build](../.github/workflows/build.yml) | push/PR 到 main | `auto-patch`（patch+1 + `cargo generate-lockfile`）→ `check`（`cargo check --locked` + `cargo test` + `cargo clippy -D warnings`）→ `build`（linux-musl x86_64 + aarch64） |
| [Bump Version](../.github/workflows/bump-version.yml) | 手动 workflow_dispatch | minor/major 升级 + 打 tag `vA.B.C` |
| [Release](../.github/workflows/release.yml) | `v*` tag | quality-gate → 构建矩阵（x86_64 / aarch64 musl）→ smoke test → provenance（attest-build-provenance）+ CycloneDX SBOM（best-effort）→ GitHub Release（`ppb-server` / `ppctl` + `SHA256SUMS`） |

> [!NOTE]
> Release 产物可验证：`attest-build-provenance` 生成构建出处，`SHA256SUMS` 提供校验和。Rust 版本由 `rust-toolchain.toml` 固定（1.96.0）。
