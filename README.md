# Opal

**Opal** is an open-source Windows desktop crypto wallet.  
It is a normal desktop wallet first — create portfolios, hold funds, send and receive — and you can also **connect a Trezor** so hardware portfolios appear in the same sidebar, clearly marked, and spendable only when the device is unlocked separately.

This README is the product & feature source of truth for planning. UI pixels come later.

## Develop (Phase 0+)

```bash
npm install
npm run tauri:dev
```

`tauri:dev` / `tauri:build` prepend `%USERPROFILE%\.cargo\bin` so Cursor terminals find `cargo` even if PATH is stale.

Vault file: `%AppData%\Opal\vault.opal`  
Installer: `src-tauri/target/release/bundle/nsis/Opal_0.1.0_x64-setup.exe`

### Implemented now

- Vault (Argon2id + AES-256-GCM), settings, EN+RU (+ i18n stubs for zh/es/pt/de/fr/ja/ko/ar)
- BIP39 create/restore (12/24), backup confirm, optional passphrase, Taproot + gap discovery
- Portfolios: software / Trezor / watch-only across BTC, ETH, Arb, Base, OP, SOL, LTC, DOGE, XMR
- Balances + receive (QR / BIP21), history (incl. SOL/DOGE/XMR via RPC), address book, tx notes
- Send: ETH(+L2)/ERC-20, BTC/LTC/DOGE, SOL + SPL USDC/USDT; fee presets; send max; RBF bump; address-poisoning checks
- Trezor Bridge session: on-device verify (ETH/BTC/LTC/DOGE), EIP-1559 SignTx for EVM
- XMR: real cryptonote addresses; Opal auto-starts `monero-wallet-rpc` from `%LOCALAPPDATA%\Opal\monero` (wallets in `%APPDATA%\Opal\xmr`)
- Tor SOCKS, tray, single-instance, autostart, vault export/import, About
- Curated **public nodes only** (no custom RPC)
- Swaps: Jupiter quotes (+ tx build) and FixedFloat rate quotes

### Still hardening / external deps

- XMR first sync against a public remote node can be slow
- Trezor UTXO/SOL/XMR SignTx still uses Suite for full PSBT flows (EVM Bridge signing works)
- Jupiter swap: quote + serialized tx; broadcast from SOL portfolio path as follow-through
- Mobile companion: see `docs/MOBILE.md` (Phase 7)
- Taproot **receive** works; Taproot **key-path spend** still prefers native SegWit portfolios

| | |
|--|--|
| **Name** | Opal |
| **Tagline** | Quiet self-custody |
| **License** | MIT |
| **Copyright** | Opal Contributors |
| **Platform (v1)** | Windows only |
| **Stack** | Tauri 2 (Rust core) + TypeScript UI (React) |
| **Mobile** | Later companion |
| **Visual** | Creamy-black dark, flat 2D (see section 12) |

---

## 1. Product thesis

- Software portfolios are the default (hot keys inside an encrypted local vault)
- Trezor portfolios live in the same sidebar with a Trezor mark; spending needs a **separate device unlock** on top of the vault password
- Portfolios are typically **chain-scoped** (e.g. Solana-only)
- Watch-only portfolios track external addresses (eye icon)
- Popular chains + a tiny token allowlist (including major ETH L2s)
- Swaps **later** (Jupiter for SOL, FixedFloat elsewhere)
- Flat **2D** UI: no shadows, no fake depth, calm and professional

Not a DeFi browser. Not a fiat on-ramp. Not a Suite clone.

---

## 2. Mental model

```
App vault (password)                 <- decrypts local data; opens the desktop wallet
 └── Sidebar portfolios
      ├── Software                   <- derived from one vault master seed
      ├── Trezor (+ logo)            <- device must be unlocked separately to spend
      └── Watch-only (+ eye icon)    <- address (or XMR view key) import; no spend
```

### Unlock layers

1. **Vault password** — always required to open Opal  
2. **Trezor session** — required only to spend / reveal verified receive for hardware portfolios (PIN on device; passphrase wallet if used)

