<div align="center">

# Phira+ Backend（PPB）

**Phira+（Phira+ V3）三件套之一** · Rust / axum / Tokio / PostgreSQL(sqlx) / OpenUDS 集成 —— 统一后端（身份 / 社区 / 控制 / 集成平面）

<br/>

[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021-dea584.svg?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Build](https://github.com/HyperSynapseNetwork/Phira-plus-Backend/actions/workflows/build.yml/badge.svg)](https://github.com/HyperSynapseNetwork/Phira-plus-Backend/actions/workflows/build.yml)
[![Axum](https://img.shields.io/badge/Axum-HTTP%2FSSE%2FWS-0b0b0b.svg?logo=rust&logoColor=white)](https://github.com/tokio-rs/axum)
[![PostgreSQL](https://img.shields.io/badge/PostgreSQL-数据库-336791.svg?logo=postgresql&logoColor=white)](https://www.postgresql.org/)
[![Docker](https://img.shields.io/badge/Docker-Ready-2496ED.svg?logo=docker&logoColor=white)](https://www.docker.com/)

</div>

> [!IMPORTANT]
> **Phira+ 三件套之一**：`ppb`（本仓库，后端）· `ppf`（Phira-plus-frontend，公开伴生站）· `panel`（Phira-plus-panel，管理控制台）。
> **跨仓冻结契约以三仓工作区的 `contracts/README.md`（Contract-Freeze v0）与本仓 [OpenAPI](contracts/openapi.json) 为准** —— 先改契约，再实现；禁止三边猜字段。
> 本仓库采用 **Apache License, Version 2.0**，详见 [LICENSE](LICENSE)。

> [!TIP]
> 第一次来？直接看[快速开始](#快速开始)。部署者手册见 [docs/getting-started.md](docs/getting-started.md)。

## 简介

**PPB（Phira+ Backend）** 是 Phira+ 的统一后端，负责**身份（identity）、策略（policy）、控制（control）、社区（community）**四类数据。一句话数据所有权：**PMP 是多人游戏真相源，Phira API 是 Phira 世界数据源，PPB 只拥有身份/策略/控制/偏好/通知/审计** —— PPB 绝不直连 PMP PostgreSQL，绝不存储 Replay 内容，Phira 密码永不落盘、Phira token 永不进入前端。

### 核心特性

- **统一认证（Auth）**：Phira 登录 → PPB JWT（`Secure+HttpOnly+SameSite=Lax` host-only cookie）→ Root 本地凭据（bcrypt，独立于 `users` 表）→ GitHub **绑定** OAuth（不建裸账号）→ 短期 `reauth_context` JWT（`X-Reauth-Token` 头，高危险 Action 二次鉴权）
- **权限系统**：Permission Manifest（`/api/v1/admin/permissions/manifest`）+ User → Groups → Permissions 三级解析；`admin_scope` 自动映射全部非 Root-only 权限；`*:*` 仅 Root（API + DB CHECK 双重拒绝）
- **Action Registry + Command Broker**：统一执行模型（`openuds | cli.execute | internal`），按 `queue_key` 串行执行，`command_runs` 全程记录；`host_allowed` 动作每次执行重查真实 host
- **OpenUDS 集成**：token / direct 双模式认证（无 token 时按 socket 文件权限直接放行），typed 命令封装（room/player/server/plugin），断线自动重连（指数退避 + 抖动），版本映射 + capability detection
- **实时通道**：PMP 事件 → PPB SSE 信封（`GET /api/v1/events`）；Live WS（`/ws/v1/rooms/{room_id}/live`）与 Replay WS（`/ws/v1/replays/{round_uuid}`）JSON 信封
- **Phira 数据网关**：已确认公开数据子集（charts/records/users，typed 方法）TTL 缓存 + 速率限制；TopChart 聚合 worker（每小时快照）
- **统一错误契约**：`{"error":{"code","message","request_id","details"}}`，code 全 UPPER_SNAKE_CASE；分页统一 `page`（1-based）/`pageNum`（≤100）

## 文档

| 分类 | 文档 |
|------|------|
| **快速开始** | [docs/getting-started.md](docs/getting-started.md)（一键安装 / Docker / Native 手动三条路径） |
| **配置** | [docs/configuration.md](docs/configuration.md)（TOML + `PPB_*` 环境变量表） |
| **部署与运维** | [docs/deployment.md](docs/deployment.md)（Docker / Native + systemd / 反代模板 / 更新 / 故障排查） |
| **PMP 集成** | [docs/pmp-integration.md](docs/pmp-integration.md)（OpenUDS 命令 / 事件 / 高频流 / 能力集） |
| **对外 API** | [docs/api.md](docs/api.md)（当前接口导航；完整 REST 以已提交 OpenAPI 为准，Router/实时通道另有可执行 Surface Contract） |
| **开发** | [docs/development.md](docs/development.md)（架构 / 模块布局 / 测试 / 数据模型） |
| **历史计划** | [docs/history/PHASE_A_PLAN.md](docs/history/PHASE_A_PLAN.md)（Phase A 实施计划存档） |

## 技术栈

| 技术 | 用途 |
|------|------|
| [Rust](https://www.rust-lang.org/) | 主开发语言（2021 Edition，`rust-toolchain.toml` 固定 1.96.0） |
| [Axum](https://github.com/tokio-rs/axum) `0.8` | HTTP / SSE / WebSocket 路由 |
| [Tokio](https://tokio.rs/) `1` | 异步运行时 |
| [sqlx](https://github.com/launchbadge/sqlx) `0.8` | PostgreSQL 访问 + 迁移（`sqlx::migrate!`，运行时查询，无编译期 `query!`） |
| [jsonwebtoken](https://docs.rs/jsonwebtoken/) `9` | JWT 签发 / 校验（HS256） |
| [aes-gcm](https://docs.rs/aes-gcm/) `0.10` | Phira refresh token 静态加密（AES-256-GCM） |
| [bcrypt](https://docs.rs/bcrypt/) `0.15` | Root 口令哈希 |
| [reqwest](https://docs.rs/reqwest/) `0.12` | Phira API / OpenUDS HTTP 客户端 |
| [tracing](https://docs.rs/tracing/) | 日志与诊断 |
| [PostgreSQL](https://www.postgresql.org/) `16+` | 唯一持久化存储 |

## 快速开始

> [!NOTE]
> **关于构建证据**：发布候选必须由目标 commit 的 CI/Release evidence 证明 `cargo check + test + clippy` 通过；README 不把历史某次 CI 结果当作当前源码的永久事实。本机 Rust 工具链可用时也可直接执行同一组检查。

### 一键安装（推荐）

Native Linux x86_64 一键安装器 [`deploy/install.sh`](deploy/install.sh)：幂等、自动下载并校验
sha256、自动生成配置与密钥、自动配置 PostgreSQL、安装 systemd 服务并健康检查。

```bash
# 在线安装（root；版本默认取最新 release，可用 PPB_VERSION 覆盖）
curl -fsSL https://raw.githubusercontent.com/HyperSynapseNetwork/Phira-plus-Backend/main/deploy/install.sh | sudo bash

# 或下载脚本后交互执行
curl -fsSL -o install.sh https://raw.githubusercontent.com/HyperSynapseNetwork/Phira-plus-Backend/main/deploy/install.sh
sudo PPB_VERSION=0.1.0 bash install.sh
```

非交互部署用环境变量覆盖（secrets 仍自动生成，绝不接受命令行入参）：

```bash
sudo PPB_NONINTERACTIVE=1 \
  PPB_API_URL=https://api.example.com \
  PPB_DATABASE_URL=postgres://ppb:CHANGE_ME@127.0.0.1:5432/ppb \
  bash deploy/install.sh
```

详见 [docs/deployment.md](docs/deployment.md)。

### 下载发行版

从 [Releases](https://github.com/HyperSynapseNetwork/Phira-plus-Backend/releases) 手动下载：
- `ppb-server-linux-x86_64-musl` — 服务端主二进制（静态 musl，可部署到任意 Linux）
- `ppctl-linux-x86_64-musl` — bootstrap / 恢复 CLI（`init` / `doctor` / `config check` / `root reset-password` / `update`）

**前置条件：**

- PostgreSQL 16+（PPB **不**直连 PMP 的 PostgreSQL，使用自己的库）
- PMP（[phira-mp-plus](https://github.com/HyperSynapseNetwork/Phira-mp-plus)）已运行，OpenUDS Unix socket 可用
- 三个固定域名（自部署时可替换，见 `config/example.toml`）：
  - `api-phira.htadiy.com`（PPB）· `phira.htadiy.com`（PPF）· `panel-phira.htadiy.com`（Panel）

### Docker 部署

需要 Docker 与 Docker Compose：

```bash
cp deploy/docker-compose.yml docker-compose.yml
# 填入强口令（自动生成见 `ppctl init`）：
export PPB_DB_PASSWORD=... PPB_JWT_SECRET=... PPB_PHIRA_CREDENTIAL_KEY=...
export PPB_IMAGE=ghcr.io/your-org/ppb-server:0.1.x
# 若 PMP OpenUDS socket 不在 /var/run/pmp，覆盖：
export PMP_OPENUDS_SOCKET_DIR=/run/phira-mp-plus
docker compose up -d
```

健康检查：`curl http://127.0.0.1:8080/healthz` → `{"status":"ok"}`。

### 手动部署（从源码编译）

```bash
git clone git@github.com:HyperSynapseNetwork/Phira-plus-Backend.git
cd Phira-plus-Backend

# 开发预览（默认配置；未设 PPB_DATABASE_URL 时以无持久化模式启动）
cargo run

# 校验配置（deploy/update.sh 的 stage 校验也用它）
cargo run --bin ppb-server -- --check-config config/example.toml

# 生产构建（静态 musl）
rustup target add x86_64-unknown-linux-musl
sudo apt install -y musl-tools
cargo build --release --target x86_64-unknown-linux-musl
```

systemd 部署模板见 [deploy/systemd/ppb.service](deploy/systemd/ppb.service)；Native 更新脚本见 [deploy/update.sh](deploy/update.sh)。

### 配置

- **运行时配置（TOML）**：加载顺序为 `PPB_RUNTIME_CONFIG` 环境变量 → `./config/ppb.toml` → 内置默认值。参考 [config/example.toml](config/example.toml)，字段说明见 [docs/configuration.md](docs/configuration.md)。
- **部署/密钥（环境变量）**：参考 [config/example.env](config/example.env)，完整 `PPB_*` 变量表见 [docs/configuration.md](docs/configuration.md#环境变量deploymentsecret)。密钥绝不写入日志/审计/API 响应。
- **配置校验**：`ppctl config check`（读取 `PPB_RUNTIME_CONFIG` 或 `config/ppb.toml`）或 `ppb-server --check-config <path>`。

### ppctl（bootstrap & 恢复 CLI）

```bash
ppctl init [--output-dir DIR] [--non-interactive] [--api-url URL] [--ppf-url URL] ...
ppctl doctor [--report PATH]      # 脱敏 support bundle
ppctl config check
ppctl root reset-password         # 重置 Root 口令（打印一次）
ppctl update [--check-config PATH] # 校验后打印 stage→原子激活流程
```

> [!NOTE]
> ppctl **不是**第二个 Panel：它只做 bootstrap / 恢复，不提供用户/房间/通知等管理。秘密一律自动生成，**绝不**接受命令行入参。

### 首次登录（Root）

PPB 首次启动会生成 Root 一次性口令，打印到服务端 stdout（仅打印一次，不进日志）。用 Panel 登录后立即改密；如需重置：

```bash
ppctl root reset-password
```

## 许可证

Phira+ Backend 采用 **Apache License, Version 2.0** — 详见 [LICENSE](LICENSE)。
