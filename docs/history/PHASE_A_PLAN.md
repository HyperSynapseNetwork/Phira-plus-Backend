# PPB Phase A — Contract Freeze + Foundation

> PPB Agent implementation plan. Authoritative sources: `contracts/README.md` (Contract-Freeze v0),
> `DESIGN/AUDIT_2026-08-12_初始审计.md`, `DESIGN/PP-B-F-P_V3_总体设计规范.md` §6/§7/§8/§9/§11/§24/§26/§27,
> and PMP source facts (v1.0.38, verified — not re-derived).

---

## 1. Goal

Build the PPB (Phira+ Backend) foundation in this repo: Rust + axum + Tokio + PostgreSQL(sqlx), unified backend
for Phira+. This phase delivers: workspace, config, sqlx migrations, Auth (Phira login → PPB JWT, Root, GitHub
bind-only, reauth skeleton), Permission manifest + resolver + groups, Action Registry + Command Broker,
OpenUDS client (token/approve auth, typed commands, events, streams, reconnect/backoff, capabilities),
SSE contract, error contract + middleware, public meta/capabilities, CI workflows, and tests.

**Hard constraints honored:** no local `cargo`/`rustc` (aarch64 toolchain broken → CI is the only verification);
commit locally only, never push; no PMP PostgreSQL direct access; no Replay content/files; `*:*` only for Root;
no tournament/Event/HSNBot/private-IM/Web-OS/arbitrary-shell; Phira password never persisted/logged;
Phira access/refresh tokens never reach frontends.

---

## 2. Toolchain & dependency versions

| Item | Resolved value |
|---|---|
| Rust channel | `1.96.0` (rust-toolchain.toml, components rustfmt/clippy/rust-src, targets x86_64+aarch64, profile minimal) |
| Workspace | resolver 2, edition 2021, single member `crates/ppb-server`, bin name `ppb-server` |
| Package version | `0.1.0` (CI auto-patch bumps patch on main push) |
| axum | `0.8` |
| tokio | `1` (full) |
| tower / tower-http | `0.5` / `0.6` (cors, trace, request-id, catch-panic) |
| sqlx | `0.8` (runtime-tokio, tls-rustls, postgres, uuid, chrono, json, migrate) — **no compile-time `query!` macros** (no DB during compile) |
| serde / serde_json | `1` |
| jsonwebtoken | `9` (HS256) |
| reqwest | `0.12` (json, rustls-tls) |
| aes-gcm | `0.10` (Phira refresh-token encryption) |
| bcrypt | `0.15` (Root password hashing) |
| cookie | `0.18` (manual Set-Cookie / parse; no tower-cookies — avoids layer ordering coupling) |
| uuid / chrono | `1` / `0.4` |
| rand | `0.8` |
| base64 | `0.22` |
| thiserror | `2` |
| tracing / tracing-subscriber | `0.1` / `0.3` (env-filter, json) |
| tokio-util / futures-util / async-trait | `0.7` / `0.3` / `0.1` |
| dashmap | `6` (runtime caches) |

Dependencies are resolved to exact versions by CI `cargo generate-lockfile` (GitHub runners, x86_64).
`Cargo.lock` is intentionally **not** committed locally (cannot generate without working cargo); the CI
auto-patch job generates and commits it on the first main push, so `--locked` checks pass on the patched ref.

---

## 3. Directory layout

