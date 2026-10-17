// Settings → Sicherheit, the lock screen and the recovery screen (src-tauri/src/security.rs):
// typed commands and the small pieces of logic they share (tested in security.test.ts). In a
// plain browser (`npm run dev`) nothing is locked or encrypted.

import { invoke } from "@tauri-apps/api/core";
import { inApp } from "./api";

export type LockMode = "off" | "start" | "idle";
export interface LockConfig {
  mode: LockMode;
  idle_minutes: number;
  on_sleep: boolean;
  biometric: boolean;
}
export type Biometric = "windows_hello" | "touch_id" | "face_id" | "android";
export type GateReason = "missing_key" | "wrong_key";
export interface LockStatus {
  config: LockConfig;
  has_pin: boolean;
  locked: boolean;
  wait_secs: number;
  failures: number;
  biometric: Biometric | null;
  encrypted: boolean;
  gate: GateReason | null;
  password: boolean;
  phone: boolean;
}
export interface UnlockReply {
  ok: boolean;
  wait_secs: number;
  failures: number;
}
export type FileState = "missing" | "plain" | "encrypted";
export interface CipherMigration {
  direction: "encrypt" | "decrypt";
  step: "requested" | "verified" | "swapped";
  requested: number;
}
export interface CipherStatus {
  state: FileState;
  key_stored: boolean;
  store_file: boolean;
  password: boolean;
  pending: CipherMigration | null;
  old_left: boolean;
  notice: string | null;
}

/** What a command refused while locked (or while the encryption is switched) answers. */
export const LOCKED_ERROR = "app-locked";

export const DEFAULT_LOCK: LockConfig = { mode: "off", idle_minutes: 5, on_sleep: true, biometric: false };

const demoStatus = (): LockStatus => ({
  config: DEFAULT_LOCK,
  has_pin: false,
  locked: false,
  wait_secs: 0,
  failures: 0,
  biometric: null,
  encrypted: false,
  gate: null,
  password: false,
  phone: false,
});
const demoCipher = (): CipherStatus => ({ state: "plain", key_stored: false, store_file: false, password: false, pending: null, old_left: false, notice: null });

function call<T>(cmd: string, args: Record<string, unknown>, fallback: () => T): Promise<T> {
  return inApp() ? invoke<T>(cmd, args) : Promise.resolve().then(fallback);
}

export const security = {
  state: () => call<LockStatus>("security_state", {}, demoStatus),
  lockStatus: () => call<LockStatus>("applock_status", {}, demoStatus),
  configureLock: (config: LockConfig, pin: string | null) => call<LockStatus>("applock_configure", { config, pin }, demoStatus),
  unlock: (pin: string) => call<UnlockReply>("applock_unlock", { pin }, () => ({ ok: true, wait_secs: 0, failures: 0 })),
  unlockBiometric: () => call<boolean>("applock_unlock_biometric", {}, () => false),
  resetLock: (recovery: string | null) => call<void>("applock_reset", { recovery }, () => undefined),
  lockNow: () => call<void>("applock_lock_now", {}, () => undefined),
  lockFlushed: () => call<void>("applock_flushed", {}, () => undefined),
  activity: () => call<void>("applock_activity", {}, () => undefined),
  cipherStatus: () => call<CipherStatus>("cipher_status", {}, demoCipher),
  recoveryKey: (create: boolean) => call<string>("cipher_recovery_key", { create }, () => "ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567-ABCD-EFGH-IJKL-MNOP-QRST-UVWX"),
  saveRecovery: (path: string, code: string) => call<void>("cipher_recovery_save", { path, code }, () => undefined),
  switchCipher: (encrypt: boolean) => call<CipherStatus>("cipher_switch", { encrypt }, demoCipher),
  setPassword: (password: string | null) => call<void>("cipher_password", { password }, () => undefined),
  dropOld: () => call<void>("cipher_drop_old", {}, () => undefined),
  gateUnlock: (method: "recovery" | "password", secret: string, remember: boolean) => call<void>("keygate_unlock", { method, secret, remember }, () => undefined),
  gateQuit: () => call<void>("keygate_quit", {}, () => undefined),
};

/**
 * A recovery key as typed: upper case, look-alikes read as the code's letters (0 → O, 1 → I,
 * 8 → B), groups of four joined by dashes. Same rules as the backend's parser.
 */
export function formatRecoveryInput(raw: string): string {
  const clean = raw
    .toUpperCase()
    .replace(/[\s-]/g, "")
    .replace(/0/g, "O")
    .replace(/1/g, "I")
    .replace(/8/g, "B")
    .replace(/[^A-Z2-7]/g, "")
    .slice(0, 56);
  return clean.match(/.{1,4}/g)?.join("-") ?? "";
}

/** Whether a typed recovery key has all its characters (the checksum is checked by the backend). */
export const recoveryComplete = (code: string) => code.replace(/-/g, "").length === 56;

/** Seconds as „0:45“ or „2:05“ (the wait after wrong PINs). */
export function formatWait(secs: number): string {
  const s = Math.max(0, Math.ceil(secs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/** Whether a new PIN is usable: 4 to 64 characters, both entries equal. */
export function pinProblem(pin: string, repeat: string): "short" | "long" | "mismatch" | null {
  const n = [...pin].length;
  if (n < 4) return "short";
  if (n > 64) return "long";
  return pin === repeat ? null : "mismatch";
}

/** Whether a password for the wrapped key is usable: at least 10 characters, both entries equal. */
export function passwordProblem(pw: string, repeat: string): "short" | "mismatch" | null {
  if ([...pw].length < 10) return "short";
  return pw === repeat ? null : "mismatch";
}

/** The rejection of a command while locked. */
export const isLockedError = (e: unknown) => e === LOCKED_ERROR || String(e) === LOCKED_ERROR;

/**
 * Reports input in the window to the lock's idle clock, at most every `every` ms. Returns the
 * function that stops it.
 */
export function watchActivity(report: () => void, every = 15000, target: Pick<Window, "addEventListener" | "removeEventListener"> = window): () => void {
  let last = 0;
  const seen = () => {
    const now = Date.now();
    if (now - last < every) return;
    last = now;
    report();
  };
  const events = ["keydown", "pointerdown", "pointermove", "wheel", "touchstart"] as const;
  for (const e of events) target.addEventListener(e, seen, { passive: true });
  return () => {
    for (const e of events) target.removeEventListener(e, seen);
  };
}
