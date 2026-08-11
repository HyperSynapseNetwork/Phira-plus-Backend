//! Application state, router assembly, and background tasks.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue};
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::{Json, Router};
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
use crate::error::{ApiError, ErrorCode};
use crate::identities::repo as identities_repo;
use crate::middleware::csrf;
use crate::middleware::rate_limit::RateLimiter;
use crate::permissions::resolver::PermissionResolver;
use crate::phira::client::{PhiraApi, PhiraClient};
use crate::phira::credential::CredentialCipher;
use crate::pmp::events::{EventBus, PpbEvent, ResourceRef};
use crate::pmp::openuds::client::{OpenUdsClient, OpenUdsConfig};

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
    });

    if let Some(db) = &state.db {
        crate::permissions::routes::run_bootstrap(db).await?;
        match RootAuthService::bootstrap(db).await? {
            Some(password) => {
                // Printed once to the CLI path — never logged via tracing.
                eprintln!("[ppb] Root first-boot password (print once): {password}");
            }
            None => {}
        }
    }

    state.openuds.start();
    spawn_pmp_event_forwarder(&state);
    spawn_heartbeat_task(&state);
    spawn_audit_purge_task(&state);

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
        .route("/events", get(crate::public::routes::events_sse))
        .route("/me", get(me))
        .route("/me/profile", get(me_profile))
        .route("/me/preferences", get(me_preferences));

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

/// GET /api/v1/me — current user summary + identity state.
pub async fn me(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Ok(Json(json!({
            "principal_type": "root",
            "permissions": ["*:*"],
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
    })))
}

/// GET /api/v1/me/profile — community profile (defaults when unset).
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
    Ok(Json(json!({
        "bio": bio.unwrap_or_default(),
        "background_url": background.unwrap_or_default(),
        "profile_visibility": visibility.unwrap_or_else(|| "public".to_string()),
    })))
}

/// GET /api/v1/me/preferences — all namespaces for the current user.
pub async fn me_preferences(
    auth: AuthPrincipal,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
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
    Ok(Json(json!({ "preferences": rows })))
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
