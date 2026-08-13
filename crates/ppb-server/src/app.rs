//! Application state, router assembly, and background tasks.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue};
use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::actions::executor::PmpActionExecutor;
use crate::actions::registry::ActionRegistry;
use crate::audit;
use crate::auth::github::GithubService;
use crate::auth::root::RootAuthService;
use crate::auth::types::AuthPrincipal;
use crate::commands::broker::CommandBroker;
use crate::config::deployment::Secrets;
use crate::config::RuntimeConfig;
use crate::jobs::registry::JobRegistry;
use crate::jobs::runner::JobRunner;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};
#[allow(unused_imports)]
use crate::openapi::MeResponse;
use crate::identities::repo as identities_repo;
use crate::join_intent::JoinIntentStore;
use crate::middleware::csrf;
use crate::middleware::rate_limit::RateLimiter;
use crate::notifications::push::{PushService, SubscriptionWire};
use crate::permissions::resolver::PermissionResolver;
use crate::preferences::UserPreference;
use crate::phira::client::{PhiraApi, PhiraClient};
use crate::phira::credential::CredentialCipher;
use crate::phira::aggregator::Aggregator;
use crate::phira::gateway::PhiraGateway;
use crate::pmp::events::{EventBus, PpbEvent, ResourceRef};
use crate::pmp::openuds::client::{OpenUdsClient, OpenUdsConfig};
use crate::rooms::service::RoomService;
use crate::users::service::PlayerService;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RuntimeConfig>,
    pub secrets: Arc<Secrets>,
    pub db: Option<sqlx::PgPool>,
    pub phira: Arc<dyn PhiraApi>,
    pub credential_cipher: CredentialCipher,
    pub openuds: Arc<OpenUdsClient>,
    pub permissions: Arc<PermissionResolver>,
    pub actions: Arc<ActionRegistry>,
    pub commands: CommandBroker,
    pub events: EventBus,
    pub github: Arc<GithubService>,
    pub metrics: Arc<crate::metrics::Metrics>,
    pub rate_limiter: RateLimiter,
    pub heartbeat: HeartbeatCache,
    pub rooms: RoomService,
    pub player: PlayerService,
    pub jobs: JobRunner,
    pub job_registry: Arc<JobRegistry>,
    pub join_intents: JoinIntentStore,
    pub push: Arc<PushService>,
    pub phira_gateway: Arc<PhiraGateway>,
    /// Privacy-friendly aggregate visit counter (P-86); resets on restart, adds
    /// to the `site.visit_count` config baseline.
    pub visit_counter: std::sync::Arc<std::sync::atomic::AtomicI64>,
}

impl AppState {
    /// Get the DB pool or an internal error.
    pub fn require_db(&self) -> Result<&sqlx::PgPool, ApiError> {
        self.db
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "database not configured"))
    }
}

/// Last-known PMP heartbeat counts (users/rooms/sessions) for the semantic
/// `server.heartbeat` event even when PMP is briefly unreachable.
#[derive(Clone, Default)]
pub struct HeartbeatCache {
    inner: Arc<Mutex<(i64, i64, i64)>>,
}

impl HeartbeatCache {
    pub fn update(&self, users: i64, rooms: i64, sessions: i64) {
        *self.inner.lock().unwrap() = (users, rooms, sessions);
    }
    pub fn get(&self) -> (i64, i64, i64) {
        *self.inner.lock().unwrap()
    }
}