Vault password ≠ Trezor PIN ≠ Trezor BIP39 passphrase ≠ software BIP39 passphrase. UI copy must never conflate them.

---

## 3. Vault & encryption (locked)

### Crypto

| Piece | Choice |
|-------|--------|
| Structure | **Envelope encryption** (password → KEK → random vault master key → data) |
| KDF | **Argon2id** |
| AEAD | **AES-256-GCM** |
| Factors | **Password only** (no keyfile, no Windows Hello/TPM, no YubiKey) |
| Password change | Re-wrap master key while unlocked; does not rotate seed |

### Security presets (user-selectable)

| Preset | Intent |
|--------|--------|
| **Normal** (default) | Strong Argon2id params; balanced unlock time on typical PCs |
| **Fast** | Weaker (still non-trivial) params for low-end machines |
| **Paranoid** | Heavier memory/time cost |

Exact Argon2 parameters are implementation constants documented in code + SECURITY.md.

### Brute-force / wipe

- **Default:** infinite wrong attempts  
- **Optional setting:** wipe all local vault data after **10** consecutive failures  

### Forgotten vault password

- No password recovery  
- Path forward: **restore from seed phrase** into a new vault (metadata lost unless user kept an encrypted vault backup — see section 10)

### What the vault encrypts at rest

- BIP39 master seed / derived key material  
- Portfolio metadata, labels, tx notes  
- Watch-only imports  
- Address book, settings  
- Cached balances / history snapshots  
- Future swap metadata  

---

## 4. Software wallet (locked)

- **One BIP39 master seed per vault**  
- User picks **12 or 24** words on create  
- **Create new** and **restore from seed** both required  
- First create forces **seed backup confirmation** before the wallet is ready to receive  
- **BIP39 passphrase** (25th word): off by default; enabled via **Settings → Security → BIP39 passphrase**. When enabled, create/restore/unlock software seed flows prompt for it. Hard warnings: second secret; forgotten passphrase = lost funds. Separate from vault password and Trezor passphrase.  
- Chain-scoped portfolios are views/accounts under that seed, not new seeds  
- Multiple accounts per chain allowed with standard gap-limit discovery  
- Derivation:
  - BTC: BIP84 native SegWit default; Taproot (BIP86) available; legacy not default  
  - LTC/DOGE: modern defaults matching Trezor/Suite conventions  
  - ETH + L2s: BIP44 `m/44'/60'/0'/0/x` (same address across EVM; network is the portfolio)  
  - SOL: standard path used by Trezor Connect / major wallets (document exact path in code)  
  - XMR: Monero wallet keys per Monero + Trezor conventions  

---

## 5. Trezor (locked)

- Hardware: **Trezor only**; primary QA device **Safe 3**; broaden models over time  
- Assume device **already initialized in Trezor Suite**  
- Sidebar entry with **Trezor mark**  
- User can **enable more chains/coins over time** (including L2s); enabling may require **on-device address verification**  
- Passphrase (hidden) wallets supported  
- Unplug / device lock → Trezor portfolios become non-spendable; software portfolios unaffected  
- No in-app firmware update or device wipe/init (send users to Suite)  
- Connect via current **Trezor Connect** stack; document Suite/bridge dependency where required  
- **Trademark:** Trezor name/logo only per SatoshiLabs brand rules; About: “Not affiliated with SatoshiLabs / Trezor”

---

## 6. Watch-only (locked)

| Chain type | Import |
|------------|--------|
| BTC, LTC, DOGE, SOL | **Single address**; auto-detect chain from format |
| EVM (`0x`) | Detected as EVM → **user must pick** Ethereum / Arbitrum / Base / Optimism |
| XMR | Address alone is not enough → **primary address + view key** (or view-only export). UI must explain this. |

Ambiguous formats: ask user to pick chain if detection is uncertain.  
Watch-only: balances + history only; send disabled.

---

## 7. Assets (first public release)

### Native (all required)

BTC, ETH, SOL, LTC, DOGE, **XMR**

