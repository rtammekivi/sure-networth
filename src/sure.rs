use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, de::DeserializeOwned};

#[derive(Debug, Clone)]
pub enum Auth {
    Bearer(String),
    ApiKey(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SureError {
    #[error("Sure rejected the credentials")]
    Unauthorized,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub name: String,
    pub balance_cents: i64,
    pub cash_balance_cents: i64,
    pub currency: String,
    pub classification: String,
    #[serde(default)]
    pub account_type: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Named {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HoldingSecurity {
    pub ticker: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Holding {
    pub date: String,
    pub amount: String,
    pub currency: String,
    pub account: Named,
    pub security: HoldingSecurity,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Security {
    pub ticker: String,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub exchange_operating_mic: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Money {
    pub amount: String,
}

impl Money {
    pub fn value(&self) -> f64 {
        self.amount.parse().unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BalanceSheet {
    pub currency: String,
    pub net_worth: Money,
    pub assets: Money,
    pub liabilities: Money,
}

#[derive(Debug, Deserialize)]
struct Pagination {
    #[serde(default)]
    total_count: u64,
    #[serde(default)]
    total_pages: u64,
}

#[derive(Debug, Deserialize)]
struct Page<T> {
    #[serde(alias = "accounts", alias = "holdings", alias = "securities")]
    items: Vec<T>,
    pagination: Option<Pagination>,
}

/// Everything one allocation is computed from.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub as_of: String,
    pub accounts: Vec<Account>,
    pub holdings: Vec<Holding>,
    pub securities: Vec<Security>,
    pub balance_sheet: BalanceSheet,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    pub fn new(http: reqwest::Client, base_url: &str) -> Self {
        Self {
            http,
            base: format!("{}/api/v1", base_url.trim_end_matches('/')),
        }
    }

    async fn get<T: DeserializeOwned>(&self, auth: &Auth, path: &str) -> Result<T, SureError> {
        let req = self.http.get(format!("{}{}", self.base, path));
        let req = match auth {
            Auth::Bearer(t) => req.bearer_auth(t),
            Auth::ApiKey(k) => req.header("X-Api-Key", k),
        };
        let res = req.send().await.context("calling Sure")?;
        match res.status() {
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                Err(SureError::Unauthorized)
            }
            s if !s.is_success() => Err(anyhow!("{path} returned {s}").into()),
            _ => Ok(res
                .json()
                .await
                .with_context(|| format!("decoding {path}"))?),
        }
    }

    async fn all<T: DeserializeOwned>(&self, auth: &Auth, path: &str) -> Result<Vec<T>, SureError> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut out = Vec::new();
        for page in 1.. {
            let p: Page<T> = self
                .get(auth, &format!("{path}{sep}per_page=100&page={page}"))
                .await?;
            out.extend(p.items);
            if page >= p.pagination.map_or(0, |p| p.total_pages) {
                break;
            }
        }
        Ok(out)
    }

    /// Sure only gapfills holdings up to its own day, so `date` can be ahead
    /// of anything that exists. A date with no holdings silently renders every
    /// securities account as 100% cash, so unless the date was asked for
    /// explicitly, fall back to the newest day that has them.
    pub async fn snapshot(
        &self,
        auth: &Auth,
        date: &str,
        exact: bool,
    ) -> Result<Snapshot, SureError> {
        let probe_path = format!("/holdings?date={date}&per_page=1");
        let (accounts, securities, balance_sheet, probe) = tokio::try_join!(
            self.all::<Account>(auth, "/accounts"),
            self.all::<Security>(auth, "/securities"),
            self.get::<BalanceSheet>(auth, "/balance_sheet"),
            self.get::<Page<Holding>>(auth, &probe_path),
        )?;

        let mut as_of = date.to_owned();
        if !exact && probe.pagination.map_or(0, |p| p.total_count) == 0 {
            let path = format!("/holdings?end_date={date}&per_page=100");
            let first: Page<Holding> = self.get(auth, &path).await?;
            let last = first.pagination.as_ref().map_or(0, |p| p.total_pages);
            let tail = if last > 1 {
                self.get(auth, &format!("{path}&page={last}")).await?
            } else {
                first
            };
            if let Some(newest) = tail.items.iter().map(|h| &h.date).max() {
                as_of = newest.clone();
            }
        }

        let holdings = self.all(auth, &format!("/holdings?date={as_of}")).await?;
        Ok(Snapshot {
            as_of,
            accounts: accounts
                .into_iter()
                .filter(|a| a.status != "archived")
                .collect(),
            holdings,
            securities,
            balance_sheet,
        })
    }
}
