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

## Releases-only repository (optional)

Some setups keep source private and publish binaries to a public `*-releases` repo (installers + `latest.json` for the in-app updater). That repo should contain **binaries only** — never vault data, seeds, or signing private keys.

## Checklist

- [ ] Version numbers match across package / Cargo / tauri.conf
- [ ] `npm run test:rust` passes locally
- [ ] No `.env`, `*.local.json`, or `*.key` files staged
- [ ] Tag pushed; Actions run is green
