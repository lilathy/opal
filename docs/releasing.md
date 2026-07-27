# Publishing a new Opal build

## First-time setup (already done if you followed the agent)

1. Private source: `https://github.com/lilathy/opal`
2. Public releases: `https://github.com/lilathy/opal-releases` (installers + `latest.json`)
3. Repo secrets on **lilathy/opal**:
   - `TAURI_SIGNING_PRIVATE_KEY` — contents of `%USERPROFILE%\.tauri\opal.key`
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — empty string if the key has no password
   - `RELEASES_TOKEN` — classic PAT with `repo` scope (can publish to `opal-releases`)

## Ship an update

1. Bump version in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json` (keep them equal).
2. Commit and push to `main`.
3. Tag and push:

```powershell
git tag v0.1.1
git push origin v0.1.1
```

4. GitHub Actions builds the NSIS installer, signs updater artifacts, and publishes
   to `lilathy/opal-releases`. Installed apps auto-download and relaunch on next start.

## Local installer (without CI)

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY_PATH = "$env:USERPROFILE\.tauri\opal.key"
npm run tauri:build
```

Installer path: `src-tauri\target\release\bundle\nsis\*.exe`