/// Build full application state (DB, bootstrap, background tasks).
pub async fn build_state(
    runtime: RuntimeConfig,
    secrets: Secrets,
) -> Result<Arc<AppState>, anyhow::Error> {
    let mut runtime = runtime;
    let db = match &secrets.database_url {
        Some(url) => {
            let pool = PgPoolOptions::new()
                .max_connections(10)
                .connect(url)
                .await
                .map_err(|e| anyhow::anyhow!("database connect failed: {e}"))?;
            sqlx::migrate!("../../migrations")
                .run(&pool)
                .await
                .map_err(|e| anyhow::anyhow!("database migrate failed: {e}"))?;
            // Merge persisted runtime overrides over the boot-time TOML config.
            if let Some(over) = crate::config::repo::get_overrides(&pool).await
                .map_err(|e| anyhow::anyhow!("config overrides read failed: {e}"))?
            {
                runtime = runtime
                    .apply_overrides(&over)
                    .map_err(|e| anyhow::anyhow!("config overrides invalid: {e}"))?;
            }
            Some(pool)
        }
        None => {
            tracing::warn!("PPB_DATABASE_URL not set — running without persistence");
            None
        }
    };

    let phira_client = PhiraClient::new(&runtime.phira.base_url, runtime.phira.timeout_ms)
        .map_err(|e| anyhow::anyhow!("phira client: {e}"))?;
    let credential_cipher = CredentialCipher::new(&secrets.phira_credential_key)
        .map_err(|e| anyhow::anyhow!("credential cipher: {e}"))?;

    let openuds_config = OpenUdsConfig::from_runtime(&runtime.pmp, secrets.pmp_openuds_token.clone());
    let openuds = Arc::new(OpenUdsClient::new(openuds_config));

    let permissions = Arc::new(PermissionResolver::new());
    let actions = Arc::new(ActionRegistry::new());
    let executor = Arc::new(PmpActionExecutor::new(
        Arc::clone(&openuds),
        Arc::clone(&actions),
        db.clone(),
    ));
    let commands = CommandBroker::new(executor);

    let events = EventBus::new(1024, 256);
    let github = Arc::new(GithubService::new(runtime.phira.timeout_ms));
    let metrics = crate::metrics::Metrics::new(); // returns Arc<Metrics>
    let rooms = RoomService::new(Arc::clone(&openuds));
    let player = PlayerService::new(Arc::clone(&openuds));
    let job_registry = Arc::new(JobRegistry::new());
    let jobs = JobRunner::new(
        db.clone(),
        events.clone(),
        Arc::clone(&openuds),
        Arc::clone(&job_registry),
    );
    let join_intents = JoinIntentStore::new();
    let notifications_config = runtime.notifications.clone();
    let push = Arc::new(PushService::new(&notifications_config, credential_cipher.clone()));
    let phira_gateway = Arc::new(PhiraGateway::new(
        &runtime.phira.base_url,
        runtime.phira.timeout_ms,
        runtime.phira.gateway_ttl_secs,
        runtime.phira.gateway_rate_per_minute,
    )?);
    let aggregator_enabled = runtime.phira.aggregator_enabled;
    let aggregator_interval_hours = runtime.phira.aggregator_interval_hours;
    let aggregator_top_n = runtime.phira.aggregator_top_n;

    let state = Arc::new(AppState {
        config: Arc::new(runtime),
        secrets: Arc::new(secrets),
        db,
        phira: Arc::new(phira_client),
        credential_cipher,
        openuds,
        permissions,
        actions,
        commands,
        events,
        github,
        metrics,
        rate_limiter: RateLimiter::new(),
        heartbeat: HeartbeatCache::default(),
        rooms,
        player,
        jobs,
        job_registry,
        join_intents,
        push,
        phira_gateway,
        visit_counter: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
    });

    if let Some(db) = &state.db {
        crate::permissions::routes::run_bootstrap(db).await?;
        if let Some(password) = RootAuthService::bootstrap(db).await? {
            // Printed once to the CLI path — never logged via tracing.
            eprintln!("[ppb] Root first-boot password (print once): {password}");
        }
    }

    state.openuds.start();
    spawn_pmp_event_forwarder(&state);
    spawn_heartbeat_task(&state);
    spawn_audit_purge_task(&state);
    spawn_join_intent_task(&state);

    if aggregator_enabled {
        let aggregator = Arc::new(Aggregator::new(
            state.db.clone(),
            Arc::clone(&state.phira_gateway),
            aggregator_top_n,
        ));
        aggregator.spawn(aggregator_interval_hours);
    }

    Ok(state)
}

