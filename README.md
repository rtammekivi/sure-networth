# sure-networth

Net-worth allocation charts for [Sure](https://github.com/we-promise/sure).

https://github.com/user-attachments/assets/5e52be4f-e4e4-4aa6-b509-e4b28c8b9f7f

- **Allocation** — donut or treemap, grouped by market, currency, asset class, account or holding; click to drill in.
- **Grid** — market or currency × asset class.
- **Over time** — the same cut at past month-ends, rebuilt from Sure's history.
- Gross assets or net worth, exclusions, and a private mode that hides amounts but keeps percentages.

Each visitor signs in to Sure with OAuth (PKCE, `read` scope) and sees only their own data. The server holds no secrets and stores nothing: the token lives in the browser tab and is passed through per request.

## Setup

1. In Sure, create an OAuth application at `/oauth/applications/new` (needs `super_admin`): redirect URI is this server's URL with a trailing slash, *Confidential* unticked, scope `read`.
2. Run with `SURE_URL` and `SURE_OAUTH_CLIENT_ID`. Without a client ID, `/` opens an onboarding page that walks through step 1.

| Variable | Default | |
| --- | --- | --- |
| `SURE_URL` | — | Sure as browsers reach it |
| `SURE_OAUTH_CLIENT_ID` | — | empty → onboarding |
| `SURE_OAUTH_SCOPE` | `read` | |
| `NETWORTH_MAPPING` | — | mapping file, see below |
| `NETWORTH_BIND` | `0.0.0.0:8080` | |

## Mapping

Sure has no asset classes, so accounts are classed by type unless mapped. The file is reloaded on change.

```json
{
  "accounts": {
    "Broker":  { "class": "Equities", "cash": "split" },
    "Pension": { "class": "Pension" },
    "Flat":    { "class": "Property" },
    "Current": { "class": "Cash", "group": "Current accounts" }
  },
  "netting": { "Mortgage": "Flat" }
}
```

- `class`: Property, Equities, Pension, Bonds, Cash, Crypto or Other.
- `cash: "split"` moves a brokerage account's uninvested cash to Cash; the default `keep` leaves it in the account's class.
- `netting` subtracts a liability from the asset it financed in net-worth mode.

Market and currency need no mapping; they come from each security.

## Run

```sh
# docker compose — http://127.0.0.1:8732, reads $NETWORTH_CONFIG_DIR/mapping.json
SURE_URL=https://sure.example.com SURE_OAUTH_CLIENT_ID=<id> NETWORTH_CONFIG_DIR=~/.config/sure-networth \
  docker compose up -d --build

# helm — mapping under `mapping.accounts` / `mapping.netting` in values
helm install sure-networth oci://ghcr.io/rtammekivi/sure-networth/sure-networth-chart \
  --set sure.url=https://sure.example.com --set sure.oauthClientId=<id> -f values.yaml
```

Releases also ship static Linux binaries (x86_64, aarch64). `sure-networth render` bakes a standalone HTML file using an API key instead of OAuth.

## Development

```sh
nix develop
cargo test && cargo clippy --all-targets -- -D warnings
cargo run -- serve
nix develop .#demo -c python3 docs/record-demo.py --upload   # re-record the demo
```

Releases are cut by release-please from conventional commits.
