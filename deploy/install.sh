#!/usr/bin/env bash
#
# PPB 一键安装器（Native Linux x86_64）。
#
# 设计 §25.1：幂等 · 仅支持 Linux x86_64（musl 产物只有 x86_64）· root 执行。
# 从 GitHub Release 下载 `ppb-server-linux-x86_64-musl` / `ppctl-linux-x86_64-musl`
# 与 `SHA256SUMS` 并校验 sha256，安装到 /usr/local/bin，用 `ppctl init` 生成
# 配置与密钥（secrets 一律自动生成，绝不接受命令行入参），安装 systemd unit，
# 最后轮询 /healthz 直到 `{"status":"ok"}`。
#
# 用法：
#   curl -fsSL https://raw.githubusercontent.com/HyperSynapseNetwork/Phira-plus-Backend/main/deploy/install.sh | sudo bash
#
# 需要传入环境变量时（如 PPB_DATABASE_URL），注意：`curl ... | sudo bash` 会因 sudo 的
# env_reset 丢掉 PPB_* 环境变量（只传给 curl，不传给 sudo bash）。推荐两种正确姿势：
#   1) sudo -E 保留环境：
#        PPB_DATABASE_URL=postgres://... sudo -E bash -c \
#          'curl -fsSL https://raw.githubusercontent.com/HyperSynapseNetwork/Phira-plus-Backend/main/deploy/install.sh | bash'
#   2) 先落盘再用 sudo env 显式传入：
#        curl -fsSL https://raw.githubusercontent.com/HyperSynapseNetwork/Phira-plus-Backend/main/deploy/install.sh -o /tmp/install.sh
#        sudo env PPB_DATABASE_URL=postgres://... PPB_NONINTERACTIVE=1 bash /tmp/install.sh
#   （也可：curl -fsSL ... | sudo env PPB_DATABASE_URL=postgres://... bash）
#
# 环境变量：
#   PPB_VERSION        指定版本（默认取最新 release）
#   PPB_NONINTERACTIVE 非交互模式（=1）
#   PPB_API_URL / PPB_PPF_URL / PPB_PANEL_URL / PPB_DOCS_URL  站点 URL 覆盖
#   PPB_OPENUDS_PATH   PMP OpenUDS socket 路径
#   PPB_DATABASE_URL   外部 PostgreSQL（提供则跳过本地 PG 自动配置）
#   PPB_LISTEN_PORT    监听端口（默认自动选择空闲端口，8080 优先）

set -euo pipefail

GITHUB_REPO="HyperSynapseNetwork/Phira-plus-Backend"
BIN_SERVER="ppb-server-linux-x86_64-musl"
BIN_PPCTL="ppctl-linux-x86_64-musl"
SUM_FILE="SHA256SUMS"
INSTALL_DIR="/usr/local/bin"
CONFIG_DIR="/etc/ppb"
DATA_DIR="/opt/ppb"
RUN_DIR="/var/run/pmp"
SERVICE_NAME="ppb"

# config/example.toml 默认值（交互模式逐项提示，回车跳过）。
DEFAULT_API_URL="https://api-phira.htadiy.com"
DEFAULT_PPF_URL="https://phira.htadiy.com"
DEFAULT_PANEL_URL="https://panel-phira.htadiy.com"
DEFAULT_DOCS_URL="https://docs.phira.htadiy.com"
DEFAULT_OPENUDS_PATH="/var/run/pmp-openuds.sock"

log() { printf '\033[1;34m[ppb]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[ppb][warn]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31m[ppb][error]\033[0m %s\n' "$*" >&2; exit 1; }

# ── 前置检查 ────────────────────────────────────────────────────

assert_root() {
  [ "$(id -u)" -eq 0 ] || die "仅支持 root 部署；非 root 不支持（请用 sudo 运行）"
}

assert_arch() {
  case "$(uname -m)" in
    x86_64|amd64) ;;
    *) die "仅支持 Linux x86_64（musl 产物只有 x86_64）；当前架构：$(uname -m)" ;;
  esac
  [ "$(uname -s)" = "Linux" ] || die "仅支持 Linux"
}

check_prereqs() {
  local missing=0
  for cmd in curl sha256sum systemctl openssl; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
      warn "缺少前置命令：$cmd"
      missing=1
    fi
  done
  [ "$missing" -eq 0 ] || die "前置检查失败，请先安装缺失的命令"
}

# ── 版本解析 ────────────────────────────────────────────────────

