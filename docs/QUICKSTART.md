# PPB Quick Start

PPB = Phira+ Backend（身份 / 社区 / 控制 / 集成平面）。本指南覆盖 Docker 与 Native Linux 两条官方路径（design §25.1）。

## 前置

- PostgreSQL 16+（PPB 不直连 PMP PostgreSQL，使用自己的库）。
- PMP（phira-mp-plus）已运行，OpenUDS Unix socket 可用。
- 三个域名（或自部署时的替换域名）：
  - `api-phira.htadiy.com`（PPB）
  - `phira.htadiy.com`（PPF）
  - `panel-phira.htadiy.com`（Panel）

## 方式 A：Docker Compose

```bash
cp deploy/docker-compose.yml docker-compose.yml
# 填入强口令（自动生成见 ppctl init）：
export PPB_DB_PASSWORD=... PPB_JWT_SECRET=... PPB_PHIRA_CREDENTIAL_KEY=...
export PPB_IMAGE=ghcr.io/your-org/ppb-server:0.1.31
# 若 PMP OpenUDS socket 不在 /var/run/pmp，覆盖：
export PMP_OPENUDS_SOCKET_DIR=/run/phira-mp-plus
docker compose up -d
```

健康检查：`curl http://127.0.0.1:8080/healthz` → `{"status":"ok"}`。

反向代理：将 `deploy/nginx/nginx.conf` 或 `deploy/caddy/Caddyfile` 的域名/证书替换后启用；WebSocket `/ws/v1/...` 已配置。

## 方式 B：Native Linux + systemd

```bash
# 1) 准备目录与配置
sudo install -d -o ppb -g ppb /opt/ppb /etc/ppb /var/run/pmp
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

## 首次登录（Root）

PPB 首次启动会生成 Root 一次性密码，打印到服务端 stdout（仅打印一次，不会写日志）。使用 Panel 登录后立即改密：

```bash
# 若需重置（ppctl）：
ppctl root reset-password --env-file /etc/ppb/ppb.env
```

## 更新

- Docker：切换镜像 tag，`docker compose up -d`。
- Native：`PPB_VERSION=<v> ./deploy/update.sh`（stage → 校验 → 原子激活）。

## 故障排查

```bash
ppctl doctor --report out.tar.gz   # 脱敏 support bundle
journalctl -u ppb -f               # 服务日志
```

## 参考

- 配置项：[CONFIG_REFERENCE.md](./CONFIG_REFERENCE.md)
- PMP 集成：[PMP_INTEGRATION.md](./PMP_INTEGRATION.md)
- 架构/设计：`DESIGN/PP-B-F-P_V3_总体设计规范.md`
