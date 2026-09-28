use anyhow::{Context, Result, anyhow};
use futures_util::future::try_join_all;
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
    pub id: String,
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
pub struct Balance {
    pub currency: String,
    pub balance_cents: i64,
    pub cash_balance_cents: i64,
    pub account: Named,
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
    #[serde(
        alias = "accounts",
        alias = "balances",
        alias = "holdings",
        alias = "securities"
    )]
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

/// The part of a snapshot that only exists as of today.
#[derive(Debug, Clone)]
pub struct Current {
    pub accounts: Vec<Account>,
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
            accounts: active(accounts),
            holdings,
            securities,
            balance_sheet,
        })
    }

    pub async fn current(&self, auth: &Auth) -> Result<Current, SureError> {
        let (accounts, securities, balance_sheet) = tokio::try_join!(
            self.all::<Account>(auth, "/accounts"),
            self.all::<Security>(auth, "/securities"),
            self.get::<BalanceSheet>(auth, "/balance_sheet"),
        )?;
        Ok(Current {
            accounts: active(accounts),
            securities,
            balance_sheet,
        })
    }

    /// `/accounts` only knows today's balances, so a past date takes each
    /// account's from `/balances` instead. Sure only keeps rows for the span it
    /// last materialised, so an account without one that day carries its latest
    /// earlier row forward; with none at all it did not exist yet and is left
    /// out. The balance sheet totals still describe today and are the caller's
    /// to replace.
    pub async fn snapshot_on(
        &self,
        auth: &Auth,
        current: &Current,
        date: &str,
    ) -> Result<Snapshot, SureError> {
        let balances_path = format!("/balances?start_date={date}&end_date={date}");
        let holdings_path = format!("/holdings?date={date}");
        let (mut balances, holdings) = tokio::try_join!(
            self.all::<Balance>(auth, &balances_path),
            self.all::<Holding>(auth, &holdings_path),
        )?;
        let missing = current
            .accounts
            .iter()
            .filter(|a| !balances.iter().any(|b| belongs(b, a)));
        let earlier = try_join_all(missing.map(|a| self.latest_balance(auth, a, date))).await?;
        balances.extend(earlier.into_iter().flatten());
        Ok(Snapshot {
            as_of: date.to_owned(),
            accounts: on_date(&current.accounts, &balances),
            holdings,
            securities: current.securities.clone(),
            balance_sheet: current.balance_sheet.clone(),
        })
    }

    async fn latest_balance(
        &self,
        auth: &Auth,
        account: &Account,
        date: &str,
    ) -> Result<Option<Balance>, SureError> {
        let path = format!(
            "/balances?account_id={}&currency={}&end_date={date}&per_page=1",
            account.id, account.currency
        );
        let page: Page<Balance> = self.get(auth, &path).await?;
        Ok(page.items.into_iter().next())
    }
}

fn belongs(balance: &Balance, account: &Account) -> bool {
    balance.account.name == account.name && balance.currency == account.currency
}

fn active(accounts: Vec<Account>) -> Vec<Account> {
    accounts
        .into_iter()
        .filter(|a| a.status != "archived")
        .collect()
}

fn on_date(accounts: &[Account], balances: &[Balance]) -> Vec<Account> {
    accounts
        .iter()
        .filter_map(|a| {
            let b = balances.iter().find(|b| belongs(b, a))?;
            Some(Account {
                balance_cents: b.balance_cents,
                cash_balance_cents: b.cash_balance_cents,
                ..a.clone()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdata::account;

    fn balance(account: &str, currency: &str, cents: i64) -> Balance {
        Balance {
            currency: currency.into(),
            balance_cents: cents,
            cash_balance_cents: cents / 2,
            account: Named {
                name: account.into(),
            },
        }
    }

    #[test]
    fn past_balances_replace_todays_and_drop_accounts_not_yet_open() {
        let accounts = vec![
            account("Broker", "asset", "USD", 900, 900),
            account("Flat", "asset", "EUR", 700, 0),
        ];
        let past = on_date(
            &accounts,
            &[balance("Broker", "EUR", 1), balance("Broker", "USD", 400)],
        );
        assert_eq!(past.len(), 1);
        assert_eq!(
            (past[0].balance_cents, past[0].cash_balance_cents),
            (400, 200)
        );
    }
}
