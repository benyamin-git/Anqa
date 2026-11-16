use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return anqa::healthcheck::run();
    }

    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("anqa: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<()> {
    init_tracing();

    let config = anqa::config::Config::from_env()?;
    let auth = anqa::auth::Auth::load(&config)?;
    let docker = anqa::docker::DockerClient::connect(&config.docker_socket)
        .try_negotiate()
        .await;

    if let Err(error) =
        anqa::docker::ensure_orphan_cleanup(&docker, &config.bots_dir, &config.data_dir).await
    {
        tracing::warn!(%error, "orphan cleanup did not complete");
    }

    let bind_addr = config.bind_addr;
    let state = anqa::state::AppState::new(config, docker, auth);
    let app = anqa::api::router(state);
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(address = %listener.local_addr()?, "anqa is listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "cannot listen for SIGINT");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received");
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("anqa=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
