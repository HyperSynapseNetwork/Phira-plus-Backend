//! OpenAPI document (contract §21: PPB OpenAPI is the HTTP Source of Truth).
//!
//! Served at `GET /api/v1/openapi.json` and dumped via `ppb-server --openapi`.
//! `contracts/types.ts` is generated from this JSON (snake_case, §20) and is
//! consumed by PPF/Panel instead of hand-written duplicate types.

use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

/// `GET /api/v1/me` session-probe response (S-4).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct MeResponse {
    pub principal: serde_json::Value,
    pub user: Option<serde_json::Value>,
    pub permissions: Vec<String>,
    pub capabilities: Vec<String>,
    pub session: serde_json::Value,
    pub csrf_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identities: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phira_credential: Option<serde_json::Value>,
}

/// Standard paginated response `{items, total, page, pageNum}`.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PaginationResponse {
    pub items: Vec<serde_json::Value>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
}

/// Replay manifest summary (per `(round_uuid, player_phira_id)` pair).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ReplayManifest {
    pub round_uuid: String,
    pub player_phira_id: i64,
    pub touches: serde_json::Value,
    pub judges: serde_json::Value,
}

/// Replay detail (summary + visibility).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct ReplayDetail {
    pub round_uuid: String,
    pub player_phira_id: i64,
    pub visibility: String,
    pub touches: serde_json::Value,
    pub judges: serde_json::Value,
}

/// Room action request `{action, args}`.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct RoomActionRequest {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
}

/// The generated OpenAPI document (paths + components).
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "Phira+ Backend API",
        version = "0.1",
        description = "PPB — identity / community / control / integration plane. Contract §20: snake_case JSON."
    ),
    paths(
        crate::auth::routes::phira_login,
        crate::auth::routes::phira_reauth,
        crate::auth::routes::refresh,
        crate::auth::routes::logout,
        crate::app::me,
        crate::app::me_profile,
        crate::app::me_preferences,
        crate::app::me_join_intents,
        crate::app::me_join_intent_get,
        crate::app::friend_remove,
        crate::rooms::routes::list_rooms,
        crate::rooms::routes::room_info,
        crate::rooms::routes::room_history,
        crate::rooms::routes::send_chat,
        crate::rooms::routes::room_action_body,
        crate::replay::routes::detail,
        crate::replay::routes::manifest,
        crate::replay::routes::resolve_share,
        crate::admin::notifications::send,
    ),
    components(
        schemas(
            crate::error::ErrorEnvelope,
            crate::error::ErrorBody,
            MeResponse,
            PaginationResponse,
            ReplayManifest,
            ReplayDetail,
            RoomActionRequest,
        )
    ),
    tags(
        (name = "auth", description = "Authentication / reauth / session"),
        (name = "me", description = "Session probe / profile / preferences / join intents"),
        (name = "friends", description = "Friends"),
        (name = "rooms", description = "Rooms (view / chat / actions / history)"),
        (name = "replays", description = "Replays ((round_uuid, player_phira_id) identity)"),
        (name = "notifications", description = "Notifications"),
    )
)]
pub struct ApiDoc;

/// Build the OpenAPI JSON document.
pub fn build_openapi_json() -> String {
    serde_json::to_string_pretty(&ApiDoc::openapi()).unwrap_or_else(|_| "{}".to_string())
}

/// Minimal schema used by tests to assert the doc covers the core contract.
pub fn openapi_paths() -> Vec<String> {
    ApiDoc::openapi()
        .paths
        .paths
        .keys()
        .cloned()
        .collect()
}

/// Test helper: assert core paths exist in the generated OpenAPI.
#[allow(dead_code)]
pub fn assert_core_paths(paths: &[String]) {
    for required in [
        "/api/v1/auth/phira/login",
        "/api/v1/me",
        "/api/v1/rooms",
        "/api/v1/rooms/{room_id}/actions",
        "/api/v1/replays/{round_uuid}/manifest",
        "/api/v1/friends/{phira_id}/remove",
        "/api/v1/me/join-intents",
    ] {
        assert!(paths.iter().any(|p| p == required), "missing OpenAPI path {required}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_covers_core_contract_paths() {
        let paths = openapi_paths();
        assert_core_paths(&paths);
        // Error envelope schema is registered.
        let schemas = ApiDoc::openapi().components.as_ref().map(|c| c.schemas.len()).unwrap_or(0);
        assert!(schemas >= 4, "expected core schemas, got {schemas}");
    }
}
