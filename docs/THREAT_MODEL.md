# Opal Threat Model

## Assets

- BIP39 master seed and derived private keys (software portfolios)
- Vault encryption password (only unlock factor)
- Portfolio metadata, address book, settings
- Optional BIP39 passphrase (25th word)

## Trust boundaries

| Boundary | Trust |
|----------|-------|
| Local OS user with unlocked session | Fully trusted for duration of unlock |
| Encrypted `vault.opal` at rest | Confidential without password |
| Public RPC / explorers | Untrusted for privacy & censorship; not for key custody |
| Trezor device + Bridge | Keys never leave device; Bridge is local IPC |
| CoinGecko price API | Untrusted; display only |
| monero-wallet-rpc (optional) | Trusted when user-run locally; must not be exposed remotely |

## Adversaries

1. **Offline vault thief** — steals `%AppData%\Opal\vault.opal`. Mitigated by Argon2id + AES-256-GCM envelope; optional wipe-after-10.
2. **Malicious RPC** — lies about balances/fees. Mitigated by curated defaults + custom RPC; user responsibility to verify.
3. **Address poisoning** — lookalike deposits. Mitigated by prefix/suffix emphasis + near-match warnings.
4. **Clipboard malware** — swap addresses. Mitigated by clipboard auto-clear timers.
5. **Screen capture of seed** — Windows display affinity best-effort; user should clear desk / use discreet mode.

## Non-goals

- Protecting an unlocked session against local malware with same-user privileges
- Guaranteeing honest third-party nodes
- Replacing Trezor Suite for device init/firmware

## Reporting

See `SECURITY.md`.
