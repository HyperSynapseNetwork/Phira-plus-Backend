# Phira+ Backend (PPB)

Phira+ V3 unified backend — identity / community / control / integration plane.

- **Stack**: Rust + axum + Tokio + PostgreSQL (sqlx) + OpenUDS client + REST/SSE + Live WS.
- **Docs**: implementation plan in `docs/PHASE_A_PLAN.md`; cross-repo contracts live in
  `../contracts/README.md` (Contract-Freeze v0).
- **Phase**: A (contract freeze + foundation). CI is the only verification gate
  (local aarch64 toolchain is broken — do not run `cargo` locally).

## Layout

```
crates/ppb-server/src/   domain-vertical modules (auth, permissions, pmp/openuds, …)
migrations/              sqlx migrations
config/example.toml      runtime config example (no secrets)
config/example.env       deployment env reference (no real values)
.github/workflows/       build / bump-version / release
```

## Principles (hard rules)

- PMP owns multiplayer truth; Phira API owns Phira-world data; PPB owns identity/policy/control.
- No direct access to PMP PostgreSQL. No Replay content/files in PPB.
- `*:*` only for Root; Administrators use `admin_scope`; no direct per-user permission override.
- Phira password never persisted or logged; Phira tokens never reach frontends.
- `cli.execute` is a first-class capability routed through the Action Registry.
