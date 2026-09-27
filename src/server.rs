use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::SystemTime,
};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    allocation, fx,
    mapping::Mapping,
    sure::{Auth, Client, SureError},
};

pub const APP_HTML: &str = include_str!("../web/index.html");
const ONBOARDING_HTML: &str = include_str!("../web/onboarding.html");

#[derive(Debug, Clone)]
pub struct Settings {
    pub sure_url: String,
    pub client_id: Option<String>,
    pub scope: String,
    pub mapping: Option<PathBuf>,
}

/// The mapping is re-read whenever the file's mtime moves, which also catches a
/// Kubernetes ConfigMap update (the kubelet swaps a symlink under the mount). A
/// file that fails to parse keeps the last good mapping in place.
pub struct MappingStore {
    path: Option<PathBuf>,
    current: RwLock<(Option<SystemTime>, Arc<Mapping>)>,
}

impl MappingStore {
    pub fn new(path: Option<PathBuf>) -> Result<Self> {
        let mapping = Mapping::load(path.as_deref())?;
        let mtime = path.as_deref().and_then(mtime);
        Ok(Self {
            path,
            current: RwLock::new((mtime, Arc::new(mapping))),
        })
    }

    pub fn get(&self) -> Arc<Mapping> {
        let Some(path) = &self.path else {
            return self.current.read().unwrap().1.clone();
        };
        let now = mtime(path);
        {
            let cur = self.current.read().unwrap();
            if cur.0 == now {
                return cur.1.clone();
            }
        }
        let mut cur = self.current.write().unwrap();
        match Mapping::load(Some(path)) {
            Ok(m) => {
                tracing::info!(
                    "reloaded mapping {} ({} accounts)",
                    path.display(),
                    m.accounts.len()
                );
                *cur = (now, Arc::new(m));
            }
            Err(e) => {
                tracing::error!("{e:#}; keeping the previous mapping");
                cur.0 = now;
            }
        }
        cur.1.clone()
    }
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

struct AppState {
    settings: Settings,
    http: reqwest::Client,
    sure: Client,
    mapping: MappingStore,
}

type Shared = Arc<AppState>;

pub fn router(settings: Settings, http: reqwest::Client) -> Result<Router> {
    let state = Arc::new(AppState {
        sure: Client::new(http.clone(), &settings.sure_url),
        mapping: MappingStore::new(settings.mapping.clone())?,
        settings,
        http,
    });
    Ok(Router::new()
        .route("/", get(index))
        .route("/onboarding", get(onboarding))
        .route("/config.json", get(config))
        .route("/api/allocation", get(allocation))
        .route("/api/onboarding/register", post(register))
        .route("/api/onboarding/verify", post(verify))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state))
}

async fn index(State(s): State<Shared>) -> Response {
    if s.settings.client_id.is_none() {
        return Redirect::temporary("/onboarding").into_response();
    }
    no_store(Html(APP_HTML))
}

async fn onboarding(State(s): State<Shared>) -> Response {
    if s.settings.client_id.is_some() {
        return Redirect::temporary("/").into_response();
    }
    no_store(Html(ONBOARDING_HTML))
}

fn no_store(body: impl IntoResponse) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], body).into_response()
}

#[derive(Serialize)]
struct PublicConfig<'a> {
    sure_url: &'a str,
    client_id: Option<&'a str>,
    scope: &'a str,
}

async fn config(State(s): State<Shared>) -> Response {
    no_store(Json(PublicConfig {
        sure_url: s.settings.sure_url.trim_end_matches('/'),
        client_id: s.settings.client_id.as_deref(),
        scope: &s.settings.scope,
    }))
}

#[derive(Deserialize)]
struct AllocationQuery {
    date: Option<String>,
}

fn valid_date(d: &str) -> bool {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()
}

