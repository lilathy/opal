# Publishing a release

Opal builds a Windows NSIS installer with Tauri. CI signs updater artifacts and can publish installers to a separate releases repository.

## Version bump

Keep these three in sync:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

## Tag a release

```powershell
git tag v0.1.1
git push origin v0.1.1
```

The workflow under `.github/workflows/release.yml` builds the installer. Configure repository secrets before relying on CI:

| Secret | Purpose |
|--------|---------|
| `TAURI_SIGNING_PRIVATE_KEY` | Updater private key (PEM / key file contents) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Key password, or empty if none |
| `RELEASES_TOKEN` | PAT that can publish to your public releases repo (if used) |

Generate a signing key once with the Tauri CLI and keep the private half off this repository. Only the public key belongs in app config.

## Local installer (no CI)

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY_PATH = "$env:USERPROFILE\.tauri\opal.key"
npm run tauri:build
```

Installer output:

`src-tauri\target\release\bundle\nsis\*.exe`

## Releases-only repository

Installers and `latest.json` (in-app updater) publish to the public [`opal-releases`](https://github.com/lilathy/opal-releases) repo. That repo should contain **binaries and updater metadata only** - never vault data, seeds, API keys, or signing private keys. Source lives in this repository.

## Checklist

- [ ] Version numbers match across package / Cargo / tauri.conf
- [ ] `npm run test:rust` passes locally
- [ ] No `.env`, `*.local.json`, or `*.key` files staged
- [ ] Tag pushed; Actions run is green
