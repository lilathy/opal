# Threat model

## Assets

- BIP39 master seed and derived private keys (software portfolios)
- Vault encryption password (only unlock factor in current builds)
- Portfolio metadata, address book, settings
- Optional BIP39 passphrase (25th word)
- Optional swap API credentials stored in the vault

## Trust boundaries

| Boundary | Trust |
|----------|-------|
| Local OS user with an unlocked session | Fully trusted for the duration of unlock |
| Encrypted `vault.opal` at rest | Confidential without the password |
| Public RPC / explorers | Untrusted for privacy and honesty; not for key custody |
| Trezor device + Bridge | Keys stay on device; Bridge is local IPC |
| Market / swap APIs | Untrusted; display and routing only |
| monero-wallet-rpc (when used) | Trusted only if local and not exposed remotely |

## Adversaries

1. **Offline vault thief** — steals `%AppData%\Opal\vault.opal`. Mitigated by Argon2id + AES-256-GCM; optional wipe after 10 failed unlocks.
2. **Malicious RPC** — lies about balances or fees. Mitigated by curated public endpoints; verify large sends independently.
3. **Address poisoning** — lookalike deposits. Mitigated by prefix/suffix emphasis and near-match warnings.
4. **Clipboard malware** — swaps pasted addresses. Mitigated by short-lived clipboard clear where used.
5. **Screen capture of seed** — best-effort Windows display affinity; user should clear the desk and use discreet mode when needed.

## Non-goals

- Protecting an unlocked session against same-user local malware
- Guaranteeing honest third-party nodes
- Replacing Trezor Suite for device init or firmware

## Reporting

See [SECURITY.md](../SECURITY.md).