resolve_version() {
  if [ -n "${PPB_VERSION:-}" ]; then
    printf '%s' "$PPB_VERSION"
    return
  fi
  local tag
  tag=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases/latest" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)
  tag="${tag#v}"
  if [ -z "$tag" ]; then
    die "无法解析最新 release 版本；请用 PPB_VERSION 指定（如 PPB_VERSION=0.1.0）"
  fi
  printf '%s' "$tag"
}

# ── 下载 + 校验 ─────────────────────────────────────────────────

download_and_verify() {
  local version=$1 tmp=$2
  local base="https://github.com/${GITHUB_REPO}/releases/download/v${version}"
  log "下载 v${version} 产物 → ${tmp}"
  curl -fL --progress-bar "$base/${BIN_SERVER}" -o "$tmp/${BIN_SERVER}"
  curl -fL --progress-bar "$base/${BIN_PPCTL}" -o "$tmp/${BIN_PPCTL}"
  curl -fsSL "$base/${SUM_FILE}" -o "$tmp/${SUM_FILE}"

  log "校验 sha256（仅校验本次下载的 x86_64 产物）"
  (
    cd "$tmp"
    grep -E "${BIN_SERVER}|${BIN_PPCTL}" "$SUM_FILE" > "${SUM_FILE}.x86_64" \
      || die "SHA256SUMS 中找不到 x86_64 产物条目"
    sha256sum -c "${SUM_FILE}.x86_64" >/dev/null || die "sha256 校验失败"
  )
}

# ── 交互提示 ────────────────────────────────────────────────────

# 读取一行交互输入：优先从 /dev/tty（`curl | sudo bash` 下 stdin 是管道而非终端），
# 无控制终端时退回 stdin；读不到（EOF）返回非 0，否则打印读到的行。
read_line() {
  local line
  if [ -e /dev/tty ]; then
    read -r line </dev/tty || return 1
  else
    read -r line || return 1
  fi
  printf '%s' "$line"
}

ask() {
  local prompt_text=$1 default=$2
  local val
  printf '%s [%s]: ' "$prompt_text" "$default"
  val=$(read_line) || val=""
  if [ -z "$val" ]; then val="$default"; fi
  printf '%s' "$val"
}

resolve_urls() {
  # 非交互（显式 PPB_NONINTERACTIVE=1，或无控制终端）→ 直接用环境变量 / 默认值。
  if [ "${PPB_NONINTERACTIVE:-0}" = "1" ] || [ ! -e /dev/tty ]; then
    [ "${PPB_NONINTERACTIVE:-0}" = "1" ] || warn "未检测到交互终端，使用默认 URL（可用 PPB_* 环境变量覆盖）"
    API_URL="${PPB_API_URL:-$DEFAULT_API_URL}"
    PPF_URL="${PPB_PPF_URL:-$DEFAULT_PPF_URL}"
    PANEL_URL="${PPB_PANEL_URL:-$DEFAULT_PANEL_URL}"
    DOCS_URL="${PPB_DOCS_URL:-$DEFAULT_DOCS_URL}"
    OPENUDS_PATH="${PPB_OPENUDS_PATH:-$DEFAULT_OPENUDS_PATH}"
    return
  fi
  log "────────────────────────────────────────────────────────"
  log "进入交互配置：接下来会逐项询问（共 5 项）。"
  log "直接按【回车】= 接受方括号内的默认值，无需手动输入。"
  log "────────────────────────────────────────────────────────"
  API_URL=$(ask "API URL（PPB）" "$DEFAULT_API_URL")
  PPF_URL=$(ask "PPF URL（公开伴生站）" "$DEFAULT_PPF_URL")
  PANEL_URL=$(ask "Panel URL（管理控制台）" "$DEFAULT_PANEL_URL")
  DOCS_URL=$(ask "Docs URL" "$DEFAULT_DOCS_URL")
  OPENUDS_PATH=$(ask "PMP OpenUDS socket 路径" "$DEFAULT_OPENUDS_PATH")
}

# ── PostgreSQL ──────────────────────────────────────────────────

# 全局探测结果：
PG_PSQL=""       # 可用 psql 二进制绝对路径
PG_METHOD=""     # 管理员连接方式：sudo / su / tcp
PG_VERSION=0     # 服务端主版本号（如 16、17；0=未知）
PG_PORT=5432     # 服务端监听端口（SHOW port 自动探测，失败回退 5432）

