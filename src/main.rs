use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return anqa::healthcheck::run();
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("anqa: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    init_tracing();
    let config = anqa::config::Config::from_env()?;
    let _auth = anqa::auth::Auth::load(&config)?;
    tracing::info!(version = anqa::version(), "anqa is ready");
    Ok(())
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("anqa=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