/// Forward PMP OpenUDS events into the PPB EventBus (with envelope mapping).
fn spawn_pmp_event_forwarder(state: &Arc<AppState>) {
    let mut rx = state.openuds.subscribe_events();
    let events = state.events.clone();
    let heartbeat = state.heartbeat.clone();
    let metrics = Arc::clone(&state.metrics);
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    if frame.event_type == "server.heartbeat" {
                        let users = frame.data.get("users").and_then(serde_json::Value::as_i64).unwrap_or(0);
                        let rooms = frame.data.get("rooms").and_then(serde_json::Value::as_i64).unwrap_or(0);
                        let sessions = frame.data.get("sessions").and_then(serde_json::Value::as_i64).unwrap_or(0);
                        heartbeat.update(users, rooms, sessions);
                    }
                    if let Some(ppb) = crate::pmp::events::map_pmp_event(&frame) {
                        events.publish(ppb);
                        metrics.events_forwarded.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Watch PMP `user.online` and fulfil JoinIntents via `room.force_move`; also
/// periodically purge expired intents (design §14.6).
fn spawn_join_intent_task(state: &Arc<AppState>) {
    let mut rx = state.openuds.subscribe_events();
    let intents = state.join_intents.clone();
    let rooms = state.rooms.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    if frame.event_type == "user.online" {
                        let phira_id = frame
                            .data
                            .get("user_id")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0);
                        if phira_id > 0 {
                            if let Some(intent) = intents.match_online(phira_id) {
                                let intent_id = intent.id;
                                let room_id = intent.room_id.clone();
                                tracing::info!(
                                    phira_id,
                                    room = %room_id,
                                    "join intent fulfilled: force_move"
                                );
                                intents.mark_moving(&intent_id);
                                let ok = rooms
                                    .force_move(&room_id, phira_id as i32, false)
                                    .await
                                    .is_ok();
                                intents.mark_terminal(
                                    &intent_id,
                                    if ok {
                                        crate::join_intent::STATUS_COMPLETED
                                    } else {
                                        crate::join_intent::STATUS_FAILED
                                    },
                                );
                            }
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    let intents = state.join_intents.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let n = intents.cleanup_expired();
            if n > 0 {
                tracing::debug!(cleaned = n, "join intent cleanup");
            }
        }
    });
}

/// Publish a semantic `server.heartbeat` every 15s (last known counts).
fn spawn_heartbeat_task(state: &Arc<AppState>) {
    let events = state.events.clone();
    let heartbeat = state.heartbeat.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        loop {
            interval.tick().await;
            let (users, rooms, sessions) = heartbeat.get();
            events.publish(PpbEvent {
                id: Uuid::new_v4().to_string(),
                event_type: "server.heartbeat".to_string(),
                version: 1,
                occurred_at: chrono::Utc::now(),
                resource: ResourceRef::server(),
                data: json!({ "users": users, "rooms": rooms, "sessions": sessions }),
            });
        }
    });
}

/// Daily purge of audit events beyond retention.
fn spawn_audit_purge_task(state: &Arc<AppState>) {
    if let Some(db) = state.db.clone() {
        let retention_days = state.config.audit.retention_days;
        tokio::spawn(async move {
            // tokio interval's first tick is immediate → purge once at startup,
            // then daily.
            let mut interval = tokio::time::interval(Duration::from_secs(24 * 3600));
            loop {
                interval.tick().await;
                match audit::repo::purge_older_than(&db, retention_days).await {
                    Ok(n) if n > 0 => tracing::info!(deleted = n, "audit purge"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "audit purge failed"),
                }
            }
        });
    }
}