XMR is mandatory for public release **for both software and Trezor**, implemented last among these.

### Tokens (allowlist only)

| Network | Tokens |
|---------|--------|
| Ethereum mainnet | USDC, USDT, DAI |
| Arbitrum One | USDC, USDT, DAI |
| Base | USDC, USDT, DAI |
| Optimism | USDC, USDT, DAI |
| Solana mainnet | USDC, USDT |

No other tokens. No arbitrary contract add in v1.  
No TRON or other non-listed USDT networks.  
No Polygon in v1.

L2 portfolios are first-class: user can create/enable **Arbitrum-only**, **Base-only**, or **Optimism-only** software or Trezor portfolios. Same seed / same Trezor; different chain id + RPC.

### Networks

- Normal UI mainnets: BTC, ETH, Arbitrum One, Base, Optimism, SOL, LTC, DOGE, XMR  
- Testnets: hidden advanced/dev only  

### Out of scope forever-unless-reopened

Lightning, unlisted L2s/alts, NFT gallery, staking product UI.

---

## 8. Core wallet features (locked)

### Portfolio / overview

- Per-portfolio and total balances  
- Multi-fiat display  
- Price sync + manual refresh  
- Offline: last encrypted cache after vault unlock  
- Discreet / hide balances  
- Local tx notes/labels  

### Receive

- Address + QR (generate)  
- Software: show after vault unlock  
- Trezor: **verify on device** before copy enabled  
- UTXO: avoid address reuse; next fresh receive address  
- Payment URI where standard (BIP21 etc.)  
- QR scan via webcam: **not v1**

### Send

- Address validation; crypto or fiat amount entry  
- Fee presets + custom where chain allows  
- Review → sign → broadcast → status  
- Send max  
- Address-poisoning defenses (prefix/suffix emphasis, near-match warnings, homoglyphs)  
- Clipboard auto-clear (default on for seed; short timer for addresses)  
- Seed UI: block window capture where Windows allows  

### Fee / stuck tx (v1)

| Chain | Behavior |
|-------|----------|
| BTC/LTC/DOGE | Fee estimate; RBF where supported; fee-bump |
| ETH + Arbitrum / Base / Optimism | EIP-1559 style; speed up / cancel via replacement (software) |
| SOL | Priority fees; rent-exempt minimums explained |
| XMR | Dynamic fee; sync/ready before send |

Coin control / multi-recipient batch send: **not v1**.

### History

- Per portfolio; pending/confirmed; explorer link; basic filters  
- Confirmation thresholds: sensible per-chain defaults  
- No tax CSV in v1  

### Address book

- Named contacts, per-chain addresses, recent recipients — local only  

### Rescan / repair

- Per-portfolio rescan / rediscover accounts  
- Clear-cache + re-fetch without deleting seed  

---

## 9. Privacy & network (locked)

| Item | Decision |
|------|----------|
| Discreet balances | Yes |
| Tor | User-provided **SOCKS5** (e.g. local Tor). Do **not** bundle Tor |
| Public nodes / RPC | Curated public nodes only (no user custom RPC) |
| HTTP corporate proxy | Not v1 |
| Analytics / crash phoning home | **No** |
| Debug logs | Local only; secrets redacted; export sanitized logs |

Price data: public market API (e.g. CoinGecko) with cache + backoff.  
Explorers: mempool.space, Etherscan, Arbiscan, Basescan, Optimistic Etherscan, Solscan, etc.; always allow copy tx id.

---

## 10. App behavior & desktop (locked)

