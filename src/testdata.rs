use crate::sure::{Account, BalanceSheet, Holding, HoldingSecurity, Money, Named, Snapshot};

pub fn account(name: &str, class: &str, currency: &str, balance: i64, cash: i64) -> Account {
    Account {
        name: name.into(),
        balance_cents: balance,
        cash_balance_cents: cash,
        currency: currency.into(),
        classification: class.into(),
        account_type: None,
        status: "active".into(),
    }
}

pub fn holding(
    account: &str,
    ticker: &str,
    name: Option<&str>,
    amount: &str,
    currency: &str,
) -> Holding {
    Holding {
        date: "2026-01-02".into(),
        amount: amount.into(),
        currency: currency.into(),
        account: Named {
            name: account.into(),
        },
        security: HoldingSecurity {
            ticker: ticker.into(),
            name: name.map(Into::into),
        },
    }
}

pub fn snapshot(
    accounts: Vec<Account>,
    holdings: Vec<Holding>,
    (assets, liabilities): (&str, &str),
) -> Snapshot {
    let money = |s: &str| Money { amount: s.into() };
    let net = assets.parse::<f64>().unwrap() - liabilities.parse::<f64>().unwrap();
    Snapshot {
        as_of: "2026-01-02".into(),
        accounts,
        holdings,
        securities: vec![crate::sure::Security {
            ticker: "AAPL".into(),
            country_code: Some("US".into()),
            exchange_operating_mic: Some("XNAS".into()),
        }],
        balance_sheet: BalanceSheet {
            currency: "EUR".into(),
            net_worth: money(&format!("{net:.2}")),
            assets: money(assets),
            liabilities: money(liabilities),
        },
    }
}
