# Security Policy

## Reporting a vulnerability

Do **not** open a public GitHub issue for security bugs.

Contact the maintainers privately with:

- A clear description
- Steps to reproduce
- Impact (vault offline attack, key material exposure, RCE, etc.)
- How you prefer to be reached

We will acknowledge as soon as practical and aim to fix before any public write-up.

## Cryptography (vault)

| Piece | Choice |
|-------|--------|
| KDF | Argon2id |
| AEAD | AES-256-GCM |
| Structure | Envelope encryption (password → KEK wraps a random 256-bit master key) |

### Argon2id presets

| Preset | Memory (KiB) | Iterations | Parallelism |
|--------|--------------|------------|-------------|
| Fast | 19_456 (19 MiB) | 2 | 1 |
| Normal (default) | 65_536 (64 MiB) | 3 | 1 |
| Paranoid | 262_144 (256 MiB) | 4 | 1 |

Parameters are stored with the salt so unlock always matches creation.

## Practices

- The vault password is the only unlock factor in current builds.
- There is no password recovery; software recovery is seed restore into a new vault.
- Optional wipe-after-10 destroys local vault data only.
- Trezor private keys never enter Opal.
- Do not paste seed phrases, private keys, or API secrets into issues, chat, or logs.
- Keep FixedFloat credentials and updater signing keys out of git (use gitignored local files / CI secrets).

## Scope

**In scope:** vault crypto, key handling in memory, Tauri command surface, dependency RCE in the desktop app.

**Out of scope for now:** third-party RPC honesty, physical access with an unlocked session, user phishing.
