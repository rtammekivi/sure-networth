mod allocation;
mod fx;
mod mapping;
mod server;
mod sure;
#[cfg(test)]
mod testdata;

use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Net worth allocation donut for Sure")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the page; each visitor signs in to Sure with OAuth.
    Serve {
        #[arg(long, env = "NETWORTH_BIND", default_value = "0.0.0.0:8080")]
        bind: String,
        #[arg(long, env = "SURE_URL")]
        sure_url: String,
        /// OAuth client id. Without one, / leads to onboarding.
        #[arg(long, env = "SURE_OAUTH_CLIENT_ID")]
        client_id: Option<String>,
        #[arg(long, env = "SURE_OAUTH_SCOPE", default_value = "read")]
        scope: String,
        /// Account mapping (JSON), reloaded when the file changes.
        #[arg(long, env = "NETWORTH_MAPPING")]
        mapping: Option<PathBuf>,
    },
    /// Exit 0 if the local server answers /healthz; for container health checks.
    Healthcheck {
        #[arg(long, env = "NETWORTH_BIND", default_value = "0.0.0.0:8080")]
        bind: String,
    },
    /// Write a standalone page with the data baked in, using an API key.
    Render {
        #[arg(long, env = "SURE_URL")]
        sure_url: String,
        #[arg(long, env = "SURE_API_KEY", hide_env_values = true)]
        api_key: String,
        #[arg(long, env = "NETWORTH_MAPPING")]
        mapping: Option<PathBuf>,
        /// Positions as of this day, taken literally. Defaults to the newest
        /// day Sure has holdings for.
        #[arg(long)]
        date: Option<String>,
        /// Override a rate, e.g. --fx USD=0.88 (base-currency units per unit).
        #[arg(long, value_parser = parse_fx)]
        fx: Vec<(String, f64)>,
        #[arg(short, long, default_value = "networth.html")]
        out: PathBuf,
        #[arg(long)]
        open: bool,
    },
}

fn parse_fx(s: &str) -> Result<(String, f64)> {
    let (c, r) = s
        .split_once('=')
        .ok_or_else(|| anyhow!("expected CUR=rate"))?;
    Ok((c.to_uppercase(), r.parse().context("rate is not a number")?))
}

fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("sure-networth/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(60))
        .build()?)
}

fn healthcheck(bind: &str) -> Result<()> {
    use std::io::{Read, Write};
    let port = bind.rsplit_once(':').map_or("8080", |(_, p)| p);
    let mut conn = std::net::TcpStream::connect(format!("127.0.0.1:{port}"))?;
    conn.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
    conn.write_all(b"GET /healthz HTTP/1.0\r\n\r\n")?;
    let mut head = [0u8; 12];
    conn.read_exact(&mut head)?;
    if &head[9..12] != b"200" {
        bail!("unhealthy: {}", String::from_utf8_lossy(&head));
    }
    Ok(())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sure_networth=info".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Healthcheck { bind } => healthcheck(&bind)?,
        Command::Serve {
            bind,
            sure_url,
            client_id,
            scope,
            mapping,
        } => {
            let client_id = client_id.filter(|c| !c.is_empty());
            if client_id.is_none() {
                tracing::warn!("SURE_OAUTH_CLIENT_ID is not set; serving onboarding");
            }
            let settings = server::Settings {
                sure_url,
                client_id,
                scope,
                mapping,
            };
            let app = server::router(settings, http()?)?;
            let listener = tokio::net::TcpListener::bind(&bind)
                .await
                .with_context(|| format!("binding {bind}"))?;
            tracing::info!("listening on {bind}");
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown())
                .await?;
        }
        Command::Render {
            sure_url,
            api_key,
            mapping,
            date,
            fx: given,
            out,
            open,
        } => {
            let http = http()?;
            let exact = date.is_some();
            let date = date.unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
            let snap = sure::Client::new(http.clone(), &sure_url)
                .snapshot(&sure::Auth::ApiKey(api_key), &date, exact)
                .await
                .map_err(|e| anyhow!(e))?;
            let given: BTreeMap<_, _> = given.into_iter().collect();
            let foreign = fx::foreign_currencies(&snap, &given);
            let ecb = fx::ecb(&http, &snap.balance_sheet.currency, &foreign).await;
            let rates = fx::resolve(&snap, &given, &ecb);
            let mapping = mapping::Mapping::load(mapping.as_deref())?;
            let data = allocation::build(&snap, &rates, &mapping, now());

            for w in &data.warnings {
                eprintln!("warning: {w}");
            }
            eprintln!(
                "reconciliation: leaves {} vs /balance_sheet {} (delta {})",
                data.reconcile.leaf_total, data.reconcile.api_total, data.reconcile.delta
            );
            std::fs::write(&out, bake(&data)?)
                .with_context(|| format!("writing {}", out.display()))?;
            eprintln!("wrote {}", out.display());
            if open {
                let _ = std::process::Command::new("xdg-open").arg(&out).spawn();
            }
        }
    }
    Ok(())
}

async fn shutdown() {
    let ctrl_c = tokio::signal::ctrl_c();
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("installing SIGTERM handler");
    tokio::select! {
        _ = ctrl_c => {},
        _ = term.recv() => {},
    }
}

const PLACEHOLDER: &str = "/*__DATA__*/null";

/// Escaping "</" keeps a security name containing "</script>" from ending the
/// script tag.
fn bake(data: &allocation::Allocation) -> Result<String> {
    if !server::APP_HTML.contains(PLACEHOLDER) {
        bail!("web/index.html has no {PLACEHOLDER} placeholder");
    }
    let json = serde_json::to_string(data)?.replace("</", "<\\/");
    Ok(server::APP_HTML.replacen(PLACEHOLDER, &json, 1))
}
