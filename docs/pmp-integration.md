# PPB ↔ PMP 集成（PMP Integration）

PPB 通过 **OpenUDS（Unix domain socket）** 控制 [phira-mp-plus](https://github.com/HyperSynapseNetwork/Phira-mp-plus)（PMP），**从不直连 PMP PostgreSQL**。数据所有权见 `contracts/README.md` §13。

## 连接与认证

- 帧：4 字节 LE 长度前缀 + UTF-8 JSON，最大 16 MiB（镜像 PMP `protocol.rs`）。
- 认证：
  - **token 模式**：`{"type":"authenticate","token":...}` → `{"type":"authenticated","session_id","server_version"}`
  - **approve 模式**：`{"type":"authenticate","client_name":...}` → `auth_pending` → 在 PMP 控制台 `approve openuds <id>`（TTL 120s）
- 认证后建立能力集（版本映射 + capability detection，见 `pmp/capabilities.rs`）。
- 断线自动重连（指数退避 + 抖动，`pmp/openuds/client.rs`）；连接中断时 pending 命令失败。

## 命令（typed 包装）

### Room（`rooms/service.rs`）

`room.create / close / start / cancel_start / ready / lock / cycle / set_host / set_live / set_chart / set_hidden / set_persistent / set_degraded / set_api_endpoint / kick / force_move / info / list / history / chat_history / chat_send / uuid / rounds / round / ban / unban / banlist / whitelist / whitelist_add / whitelist_remove`

- `chat_send {room_id, user_id, content}`：`user_id` 必须由服务端从 Session 解析（contract §12/§13），客户端不得指定可信 `user_id`。
- `host_allowed` 动作每次执行经 `room.info` 重查真实 host（contract §6/§18）。

### Player（`users/service.rs`）

`player.ban / unban / banlist / ban_ip / unban_ip / ip_history / info / kick`

### Server / Ops（`admin/server.rs`、`admin/plugins.rs`、`pmp/cli`）

`server.stats / server.config_reload / server.shutdown / server.roomcreation / runtime.status / plugin.* / cli.execute`

## 事件 → PPB SSE（`pmp/events/mod.rs`）

映射：`user.online/offline`、`room.created/updated/joined/left`、`round.started/completed`、`server.heartbeat`。

- 禁止用 `broadcast.room` 冒充玩家聊天。
- 信封：`{id, type, version, occurred_at, resource:{type,id}, data}`。
- 前台出口：`GET /api/v1/events`、`GET /api/v1/admin/events`。

## 高频流（`live/`、`replay/`）

- `subscribe_stream touches|judges` → PPB jitter buffer → `WSS /ws/v1/rooms/{room_uuid}/live`。
  - 帧：`{"type":"stream","stream":"touches|judges","user_id","frames","sequence","room","round","timestamp"}`；
  - touch 项 `{time,finger,x,y}`；judge 项 `{time,line_id,note_id,judgement}`。
  - PPB 转发 JSON 信封（`stream/player/sequence/round/timestamp/frames`）+ `resync`/`round_switch`/`heartbeat`。
- `persist.touches/judges {since, limit, round_uuid, player_id}` → 裸批次数组 `[{sequence,round_uuid,player_id,count,first_game_time,last_game_time,payload,created_at}]`。
  - Replay REST + `WSS /ws/v1/replays/{round_uuid}` 分页拉取（游标 `sequence`）。

## 能力集（PMP 1.0.38 已核实）

`persist.touches, persist.judges, room.chat_send, stream.touches, stream.judges`

缺失能力 → `CAPABILITY_NOT_SUPPORTED`，前端隐藏/禁用；**不**静默走危险替代路径。

## 资源隔离

Phira 数据网关 / Aggregator 与 PMP 实时命令路径资源隔离（design §15.8）；聚合 worker 失败不影响控制面。