```
ppb/
├── Cargo.toml                      # workspace root + workspace.dependencies
├── rust-toolchain.toml
├── .gitignore
├── README.md
├── docs/PHASE_A_PLAN.md
├── config/example.toml             # PPB runtime config example (NO secrets)
├── config/example.env              # deployment env var reference (NO real values)
├── migrations/0001_init.sql        # sqlx migration (all tables)
├── scripts/sync-workspace-version.py
├── .github/workflows/{build.yml,bump-version.yml,release.yml}
└── crates/ppb-server/
    ├── Cargo.toml
    └── src/
        ├── main.rs                 # bootstrap: config → tracing → DB → router → serve
        ├── lib.rs                  # module graph (domain-vertical, §24.1)
        ├── app.rs                  # AppState + build_router
        ├── telemetry.rs
        ├── config/                 # deployment env + runtime TOML
        ├── error/                  # unified error contract + pagination helpers
        ├── middleware/             # request_id, csrf, cors, auth extractors
        ├── auth/                   # phira/root/github/reauth/jwt/session/routes
        ├── users/                  # model/repo/service/routes
        ├── identities/             # phira|github identity bindings + phira credential state
        ├── permissions/            # manifest/resolver/groups/model/repo/routes
        ├── preferences/            # JSONB + revision
        ├── social/                 # friends/blocks (models+repo)
        ├── rooms/                  # room command facade (scaffold)
        ├── replay/                 # replay_overrides/acl/share_links (policy only)
        ├── notifications/          # notification_events/user_notifications/push_endpoints
        ├── actions/                # Action Registry + manifest
        ├── commands/               # Command Broker + command_runs
        ├── jobs/                   # jobs model/repo
        ├── automation/             # runbooks scaffold (Phase B)
        ├── audit/                  # audit_events model/repo/service
        ├── logs/                   # logs scaffold
        ├── metrics/                # metrics scaffold
        ├── phira/                  # Phira API client + credential crypto
        ├── pmp/                    # capabilities map + submodules
        │   ├── openuds/            # protocol/client/events/streams/auth
        │   ├── events/             # PMP event → PPB SSE envelope mapping
        │   ├── live/               # live WS gateway scaffold
        │   └── cli/                # cli.execute wrapper
        ├── public/                 # /api/v1/public/* (meta, site scaffold)
        └── admin/                  # /api/v1/admin/* (root login + scaffolds)
```

Domain-vertical rule honored: each domain owns its model/service/repo/routes. No global `models/`+`services/`.

---

## 4. Data model (migrations/0001_init.sql)

Tables per design §24.2 **plus two justified additions** (see Proposal P3):

- `users` (id UUID, phira_id BIGINT UNIQUE, username_cache, avatar_cache, status, created_at/updated_at/last_seen_at)
- `user_identities` (user_id, provider phira|github, provider_id, provider_name, linked_at; UNIQUE(provider, provider_id))
- `phira_credentials` **[addition]** (user_id PK/FK, refresh_token_ciphertext BYTEA, refresh_expires_at, state active|reauth_required|revoked, updated_at)
- `sessions` (id, principal_type user|root, user_id nullable, client_type ppf|panel|windows|android, refresh_hash, created_at/expires_at/revoked_at/last_seen_at, device_name, ip)
- `root_credentials` **[addition]** (id, password_hash, must_change_password, created_at, updated_at)
- `groups` (id, name, system_kind nullable|admin_scope, is_default, protected, timestamps)
- `group_members` (group_id, user_id)
- `group_permissions` (group_id, permission, **CHECK (permission <> '*:*')** — DB-layer rejection)
- `user_profiles` (bio, background_url, visibility, show_online_status, show_recent_activity)
- `friend_requests` (from/to/status/responded_at)
- `friendships` (normalized unique pair: user_a < user_b, UNIQUE(user_a,user_b))
- `user_blocks` (blocker_id, blocked_id)
- `replay_overrides` (pmp_replay_id, owner_user_id, visibility, updated_at)
- `replay_acl` (replay_id, user_id, effect allow|deny)
- `replay_share_links` (id, replay identity, token_hash, created_by, expires_at, revoked_at)
- `user_preferences` (user_id, namespace common|ppf|panel|experiments, revision, json_data JSONB, updated_at; PK(user_id,namespace))
- `notification_events` (id, type, actor_user_id, payload JSONB, created_at)
- `user_notifications` (event_id, user_id, read_at, dismissed_at, created_at)
- `push_endpoints` (user_id, device_id, channel, endpoint_encrypted, platform, timestamps, disabled_at)
- `audit_events` (occurred_at, principal_type, actor_user_id, actor_session_id, action, resource_type, resource_id, parameters_redacted JSONB, result, error_code, request_id, command_id, ip, user_agent)
- `command_runs` (action, actor, resource_key, arguments_redacted, status, started_at, finished_at, result_summary, error_code)
- `jobs` (type, state, progress, stage, timestamps, error)

Replay tables store **policy only** — never Replay content. `room.*`/online state are runtime caches, never persisted as facts.

