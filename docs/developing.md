# Developing Opal

## Prerequisites (Windows)

1. [Node.js](https://nodejs.org/) 20 or newer
2. [Rust](https://rustup.rs/) stable
3. Visual Studio Build Tools with the **Desktop development with C++** workload
4. WebView2 (comes with current Windows 10/11)

`npm run tauri:dev` and `npm run tauri:build` prepend `%USERPROFILE%\.cargo\bin` so shells with a stale PATH still find `cargo`.

If PowerShell blocks npm scripts:

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

## Everyday workflow

```bash
npm install
npm run tauri:dev
```

Frontend-only Vite (no native shell):

```bash
npm run dev
```

## Tests

```bash
npm run test:rust
npm run test:balances
```

Rust coverage lives next to the code under `#[cfg(test)]`. There is no Vitest suite yet; balance merge logic is exercised by the Node smoke script.

## Local secrets

Copy examples, never commit real values:

| File | Purpose |
|------|---------|
| `.env.example` | Documented env vars (optional for most local work) |
| `fixedfloat.local.json` | Optional FixedFloat key/secret for in-app order create (gitignored) |

Updater signing keys belong under `%USERPROFILE%\.tauri\` and in GitHub Actions secrets — not in this tree.

## Code habits

- Prefer existing vault helpers in `src-tauri/src/vault` for crypto. Do not invent ad-hoc encryption.
- Do not log seeds, private keys, or API secrets.
- Keep UI flat and warm-dark; match patterns already in `src/styles`.
- Portfolios are the unit of custody (software / Trezor / watch-only). New features should hang off that model.

## Where things live

| Area | Path |
|------|------|
| Screens | `src/screens/` |
| Shared UI | `src/components/` |
| Frontend libs | `src/lib/` |
| Tauri commands | `src-tauri/src/wallet_commands.rs`, `commands.rs` |
| Chain / RPC | `src-tauri/src/network/` |
| Send paths | `src-tauri/src/wallet/send/` |
| Trezor | `src-tauri/src/trezor/` |

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md). Security reports go through [SECURITY.md](../SECURITY.md).
