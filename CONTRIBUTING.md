# Contributing to Opal

## Stack

- Tauri 2 + Rust (vault crypto, OS integration)
- React + TypeScript (UI)
- Product source of truth: root `README.md`

## Setup (Windows)

1. Install Node 20+, Rust stable (`rustup`), and Visual Studio Build Tools (C++ workload)
2. `npm install`
3. `npm run tauri:dev` — this script prepends `%USERPROFILE%\.cargo\bin` so Cursor terminals find `cargo`

If PowerShell blocks npm: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`

## Rules

- Keys never log. Redact secrets in any diagnostics.
- Prefer envelope encryption helpers in `src-tauri/src/vault` — do not invent ad-hoc crypto.
- UI stays flat 2D creamy-black (see README section 12). No shadows/glows.
- Match existing module layout; don’t bolt features outside the vault → portfolio model.

## Security reports

See `SECURITY.md`. Threat model: `docs/THREAT_MODEL.md`.