| Item | Decision |
|------|----------|
| Auto-lock vault | Default **5 minutes** idle; user configurable (including “never” — discouraged) |
| Lock on sleep / screensaver | Yes |
| Lock when Trezor unplugged | Locks **Trezor spend session** only |
| System tray | Yes; balances respect discreet mode |
| Single instance | Yes |
| Start with Windows | Optional, default off |
| Data directory | `%AppData%\Opal\` |
| Portable USB mode | Not v1 |
| Encrypted vault file backup/export | Yes (metadata); seed backup separate |
| Notifications | Optional toasts for incoming confirmed txs |
| Sound | Off |
| Webcam | Not used in v1 |
| High DPI | Supported |
| Installer | Tauri Windows bundle; **unsigned OK** (SmartScreen may warn — accepted) |

---

## 11. Localization & fiat (locked)

**UI languages (i18n-ready day one):**

- Must: English, Russian  
- Also: Chinese (Simplified), Spanish, Portuguese (BR), German, French, Japanese, Korean, Arabic (RTL)

Ship EN+RU complete first; others may trail without breaking the string system.

**Fiat picker:** USD, EUR, GBP, RUB, JPY, CNY, KRW, BRL, TRY, INR (extend later).  
**Defaults:** UI language English; fiat USD.

---

## 12. Brand & design language (locked)

### Brand

| Item | Decision |
|------|----------|
| Name | **Opal** |
| Tagline | **Quiet self-custody** |
| Personality | Calm, precise, adult — jewelry-case dark, not neon exchange |
| Logo | Flat geometric stone / cabochon mark, monochrome cream on dark; no gradients or iridescent glow in-app |
| Domain / social | Not required for v1 |

### Color system (creamy black / dark)

Warm near-black surfaces (“creamy black”) with **clean near-white** type — not ivory/beige cream text.

| Token | Hex | Use |
|-------|-----|-----|
| `--bg` | `#12100E` | App background (creamy black) |
| `--bg-elevated` | `#181511` | Sidebar / panels |
| `--bg-input` | `#211D19` | Inputs / selected row |
| `--border` | `#2C2722` | Hairline structure |
| `--text` | `#F5F5F5` | Primary text (clean white) |
| `--text-muted` | `#8F8F8F` | Secondary labels (neutral gray) |
| `--accent` | `#2DD4BF` | Primary actions (teal, not cream) |
| `--text` | `#F5F5F4` | Clean near-white |
| `--text-muted` | `#8A857E` | Neutral gray labels |
| `--positive` | `#34D399` | Confirmed / receive |
| `--negative` | `#F87171` | Errors / outgoing |
| `--warning` | `#FBBF24` | Warnings |

Layout follows Ledger sidebar + Rabby home stack (balance → Send/Receive/Swap → asset rows). Flat surfaces with soft radius; no cream/beige type.

### UI rules

- Sidebar = accounts only; Swap / Settings sit quietly in the chrome  
- Selected account drives the main pane (Assets / Receive / Send / History tabs)  
- No in-app logo / wordmark branding  
- Avoid generic AI-crypto purple / glow / pill soup  
- Typography: **Plus Jakarta Sans** (UI) + **JetBrains Mono** (addresses, hashes — not everyday balance rows)  
- **Never show em dashes (`—`) for missing or zero balances.** Always render numeric zeros as **`0.00`** (fiat and crypto). Discreet mode may use `••••` instead.

---

## 13. Swaps (deferred — post solid core)

| Route | Provider |
|-------|----------|
| SOL | Jupiter |
| Other | FixedFloat (~0.5% fixed-rate band) |

Not a release blocker for first public wallet release.

---

## 14. Explicit non-goals (v1)

- Swaps (deferred)  
- WalletConnect / dApps  
- Staking / NFTs / DeFi surfaces  
- Fiat on-ramp/off-ramp  
- Tax export  
- L2s beyond Arbitrum One / Base / Optimism  
- Polygon and other unlisted EVM chains  
- Ledger / other non-Trezor hardware  
- macOS / Linux  
- Mobile  
- Keyfile / Hello / YubiKey  
- Bundled Tor  
- Authenticode code signing  
- Arbitrary tokens  
- Coin control / batch send  
- QR webcam scan  
- Cloud accounts / custodial recovery  
- Suite replacement for device init/firmware  

---

## 15. Open source & release engineering (locked)

- MIT `LICENSE`  
- `SECURITY.md` — private vulnerability reporting  
- `CONTRIBUTING.md`, issue templates, threat-model doc  
- Windows releases on GitHub Releases (**unsigned OK**)  
- Dependency pinning + supply-chain hygiene  
- Reproducible builds as a best-effort goal  
- About screen: version, license, Trezor disclaimer, source link  
- No telemetry  

