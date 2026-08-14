# 对外 API（REST / SSE / WebSocket）

本页是**当前接口导航**，不复制完整 schema。完整普通 HTTP REST 契约以 [`contracts/openapi.json`](../contracts/openapi.json) 为唯一事实源；PPB 实际 Router 必须通过 `scripts/check-route-surface.py` 证明没有未登记的隐藏 REST。非 REST / 浏览器 /基础设施入口以 [`contracts/route-surface.json`](../contracts/route-surface.json) 为准，实时参数语义以 [`contracts/realtime.json`](../contracts/realtime.json) 为准。

## 约定

- REST 前缀：`/api/v1`。
- ErrorEnvelope：`{"error":{"code","message","request_id","details"}}`；产品 UI 以稳定 `code` + i18n 为正式语义，`message` 不是产品文案契约。
- 高风险管理 mutation 的 Reauth 由 PPB 服务端强制，不以 Panel 前端检查替代。
- Room URL / Live WS 使用 **PMP `room_id`**；`room_uuid` 只用于明确声明的稳定身份/分享语义，不能互换。
- Secret 永不通过 Config read API 回显明文。

## Browser / Realtime / Infrastructure

| 方法 | 路径 | 类型 | 说明 |
|---|---|---|---|
| GET | `/auth/phira/login` | Browser HTML | PPB-owned Auth Gateway |
| GET | `/api/v1/auth/github/callback` | Browser OAuth | GitHub OAuth callback；可恢复错误回 Auth Gateway |
| GET | `/api/v1/events` | SSE | 公共事件流 |
| GET | `/api/v1/admin/events` | SSE | 管理事件流 |
| GET | `/api/v1/admin/logs/stream` | SSE | 管理日志流 |
| GET | `/ws/v1/rooms/{room_id}/live` | WebSocket | `room_id` semantic = `pmp_room_id` |
| GET | `/ws/v1/replays/{round_uuid}` | WebSocket | Replay round UUID |
| GET | `/api/v1/openapi.json` | Infrastructure | 当前 OpenAPI 文档 |
| GET | `/healthz` | Infrastructure | Liveness only |

## REST 快速导航

以下仅列高频入口；**不是完整 endpoint 清单**。未列出的普通 REST 仍必须存在于 OpenAPI，且必须被 Runtime Route Surface Gate 覆盖。

### Public / Account

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/public/meta` | API / capability metadata |
| GET | `/api/v1/public/site` | 站点信息 |
| GET | `/api/v1/public/announcements` | 公告 |
| GET | `/api/v1/public/downloads` | 下载入口 |
| POST | `/api/v1/auth/phira/login` | Phira credentials → PPB account session |
| POST | `/api/v1/auth/phira/reauth` | Critical-action reauth token |
| GET | `/api/v1/auth/github/login/start` | 已绑定 GitHub 的后续登录入口 |
| GET | `/api/v1/auth/github/start` | 已登录账户 GitHub 绑定入口 |
| POST | `/api/v1/auth/github/unbind` | 解绑 GitHub |
| GET | `/api/v1/me` | 当前用户摘要 |
| GET | `/api/v1/me/profile` | 当前用户社区资料 |
| GET | `/api/v1/me/preferences` | 账户偏好 |

### Rooms / Replays

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/rooms` | 房间列表 |
| GET | `/api/v1/rooms/{room_id}` | 房间详情 |
| GET | `/api/v1/rooms/{room_id}/chat` | 房间聊天历史 |
| POST | `/api/v1/rooms/{room_id}/chat` | 发送聊天 |
| POST | `/api/v1/rooms/{room_id}/actions` | Action Registry 房间动作 |
| GET | `/api/v1/replays/{round_uuid}` | Replay 详情 |
| GET | `/api/v1/replays/{round_uuid}/manifest` | Replay manifest |
| POST | `/api/v1/replays/{round_uuid}/visibility` | Replay visibility |
| POST | `/api/v1/replays/{round_uuid}/share` | 创建分享 |

### Root / Admin

Root 是独立本地主体，正式路径只有 `/api/v1/admin/auth/root/*`；不存在第二套“等价 Root 路径”。

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/v1/admin/auth/root/login` | Root 登录 |
| GET | `/api/v1/admin/auth/root/session` | Root session probe |
| POST | `/api/v1/admin/auth/root/change-password` | Root 改密 |
| GET | `/api/v1/admin/users` | 用户列表 |
| GET | `/api/v1/admin/users/{phira_id}` | 用户 workspace |
| POST | `/api/v1/admin/users/{phira_id}/actions` | 用户管理动作统一入口 |
| GET | `/api/v1/admin/rooms` | 管理房间列表 |
| POST | `/api/v1/admin/rooms/{room_id}/actions` | 管理房间动作统一入口 |
| GET | `/api/v1/admin/groups` | 管理组列表 |
| PUT | `/api/v1/admin/groups/{id}/permissions` | 原子替换组权限 |
| PUT | `/api/v1/admin/groups/{id}/members` | 原子替换组成员 |

### Config — 唯一执行面

Config 不再保留 whole-YAML write、RuntimeConfig echo 或额外 rollback plane。正式写链：Validate / Diff → Critical Reauth → Save；Rollback 同样由服务端 Critical Reauth，并记录不含 Secret 值的 Audit metadata。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/admin/config/descriptors` | 字段 descriptor / sensitive metadata |
| GET | `/api/v1/admin/config/values` | canonical values；secret 只返回 redacted 状态 |
| POST | `/api/v1/admin/config/validate` | 校验 draft |
| POST | `/api/v1/admin/config/diff` | changed field paths / safe diff |
| POST | `/api/v1/admin/config/save` | Critical Reauth；descriptor patch / secret preserve / snapshot / atomic write / reload / audit |
| GET | `/api/v1/admin/config/snapshots` | snapshot list |
| GET | `/api/v1/admin/config/raw` | 只读 redacted canonical projection；unknown raw YAML 不发送给浏览器 |
| POST | `/api/v1/admin/config/rollback` | Critical Reauth；rollback / reload / audit |
| GET | `/api/v1/admin/config/ppf` | PPF build config |
| PUT | `/api/v1/admin/config/ppf` | PPF build config update |

### Automation

Automation 正式前缀是 `/api/v1/admin/automation`。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/v1/admin/automation/runbooks` | Runbook list |
| POST | `/api/v1/admin/automation/runbooks` | Create runbook |
| GET | `/api/v1/admin/automation/runbooks/{id}` | Runbook detail |
| PATCH | `/api/v1/admin/automation/runbooks/{id}` | Update runbook |
| POST | `/api/v1/admin/automation/runbooks/{id}/run` | Start run |
| GET | `/api/v1/admin/automation/runbook-runs` | Run list |
| GET | `/api/v1/admin/automation/runbook-runs/{id}` | Run status |
| POST | `/api/v1/admin/automation/runbook-runs/{id}/cancel` | Cancel run |

## Contract / Gate

本地静态校验：

```bash
python3 scripts/check-error-contract.py
python3 scripts/check-rest-extractors.py --self-test
python3 scripts/check-rest-extractors.py
python3 scripts/check-route-surface.py --self-test
python3 scripts/check-route-surface.py
python3 scripts/check-realtime-contract.py --self-test
python3 scripts/check-realtime-contract.py
python3 scripts/check-config-security.py --self-test
python3 scripts/check-config-security.py
python3 scripts/check-current-docs.py
python3 scripts/verify-contract-bundle.py
```

真正发布仍要求目标 commit/tag 的 `cargo check/test/clippy`、数据库/浏览器/部署证据；本文档不把历史 CI 结果当作当前候选已验证。