---

## 5. Auth design (design §6)

- **Phira login**: `POST /api/v1/auth/phira/login {email, password, client_type, return_to}` → PPB calls Phira
  `/login` (gets access+refresh), then `/me` (gets phira_id/profile). Creates/updates PPB user, upserts phira
  identity, encrypts+stores refresh token in `phira_credentials`, auto-joins default group, issues:
  - `ppb_access` JWT (claims `{sub,sid,principal_type,client_type,iat,exp}`) in **Secure+HttpOnly** cookie,
    host-only domain `api-phira.htadiy.com`, SameSite=Lax (same-site cross-origin with PPF/Panel).
  - `ppb_csrf` cookie (non-HttpOnly) for CSRF double-submit.
  - Also returns a short-lived `exchange` code for Tauri bearer flow (JWT not placed in URL).
- **Refresh**: `POST /api/v1/auth/refresh` (cookie) → rotates session, issues new JWT. If Phira refresh token
  expired/revoked → `PHIRA_REAUTH_REQUIRED`.
- **Logout**: `POST /api/v1/auth/logout` → revoke session, clear cookies.
- **Reauth skeleton**: `POST /api/v1/auth/phira/reauth {password}` → verifies against Phira `/login`, issues a
  5-min `reauth_context` JWT bound to {session, principal, client, risk}. High-risk Actions later require
  `X-Reauth-Token`.
- **Root**: `POST /api/v1/admin/auth/root/login` → local `root_credentials` (bcrypt), NOT in `users` table.
  First-boot random password generated by CLI path (`ppb-server root init` prints it once); forced password
  change on first login. `ppctl root reset-password` remains a code path only (out of repo scope).
- **GitHub OAuth**: `GET /auth/github/start` (requires authenticated session) → state token → GitHub →
  `GET /auth/github/callback` (fixed `https://api-phira.htadiy.com/api/v1/auth/github/callback`) →
  **binds to the existing user captured in `state`**. Never creates bare accounts. `POST /auth/github/unbind`.
- **return_to** validated against an exact allowlist (PPF/Panel public URLs) → no open redirect.
- **CORS**: credentialed, exact-origin allowlist (PPF, Panel, dev origins from config). No `*` + credentials.
- **CSRF**: double-submit — state-changing cookie-auth requests require `X-CSRF-Token` == `ppb_csrf` cookie.
  Bearer (Tauri) requests skip CSRF.

---

## 6. Permissions (design §8)

- Bootstrap groups: Administrators (`system_kind=admin_scope`, protected), Moderators, Developers,
  Members (is_default, protected).
- `PermissionResolver`: User → Groups → Permissions (V1 path, no direct per-user override).
  `admin_scope` auto-maps **all** `root_only=false` permissions (including newly added ones).
- Manifest endpoint `GET /api/v1/admin/permissions/manifest` returns `{id, group, label, description,
  root_only, risk}` (Panel renders grouped, no hardcoded full set in frontend).
- `*:*` rejected at API and DB (CHECK constraint) for any non-root group. Non-root users must belong to ≥1 group.
- Default group switchable; current default group cannot be deleted.
- Seed manifest covers the contract §5 integration set: `room:{view,kick,move,start,config,whitelist,blacklist,manage}`,
  `user:{view,kick,ban,ban_ip,view_ip_history}`, `server:{view,manage,update,shutdown,start}`,
  `config:{view,reload,rollback}`, `plugin:{view,manage,call}`, `audit:{view,export}`,
  `broadcast:{all,room,user}`, `pmp:cli`, `notification:send_system`, `coupon:{view,create,manage,revoke}`,
  `group:{view,create,edit,delete,assign_user}`, `dashboard:view`, `preference:manage`.

---

## 7. Action Registry + Command Broker (design §9)

- `ActionDescriptor { id, permission, executor: openuds|cli.execute|internal|cli.raw, risk,
  audit, reauth, host_allowed, queue_key, args }`.
