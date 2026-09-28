use std::collections::HashMap;

use serde::Serialize;

use crate::{
    fx::Rates,
    mapping::{self, AccountClass, CashRule, Mapping},
    sure::{Account, Snapshot},
};

#[derive(Debug, Clone, Serialize)]
pub struct Leaf {
    pub class: String,
    pub group: String,
    pub account: String,
    pub item: Option<String>,
    pub label: String,
    pub value: f64,
    pub currency: String,
    pub market: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Liability {
    pub name: String,
    pub nets_against: Option<String>,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reconcile {
    pub leaf_total: f64,
    pub api_total: f64,
    pub delta: f64,
}

/// The payload the page renders.
#[derive(Debug, Clone, Serialize)]
pub struct Allocation {
    pub as_of: String,
    pub generated_at: String,
    pub currency: String,
    pub class_order: Vec<String>,
    pub market_order: Vec<String>,
    pub net_worth: f64,
    pub assets: f64,
    pub liabilities: f64,
    pub leaves: Vec<Leaf>,
    pub liability_accounts: Vec<Liability>,
    pub fx: Vec<String>,
    pub reconcile: Reconcile,
    pub warnings: Vec<String>,
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn parse_amount(s: &str) -> f64 {
    s.chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect::<String>()
        .parse()
        .unwrap_or(0.0)
}

struct Held {
    ticker: String,
    name: String,
    market: &'static str,
    currency: String,
    value: f64,
}

pub fn build(snap: &Snapshot, rates: &Rates, map: &Mapping, generated_at: String) -> Allocation {
    let base = &snap.balance_sheet.currency;
    let rate = |c: &str| rates.get(c).unwrap_or(1.0);
    let secs: HashMap<&str, _> = snap
        .securities
        .iter()
        .map(|s| (s.ticker.as_str(), s))
        .collect();

    let mut by_account: HashMap<&str, Vec<Held>> = HashMap::new();
    for h in &snap.holdings {
        let value = r2(parse_amount(&h.amount) * rate(&h.currency));
        if value <= 0.005 {
            continue;
        }
        let sec = secs.get(h.security.ticker.as_str());
        by_account.entry(&h.account.name).or_default().push(Held {
            ticker: h.security.ticker.clone(),
            name: h
                .security
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| h.security.ticker.clone()),
            market: mapping::market(
                Some(&h.security.ticker),
                sec.and_then(|s| s.country_code.as_deref()),
                sec.and_then(|s| s.exchange_operating_mic.as_deref()),
                Some(&h.currency),
            ),
            currency: h.currency.clone(),
            value,
        });
    }
    for held in by_account.values_mut() {
        held.sort_by(|a, b| b.value.total_cmp(&a.value));
    }

    let assets: Vec<_> = snap
        .accounts
        .iter()
        .filter(|a| a.classification == "asset")
        .collect();
    let mut leaves = Vec::new();
    for a in &assets {
        let class = map
            .accounts
            .get(&a.name)
            .cloned()
            .unwrap_or_else(|| AccountClass {
                class: mapping::fallback_class(a.account_type.as_deref()).to_owned(),
                group: None,
                cash: CashRule::Keep,
            });
        let group = class.group.clone().unwrap_or_else(|| class.class.clone());
        let r = rate(&a.currency);
        let balance = r2(a.balance_cents as f64 / 100.0 * r);
        let held = by_account
            .get(a.name.as_str())
            .map_or(&[][..], Vec::as_slice);
        let held_sum: f64 = held.iter().map(|h| h.value).sum();
        // Brokerage cash is balance minus holdings rather than Sure's
        // cash_balance, which is computed against Sure's own current day while
        // holdings are as of `as_of`; mixing the two clocks zeroes the account.
        let split = match class.cash {
            CashRule::Keep => 0.0,
            CashRule::Split if held.is_empty() => r2(a.cash_balance_cents as f64 / 100.0 * r),
            CashRule::Split => r2((balance - held_sum).max(0.0)),
        };
        let residual = r2(balance - split - held_sum);
        let account_market = mapping::market(None, None, None, Some(&a.currency));
        let leaf = |item: Option<String>, label: String, value: f64, market: &str| Leaf {
            class: class.class.clone(),
            group: group.clone(),
            account: a.name.clone(),
            item,
            label,
            value,
            currency: a.currency.clone(),
            market: market.to_owned(),
        };

        if split > 0.005 {
            leaves.push(Leaf {
                class: "Cash".into(),
                group: "Brokerage cash".into(),
                label: format!("{} cash", a.name),
                ..leaf(None, String::new(), split, account_market)
            });
        }
        for h in held {
            leaves.push(Leaf {
                currency: h.currency.clone(),
                ..leaf(Some(h.ticker.clone()), h.name.clone(), h.value, h.market)
            });
        }
        if held.is_empty() && balance - split > 0.005 {
            leaves.push(leaf(None, a.name.clone(), balance - split, account_market));
        } else if !held.is_empty() && residual.abs() > 1.0 {
            leaves.push(leaf(
                Some("residual".into()),
                "Unallocated".into(),
                residual,
                account_market,
            ));
        }
    }

    let liability_accounts = snap
        .accounts
        .iter()
        .filter(|a| a.classification == "liability")
        .map(|l| Liability {
            name: l.name.clone(),
            nets_against: map.netting.get(&l.name).cloned(),
            value: r2(l.balance_cents as f64 / 100.0 * rate(&l.currency)),
        })
        .collect();

    let names = |keep: &dyn Fn(&Account) -> bool| {
        let names: Vec<_> = assets
            .iter()
            .filter(|a| keep(a))
            .map(|a| a.name.as_str())
            .collect();
        names.join(", ")
    };
    let unmapped = names(&|a| !map.accounts.contains_key(&a.name));
    let unconverted = names(&|a| &a.currency != base && rates.get(&a.currency).is_none());
    let leaf_total = r2(leaves.iter().map(|l| l.value).sum());
    let api_total = r2(snap.balance_sheet.assets.value());

    let mut warnings = Vec::new();
    if !unmapped.is_empty() {
        warnings.push(format!(
            "Unmapped accounts (bucketed by type, add them to the mapping): {unmapped}"
        ));
    }
    if !unconverted.is_empty() {
        warnings.push(format!("No FX rate, counted at face value: {unconverted}"));
    }
    if (leaf_total - api_total).abs() > 1.0 {
        warnings.push(format!(
            "Leaves total {leaf_total} vs API assets {api_total} — something is unaccounted for"
        ));
    }

    Allocation {
        as_of: snap.as_of.clone(),
        generated_at,
        currency: base.clone(),
        class_order: mapping::CLASS_ORDER.map(String::from).to_vec(),
        market_order: mapping::MARKET_ORDER.map(String::from).to_vec(),
        net_worth: r2(snap.balance_sheet.net_worth.value()),
        assets: api_total,
        liabilities: r2(snap.balance_sheet.liabilities.value()),
        leaves,
        liability_accounts,
        fx: rates
            .sources
            .iter()
            .map(|(c, s)| format!("1 {c} = {:.5} {base} ({})", rates.rates[c], s.label()))
            .collect(),
        reconcile: Reconcile {
            leaf_total,
            api_total,
            delta: r2(leaf_total - api_total),
        },
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        fx,
        testdata::{account, holding, snapshot},
    };

    fn mapping() -> Mapping {
        serde_json::from_str(
            r#"{"accounts": {
                  "Current": {"class": "Cash", "group": "Current accounts"},
                  "Broker": {"class": "Equities", "cash": "split"},
                  "Pension": {"class": "Pension", "cash": "keep"},
                  "Flat": {"class": "Property"}},
                "netting": {"Mortgage": "Flat"}}"#,
        )
        .unwrap()
    }

