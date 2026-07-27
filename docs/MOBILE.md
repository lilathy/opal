# Mobile companion (Phase 7)

The desktop wallet is the v1 product. A mobile companion is planned later and is **not** required for the first public Windows release.

## Intended companion scope (locked direction)

- Watch balances / receive addresses for portfolios already in the desktop vault
- Approve or reject spends initiated on desktop (optional push)
- No hot seed storage on phone in v1 companion design — pair via encrypted QR / one-time pairing secret
- Same creamy-black visual language

## Status

Not started as an application binary. Desktop APIs (`portfolio_balances`, `portfolio_receive_uri`, vault export) are the pairing foundation.