# 返回可用的 psql 路径（PATH 优先，其次 Debian 的 /usr/lib/postgresql/*/bin/psql）。
find_psql() {
  if command -v psql >/dev/null 2>&1; then
    command -v psql
    return 0
  fi
  local p
  for p in /usr/lib/postgresql/*/bin/psql; do
    if [ -x "$p" ]; then printf '%s' "$p"; return 0; fi
  done
  return 1
}

# 通过已探测到的管理员连接执行 psql（其余参数透传；SQL 可从 stdin 传入）。
pg_run() {
  case "$PG_METHOD" in
    su)  su postgres -c "$PG_PSQL $(printf '%q ' "$@")" ;;
    tcp) "$PG_PSQL" -U postgres -h 127.0.0.1 -w "$@" ;;
    *)   sudo -u postgres "$PG_PSQL" "$@" ;;
  esac
}

# 分步检测本地 PostgreSQL。返回码：
#   0 = 已安装且可用（PG_PSQL/PG_METHOD/PG_VERSION 已填充）
#   1 = 已安装但无法以 postgres 连接
#   2 = 未检测到 PostgreSQL
detect_pg() {
  PG_VERSION=0
  PG_METHOD=""
  PG_PSQL=""
  PG_PORT=5432
  log "检测本地 PostgreSQL..."

  # 1) 确认 PostgreSQL 是否存在（服务 / 客户端 / Debian 版本目录任一命中即视为已安装）。
  local present=0
  if systemctl is-active --quiet postgresql 2>/dev/null; then
    present=1
  elif command -v pg_isready >/dev/null 2>&1; then
    present=1
  elif command -v psql >/dev/null 2>&1; then
    present=1
  elif ls /usr/lib/postgresql/*/bin/psql >/dev/null 2>&1; then
    present=1
  fi
  [ "$present" -eq 1 ] || return 2

  PG_PSQL=$(find_psql) || return 1

  # 2) 探测可连接方式（依次：sudo -u postgres → su postgres -c → psql -U postgres -h 127.0.0.1）。
  local m
  for m in sudo su tcp; do
    PG_METHOD="$m"
    pg_run -tAc 'SELECT 1' >/dev/null 2>&1 && break
    PG_METHOD=""
  done
  [ -n "$PG_METHOD" ] || return 1
  log "本地 PostgreSQL 可连接（方式：${PG_METHOD}）"

  # 3) 服务端主版本号。
  local ver
  ver=$(pg_run -tAc 'SHOW server_version_num' 2>/dev/null | tr -d '[:space:]')
  case "$ver" in
    ''|*[!0-9]*) return 1 ;;
  esac
  PG_VERSION=$((ver / 10000))
  log "本地 PostgreSQL 服务端版本：${PG_VERSION}"

  # 4) 服务端监听端口（自动寻找，不回退到硬编码）。
  local port
  port=$(pg_run -tAc 'SHOW port' 2>/dev/null | tr -d '[:space:]')
  case "$port" in
    ''|*[!0-9]*) port=5432 ;;
  esac
  PG_PORT=$port
  log "本地 PostgreSQL 端口：${PG_PORT}"
  return 0
}

# 交互模式下提示输入完整连接串；非交互模式 die。
ask_database_url() {
  local url=""
  while [ -z "$url" ]; do
    printf '请输入完整 PostgreSQL 连接串（示例：postgres://ppb:pass@127.0.0.1:5432/ppb）: '
    url=$(read_line) || die "未读到输入（无交互终端）；请改用 PPB_DATABASE_URL=... 传入"
    case "$url" in
      postgres://*|postgresql://*) ;;
      *) warn "连接串需以 postgres:// 或 postgresql:// 开头"; url=""; ;;
    esac
  done
  DATABASE_URL="$url"
}

# 本地 PostgreSQL 不可用时：交互模式询问，非交互/无终端模式直接 die。
prompt_or_die() {
  local reason=$1
  if [ "${PPB_NONINTERACTIVE:-0}" = "1" ] || [ ! -e /dev/tty ]; then
    die "${reason}；请设置 PPB_DATABASE_URL=postgres://user:pass@host:port/db 后重试"
  fi
  warn "$reason"
  ask_database_url
}

