# Architecture

Opal is a Tauri 2 desktop app: React for the shell UI, Rust for vault crypto, chain I/O, and Trezor sessions.

## High-level flow

```
UI (React)
  └─ invoke Tauri commands
        ├─ vault / settings          → encrypted vault.opal
        ├─ portfolio_*               → balances, history, send, receive
        ├─ trezor_*                  → Bridge or native USB
        └─ prices / charts           → public market endpoints + local cache
```

While unlocked, portfolio metadata and the BIP39 seed (if any) live in memory inside the Rust process. On lock or exit they are wiped from the session; ciphertext stays on disk.

## Vault

- Password → Argon2id → KEK wraps a random 256-bit master key
- Payload (seed, portfolios, settings, address book, balance cache) encrypted with AES-256-GCM
- Path: `%AppData%\Opal\vault.opal`

Security presets (Fast / Normal / Paranoid) only change Argon2 cost. Parameters are stored with the vault so unlock matches creation.

## Portfolios

Each portfolio is chain-scoped and one of:

| Kind | Spend |
|------|--------|
| Software | Keys derived from the vault seed |
| Trezor | Device signs; Opal never sees the private key |
| Watch-only | No spend |

Balances are scraped from curated public nodes, cached in the vault for offline paint, then refreshed in the background. Optimistic spends keep the UI honest for a short window after send/swap.

## Charts and analytics

- Price history feeds overview and portfolio charts
- Growth series prefer reconstructed balance history from the transfer ledger when available; otherwise mark-to-market
- Overview analytics tiles are driven by the same holdings/ledger data; visibility and order live in vault settings

## Swaps

- Solana: Jupiter quote + local sign/broadcast when the portfolio can spend
- Cross-chain / other assets: FixedFloat rates in-app; order create needs API credentials stored in vault settings (optional)

## Trezor

Sessions go through Trezor Bridge when Suite/trezord is up, otherwise native USB. Device I/O is serialized so status polls and auto-sync cannot interrupt an in-flight SignTx. PIN entry still happens on the device / Suite — Opal does not collect the PIN.

## Networking

Public RPC and explorers only (no user custom RPC in current builds). Optional Tor SOCKS from Settings. Price book refreshes in a background loop so scrapes do not block on market HTTP.