    fn build_book() -> Allocation {
        let snap = snapshot(
            vec![
                account("Current", "asset", "EUR", 100000, 100000),
                account("Broker", "asset", "USD", 1000000, 999999),
                account("Pension", "asset", "EUR", 500000, 500000),
                account("Flat", "asset", "EUR", 20000000, 0),
                account("Car", "asset", "EUR", 800000, 0),
                account("Mortgage", "liability", "EUR", 15000000, 0),
            ],
            vec![
                holding("Broker", "AAPL", Some("Apple"), "$6,000.00", "USD"),
                holding("Broker", "ACME", None, "$3,000.00", "USD"),
                holding("Broker", "DUST", Some("Dust"), "$0.00", "USD"),
            ],
            ("222800.00", "150000.00"),
        );
        let rates = fx::resolve(&snap, &BTreeMap::new(), &BTreeMap::new());
        build(&snap, &rates, &mapping(), "t".into())
    }

    fn leaf<'a>(a: &'a Allocation, account: &str, label: &str) -> &'a Leaf {
        a.leaves
            .iter()
            .find(|l| l.account == account && l.label == label)
            .unwrap_or_else(|| panic!("no leaf {account}/{label}"))
    }

    #[test]
    fn reconciles_to_the_balance_sheet() {
        let a = build_book();
        assert_eq!(a.reconcile.delta, 0.0, "{:?}", a.reconcile);
        assert_eq!(a.fx, ["1 USD = 0.88000 EUR (Sure)"]);
    }

    #[test]
    fn brokerage_cash_is_balance_minus_holdings() {
        let a = build_book();
        let cash = leaf(&a, "Broker", "Broker cash");
        assert_eq!((cash.class.as_str(), cash.value), ("Cash", 880.0));
        assert_eq!(leaf(&a, "Broker", "Apple").value, 5280.0);
        assert_eq!(leaf(&a, "Broker", "ACME").class, "Equities");
        assert!(!a.leaves.iter().any(|l| l.label == "Dust"));
    }

    #[test]
    fn markets_follow_the_security_then_the_currency() {
        let a = build_book();
        assert_eq!(leaf(&a, "Broker", "Apple").market, "United States");
        assert_eq!(leaf(&a, "Flat", "Flat").market, "Europe");
    }

    #[test]
    fn holdings_carry_their_own_currency() {
        let snap = snapshot(
            vec![account("Broker", "asset", "EUR", 1000000, 0)],
            vec![
                holding("Broker", "AAPL", Some("Apple"), "$6,000.00", "USD"),
                holding("Broker", "VWCE", Some("World"), "€4,000.00", "EUR"),
            ],
            ("10000.00", "0.00"),
        );
        let rates = fx::resolve(&snap, &BTreeMap::new(), &BTreeMap::new());
        let a = build(&snap, &rates, &mapping(), "t".into());
        assert_eq!(leaf(&a, "Broker", "Apple").currency, "USD");
        assert_eq!(leaf(&a, "Broker", "World").currency, "EUR");
    }

    #[test]
    fn keep_accounts_stay_whole() {
        let a = build_book();
        let p = leaf(&a, "Pension", "Pension");
        assert_eq!((p.class.as_str(), p.value), ("Pension", 5000.0));
        assert_eq!(leaf(&a, "Current", "Current").group, "Current accounts");
    }

    #[test]
    fn unmapped_accounts_fall_back_by_type_and_warn() {
        let a = build_book();
        assert_eq!(leaf(&a, "Car", "Car").class, "Other");
        assert_eq!(a.warnings.len(), 1);
        assert!(a.warnings[0].contains("Car"));
    }

    #[test]
    fn liabilities_net_against_their_asset() {
        let a = build_book();
        assert_eq!(
            a.liability_accounts[0].nets_against.as_deref(),
            Some("Flat")
        );
        assert_eq!(a.liability_accounts[0].value, 150_000.0);
    }

    #[test]
    fn parses_formatted_amounts() {
        assert_eq!(parse_amount("€1,234.56"), 1234.56);
        assert_eq!(parse_amount("-$5.00"), -5.0);
    }
}
