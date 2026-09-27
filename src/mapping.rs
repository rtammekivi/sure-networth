use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

/// How the account's cash balance is treated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CashRule {
    /// Brokerage cash is peeled off into the Cash class.
    Split,
    /// The whole balance stays in the account's own class.
    #[default]
    Keep,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountClass {
    pub class: String,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub cash: CashRule,
}

/// Per-deployment knowledge Sure does not store: which asset class each account
/// represents, and which liability finances which asset.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    #[serde(default)]
    pub accounts: HashMap<String, AccountClass>,
    #[serde(default)]
    pub netting: HashMap<String, String>,
}

impl Mapping {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading mapping {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing mapping {}", path.display()))
    }
}

pub const CLASS_ORDER: [&str; 7] = [
    "Property", "Equities", "Pension", "Bonds", "Cash", "Crypto", "Other",
];
pub const MARKET_ORDER: [&str; 3] = ["Europe", "United States", "Global"];

pub fn fallback_class(account_type: Option<&str>) -> &'static str {
    match account_type.unwrap_or("") {
        "depository" => "Cash",
        "investment" => "Equities",
        "crypto" => "Crypto",
        "property" => "Property",
        _ => "Other",
    }
}

const US: &[&str] = &[
    "US", "USD", "XNAS", "XNGS", "XNMS", "XNCM", "XNYS", "XASE", "ARCX", "BATS", "PINX",
];

const EUROPE: &[&str] = &[
    "EUR", "EE", "LV", "LT", "FI", "SE", "DK", "NO", "IS", "IE", "GB", "DE", "FR", "NL", "BE",
    "LU", "AT", "CH", "ES", "PT", "IT", "PL", "CZ", "SK", "HU", "SI", "HR", "RO", "BG", "GR", "MT",
    "CY", "XTAL", "XRIS", "XLIT", "FNEE", "FNLV", "FNLT", "XHEL", "XSTO", "XCSE", "XOSL", "XAMS",
    "XBRU", "XPAR", "XETR", "XFRA", "XMIL", "XMAD", "XLON", "XSWX", "XWBO", "XLIS", "XDUB",
];

fn market_code(code: Option<&str>) -> Option<&'static str> {
    let code = code.filter(|c| !c.is_empty())?;
    if US.contains(&code) {
        Some("United States")
    } else if EUROPE.contains(&code) {
        Some("Europe")
    } else {
        None
    }
}

/// Where a value is priced, not where it is custodied. Country before MIC,
/// because a provider that mis-resolved the venue usually still got the
/// domicile right; currency last. Country codes, MICs and currencies never
/// collide, so one lookup serves all three.
pub fn market(
    ticker: Option<&str>,
    country: Option<&str>,
    mic: Option<&str>,
    currency: Option<&str>,
) -> &'static str {
    if ticker.is_some_and(|t| t.starts_with("CRYPTO:")) {
        return "Global";
    }
    market_code(country)
        .or_else(|| market_code(mic))
        .or_else(|| market_code(currency))
        .unwrap_or("Global")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_prefers_country_over_mic() {
        assert_eq!(
            market(Some("IE"), Some("EE"), Some("XNSE"), Some("INR")),
            "Europe"
        );
        assert_eq!(
            market(Some("AAPL"), None, Some("XNGS"), Some("EUR")),
            "United States"
        );
        assert_eq!(market(Some("BOND"), None, None, Some("EUR")), "Europe");
        assert_eq!(market(Some("CRYPTO:BTC"), Some("US"), None, None), "Global");
        assert_eq!(market(None, Some(""), None, Some("JPY")), "Global");
    }

    #[test]
    fn mapping_parses_cash_rules() {
        let m: Mapping = serde_json::from_str(
            r#"{"accounts": {"Broker": {"class": "Bonds", "cash": "split"},
                             "Flat": {"class": "Property"}},
                "netting": {"Mortgage": "Flat"}}"#,
        )
        .unwrap();
        assert_eq!(m.accounts["Broker"].cash, CashRule::Split);
        assert_eq!(m.accounts["Flat"].cash, CashRule::Keep);
        assert_eq!(m.netting["Mortgage"], "Flat");
    }
}
