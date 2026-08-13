//! OpenAPI document (contract §21: PPB OpenAPI is the HTTP Source of Truth).
//!
//! Served at `GET /api/v1/openapi.json` and dumped via `ppb-server --openapi`.
//! `contracts/types.ts` is generated from this JSON (snake_case, §20) and is
//! consumed by PPF/Panel instead of hand-written duplicate types.

use serde::Serialize;
use utoipa::{OpenApi, ToSchema};

use crate::admin::coupons::CreateCouponBody;
use crate::admin::notifications::SendBody;
use crate::admin::plugins::PluginCallBody;
use crate::admin::server::{BroadcastBody, RoomCreationBody, ServerActionBody};
use crate::auth::routes::{ChangePasswordRequest, PhiraLoginRequest, ReauthRequest, RootLoginRequest};
use crate::actions::routes::{ExecuteActionBody, ExecuteCommandBody};
use crate::app::{JoinIntentBody, PreferencesListResponse, PushEndpointBody};
use crate::automation::routes::CreateRunbookBody;
use crate::automation::{RunbookDefinition, RunbookStep};
use crate::config::pmp::{ConfigFieldDescriptor, ConfigFieldGroup};
use crate::config::repo::ConfigSnapshot;
use crate::config::routes::{
    ConfigDescriptorsResponse, ConfigDiffChange, ConfigDiffResponse, ConfigRawResponse,
    ConfigRollbackResponse, ConfigSaveResponse, ConfigSnapshotsResponse, ConfigValidateResponse,
    ConfigValidationError, ConfigValuesBody, ConfigValuesResponse, RollbackBody,
};
use crate::error::{ErrorBody, ErrorEnvelope};
use crate::jobs::routes::{CreateJobBody, JobListResponse};
use crate::jobs::Job;
use crate::audit::model::AuditEvent;
use crate::audit::routes::AuditListResponse;
use crate::preferences::UserPreference;
use crate::logs::routes::{LogInputBody, TranslateParams, TranslateResponse};
use crate::logs::translator::TranslatedError;
use crate::notifications::push::SubscriptionWire;
use crate::notifications::routes::{
    ActionBody as NotificationActionBody, AppNotificationWire, InputBody,
    NotificationInboxResponse,
};
use crate::permissions::groups::{Group, GroupListItem, GroupListResponse};
use crate::permissions::manifest::{PermissionDef, Risk as PermissionRisk};
use crate::permissions::repo::GroupMember;
use crate::permissions::routes::{
    CreateGroupBody, PatchGroupBody, ReplaceMembersBody, ReplacePermissionsBody,
};
use crate::preferences::routes::UpdatePreferencesBody;
use crate::social::routes::SendRequestBody;
use crate::rooms::routes::{
    AdminRoomActionBody, ChatSendBody, CreateRoomBody, RoomActionBody2, RoomBatchBody,
};
use crate::users::model::{
    AdminUserItem, GroupRef, SessionItem, UserDetailResponse, UserListResponse,
    UserMultiplayerResponse, UserSecurityResponse, UserSessionsResponse,
};
use crate::users::routes::UserActionBody;
use crate::admin::routes::{PmpStatus, ServerStatusResponse};
use crate::admin::server::ServerStatsResponse;

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
        crate::rooms::routes::room_chat_history,
        crate::rooms::routes::send_chat,
        crate::rooms::routes::room_action_body,
        crate::replay::routes::detail,
        crate::replay::routes::manifest,
        crate::replay::routes::resolve_share,
        crate::admin::notifications::send,
        crate::admin::notifications::delivery,
        crate::admin::coupons::list,
        crate::admin::coupons::create,
        crate::admin::coupons::revoke,
        crate::audit::routes::list,
        crate::audit::routes::detail,
        crate::audit::routes::export,
        crate::audit::routes::export_csv,
        crate::jobs::routes::list,
        crate::jobs::routes::create,
        crate::jobs::routes::get_job,
        crate::jobs::routes::cancel,
        crate::config::routes::descriptors,
        crate::config::routes::values,
        crate::config::routes::validate,
        crate::config::routes::diff,
        crate::config::routes::save,
        crate::config::routes::snapshots,
        crate::config::routes::raw,
        crate::config::routes::rollback,
        crate::config::routes::ppb_config,
        crate::config::routes::ppf_config,
        crate::logs::routes::history,
        crate::logs::routes::submit_input,
        crate::logs::routes::translate_endpoint,
        crate::logs::routes::translate_post,
        crate::admin::plugins::list,
        crate::admin::plugins::info,
        crate::admin::plugins::enable,
        crate::admin::plugins::disable,
        crate::admin::plugins::reload,
        crate::admin::plugins::remove,
        crate::admin::plugins::action_dispatch,
        crate::admin::plugins::call,
        crate::admin::server::server_stats,
        crate::admin::server::runtime_status,
        crate::admin::server::config_reload,
        crate::admin::server::server_actions,
        crate::admin::server::room_creation,
        crate::admin::server::shutdown,
        crate::admin::server::broadcast_all,
        crate::admin::server::broadcast_room,
        crate::admin::server::broadcast_user,
        crate::actions::routes::list_actions,
        crate::actions::routes::execute_action,
        crate::actions::routes::list_commands,
        crate::actions::routes::execute_command,
        crate::automation::routes::list,
        crate::automation::routes::create,
        crate::automation::routes::get_one,
        crate::automation::routes::update,
        crate::automation::routes::delete_runbook,
        crate::automation::routes::run,
        crate::automation::routes::runs,
        crate::auth::routes::root_login,
        crate::auth::routes::root_session,
        crate::auth::routes::root_change_password,
        crate::permissions::routes::manifest,
        crate::permissions::routes::list,
        crate::permissions::routes::create,
        crate::permissions::routes::detail,
        crate::permissions::routes::patch_group,
        crate::permissions::routes::delete_group,
        crate::permissions::routes::replace_members,
        crate::permissions::routes::replace_permissions,
        crate::users::routes::list_users,
        crate::users::routes::user_detail,
        crate::users::routes::user_multiplayer,
        crate::users::routes::user_sessions,
        crate::users::routes::user_security,
        crate::users::routes::user_audit,
        crate::users::routes::user_actions,
        crate::rooms::routes::admin_list_rooms,
        crate::rooms::routes::admin_create_room,
        crate::rooms::routes::admin_room_info,
        crate::rooms::routes::admin_close_room,
        crate::rooms::routes::admin_room_action,
        crate::rooms::routes::admin_room_actions_batch,
        crate::app::me_join_intent_create,
        crate::app::me_join_intent_cancel,
        crate::app::me_push_endpoints,
        crate::app::me_push_endpoint_register,
        crate::app::me_push_endpoint_delete,
        crate::phira::routes::chart_list,
        crate::phira::routes::chart_popular,
        crate::phira::routes::chart_detail,
        crate::phira::routes::chart_preview,
        crate::phira::routes::chart_viewer,
        crate::phira::routes::chart_records,
        crate::phira::routes::chart_top,
        crate::phira::routes::records_by_player,
        crate::phira::routes::records_query,
        crate::phira::routes::records_list15,
        crate::phira::routes::records_pool,
        crate::phira::routes::record_detail,
        crate::phira::routes::users_search,
        crate::phira::routes::user_detail,
        crate::phira::routes::user_stats,
        crate::public::routes::meta,
        crate::public::routes::site,
        crate::public::routes::announcements,
        crate::public::routes::downloads,
        crate::public::routes::nodes,
        crate::social::routes::list,
        crate::social::routes::list_requests,
        crate::social::routes::send_request,
        crate::social::routes::respond_accept,
        crate::social::routes::respond_reject,
        crate::social::routes::block,
        crate::notifications::routes::list,
        crate::notifications::routes::read,
        crate::notifications::routes::dismiss,
        crate::notifications::routes::action,
        crate::notifications::routes::input,
        crate::preferences::routes::get_one,
        crate::preferences::routes::update,
        crate::preferences::routes::delete_one,
        crate::app::me_identities,
        crate::jobs::routes::retry_job,
        crate::jobs::routes::list_tasks,
        crate::jobs::routes::complete_task,
        crate::automation::routes::get_run,
        crate::automation::routes::cancel_run,
        crate::admin::routes::server_status,
    ),
    components(
        schemas(
            ErrorEnvelope,
            ErrorBody,
            MeResponse,
            PaginationResponse,
            ReplayManifest,
            ReplayDetail,
            RoomActionRequest,
            PhiraLoginRequest,
            ReauthRequest,
            ChatSendBody,
            RoomActionBody2,
            SendBody,
            CreateCouponBody,
            CreateJobBody,
            ConfigValuesBody,
            ConfigFieldDescriptor,
            ConfigFieldGroup,
            ConfigSnapshot,
            RollbackBody,
            ConfigDescriptorsResponse,
            ConfigValuesResponse,
            ConfigValidationError,
            ConfigValidateResponse,
            ConfigDiffChange,
            ConfigDiffResponse,
            ConfigSaveResponse,
            ConfigSnapshotsResponse,
            ConfigRawResponse,
            ConfigRollbackResponse,
            Job,
            JobListResponse,
            AuditEvent,
            AuditListResponse,
            UserPreference,
            LogInputBody,
            TranslateParams,
            TranslateResponse,
            TranslatedError,
            PluginCallBody,
            ServerActionBody,
            RoomCreationBody,
            BroadcastBody,
            ExecuteActionBody,
            ExecuteCommandBody,
            CreateRunbookBody,
            RunbookDefinition,
            RunbookStep,
            RootLoginRequest,
            ChangePasswordRequest,
            CreateGroupBody,
            PatchGroupBody,
            ReplaceMembersBody,
            ReplacePermissionsBody,
            Group,
            GroupListItem,
            GroupListResponse,
            GroupMember,
            PermissionDef,
            PermissionRisk,
            UserActionBody,
            AdminUserItem,
            GroupRef,
            SessionItem,
            UserListResponse,
            UserDetailResponse,
            UserMultiplayerResponse,
            UserSessionsResponse,
            UserSecurityResponse,
            CreateRoomBody,
            AdminRoomActionBody,
            RoomBatchBody,
            JoinIntentBody,
            PushEndpointBody,
            PreferencesListResponse,
            SubscriptionWire,
            PmpStatus,
            ServerStatusResponse,
            ServerStatsResponse,
            SendRequestBody,
            NotificationActionBody,
            InputBody,
            AppNotificationWire,
            NotificationInboxResponse,
            UpdatePreferencesBody,
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

    /// Every operationId must be globally unique — duplicate ids make
    /// openapi-typescript emit duplicate `operations` identifiers (invalid TS).
    #[test]
    fn operation_ids_are_globally_unique() {
        let doc: serde_json::Value =
            serde_json::from_str(&build_openapi_json()).expect("openapi is valid json");
        let mut seen = std::collections::HashSet::new();
        let mut missing = 0usize;
        if let Some(paths) = doc.get("paths").and_then(|p| p.as_object()) {
            for (_path, item) in paths {
                if let Some(methods) = item.as_object() {
                    for method in ["get", "post", "put", "patch", "delete"] {
                        if let Some(op) = methods.get(method) {
                            match op.get("operationId").and_then(|v| v.as_str()) {
                                Some(opid) => assert!(
                                    seen.insert(opid.to_string()),
                                    "duplicate operationId {opid}"
                                ),
                                None => missing += 1,
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(missing, 0, "every operation must declare a unique operationId");
    }
}