# 本地 PostgreSQL 可用时：自动建 role `ppb` + 库 `ppb`（口令自动生成）。
auto_configure_local_pg() {
  local db_password
  db_password=$(openssl rand -base64 24 | tr '+/' '-_')
  log "创建/更新 role ppb（口令 openssl rand 自动生成）"
  pg_run -v ON_ERROR_STOP=1 >/dev/null <<SQL
DO \$\$
BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'ppb') THEN
    CREATE ROLE ppb LOGIN PASSWORD '${db_password}';
  ELSE
    ALTER ROLE ppb WITH LOGIN PASSWORD '${db_password}';
  END IF;
END
\$\$;
SQL
  if ! pg_run -tAc "SELECT 1 FROM pg_database WHERE datname = 'ppb'" | grep -q 1; then
    log "创建数据库 ppb（owner=ppb）"
    pg_run -c "CREATE DATABASE ppb OWNER ppb" >/dev/null
  else
    log "数据库 ppb 已存在，跳过创建"
  fi
  DATABASE_URL="postgres://ppb:${db_password}@127.0.0.1:${PG_PORT}/ppb"
  log "已生成连接串 postgres://ppb:***@127.0.0.1:${PG_PORT}/ppb"
}

resolve_database_url() {
  # (b) 外部数据库：直接用 PPB_DATABASE_URL。
  if [ -n "${PPB_DATABASE_URL:-}" ]; then
    log "使用外部 PPB_DATABASE_URL"
    DATABASE_URL="$PPB_DATABASE_URL"
    return
  fi

  # (a) 本地 PostgreSQL：分步检测，结果决定自动配置 / 交互询问 / 报错。
  local rc
  detect_pg; rc=$?
  case $rc in
    0)
      if [ "$PG_VERSION" -lt 16 ]; then
        die "检测到 PostgreSQL ${PG_VERSION}（<16），PPB 需要 16+，请升级后再试"
      fi
      auto_configure_local_pg
      ;;
    2)
      prompt_or_die "未检测到本地 PostgreSQL"
      ;;
    1)
      prompt_or_die "检测到本地 PostgreSQL，但无法以 postgres 用户连接（服务未运行或认证拒绝）"
      ;;
  esac
}

# ── 安装 ────────────────────────────────────────────────────────

install_binaries() {
  local tmp=$1
  log "安装二进制到 ${INSTALL_DIR}"
  install -m 0755 "$tmp/${BIN_SERVER}" "${INSTALL_DIR}/ppb-server"
  install -m 0755 "$tmp/${BIN_PPCTL}" "${INSTALL_DIR}/ppctl"
}

ensure_user_and_dirs() {
  # 直接以 root 运行，不创建 ppb 系统用户（见 docs/deployment.md）。
  log "创建目录 ${DATA_DIR} / ${CONFIG_DIR} / ${RUN_DIR}（root 拥有）"
  install -d "$DATA_DIR"
  install -d "$CONFIG_DIR"
  install -d "$RUN_DIR"
}

# ── 监听端口 ────────────────────────────────────────────────────

LISTEN_PORT=""   # 运行时监听端口（自动选择空闲端口，或 PPB_LISTEN_PORT 覆盖）

