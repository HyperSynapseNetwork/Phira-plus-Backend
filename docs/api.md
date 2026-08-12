# 对外 API（REST / SSE / WebSocket）

> 权威契约见 `contracts/README.md`（Contract-Freeze v0）§1–§4、§18、§19。
> 端点清单来自当前路由实现（`crates/ppb-server/src/**/routes.rs` 与 `app.rs`）。**OpenAPI 生成物（PPB Phase E）落地后以生成物为准，本清单为人工维护的快速导航。**

## 前缀与约定

- REST 统一前缀：`/api/v1`（`api_version=1`）。
- 实时：
  - SSE：`GET /api/v1/events`（普通）、`GET /api/v1/admin/events`（管理）
  - Live WS：`WSS /ws/v1/rooms/{room_uuid}/live`
  - Replay WS：`WSS /ws/v1/replays/{round_uuid}`
- Auth 网关页面（HTML）：`GET /auth/phira/login?return_to=<relative>`（`return_to` 必须命中 PPB 白名单，防 open redirect）。
- 健康检查：`GET /healthz` → `{"status":"ok"}`。
- 错误契约：`{"error":{"code","message","request_id","details"}}`，code 全 UPPER_SNAKE_CASE。
- 分页：请求 `page`（1-based）、`pageNum`（每页 ≤100）；响应 `{items, total, page, pageNum}`。

## 公开（`/api/v1/public`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/public/meta` | 版本 / api_version / capabilities / pmp 能力集 |
| GET | `/api/v1/public/site` | 站点信息（含 `visit_count`，P-86） |
| GET | `/api/v1/public/announcements` | 公告 |
| GET | `/api/v1/public/downloads` | 下载入口 |
| GET | `/api/v1/public/nodes` | 节点 |
| GET | `/api/v1/public/events` | 公开 SSE 事件流 |