/// Build the full HTTP router.
pub fn build_router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .nest("/public", crate::public::routes::routes())
        .nest("/auth", crate::auth::routes::routes())
        .nest("/admin", crate::admin::routes::routes())
        .nest("/rooms", crate::rooms::routes::routes())
        .merge(crate::phira::routes::routes())
        .merge(crate::replay::routes::routes())
        .merge(crate::social::routes::routes())
        .merge(crate::notifications::routes::routes())
        .merge(crate::preferences::routes::routes())
        .route("/friends/{phira_id}/remove", post(friend_remove))
        .route("/openapi.json", get(openapi_json))
        .route("/events", get(crate::public::routes::events_sse))
        .route("/me", get(me))
        .route("/me/profile", get(me_profile))
        .route("/me/identities", get(me_identities))
        .route("/me/preferences", get(me_preferences))
        .route("/me/join-intents", get(me_join_intents).post(me_join_intent_create))
        .route("/me/join-intents/{intent_id}", get(me_join_intent_get).delete(me_join_intent_cancel))
        .route("/me/push-endpoints", get(me_push_endpoints).post(me_push_endpoint_register))
        .route("/me/push-endpoints/{endpoint_id}", delete(me_push_endpoint_delete));

    let cors = build_cors(&state);

    let middleware = ServiceBuilder::new()
        .layer(crate::middleware::request_id::layers())
        .layer(crate::middleware::request_id::propagate_layer())
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .layer(from_fn_with_state(state.clone(), csrf::csrf_middleware))
        .into_inner();

    Router::new()
        .nest("/api/v1", api)
        .route("/ws/v1/rooms/{room_uuid}/live", get(crate::live::routes::live_ws))
        .route("/ws/v1/replays/{round_uuid}", get(crate::replay::routes::replay_ws))
        .route("/auth/phira/login", get(crate::auth::gateway::phira_login_page))
        .route("/healthz", get(healthz))
        .with_state(state.clone())
        .layer(cors)
        .layer(middleware)
        .fallback(not_found)
}

fn build_cors(state: &Arc<AppState>) -> CorsLayer {
    let mut origins: Vec<HeaderValue> = state
        .config
        .cors
        .allowed_origins
        .iter()
        .chain(state.config.cors.dev_origins.iter())
        .filter_map(|o| o.parse::<HeaderValue>().ok())
        .collect();

    if !state.config.cors.credentials {
        origins.push(HeaderValue::from_static("*"));
    }

    let headers = vec![
        CONTENT_TYPE,
        AUTHORIZATION,
        ACCEPT,
        HeaderName::from_static("x-csrf-token"),
        HeaderName::from_static("x-reauth-token"),
        HeaderName::from_static("x-request-id"),
        HeaderName::from_static("last-event-id"),
    ];

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(state.config.cors.credentials)
        .allow_methods(AllowMethods::any())
        .allow_headers(AllowHeaders::list(headers))
}

/// GET /api/v1/me — unified session probe (contract §20 S-4).
///
/// Returns `{principal, user, permissions[], capabilities[], session, ...}`
/// with permissions resolved at runtime (never baked into the JWT), plus the
/// session-bound `csrf_token` for state-changing requests.
#[utoipa::path(
    get,
    path = "/api/v1/me",
    operation_id = "me_get",
    responses(
        (status = 200, description = "session probe", body = MeResponse),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let csrf_token = crate::middleware::csrf::csrf_token_for(
        &auth.sid,
        &crate::middleware::csrf::csrf_key(&state.secrets.jwt_secret),
    );
    let session_created_at = session_created_at(&state, auth.sid).await?;
    let capabilities: Vec<&str> = crate::public::PPB_CAPABILITIES.to_vec();
    let client_type = auth.client_type.to_string();

    if auth.is_root() {
        return Ok(Json(json!({
            "principal": { "type": "root", "id": null },
            "principal_type": "root",
            "user": null,
            "identities": [],
            "phira_credential": null,
            "permissions": ["*:*"],
            "capabilities": capabilities,
            "session": {
                "sid": auth.sid,
                "client_type": client_type,
                "created_at": session_created_at,
            },
            "csrf_token": csrf_token,
        })));
    }

    let db = state.require_db()?;
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    let identities = identities_repo::list_for_user(db, auth.sub).await?;
    let credential = identities_repo::credential_state(db, auth.sub).await?;
    let permissions = state
        .permissions
        .permissions_for_user(db, auth.sub)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "permission resolution failed");
            ApiError::internal()
        })?;
    let mut perm_list: Vec<String> = permissions.into_iter().collect();
    perm_list.sort();

    Ok(Json(json!({
        "principal": { "type": "user", "id": user.id, "phira_id": user.phira_id },
        "principal_type": "user",
        "user": {
            "id": user.id,
            "phira_id": user.phira_id,
            "username": user.username_cache,
            "avatar": user.avatar_cache,
        },
        "identities": identities,
        "phira_credential": credential,
        "permissions": perm_list,
        "capabilities": capabilities,
        "session": {
            "sid": auth.sid,
            "client_type": client_type,
            "created_at": session_created_at,
        },
        "csrf_token": csrf_token,
    })))
}

