import { spawn } from "node:child_process";
import path from "node:path";
import os from "node:os";
import fs from "node:fs";

const cargoBin = path.join(os.homedir(), ".cargo", "bin");
const pathKey = process.platform === "win32" ? "Path" : "PATH";
const current = process.env[pathKey] ?? process.env.PATH ?? "";
if (!current.toLowerCase().includes(cargoBin.toLowerCase())) {
  process.env[pathKey] = `${cargoBin}${path.delimiter}${current}`;
  process.env.PATH = process.env[pathKey];
}

const cargoExe =
  process.platform === "win32"
    ? path.join(cargoBin, "cargo.exe")
    : path.join(cargoBin, "cargo");

if (!fs.existsSync(cargoExe)) {
  console.error(
    `[opal] cargo not found at ${cargoExe}. Install Rust from https://rustup.rs and reopen the terminal.`,
  );
  process.exit(1);
}

const args = process.argv.slice(2);
const tauriCli = path.join(
  process.cwd(),
  "node_modules",
  "@tauri-apps",
  "cli",
  process.platform === "win32" ? "tauri.js" : "tauri.js",
);

const child = spawn(process.execPath, [tauriCli, ...args], {
  stdio: "inherit",
  env: process.env,
  shell: false,
});

child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  process.exit(code ?? 1);
});