---

## 16. Delivery phases

0. **Foundations** — Tauri shell, creamy-black theme, Argon2id+AES-GCM vault, presets, wipe-optional, i18n skeleton, settings (incl. BIP39 passphrase toggle)  
1. **Software BTC + ETH + L2s** — create/restore 12/24, passphrase via settings, portfolios, send/receive/history, allowlisted ERC-20 on Ethereum / Arbitrum / Base / Optimism, watch-only with network pick for `0x`  
2. **Trezor Safe 3** — sidebar hardware portfolios, device session, add chains (incl. L2s) + verify-on-device, Trezor passphrase wallets  
3. **SOL + LTC + DOGE** — software + Trezor; SPL USDC/USDT  
4. **XMR** — software + Trezor both; sync UX; XMR watch-only via view key  
5. **Public release polish** — languages, Tor SOCKS, tray, installer, docs  
6. **Swaps** — Jupiter + FixedFloat  
7. **Mobile companion** — later  

---

## 17. Success criteria (first public release)

1. Vault: Argon2id + AES-256-GCM, presets, optional wipe-at-10  
2. Create/restore software wallet; portfolios including **ETH + Arbitrum + Base + Optimism**; BTC/SOL + allowlisted stables  
3. BIP39 passphrase controllable from Settings → Security  
4. Trezor Safe 3 portfolios with logo; separate unlock; add chains with device verify  
5. Watch-only (EVM asks network; XMR uses view key)  
6. LTC, DOGE, **software XMR + Trezor XMR**  
7. EN+RU complete; i18n ready for the rest  
8. Creamy-black flat UI; discreet mode; Tor SOCKS; public nodes  
9. Windows installer (unsigned OK)  
10. Swaps not required  

---

## 18. Gap review ledger (all closed)

| Gap | Decision |
|-----|----------|
| Vault crypto | Argon2id + AES-256-GCM + envelope; password only |
| KDF UX | Normal / Fast / Paranoid; Normal default |
| Password change / forgot | Change while unlocked; forgot → seed restore only |
| BIP39 passphrase | Settings → Security; off by default; hard warnings |
| BTC address types | Native SegWit default; Taproot available |
| Multi-account same chain | Yes |
| L2s | Arbitrum One, Base, Optimism + USDC/USDT/DAI |
| EVM watch-only | User picks network |
| Testnets | Hidden advanced only |
| Stuck txs | RBF / ETH-L2 replace / SOL rent messaging |
| Coin control / batch | Not v1 |
| Poisoning / clipboard / screenshots | Defenses as specified |
| Auto-lock / tray / AppData | Specified |
| Vault metadata backup | Encrypted export/import + seed backup |
| Tor | User SOCKS5; no bundled Tor |
| Custom RPC | **No** — curated public nodes only |
| Prices / explorers | Public APIs + per-chain defaults |
| QR camera | Not v1 |
| XMR watch-only | View key required |
| Notifications / sound / portable | Toast optional; sound off; no portable |
| Code signing | Not required |
| Brand | Creamy-black palette + tagline (section 12) |
| Telemetry | None |
| Trezor trademark / Suite | Disclaimer + document Connect needs |
| Frontend | React + TypeScript on Tauri 2 |
| Fonts | Plus Jakarta Sans + JetBrains Mono |
| Defaults | Language English; fiat USD |
| Copyright line | Opal Contributors |
| Polygon | Not v1 |
| OSS hygiene | SECURITY / CONTRIBUTING / threat model |
| Lightning / dApps / staking / tax / swaps in v1 | Out / deferred |

---

## 19. Plan status: COMPLETE

No further product questions are required to execute this plan.

Implementation details (exact RPC URLs, Connect wiring, Monero libraries, etc.) are engineering choices inside these constraints — not product re-opens — unless they would break a locked requirement above.

**Document updated:** 2026-07-25 (final planning lock)
