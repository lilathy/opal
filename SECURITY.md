# Security Policy

## Reporting a vulnerability

Do **not** open a public GitHub issue for security vulnerabilities.

Email or privately message the maintainers with:

- A clear description of the issue
- Steps to reproduce
- Impact assessment (e.g. vault ciphertext offline attack, key material exposure, RCE)
- Your preferred contact for follow-up

We will acknowledge receipt as soon as practical and work on a fix before any public disclosure.

## Cryptography (vault)

| Piece | Choice |
|-------|--------|
| KDF | Argon2id |
| AEAD | AES-256-GCM |
| Structure | Envelope encryption (password → KEK wraps random 256-bit vault master key) |

### Argon2id presets

| Preset | Memory (KiB) | Iterations | Parallelism |
|--------|--------------|------------|-------------|
| Fast | 19_456 (19 MiB) | 2 | 1 |
| Normal (default) | 65_536 (64 MiB) | 3 | 1 |
| Paranoid | 262_144 (256 MiB) | 4 | 1 |

Parameters are stored alongside the salt in the vault file so unlock always matches creation.

### Threat notes

- The vault password is the only unlock factor in v1.
- There is no password recovery; seed restore is the recovery path for software keys (once seeded).
- Optional wipe-after-10 consecutive failures destroys local vault data only.
- Trezor private keys never enter Opal.
- Do not paste seed phrases into chat, issues, or logs.

## Scope

In scope: vault crypto, key handling in memory, Tauri command surface, dependency RCE in the desktop app.

Out of scope (for now): third-party RPC/node honesty, physical access with unlocked session, phishing of the user.
