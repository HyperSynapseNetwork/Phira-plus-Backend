//! PPB server binary.

use std::error::Error;

use ppb_server::app::{build_router, build_state};
use ppb_server::config::deployment::Secrets;
use ppb_server::config::resolve_runtime_config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("ppb-server {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.len() >= 3 && args[1] == "root" && args[2] == "init" {
        return root_init().await;
    }
    if args.len() >= 3 && args[1] == "--check-config" {
        // Used by deploy/update.sh stage validation.
        let path = &args[2];
        ppb_server::config::RuntimeConfig::from_toml_file(std::path::Path::new(path))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        println!("config OK: {path}");
        return Ok(());
    }

    ppb_server::telemetry::init();

    // `--config <path>` takes precedence; otherwise env / ./config/ppb.toml / defaults.
    let runtime = match config_flag(&args) {
        Some(path) => ppb_server::config::RuntimeConfig::from_toml_file(std::path::Path::new(&path))
            .map_err(|e| std::io::Error::other(e.to_string()))?,
        None => resolve_runtime_config()?,
    };
    let secrets = Secrets::from_env()?;
    let state = build_state(runtime, secrets).await?;
    let router = build_router(state.clone());

    let addr = state.config.server.listen_addr;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(
        version = %env!("CARGO_PKG_VERSION"),
        %addr,
        "ppb-server listening"
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Extract `--config <path>` from the CLI args (path may be `=`-joined too).
fn config_flag(args: &[String]) -> Option<String> {
    let pos = args.iter().position(|a| a == "--config")?;
    if let Some(p) = args.get(pos + 1) {
        return Some(p.clone());
    }
    args.iter()
        .find(|a| a.starts_with("--config="))
        .map(|a| a.trim_start_matches("--config=").to_string())
}

/// `ppb-server root init` — generate/print the first-boot Root password (CLI path).
async fn root_init() -> Result<(), Box<dyn Error>> {
    let secrets = Secrets::from_env()?;
    let url = secrets
        .database_url
        .ok_or("PPB_DATABASE_URL is required for `root init`")?;
    let pool = sqlx::postgres::PgPoolOptions::new().connect(&url).await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    match ppb_server::auth::root::RootAuthService::bootstrap(&pool).await? {
        Some(password) => {
            println!("Root first-boot password (print once, change on first login): {password}");
        }
        None => {
            println!("Root credentials already initialized.");
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