- Seed actions:
  - `room.kick` (executor openuds, permission room:kick, host_allowed, queue_key `room:{room_id}`)
  - `room.set_chart` (openuds, room:config, host_allowed, queue_key `room:{room_id}`)
  - `broadcast.all` (openuds, broadcast:all, queue_key `server`)
  - `pmp.cli.execute` (cli.raw, pmp:cli, risk high, audit always)
  - `pmp.update.apply` (cli.execute, server:update, reauth, long_running, queue_key `server`)
- Command Broker: per-`queue_key` serial execution; records `command_runs` (queued → running → succeeded/failed).
- `host_allowed` actions re-check real host at execution time (resource policy), never trusting client flags.

---

## 8. OpenUDS client (design §11)

- Frame: 4-byte LE length + UTF-8 JSON, max 16 MiB (mirrors PMP `protocol.rs`).
- Auth: token mode `{"type":"authenticate","token":...}` → `{"type":"authenticated","session_id","server_version"}`
  OR approve mode `{"type":"authenticate","client_name":...}` → `auth_pending` → poll for approval (TTL 120s).
- Typed command: `{"type":"command","command":C,"params":P,"id":ID}` → `{"type":"response","id","ok":true,"data"}`
  / `{"ok":false,"error":{"code","message"}}` (codes MISSING_COMMAND/UNKNOWN_COMMAND/COMMAND_ERROR).
- `subscribe` (wildcard `room.*` supported), `unsubscribe`, `subscribe_stream` (touches/judges/logs).
- Event frame → parsed `EventFrame`; stream frame → parsed `StreamFrame` (sequence/room/round/timestamp).
- Reconnect/backoff: exponential + jitter, independent read/command tasks; pending-command map fails pending
  commands on disconnect.
- Capabilities: `server_version → capabilities[]` map (PMP 1.0.38 → `persist.touches, persist.judges,
  room.chat_send, stream.touches, stream.judges`). Missing → `CAPABILITY_NOT_SUPPORTED`.

---

## 9. SSE + error contract

- `GET /api/v1/events`, `GET /api/v1/admin/events` → SSE with envelope
  `{id, type, version, occurred_at, resource:{type,id}, data}`; heartbeat `server.heartbeat`; `Last-Event-ID`
  replay from bounded ring buffer; fallback snapshot+realtime.
- PMP event mapping: `user.online/offline`, `room.created/updated/joined/left`, `round.started/completed`,
  `server.heartbeat`. No `broadcast.room` masquerading as player chat.
- Error contract: `{"error":{"code","message","request_id","details"}}`; codes upper-snake:
  `REQUEST_ID/PAGINATION/VALIDATION/RATE_LIMIT/AUTH/SESSION/PERMISSION_DENIED/PMP_UNAVAILABLE/
  CAPABILITY_NOT_SUPPORTED/PHIRA_API_UNAVAILABLE/PHIRA_REAUTH_REQUIRED/LONG_JOB_ACCEPTED`.
- Pagination: request `page` (1-based), `pageNum` (per-page, ≤100); response `{items, total, page, pageNum}`.

---

## 10. CI (design §26.2, PMP-style)

- `build.yml`: push/PR to main → `auto-patch` (patch+1 on main push, `cargo generate-lockfile`, commit
  `chore: sync version + lockfile [skip ci]`, `contents:write` only here) → `check`
  (`cargo check --locked --workspace --all-targets --all-features` + `cargo test --locked --workspace
  --all-features` + `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`) →
  `build` (linux-musl x86_64).
- `bump-version.yml`: manual minor/major; C = commits since last `v*.*.*` tag; tags `vA.B.C`.
- `release.yml`: on `v*` tag → quality-gate → build matrix → smoke test → provenance (`attest-build-provenance`)
  + CycloneDX SBOM (best-effort) → `softprops/action-gh-release`.
- `[profile.release]` lto=true, codegen-units=1, strip=symbols.

---

## 11. Contract issues / risks / proposals (for Main to decide)

- **P1 (pagination naming)** — Contract says request `page,pageNum(≤100)`; `pageNum` is the *per-page size*
  (Phira API semantics), not a page number. PROPOSE: contracts README explicitly documents `page` = 1-based
  page index and `pageNum` = page size (≤100), matching response `{items,total,page,pageNum}`.