async fn allocation(
    State(s): State<Shared>,
    headers: HeaderMap,
    Query(q): Query<AllocationQuery>,
) -> Response {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let date = match q.date {
        Some(d) if valid_date(&d) => d,
        Some(_) => return (StatusCode::BAD_REQUEST, "date must be YYYY-MM-DD").into_response(),
        None => chrono::Local::now().date_naive().to_string(),
    };
    let auth = Auth::Bearer(token.to_owned());
    match compute(&s, &auth, &date).await {
        Ok(a) => no_store(Json(a)),
        Err(SureError::Unauthorized) => {
            (StatusCode::UNAUTHORIZED, "Sure rejected the token").into_response()
        }
        Err(SureError::Other(e)) => {
            tracing::error!("allocation failed: {e:#}");
            (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response()
        }
    }
}

async fn compute(
    s: &AppState,
    auth: &Auth,
    date: &str,
) -> Result<allocation::Allocation, SureError> {
    let snap = s.sure.snapshot(auth, date, false).await?;
    let given = BTreeMap::new();
    let foreign = fx::foreign_currencies(&snap, &given);
    let ecb = fx::ecb(&s.http, &snap.balance_sheet.currency, &foreign).await;
    let rates = fx::resolve(&snap, &given, &ecb);
    Ok(allocation::build(
        &snap,
        &rates,
        &s.mapping.get(),
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    ))
}

#[derive(Deserialize)]
struct RegisterRequest {
    redirect_uri: String,
}

/// Sure's `/register` sends no CORS headers, so the onboarding page registers
/// through here. Only offered while no client is configured.
async fn register(State(s): State<Shared>, Json(req): Json<RegisterRequest>) -> Response {
    if s.settings.client_id.is_some() {
        return (StatusCode::CONFLICT, "a client is already configured").into_response();
    }
    let url = format!("{}/register", s.settings.sure_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "client_name": "Net worth allocation",
        "redirect_uris": [req.redirect_uri],
    });
    let res = match s.http.post(url).json(&body).send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = res.text().await.unwrap_or_default();
    (status, [(header::CONTENT_TYPE, "application/json")], text).into_response()
}

#[derive(Deserialize)]
struct VerifyRequest {
    client_id: String,
}

#[derive(Serialize)]
struct VerifyResponse {
    known: bool,
}

/// Sure's authorize endpoint asks for a login before it looks at the client, so
/// existence is probed at the token endpoint instead: a bogus code against a
/// known client is `invalid_grant`, against an unknown one `invalid_client`.
async fn verify(State(s): State<Shared>, Json(req): Json<VerifyRequest>) -> Response {
    if s.settings.client_id.is_some() {
        return (StatusCode::CONFLICT, "a client is already configured").into_response();
    }
    let url = format!("{}/oauth/token", s.settings.sure_url.trim_end_matches('/'));
    let form = [
        ("grant_type", "authorization_code"),
        ("code", "onboarding-probe"),
        ("client_id", req.client_id.trim()),
        ("redirect_uri", "http://127.0.0.1/"),
        ("code_verifier", "onboarding-probe"),
    ];
    let error = match s.http.post(url).form(&form).send().await {
        Ok(r) => r
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|v| v["error"].as_str().map(String::from)),
        Err(e) => return (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    };
    match error.as_deref() {
        Some("invalid_grant") => Json(VerifyResponse { known: true }).into_response(),
        Some("invalid_client") => Json(VerifyResponse { known: false }).into_response(),
        other => (
            StatusCode::BAD_GATEWAY,
            format!("unexpected answer from Sure: {other:?}"),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_reloads_on_change_and_survives_bad_edits() {
        let dir = std::env::temp_dir().join(format!("sn-map-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mapping.json");
        std::fs::write(&path, r#"{"accounts": {"A": {"class": "Bonds"}}}"#).unwrap();
        let store = MappingStore::new(Some(path.clone())).unwrap();
        assert_eq!(store.get().accounts["A"].class, "Bonds");

        let bump = |text: &str, secs: u64| {
            std::fs::write(&path, text).unwrap();
            let f = std::fs::File::options().write(true).open(&path).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
                .unwrap();
        };
        bump(r#"{"accounts": {"A": {"class": "Cash"}}}"#, 1_000);
        assert_eq!(store.get().accounts["A"].class, "Cash");

        bump("{not json", 2_000);
        assert_eq!(store.get().accounts["A"].class, "Cash");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dates_are_validated() {
        assert!(valid_date("2026-09-28"));
        assert!(!valid_date("2026-09-28&page=2"));
    }
}
