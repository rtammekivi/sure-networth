use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::sure::Snapshot;

/// A solved rate is trusted only this close to ECB's. Further out, an account
/// the balance sheet leaves out (`exclude_from_reports`) is skewing the solve.
const TOLERANCE: f64 = 0.02;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Given,
    Sure,
    Ecb,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Given => "given",
            Source::Sure => "Sure",
            Source::Ecb => "ECB",
        }
    }
}

/// Units of the base currency per unit of each currency.
#[derive(Debug, Clone, Default)]
pub struct Rates {
    pub rates: BTreeMap<String, f64>,
    pub sources: BTreeMap<String, Source>,
}

impl Rates {
    pub fn get(&self, currency: &str) -> Option<f64> {
        self.rates.get(currency).copied()
    }

    fn set(&mut self, currency: &str, rate: f64, source: Source) {
        self.rates.insert(currency.to_owned(), rate);
        self.sources.insert(currency.to_owned(), source);
    }
}

pub fn foreign_currencies(snap: &Snapshot, given: &BTreeMap<String, f64>) -> Vec<String> {
    let base = &snap.balance_sheet.currency;
    snap.accounts
        .iter()
        .map(|a| &a.currency)
        .chain(snap.holdings.iter().map(|h| &h.currency))
        .filter(|c| *c != base && !given.contains_key(*c))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The API returns balances in each account's own currency with no converted
/// figure, but `/balance_sheet` totals them at Sure's own rate. With a single
/// foreign currency per side that rate falls out exactly; ECB cross-checks it
/// and covers whatever the solve cannot.
pub fn resolve(
    snap: &Snapshot,
    given: &BTreeMap<String, f64>,
    ecb: &BTreeMap<String, f64>,
) -> Rates {
    let base = &snap.balance_sheet.currency;
    let mut out = Rates::default();
    out.rates.insert(base.clone(), 1.0);
    for (c, r) in given {
        out.set(c, *r, Source::Given);
    }

    let sides = [
        ("asset", snap.balance_sheet.assets.value()),
        ("liability", snap.balance_sheet.liabilities.value()),
    ];
    for (class, total) in sides {
        let accounts: Vec<_> = snap
            .accounts
            .iter()
            .filter(|a| a.classification == class)
            .collect();
        let unknown: BTreeSet<_> = accounts
            .iter()
            .map(|a| a.currency.as_str())
            .filter(|c| out.get(c).is_none())
            .collect();
        let [currency] = unknown.into_iter().collect::<Vec<_>>()[..] else {
            continue;
        };
        let known: f64 = accounts
            .iter()
            .filter_map(|a| {
                out.get(&a.currency)
                    .map(|r| a.balance_cents as f64 / 100.0 * r)
            })
            .sum();
        let units: f64 = accounts
            .iter()
            .filter(|a| a.currency == currency)
            .map(|a| a.balance_cents as f64 / 100.0)
            .sum();
        if units == 0.0 {
            continue;
        }
        let solved = (total - known) / units;
        let plausible = ecb
            .get(currency)
            .is_none_or(|e| (solved / e - 1.0).abs() < TOLERANCE);
        if solved > 0.0 && plausible {
            out.set(currency, solved, Source::Sure);
        }
    }

    for (c, r) in ecb {
        if out.get(c).is_none() {
            out.set(c, *r, Source::Ecb);
        }
    }
    out
}

/// A past date has no balance sheet of its own to solve against.
pub fn from_ecb(base: &str, ecb: &BTreeMap<String, f64>) -> Rates {
    let mut out = Rates::default();
    out.rates.insert(base.to_owned(), 1.0);
    for (c, r) in ecb {
        out.set(c, *r, Source::Ecb);
    }
    out
}

#[derive(Deserialize)]
struct Frankfurter {
    rates: BTreeMap<String, f64>,
}

/// ECB reference rates, inverted to base-currency units. Best effort: an
/// outage leaves the solve unchecked rather than failing the page.
pub async fn ecb(
    http: &reqwest::Client,
    date: &str,
    base: &str,
    currencies: &[String],
) -> BTreeMap<String, f64> {
    if currencies.is_empty() {
        return BTreeMap::new();
    }
    let url = format!(
        "https://api.frankfurter.dev/v1/{date}?base={base}&symbols={}",
        currencies.join(",")
    );
    let fetched = async {
        let res = http.get(url).send().await?.error_for_status()?;
        res.json::<Frankfurter>().await
    }
    .await;
    match fetched {
        Ok(f) => f
            .rates
            .into_iter()
            .filter(|(_, r)| *r > 0.0)
            .map(|(c, r)| (c, 1.0 / r))
            .collect(),
        Err(e) => {
            tracing::warn!("ECB rates unavailable: {e}");
            BTreeMap::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdata::{account, snapshot};

    fn book() -> Snapshot {
        snapshot(
            vec![
                account("EUR cash", "asset", "EUR", 100000, 0),
                account("USD broker", "asset", "USD", 100000, 0),
                account("Mortgage", "liability", "EUR", 50000, 0),
            ],
            vec![],
            ("1880.00", "500.00"),
        )
    }

    #[test]
    fn solves_single_foreign_currency_from_balance_sheet() {
        let r = resolve(&book(), &BTreeMap::new(), &BTreeMap::new());
        assert!((r.get("USD").unwrap() - 0.88).abs() < 1e-9);
        assert_eq!(r.sources["USD"], Source::Sure);
    }

    #[test]
    fn falls_back_to_ecb_when_solve_disagrees() {
        let ecb = BTreeMap::from([("USD".to_owned(), 0.80)]);
        let r = resolve(&book(), &BTreeMap::new(), &ecb);
        assert_eq!(r.get("USD"), Some(0.80));
        assert_eq!(r.sources["USD"], Source::Ecb);
    }

    #[test]
    fn keeps_solve_when_ecb_agrees() {
        let ecb = BTreeMap::from([("USD".to_owned(), 0.879)]);
        let r = resolve(&book(), &BTreeMap::new(), &ecb);
        assert_eq!(r.sources["USD"], Source::Sure);
    }

    #[test]
    fn given_rates_win() {
        let given = BTreeMap::from([("USD".to_owned(), 0.9)]);
        let r = resolve(&book(), &given, &BTreeMap::new());
        assert_eq!(r.get("USD"), Some(0.9));
        assert!(foreign_currencies(&book(), &given).is_empty());
    }

    #[test]
    fn two_foreign_currencies_are_not_solved() {
        let mut s = book();
        s.accounts.push(account("GBP", "asset", "GBP", 10000, 0));
        let r = resolve(&s, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(r.get("USD"), None);
        assert_eq!(foreign_currencies(&s, &BTreeMap::new()), ["GBP", "USD"]);
    }
}