- **P2 (cookie SameSite)** — PPF/Panel are same-site but cross-origin with PPB. PROPOSE: freeze cookie policy
  as Secure + HttpOnly + SameSite=Lax, host-only domain `api-phira.htadiy.com` (design §27.3 said "按实际跨子域
  行为验证"; Lax is the verified correct choice for same-site cross-origin credentialed fetch).
- **P3 (two extra tables)** — `phira_credentials` (encrypted refresh token, design §24.2 "Phira credential
  state") and `root_credentials` (Root local principal, §6.8) are required for Auth but not in the Phase A
  migration list. PROPOSE: confirm these additions.
- **P4 (error code casing)** — Contract lists lower-case codes but the canonical example uses upper-snake
  (`PHIRA_REAUTH_REQUIRED`). PROPOSE: standardize all codes upper-snake for consistency with the example.
- **P5 (reauth transport)** — Design fixes reauth_context TTL/binding but not transport. PROPOSE: reauth_context
  is a separate short-lived JWT presented via `X-Reauth-Token` header (not the access cookie), so high-risk
  Actions can require it without disturbing the browser cookie.
- **P6 (capability set)** — Verified PMP 1.0.38 set is `persist.touches, persist.judges, room.chat_send,
  stream.touches, stream.judges`, which matches the frozen meta example exactly. No change; PPB keeps the
  version→capability map and capability-detection (never hardcodes version in business branches).
- **Risk (unverified compile)** — aarch64 toolchain broken locally; code+tests are unverified pending CI.
  CI is the only gate. Dependency APIs chosen conservatively (axum 0.8, sqlx 0.8 runtime queries, no `query!`).
- **Risk (no local Cargo.lock)** — CI auto-patch generates/commits the lockfile on first main push; PRs whose
  dependencies changed must let auto-patch re-sync the lockfile (mirrors PMP).

---

## 12. Test plan (Phase A)

- phira login success/failure (mocked Phira API), token refresh, refresh-expiry → reauth_required
- Root separate from ordinary users; Root not in `users` table
- admin_scope auto-permission (new permission auto-visible)
- group cannot receive `*:*` (API + DB CHECK)
- OpenUDS envelope parse + reconnect/backoff (mock transport)
- error contract shape (request_id present, correct code)
- JWT claims round-trip

Verification: CI `cargo test` + `cargo clippy -D warnings` on x86_64 GitHub runners.

---

## 13. CI verification addendum (2026-08-12)

First full-green Build run: `31529813211` (commit `d2e49a9`) — **success**.
`auto-patch` ✓, `check` (`cargo check --locked --workspace --all-targets --all-features` +
`cargo test --locked --workspace --all-features` + `cargo clippy --locked --workspace
--all-targets --all-features -- -D warnings`) ✓, `build` (linux-musl x86_64) ✓.

Resolved during verification (dependency/API facts worth recording):

- **cookie 0.18**: `Cookie::build` takes **one** arg (name) and `CookieBuilder` has **no**
  `.value()` method. PPB constructs `Set-Cookie` strings manually (`middleware/cookies.rs`).
  `cookie::Duration` is private → use `cookie::time::Duration`.
- **jsonwebtoken 9**: `Validation` has **no** `validate_sub` field (removed).
- **tower-http 0.6**: `tower_http::ServiceBuilderExt` is **not** exported under the enabled
  feature set → use explicit `SetRequestIdLayer` / `PropagateRequestIdLayer` from
  `tower_http::request_id`. `RequestId` (extension) does not deref to `HeaderValue`; PPB reads
  the `X-Request-Id` header directly.
- **futures `StreamExt::filter_map`** requires a future-returning closure
  (`|r| std::future::ready(r.ok())`), not a sync fn.
- **sqlx `query_as` without turbofish** can infer `Option<tuple>` as the row type when the
  binding is annotated `Option<tuple>` and a chained `?`/`.ok_or_else()` is used → always use
  explicit `query_as::<_, (…)>` for tuple rows.
- **`reqwest 0.12` + `rustls-tls` (aws-lc-rs) builds cleanly for `x86_64-unknown-linux-musl`**
  on GitHub runners (musl-gcc from `musl-tools`).
- **AES-256-GCM** ciphertext blob = `nonce(12) || ct(tag appended)` — length assertions must
  account for the 16-byte GCM tag.
