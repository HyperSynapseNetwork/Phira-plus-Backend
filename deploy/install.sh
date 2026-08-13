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
#   或：
#   sudo PPB_VERSION=0.1.0 PPB_NONINTERACTIVE=1 \
#     PPB_API_URL=https://api.example.com \
#     PPB_DATABASE_URL=postgres://... bash deploy/install.sh
#
# 环境变量：
#   PPB_VERSION        指定版本（默认取最新 release）
#   PPB_NONINTERACTIVE 非交互模式（=1）
#   PPB_API_URL / PPB_PPF_URL / PPB_PANEL_URL / PPB_DOCS_URL  站点 URL 覆盖
#   PPB_OPENUDS_PATH   PMP OpenUDS socket 路径
#   PPB_DATABASE_URL   外部 PostgreSQL（提供则跳过本地 PG 自动配置）

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
  [ "$(id -u)" -eq 0 ] || die "请以 root 运行（sudo bash install.sh）"
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
  for cmd in curl sha256sum systemctl useradd openssl; do
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
  curl -fsSL "$base/${BIN_SERVER}" -o "$tmp/${BIN_SERVER}"
  curl -fsSL "$base/${BIN_PPCTL}" -o "$tmp/${BIN_PPCTL}"
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

ask() {
  local prompt_text=$1 default=$2
  local val
  printf '%s [%s]: ' "$prompt_text" "$default"
  read -r val
  if [ -z "$val" ]; then val="$default"; fi
  printf '%s' "$val"
}

resolve_urls() {
  if [ "${PPB_NONINTERACTIVE:-0}" = "1" ]; then
    API_URL="${PPB_API_URL:-$DEFAULT_API_URL}"
    PPF_URL="${PPB_PPF_URL:-$DEFAULT_PPF_URL}"
    PANEL_URL="${PPB_PANEL_URL:-$DEFAULT_PANEL_URL}"
    DOCS_URL="${PPB_DOCS_URL:-$DEFAULT_DOCS_URL}"
    OPENUDS_PATH="${PPB_OPENUDS_PATH:-$DEFAULT_OPENUDS_PATH}"
    return
  fi
  log "交互模式：逐项回车可跳过（使用默认值）"
  API_URL=$(ask "API URL（PPB）" "$DEFAULT_API_URL")
  PPF_URL=$(ask "PPF URL（公开伴生站）" "$DEFAULT_PPF_URL")
  PANEL_URL=$(ask "Panel URL（管理控制台）" "$DEFAULT_PANEL_URL")
  DOCS_URL=$(ask "Docs URL" "$DEFAULT_DOCS_URL")
  OPENUDS_PATH=$(ask "PMP OpenUDS socket 路径" "$DEFAULT_OPENUDS_PATH")
}

# ── PostgreSQL ──────────────────────────────────────────────────

local_postgres_ok() {
  command -v psql >/dev/null 2>&1 || return 1
  local ver
  ver=$(psql --version 2>/dev/null | sed -n 's/.* \([0-9][0-9]*\)\..*/\1/p')
  [ -n "$ver" ] && [ "$ver" -ge 16 ] 2>/dev/null || return 1
  sudo -u postgres psql -tAc 'SELECT 1' >/dev/null 2>&1 || return 1
  return 0
}

resolve_database_url() {
  # (b) 外部数据库：直接用 PPB_DATABASE_URL。
  if [ -n "${PPB_DATABASE_URL:-}" ]; then
    log "使用外部 PPB_DATABASE_URL"
    DATABASE_URL="$PPB_DATABASE_URL"
    return
  fi
  # (a) 本地 PostgreSQL：自动建 role `ppb` + 库 `ppb`（口令自动生成）。
  if ! local_postgres_ok; then
    die "未提供 PPB_DATABASE_URL，且未检测到本地 PostgreSQL 16+（可访问的 postgres 用户）"
  fi
  log "检测到本地 PostgreSQL，自动创建 role ppb + 数据库 ppb"
  local db_password
  db_password=$(openssl rand -base64 24 | tr '+/' '-_')
  sudo -u postgres psql -v ON_ERROR_STOP=1 >/dev/null <<SQL
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
  if ! sudo -u postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname = 'ppb'" | grep -q 1; then
    sudo -u postgres createdb -O ppb ppb
  fi
  DATABASE_URL="postgres://ppb:${db_password}@127.0.0.1:5432/ppb"
}

# ── 安装 ────────────────────────────────────────────────────────

install_binaries() {
  local tmp=$1
  log "安装二进制到 ${INSTALL_DIR}"
  install -m 0755 "$tmp/${BIN_SERVER}" "${INSTALL_DIR}/ppb-server"
  install -m 0755 "$tmp/${BIN_PPCTL}" "${INSTALL_DIR}/ppctl"
}

ensure_user_and_dirs() {
  if ! id ppb >/dev/null 2>&1; then
    log "创建系统用户 ppb"
    useradd --system --home "$DATA_DIR" --shell /usr/sbin/nologin ppb
  fi
  install -d -o ppb -g ppb "$DATA_DIR"
  install -d -o ppb -g ppb "$CONFIG_DIR"
  install -d -o ppb -g ppb "$RUN_DIR"
}

generate_config() {
  # 幂等：已有配置则保留既有密钥（重装不换 secret）。
  if [ -f "${CONFIG_DIR}/ppb.toml" ] && [ -f "${CONFIG_DIR}/ppb.env" ]; then
    log "已存在 ${CONFIG_DIR}/ppb.toml + ppb.env，跳过 ppctl init（保留既有密钥）"
    return
  fi
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
  log "等待服务健康检查 http://127.0.0.1:8080/healthz"
  local i
  for i in $(seq 1 30); do
    if curl -fsS http://127.0.0.1:8080/healthz 2>/dev/null | grep -q '"status":"ok"'; then
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
  generate_config
  install_systemd_unit "$tmp"
  health_check
  print_next_steps
}

main "$@"
