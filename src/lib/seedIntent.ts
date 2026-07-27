import type { SeedSetupIntent } from "./seedIntent";

export type { SeedSetupIntent };

export type SetupStep =
  | "path"
  | "password"
  | "confirm"
  | "security"
  | "words"
  | "backup"
  | "phrase"
  | "passphrase";

export const SETUP_SESSION_KEY = "opal:vault-setup";

export interface SetupSession {
  intent: SeedSetupIntent | null;
  step: SetupStep;
  vaultCreated: boolean;
}

const DEFAULT_SESSION: SetupSession = {
  intent: null,
  step: "path",
  vaultCreated: false,
};

export function readSetupSession(): SetupSession {
  try {
    const raw = sessionStorage.getItem(SETUP_SESSION_KEY);
    if (!raw) return { ...DEFAULT_SESSION };
    const parsed = JSON.parse(raw) as Partial<SetupSession>;
    return {
      intent:
        parsed.intent === "create" || parsed.intent === "restore" ? parsed.intent : null,
      step: typeof parsed.step === "string" ? (parsed.step as SetupStep) : "path",
      vaultCreated: Boolean(parsed.vaultCreated),
    };
  } catch {
    return { ...DEFAULT_SESSION };
  }
}

export function writeSetupSession(next: SetupSession) {
  try {
    sessionStorage.setItem(SETUP_SESSION_KEY, JSON.stringify(next));
  } catch {
    /* private mode */
  }
}

export function clearSetupSession() {
  try {
    sessionStorage.removeItem(SETUP_SESSION_KEY);
    sessionStorage.removeItem("opal:seed-setup-intent");
  } catch {
    /* private mode */
  }
}

export function isSetupSessionActive(): boolean {
  try {
    return sessionStorage.getItem(SETUP_SESSION_KEY) != null;
  } catch {
    return false;
  }
}

/** @deprecated use readSetupSession */
export const SEED_SETUP_INTENT_KEY = "opal:seed-setup-intent";

export function readSeedSetupIntent(): SeedSetupIntent | null {
  return readSetupSession().intent;
}

export function clearSeedSetupIntent() {
  clearSetupSession();
}
