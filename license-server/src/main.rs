mod admin;
mod apple;
mod config;
mod db;
mod google;
mod handlers;
mod tokens;

use std::sync::Arc;

use clap::Parser;

use crate::admin::AdminCommand;

/// Prism Pro 授权服务；带子命令时执行管理操作，否则启动 HTTP 服务。
#[derive(Parser)]
#[command(name = "prism-license-server", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<AdminCommand>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,prism_license_server=debug".into()),
        )
        .init();

    let cli = Cli::parse();

    // CLI 模式：只需要 DB，不需要管理密钥/完整配置
    if let Some(command) = cli.command {
        let db_path = std::env::var("PRISM_LICENSE_DB")
            .unwrap_or_else(|_| "data/license.db".into());
        let db = db::Db::open(std::path::Path::new(&db_path)).await?;
        return admin::run_cli(db, command).await;
    }

    // 服务模式
    let config = Arc::new(config::Config::from_env()?);
    let db = db::Db::open(&config.db_path).await?;
    let signer = Arc::new(tokens::Signer::load_or_create(&config.key_path)?);
    let play = google::build_client(&config.google)?
        .map(Arc::new);

    let state = handlers::AppState {
        db,
        signer,
        config: config.clone(),
        play,
    };

    let app = handlers::routes().merge(admin::router()).with_state(state);

    let listener = tokio::net::TcpListener::bind(&config.bind).await?;
    tracing::info!("license server listening on {}", config.bind);
    axum::serve(listener, app).await?;
    Ok(())
}