## 认证（`/api/v1/auth`）

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/v1/auth/phira/login` | Phira 登录 → PPB session |
| POST | `/api/v1/auth/phira/reauth` | 高危险动作二次鉴权（`{password}` → 5min `reauth_context` JWT，走 `X-Reauth-Token`） |
| POST | `/api/v1/auth/refresh` | 轮换 session / JWT |
| POST | `/api/v1/auth/logout` | 撤销 session、清 cookie |
| GET | `/api/v1/auth/github/start` | GitHub 绑定流程开始（需已登录） |
| GET | `/api/v1/auth/github/callback` | GitHub 回调（固定 URL） |
| POST | `/api/v1/auth/github/unbind` | 解绑 GitHub |
| POST | `/api/v1/auth/auth/root/login` | Root 登录 `{password}` → `{principal_type, must_change_password}` |
| GET | `/api/v1/auth/auth/root/session` | Root session 探针（P1） |
| POST | `/api/v1/auth/auth/root/change-password` | Root 改密 `{current_password, new_password}` |

> [!NOTE]
> `admin/routes.rs` 把 root 认证子路径 merge 在 `/api/v1/admin` 下，故出现 `/api/v1/admin/auth/root/login`（与 `/api/v1/auth/auth/root/login` 双路径等价，契约以 `/api/v1/admin/auth/root/*` 为准）。

## 本人（`/api/v1/me`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/me` | 当前用户摘要 + 身份 + 权限 |
| GET | `/api/v1/me/profile` | 社区资料（bio/背景/可见性 + rks/stats/online_status/friends_count，缺失为 null） |
| GET | `/api/v1/me/preferences` | 全部命名空间偏好 |
| GET/POST | `/api/v1/me/join-intents` | 列出 / 创建 JoinIntent（§19） |
| DELETE | `/api/v1/me/join-intents/{intent_id}` | 取消 JoinIntent |
| GET/POST | `/api/v1/me/push-endpoints` | 列出 / 注册推送端点（channel: web_push|fcm|wns） |
| DELETE | `/api/v1/me/push-endpoints/{endpoint_id}` | 删除推送端点 |

## 房间（`/api/v1/rooms`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/rooms` | 房间列表 |
| GET | `/api/v1/rooms/{room_id}` | 房间详情 |
| GET | `/api/v1/rooms/{room_id}/history` | 房间历史 |
| GET/POST | `/api/v1/rooms/{room_id}/chat` | 聊天历史 / 发送（`room.chat_send`） |
| POST | `/api/v1/rooms/{room_id}/actions` | 房间动作 `{action, args}`（Action Registry） |
| GET | `/api/v1/rooms/{room_id}/banlist` | 封禁名单 |
| GET | `/api/v1/rooms/{room_id}/whitelist` | 白名单 |

## Phira 数据网关（`/api/v1/charts|records|users`）

已确认公开数据子集（typed 方法，TTL 缓存 + 速率限制）：

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/charts` · `/charts/popular` | 谱面列表 / 热门 |
| GET | `/api/v1/charts/{id}` · `/preview` | 谱面详情 / 预览 |
| GET | `/api/v1/charts/{id}/viewer` | Chart viewer bincode blob（§19，P-84） |
| GET | `/api/v1/charts/{id}/records` · `/top` | 谱面成绩 / 排行 |
| GET | `/api/v1/records` · `/records/query/{chart_id}` · `/list15/{chart_id}` · `/pool/{user_id}` · `/{id}` | 成绩查询 |
| GET | `/api/v1/users` · `/users/{phira_id}` · `/stats` | 用户搜索 / 详情 / 统计 |

## Replay（`/api/v1/replays`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/replays` | Replay 列表 |
| GET | `/api/v1/replays/{round_uuid}` | Replay 详情 |
| GET | `/api/v1/replays/{round_uuid}/manifest` | Replay manifest（§19） |
| POST | `/api/v1/replays/{round_uuid}/visibility` | 设置可见性 |
| POST | `/api/v1/replays/{round_uuid}/share` | 创建分享链接（只存 hash，可 revoke） |
| DELETE | `/api/v1/replays/{round_uuid}/share/{link_id}` | 撤销分享 |
| GET | `/api/v1/replays/share/{token}` | 解析分享 token |

## 管理（`/api/v1/admin`）

> 管理子路径为普通端点超集，不加重复业务模型。泛型动作统一经 Action Registry / Command Broker（§17）。

- **Server**：`/server/status` · `/server/stats` · `/server/runtime` · `/server/actions` · `/server/config-reload` · `/server/roomcreation` · `/server/shutdown` · `/server/broadcast/{all,room,user}`
- **Plugins**：`/plugins` · `/plugins/{name}` · `/{name}/enable|disable|remove` · `/{name}/{action}` · `/plugins/call`
- **Notifications**：`/notifications/send` · `/notifications/delivery`
- **Coupons**：`/coupons` · `/coupons/create` · `/coupons/{id}/revoke`
- **Audit**：`/audit` · `/audit/{id}` · `/audit/export` · `/audit/export.csv`
- **Config**：`/config/descriptors` · `/config/values` · `/config/validate` · `/config/diff` · `/config/save` · `/config/snapshots` · `/config/raw` · `/config/rollback` · `/config/ppb` · `/config/pmp[/snapshots[/{id}/rollback]]` · `/config/ppf` · `/config/public/{key}`
- **Users**：`/users` · `/users/{user_id}` · `/{user_id}/multiplayer|sessions|security|audit` · `/{user_id}/actions` · `/{user_id}/ban|unban|kick` · `/{user_id}/ip-history`
- **Rooms**：`/rooms`（GET+POST）· `/rooms/{room_id}`（GET+DELETE）· `/rooms/{room_id}/actions` · `/rooms/actions/batch`（preview + partial failure）· `/rooms/{room_id}/banlist|whitelist`
- **Permissions**：`/permissions/manifest` · `/groups` · `/groups/{id}` · `/{id}/set-default` · `/{id}/permissions` · `/{id}/members` · `/{id}/members/{user_id}`
- **Actions / Commands**：`/actions` · `/actions/{action_id}/execute` · `/commands` · `/commands/history` · `/commands/execute`（raw `cli.execute`，全量 Audit）
- **Logs**：`/logs` · `/logs/stream` · `/logs/input` · `/logs/translate`
- **Jobs**：`/jobs` · `/jobs/{job_id}` · `/jobs/{job_id}/cancel`
- **Automation**：`/runbooks` · `/runbooks/{id}` · `/runbooks/{id}/run` · `/runbook-runs`

## SSE 事件信封

```json
{"id":"uuid","type":"room.updated","version":1,"occurred_at":"RFC3339",
 "resource":{"type":"room","id":"..."},"data":{}}
```

支持 `server.heartbeat`、`Last-Event-ID` 短 replay、无法续传时 snapshot+realtime。PMP 事件映射见 [pmp-integration.md](./pmp-integration.md)。
