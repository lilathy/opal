# Contributing

## Stack

- Tauri 2 + Rust — vault, OS integration, chain I/O, Trezor
- React + TypeScript — UI

Product overview: [README.md](README.md)  
Dev setup: [docs/developing.md](docs/developing.md)

## Setup (Windows)

1. Install Node 20+, Rust stable, and Visual Studio Build Tools (C++ workload)
2. `npm install`
3. `npm run tauri:dev`

If PowerShell blocks npm: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`

## Rules

- Keys never log. Redact secrets in diagnostics.
- Use the vault helpers in `src-tauri/src/vault` — do not invent ad-hoc crypto.
- Keep UI flat and warm-dark; follow existing patterns in `src/styles`.
- Fit new work into the vault → portfolio model instead of bolting on side paths.
- Do not commit `.env`, `*.local.json`, vault backups, seeds, or signing private keys.

## Security reports

See [SECURITY.md](SECURITY.md). Threat model: [docs/threat-model.md](docs/threat-model.md).
