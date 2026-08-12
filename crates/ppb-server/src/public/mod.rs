//! Public content + capabilities.

pub mod routes;

/// PPB capability list (single source for `/api/v1/public/meta` and the
/// `/api/v1/me` session probe). Add new capability flags here.
pub const PPB_CAPABILITIES: &[&str] = &[
    "rooms.v1",
    "replay.persist.v1",
    "room.chat.v1",
    "notifications.actions.v1",
    "rooms.admin.v1",
    "users.v1",
    "users.admin.v1",
    "groups.admin.v1",
    "config.manage.v1",
    "audit.v1",
    "logs.v1",
    "jobs.v1",
    "commands.v1",
    "charts.v1",
    "records.v1",
    "server.admin.v1",
    "plugins.admin.v1",
    "broadcast.v1",
    "pmp.console.v1",
    "phira-data.v1",
    "aggregator.v1",
];
