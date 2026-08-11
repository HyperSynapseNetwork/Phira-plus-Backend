#!/usr/bin/env bash
# PPB 更新模板：stage → 校验 → 原子激活（设计 §20.5 / §25.1）。
# 禁止半部署中间态：新版本先落到 staging，校验通过后原子切换。
#
# 用法：
#   PPB_VERSION=0.1.31 ./deploy/update.sh
#
# 说明：Docker 部署见 docker-compose（image 标签切换）；本脚本面向 Native。

set -euo pipefail

PPB_VERSION="${PPB_VERSION:?set PPB_VERSION}"
STAGE_DIR="/opt/ppb/stage/ppb-$PPB_VERSION"
ACTIVE_LINK="/opt/ppb/current"
SYSTEMD_UNIT="ppb.service"

echo "[1/5] 下载/解压到 staging"
mkdir -p "$STAGE_DIR"
# 示例：从 GitHub Release 下载（替换为实际 release URL）
# curl -fsSL "https://github.com/your-org/phira-plus-backend/releases/download/v$PPB_VERSION/ppb-server-linux-x86_64-musl" \
#   -o "$STAGE_DIR/ppb-server"
chmod +x "$STAGE_DIR/ppb-server"

echo "[2/5] stage 校验"
"$STAGE_DIR/ppb-server" --version
"$STAGE_DIR/ppb-server" --check-config /etc/ppb/ppb.toml || {
  echo "配置校验失败，中止更新（保持旧版本运行）" >&2
  exit 1
}

echo "[3/5] 停止服务（旧版本）"
systemctl stop "$SYSTEMD_UNIT"

echo "[4/5] 原子激活（symlink 切换）"
ln -sfn "$STAGE_DIR" "$ACTIVE_LINK"
cp -f "$ACTIVE_LINK/ppb-server" /usr/local/bin/ppb-server

echo "[5/5] 启动 + 健康检查"
systemctl start "$SYSTEMD_UNIT"
for i in $(seq 1 12); do
  if curl -fsS http://127.0.0.1:8080/healthz >/dev/null 2>&1; then
    echo "健康检查通过"
    exit 0
  fi
  sleep 2
done
echo "健康检查失败，请回滚：systemctl start $SYSTEMD_UNIT && 手动切换 $ACTIVE_LINK" >&2
exit 1