/// Best-effort session created_at for the session probe.
async fn session_created_at(
    state: &Arc<AppState>,
    sid: uuid::Uuid,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiError> {
    let Some(db) = &state.db else { return Ok(None) };
    let row = sqlx::query_as::<_, (Option<chrono::DateTime<chrono::Utc>>,)>(
        "SELECT created_at FROM sessions WHERE id = $1",
    )
    .bind(sid)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "session query failed");
        ApiError::internal()
    })?;
    Ok(row.and_then(|(c,)| c))
}

/// GET /api/v1/me/profile — community profile (defaults when unset).
#[utoipa::path(
    get,
    path = "/api/v1/me/profile",
    operation_id = "me_profile_get",
    responses(
        (status = 200, description = "community profile", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
///
/// Optional fields (contract supplement): `rks`/`stats` (Phira gateway),
/// `online_status` (presence), `friends_count` (PPB social). Each is `null`
/// when the source is unavailable; frontends render a placeholder (`—`).
pub async fn me_profile(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let profile = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
        "SELECT bio, background_url, profile_visibility FROM user_profiles WHERE user_id = $1",
    )
    .bind(auth.sub)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "profile query failed");
        ApiError::internal()
    })?;

    let (bio, background, visibility) = profile.unwrap_or((None, None, None));
    let user = crate::users::repo::find_by_id(db, auth.sub).await?;
    let phira_id = user.map(|u| u.phira_id).unwrap_or(0);

    // Best-effort Phira stats (gateway). Missing → null (frontend shows "—").
    let (rks, stats) = if phira_id > 0 {
        match state.phira_gateway.user_stats(phira_id).await {
            Ok(v) => (
                v.get("rks").cloned().unwrap_or(serde_json::Value::Null),
                v,
            ),
            Err(_) => (serde_json::Value::Null, serde_json::Value::Null),
        }
    } else {
        (serde_json::Value::Null, serde_json::Value::Null)
    };

    // Best-effort presence from PMP. None → unknown (frontend shows "—").
    let online_status = if phira_id > 0 {
        state.player.info(phira_id as i32).await.ok().map(|p| {
            let in_room = p
                .get("room_id")
                .and_then(serde_json::Value::as_str)
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if in_room { "online" } else { "offline" }
        })
    } else {
        None
    };

    let friends_count = crate::social::list_friends(db, auth.sub)
        .await
        .map(|f| f.len() as i64)
        .ok();

    Ok(Json(json!({
        "bio": bio.unwrap_or_default(),
        "background_url": background.unwrap_or_default(),
        "profile_visibility": visibility.unwrap_or_else(|| "public".to_string()),
        "rks": rks,
        "stats": stats,
        "online_status": online_status,
        "friends_count": friends_count,
    })))
}