# 判断端口是否空闲（本地 127.0.0.1 无监听即为空闲）。
port_free() {
  local p=$1
  ! (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null
}

# 选定监听端口：显式覆盖 → 沿用既有配置 → 8080（被占用则向后找第一个空闲端口）。
pick_listen_port() {
  if [ -n "${PPB_LISTEN_PORT:-}" ]; then
    LISTEN_PORT="$PPB_LISTEN_PORT"
    log "监听端口：${LISTEN_PORT}（PPB_LISTEN_PORT 显式指定）"
    return
  fi
  if [ -f "${CONFIG_DIR}/ppb.toml" ]; then
    local existing
    existing=$(sed -n 's/^[[:space:]]*listen_addr[[:space:]]*=[[:space:]]*"[^:]*:\([0-9][0-9]*\)".*/\1/p' "${CONFIG_DIR}/ppb.toml" | head -n1)
    if [ -n "$existing" ]; then
      LISTEN_PORT="$existing"
      log "监听端口：${LISTEN_PORT}（沿用既有配置）"
      return
    fi
  fi
  local p
  for p in 8080 8081 8082 8083 8084 8085 8086 8087 8088 8089 8090 8091 8092 8093 8094 8095; do
    if port_free "$p"; then
      LISTEN_PORT="$p"
      log "监听端口：${p}（自动选择空闲端口）"
      return
    fi
  done
  die "端口 8080–8095 均被占用；请用 PPB_LISTEN_PORT 指定空闲端口"
}

# 幂等：把 ppb.toml 的 listen_addr 端口改写为选定端口。
apply_listen_port() {
  if ! grep -q '^[[:space:]]*listen_addr[[:space:]]*=' "${CONFIG_DIR}/ppb.toml"; then
    return
  fi
  sed -i "s/^\([[:space:]]*listen_addr[[:space:]]*=[[:space:]]*\"\)[^:]*:[0-9][0-9]*\"/\10.0.0.0:${LISTEN_PORT}\"/" "${CONFIG_DIR}/ppb.toml"
}

generate_config() {
  # 幂等：已有配置则保留既有密钥（重装不换 secret）。
  if [ -f "${CONFIG_DIR}/ppb.toml" ] && [ -f "${CONFIG_DIR}/ppb.env" ]; then
    log "已存在 ${CONFIG_DIR}/ppb.toml + ppb.env，跳过 ppctl init（保留既有密钥）"
  else
    log "ppctl init 生成配置 + 自动生成密钥（secrets 绝不接受命令行入参）"
    /usr/local/bin/ppctl init --non-interactive \
      --output-dir "$CONFIG_DIR" \
      --api-url "$API_URL" \
      --ppf-url "$PPF_URL" \
      --panel-url "$PANEL_URL" \
      --docs-url "$DOCS_URL" \
      --openuds-path "$OPENUDS_PATH" \
      --database-url "$DATABASE_URL"

    # 关键：systemd 部署必须显式指定运行时配置（见 docs/deployment.md 的 NOTE）。
    if ! grep -q '^PPB_RUNTIME_CONFIG=' "${CONFIG_DIR}/ppb.env"; then
      printf '\n# systemd 部署显式指定运行时配置（PPB_RUNTIME_CONFIG 优先于 ./config/ppb.toml）。\nPPB_RUNTIME_CONFIG=%s/ppb.toml\n' "$CONFIG_DIR" >> "${CONFIG_DIR}/ppb.env"
    fi
    chmod 0600 "${CONFIG_DIR}/ppb.env"
  fi
  apply_listen_port
}

install_systemd_unit() {
  local tmp=$1 unit_src=""
  if [ -f "$(cd "$(dirname "$0")" && pwd)/systemd/ppb.service" ]; then
    unit_src="$(cd "$(dirname "$0")" && pwd)/systemd/ppb.service"
  else
    unit_src="$tmp/ppb.service"
    curl -fsSL "https://raw.githubusercontent.com/${GITHUB_REPO}/v${PPB_VERSION}/deploy/systemd/ppb.service" -o "$unit_src"
  fi
  log "安装 systemd unit"
  install -m 0644 "$unit_src" "/etc/systemd/system/${SERVICE_NAME}.service"
  systemctl daemon-reload
  systemctl enable --now "$SERVICE_NAME"
}

health_check() {
  log "等待服务健康检查 http://127.0.0.1:${LISTEN_PORT}/healthz"
  local i
  for i in $(seq 1 30); do
    if curl -fsS "http://127.0.0.1:${LISTEN_PORT}/healthz" 2>/dev/null | grep -q '"status":"ok"'; then
      log "健康检查通过：{\"status\":\"ok\"}"
      return 0
    fi
    sleep 2
  done
  die "健康检查超时；请查看 journalctl -u ${SERVICE_NAME} -e 排查"
}

print_next_steps() {
  cat <<EOF

✓ 安装完成。下一步：
  · Root 一次性口令：journalctl -u ${SERVICE_NAME}（首启打印一次，仅一次）
  · 重置 Root 口令：ppctl root reset-password
  · 反代模板：deploy/nginx/nginx.conf、deploy/caddy/Caddyfile
  · 配置：${CONFIG_DIR}/ppb.toml（运行时）+ ${CONFIG_DIR}/ppb.env（密钥，0600）
  · 日志：journalctl -u ${SERVICE_NAME} -f
EOF
}

# ── 主流程 ──────────────────────────────────────────────────────

main() {
  assert_root
  assert_arch
  check_prereqs
  PPB_VERSION=$(resolve_version)
  log "目标版本：v${PPB_VERSION}"

  local tmp
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT

  download_and_verify "$PPB_VERSION" "$tmp"
  install_binaries "$tmp"
  ensure_user_and_dirs
  resolve_urls
  resolve_database_url
  pick_listen_port
  generate_config
  install_systemd_unit "$tmp"
  health_check
  print_next_steps
}

main "$@"