/// GET /api/v1/me/identities — identity bindings for the current user.
#[utoipa::path(
    get,
    path = "/api/v1/me/identities",
    operation_id = "me_identities_get",
    responses(
        (status = 200, description = "identity bindings", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me_identities(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let identities = identities_repo::list_for_user(db, auth.sub).await?;
    Ok(Json(json!({ "identities": identities })))
}

/// Preferences list response (§22).
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct PreferencesListResponse {
    pub preferences: Vec<UserPreference>,
}

/// GET /api/v1/me/preferences — all namespaces for the current user.
#[utoipa::path(
    get,
    path = "/api/v1/me/preferences",
    operation_id = "me_preferences_get",
    responses(
        (status = 200, description = "user preferences", body = PreferencesListResponse),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me_preferences(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<PreferencesListResponse>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let rows = sqlx::query_as::<_, crate::preferences::UserPreference>(
        "SELECT user_id, namespace, revision, json_data, updated_at
         FROM user_preferences WHERE user_id = $1",
    )
    .bind(auth.sub)
    .fetch_all(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "preferences query failed");
        ApiError::internal()
    })?;
    Ok(Json(PreferencesListResponse { preferences: rows }))
}

/// GET /api/v1/me/join-intents — list the caller's active join intents.
#[utoipa::path(
    get,
    path = "/api/v1/me/join-intents",
    operation_id = "me_join_intents_get",
    responses(
        (status = 200, description = "join intents", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me_join_intents(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let items = state.join_intents.list_for_user(auth.sub);
    Ok(Json(json!({ "items": items })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct JoinIntentBody {
    pub room_id: String,
    pub ttl_secs: Option<i64>,
}

/// POST /api/v1/me/join-intents — create a short-lived join intent (design §14.6).
#[utoipa::path(
    post,
    path = "/api/v1/me/join-intents",
    operation_id = "me_join_intents_post",
    request_body = JoinIntentBody,
    responses(
        (status = 200, description = "join intent created", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me_join_intent_create(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(body): Json<JoinIntentBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    let intent = state
        .join_intents
        .create(auth.sub, user.phira_id, &body.room_id, body.ttl_secs)?;
    Ok(Json(json!({ "intent": intent })))
}

/// DELETE /api/v1/me/join-intents/{id} — cancel a join intent.
#[utoipa::path(
    delete,
    path = "/api/v1/me/join-intents/{intent_id}",
    operation_id = "me_join_intents_intent_id_delete",
    responses(
        (status = 204, description = "cancelled"),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me_join_intent_cancel(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Path(intent_id): axum::extract::Path<uuid::Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    state.join_intents.cancel(auth.sub, intent_id)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// GET /api/v1/me/join-intents/{id} — poll an intent's status
/// (`pending | user_online | moving | completed | failed | expired`, §21).
#[utoipa::path(
    get,
    path = "/api/v1/me/join-intents/{intent_id}",
    operation_id = "me_join_intents_intent_id_get",
    responses(
        (status = 200, description = "intent status", body = serde_json::Value),
        (status = 404, description = "not found", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me_join_intent_get(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Path(intent_id): axum::extract::Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let intent = state.join_intents.get(auth.sub, intent_id)?;
    Ok(Json(json!({ "intent": intent })))
}

/// POST /api/v1/friends/{phira_id}/remove — remove a friend by Phira id (§21).
#[utoipa::path(
    post,
    path = "/api/v1/friends/{phira_id}/remove",
    operation_id = "friends_phira_id_remove_post",
    responses(
        (status = 204, description = "removed"),
        (status = 404, description = "user not found", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn friend_remove(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Path(phira_id): axum::extract::Path<i64>,
) -> Result<axum::http::StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let friend = crate::users::repo::find_by_phira_id(db, phira_id)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    crate::social::remove_friend(db, auth.sub, friend.id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// GET /api/v1/me/push-endpoints — list the caller's registered push endpoints.
#[utoipa::path(
    get,
    path = "/api/v1/me/push-endpoints",
    operation_id = "me_push_endpoints_get",
    responses(
        (status = 200, description = "push endpoints", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me_push_endpoints(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let items = crate::notifications::list_push_endpoints(db, auth.sub).await?;
    Ok(Json(json!({ "items": items })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PushEndpointBody {
    pub device_id: String,
    pub channel: String, // web_push | fcm | wns
    #[serde(default)]
    pub platform: String,
    pub subscription: SubscriptionWire,
}

/// POST /api/v1/me/push-endpoints — register a push endpoint (subscription
/// material encrypted at rest with the deployment key).
#[utoipa::path(
    post,
    path = "/api/v1/me/push-endpoints",
    operation_id = "me_push_endpoints_post",
    request_body = PushEndpointBody,
    responses(
        (status = 200, description = "registered", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me_push_endpoint_register(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(body): Json<PushEndpointBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    if !matches!(body.channel.as_str(), "web_push" | "fcm" | "wns") {
        return Err(ApiError::validation("channel must be web_push|fcm|wns"));
    }
    let db = state.require_db()?;
    let plaintext = serde_json::to_vec(&body.subscription)
        .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
    let ct = state.credential_cipher.encrypt(&plaintext)?;
    crate::notifications::register_push_endpoint(
        db,
        auth.sub,
        &body.device_id,
        &body.channel,
        &ct,
        &body.platform,
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// DELETE /api/v1/me/push-endpoints/{id} — remove a push endpoint.
#[utoipa::path(
    delete,
    path = "/api/v1/me/push-endpoints/{endpoint_id}",
    operation_id = "me_push_endpoints_endpoint_id_delete",
    responses(
        (status = 204, description = "removed"),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn me_push_endpoint_delete(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    axum::extract::Path(endpoint_id): axum::extract::Path<uuid::Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    crate::notifications::delete_push_endpoint(db, auth.sub, endpoint_id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// GET /api/v1/openapi.json — the OpenAPI document (HTTP Source of Truth, §21).
async fn openapi_json() -> Json<serde_json::Value> {
    let doc: serde_json::Value = serde_json::from_str(&crate::openapi::build_openapi_json())
        .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));
    Json(doc)
}

/// GET /healthz — liveness.
async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

/// Fallback for unknown routes.
async fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound, "route not found")
}

#[cfg(test)]
impl AppState {
    /// A test state with no DB, a mock Phira API, and a never-started OpenUDS client.
    pub fn for_test(config: RuntimeConfig) -> AppState {
        use crate::phira::client::MockPhiraApi;
        let openuds_config = OpenUdsConfig {
            path: std::path::PathBuf::from("/nonexistent/ppb-test.sock"),
            auth: crate::pmp::openuds::client::OpenUdsAuth::Token("test".to_string()),
            reconnect_base_ms: 1,
            reconnect_max_ms: 5,
            request_timeout_ms: 100,
            capabilities: config.pmp.capabilities.clone(),
        };
        let openuds = Arc::new(OpenUdsClient::new(openuds_config));
        let actions = Arc::new(ActionRegistry::new());
        let executor = Arc::new(PmpActionExecutor::new(
            Arc::clone(&openuds),
            Arc::clone(&actions),
            None,
        ));
        let rooms = RoomService::new(Arc::clone(&openuds));
        let player = PlayerService::new(Arc::clone(&openuds));
        let job_registry = Arc::new(JobRegistry::new());
        let jobs = JobRunner::new(
            None,
            EventBus::new(16, 8),
            Arc::clone(&openuds),
            Arc::clone(&job_registry),
        );
        let join_intents = JoinIntentStore::new();
        let push = Arc::new(PushService::new(
            &config.notifications,
            CredentialCipher::new(&[7u8; 32]).expect("valid key"),
        ));
        let phira_gateway = Arc::new(
            PhiraGateway::new("https://phira.example.test", 1000, 60, 100)
                .expect("test gateway builds"),
        );
        AppState {
            config: Arc::new(config),
            secrets: Arc::new(Secrets {
                database_url: None,
                jwt_secret: "test-jwt-secret-test-jwt-secret!!".to_string(),
                phira_credential_key: [7u8; 32].to_vec(),
                github_client_id: None,
                github_client_secret: None,
                pmp_openuds_token: None,
            }),
            db: None,
            phira: Arc::new(MockPhiraApi::default()),
            credential_cipher: CredentialCipher::new(&[7u8; 32]).expect("valid key"),
            openuds,
            permissions: Arc::new(PermissionResolver::new()),
            actions,
            commands: CommandBroker::new(executor),
            events: EventBus::new(16, 8),
            github: Arc::new(GithubService::new(1000)),
            metrics: Arc::new(crate::metrics::Metrics::default()),
            rate_limiter: RateLimiter::new(),
            heartbeat: HeartbeatCache::default(),
            rooms,
            player,
            jobs,
            job_registry,
            join_intents,
            push,
            phira_gateway,
            visit_counter: std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0)),
        }
    }
}

/// Helper: sign a test access token (used by tests).
#[cfg(test)]
pub fn test_access_token(
    state: &AppState,
    sub: uuid::Uuid,
    principal_type: crate::auth::types::PrincipalType,
    client_type: crate::auth::types::ClientType,
) -> String {
    let claims = crate::auth::jwt::AccessClaims::new(
        sub,
        uuid::Uuid::new_v4(),
        principal_type,
        client_type,
        3600,
    );
    crate::auth::jwt::encode_access(&claims, &state.secrets.jwt_secret).expect("signs")
}
