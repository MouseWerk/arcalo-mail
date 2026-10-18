// Security, as in Arcalo: the lock screen (the app behind it is not mounted, so no mail stays in
// the page), the recovery screen of an encrypted database whose key is missing, and Settings →
// Sicherheit (the app lock, the encryption on the device, the phishing warning). The backend
// is src-tauri/src/security.rs.

import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { Copy, Download, Fingerprint, KeyRound, Lock, LogOut, ShieldAlert, ShieldCheck, Trash2 } from "lucide-react";
import { api, flushDrafts, inApp, onEvent, pickSavePath } from "../lib/api";
import { langFromLocale, setLang, useT, type TKey } from "../lib/i18n";
import { applyTheme } from "../lib/theme";
import { showToast } from "../lib/toasts";
import {
  DEFAULT_LOCK,
  formatRecoveryInput,
  formatWait,
  passwordProblem,
  pinProblem,
  recoveryComplete,
  security,
  watchActivity,
  type Biometric,
  type CipherStatus,
  type LockConfig,
  type LockMode,
  type LockStatus,
} from "../lib/security";
import type { Settings } from "../lib/types";
import { ConfirmDialog, Dialog, Segmented, Select, Spinner, Switch } from "./ui";
import { isKey } from "../lib/ime";

const BIOMETRIC: Record<Biometric, TKey> = {
  windows_hello: "lock.windowsHello",
  touch_id: "lock.touchId",
  face_id: "lock.faceId",
  android: "lock.android",
};

/** Removes the lock by hand where nothing else can (README, „App-Sperre“). */
const RESET_COMMAND = `sqlite3 mail.db "DELETE FROM settings WHERE key LIKE 'applock%'"`;

/** Theme and language of the settings for the lock and recovery screens (the app is not loaded). */
function useScreenPrefs() {
  useEffect(() => {
    void Promise.all([api.settings(), api.appInfo()])
      .then(([s, info]) => {
        applyTheme(s.theme);
        setLang(s.language === "system" ? langFromLocale(info.locale || navigator.language) : s.language);
      })
      .catch(() => {})
      .finally(() => document.body.classList.add("ready"));
  }, []);
}

/** The app, the lock screen while locked, or the recovery screen while the database cannot open. */
export function SecurityGate({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<LockStatus | null>(null);
  const refresh = useCallback(() => {
    security.state().then(setStatus, () =>
      // No answer: never block what the app shows.
      setStatus((s) => s ?? { config: DEFAULT_LOCK, has_pin: false, locked: false, wait_secs: 0, failures: 0, biometric: null, encrypted: false, gate: null, password: false, phone: false }),
    );
  }, []);
  useEffect(() => {
    refresh();
    const offs = [onEvent("applock-changed", refresh), onEvent("security-changed", refresh)];
    return () => offs.forEach((off) => off());
  }, [refresh]);
  // Every lock waits for the open message to be saved as a draft: the lock screen closes it.
  useEffect(
    () =>
      onEvent("applock-locking", () => {
        void flushDrafts()
          .then(() => security.lockFlushed())
          .catch(() => {});
      }),
    [],
  );
  const open = !!status && !status.locked && !status.gate;
  // Input in the window resets the idle clock of the lock.
  useEffect(() => {
    if (!open || !status?.has_pin || status.config.mode !== "idle" || !inApp()) return;
    return watchActivity(() => void security.activity().catch(() => {}));
  }, [open, status?.has_pin, status?.config.mode]);
  if (!status) return null;
  if (status.gate) return <KeyGate status={status} />;
  if (status.locked) return <LockScreen status={status} refresh={refresh} />;
  return <>{children}</>;
}

function LockScreen({ status, refresh }: { status: LockStatus; refresh: () => void }) {
  const t = useT();
  useScreenPrefs();
  const [pin, setPin] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [wait, setWait] = useState(status.wait_secs);
  const [forgot, setForgot] = useState(false);
  const [code, setCode] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const bioName = status.biometric ? t(BIOMETRIC[status.biometric]) : null;
  const offerBio = !!bioName && status.config.biometric;

  useEffect(() => {
    if (wait <= 0) return;
    const id = setInterval(() => setWait((w) => Math.max(0, w - 1)), 1000);
    return () => clearInterval(id);
  }, [wait > 0]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (wait === 0) input.current?.focus();
  }, [wait]);

  const viaBio = async () => {
    setBusy(true);
    setError(null);
    try {
      if (await security.unlockBiometric()) refresh();
      else setError(t("lock.bioFailed", { name: bioName ?? "" }));
    } catch {
      setError(t("lock.bioFailed", { name: bioName ?? "" }));
    } finally {
      setBusy(false);
    }
  };

  // On a phone the system's prompt comes up by itself.
  useEffect(() => {
    if (offerBio && status.phone) void viaBio();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!pin || busy || wait > 0) return;
    setBusy(true);
    setError(null);
    try {
      const r = await security.unlock(pin);
      if (r.ok) return refresh();
      setPin("");
      setWait(r.wait_secs);
      setError(r.wait_secs > 0 ? t("lock.wrongWait", { n: r.failures }) : t("lock.wrong"));
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  const reset = async (recovery: string | null) => {
    setBusy(true);
    setError(null);
    try {
      await security.resetLock(recovery);
      refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="lock-screen" role="dialog" aria-modal="true" aria-labelledby="lock-title" data-lock-screen>
      <form className="lock-card" onSubmit={(e) => void submit(e)}>
        <span className="lock-mark" aria-hidden>
          <Lock size={22} strokeWidth={1.75} />
        </span>
        <h1 id="lock-title" className="lock-title">
          {t("lock.title")}
        </h1>
        <p className="lock-sub">{t("lock.sub")}</p>
        <input
          ref={input}
          type="password"
          className="input lock-pin"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          enterKeyHint="go"
          autoComplete="off"
          autoFocus
          value={pin}
          disabled={wait > 0}
          placeholder={t("lock.pin")}
          aria-label={t("lock.pin")}
          aria-invalid={!!error}
          aria-describedby="lock-msg"
          onChange={(e) => setPin(e.target.value)}
        />
        <button type="submit" className="btn btn-primary lock-wide" disabled={!pin || wait > 0 || busy}>
          {busy ? <Spinner /> : <KeyRound size={15} strokeWidth={1.75} aria-hidden />}
          <span>{t("lock.unlock")}</span>
        </button>
        {offerBio && (
          <button type="button" className="btn lock-wide" onClick={() => void viaBio()} disabled={busy}>
            <Fingerprint size={15} strokeWidth={1.75} aria-hidden />
            <span>{t("lock.withBio", { name: bioName ?? "" })}</span>
          </button>
        )}
        <p id="lock-msg" className={`lock-msg${error ? " lock-msg-error" : ""}`} role="status" aria-live="polite">
          {wait > 0 ? t("lock.waiting", { time: formatWait(wait) }) : (error ?? "")}
        </p>
        {!forgot ? (
          <button type="button" className="lock-link" onClick={() => setForgot(true)}>
            {t("lock.forgot")}
          </button>
        ) : (
          <div className="lock-forgot">
            <p>{status.encrypted ? t("lock.forgotEncrypted") : bioName ? t("lock.forgotBio", { name: bioName }) : t("lock.forgotNone")}</p>
            {!status.encrypted && !bioName && <code className="lock-cmd">{RESET_COMMAND}</code>}
            {status.encrypted && (
              <div className="lock-recovery">
                <input
                  className="input lock-code"
                  autoCapitalize="characters"
                  autoCorrect="off"
                  value={code}
                  placeholder={t("keygate.codePlaceholder")}
                  aria-label={t("keygate.code")}
                  spellCheck={false}
                  onChange={(e) => setCode(formatRecoveryInput(e.target.value))}
                />
                <button type="button" className="btn" disabled={!recoveryComplete(code) || busy} onClick={() => void reset(code)}>
                  {t("lock.resetWithKey")}
                </button>
              </div>
            )}
            {bioName && (
              <button type="button" className="btn" disabled={busy} onClick={() => void reset(null)}>
                <Fingerprint size={15} strokeWidth={1.75} aria-hidden />
                <span>{t("lock.resetWithBio", { name: bioName })}</span>
              </button>
            )}
          </div>
        )}
        {!status.phone && (
          <button type="button" className="lock-link lock-quit" onClick={() => void security.gateQuit().catch(() => {})}>
            <LogOut size={13} strokeWidth={1.75} aria-hidden /> {t("lock.quit")}
          </button>
        )}
      </form>
    </div>
  );
}

/** The database is encrypted and its key is missing (or wrong) on this device. */
function KeyGate({ status }: { status: LockStatus }) {
  const t = useT();
  useScreenPrefs();
  const [method, setMethod] = useState<"recovery" | "password">(status.password ? "password" : "recovery");
  const [secret, setSecret] = useState("");
  const [remember, setRemember] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ready = method === "recovery" ? recoveryComplete(secret) : secret.length > 0;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!ready || busy) return;
    setBusy(true);
    setError(null);
    try {
      await security.gateUnlock(method, secret, remember);
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  };
  return (
    <div className="lock-screen" role="dialog" aria-modal="true" aria-labelledby="gate-title" data-key-gate>
      <form className="lock-card lock-card-wide" onSubmit={(e) => void submit(e)}>
        <span className="lock-mark" aria-hidden>
          <KeyRound size={22} strokeWidth={1.75} />
        </span>
        <h1 id="gate-title" className="lock-title">
          {t("keygate.title")}
        </h1>
        <p className="lock-sub">{status.gate === "wrong_key" ? t("keygate.wrongKey") : t("keygate.missingKey")}</p>
        {status.password && (
          <Segmented
            label={t("keygate.method")}
            value={method}
            options={[
              ["password", t("keygate.password")],
              ["recovery", t("keygate.recovery")],
            ]}
            onChange={(m) => {
              setMethod(m);
              setSecret("");
            }}
          />
        )}
        {method === "recovery" ? (
          <input
            className="input lock-code"
            autoFocus
            value={secret}
            placeholder={t("keygate.codePlaceholder")}
            aria-label={t("keygate.code")}
            spellCheck={false}
            onChange={(e) => setSecret(formatRecoveryInput(e.target.value))}
          />
        ) : (
          <input className="input" type="password" autoFocus value={secret} aria-label={t("keygate.password")} placeholder={t("keygate.password")} onChange={(e) => setSecret(e.target.value)} />
        )}
        <div className="lock-switch">
          <Switch label={t("keygate.remember")} checked={remember} onChange={setRemember} />
          <span>{t("keygate.remember")}</span>
        </div>
        <button type="submit" className="btn btn-primary lock-wide" disabled={!ready || busy}>
          {busy && <Spinner />}
          <span>{t("keygate.open")}</span>
        </button>
        <p className={`lock-msg${error ? " lock-msg-error" : ""}`} role="status" aria-live="polite">
          {error ?? t("keygate.noKeyNoData")}
        </p>
        <button type="button" className="lock-link lock-quit" onClick={() => void security.gateQuit().catch(() => {})}>
          <LogOut size={13} strokeWidth={1.75} aria-hidden /> {t("lock.quit")}
        </button>
      </form>
    </div>
  );
}

// ---------------------------------------------------------------- settings

function Row({ label, description, children }: { label: string; description?: string; children: ReactNode }) {
  return (
    <div className="set-row">
      <div className="set-text">
        <div className="set-label">{label}</div>
        {description && <div className="set-desc">{description}</div>}
      </div>
      <div className="set-control">{children}</div>
    </div>
  );
}

const IDLE_MINUTES = [1, 2, 5, 10, 15, 30, 60, 120];

/** „PIN festlegen“ / „PIN ändern“: twice, 4 to 64 characters. */
function PinDialog({ onDone, onClose }: { onDone: (pin: string) => Promise<void>; onClose: () => void }) {
  const t = useT();
  const [pin, setPin] = useState("");
  const [repeat, setRepeat] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const problem = pinProblem(pin, repeat);
  const save = async () => {
    if (problem) return setError(t(`sec.pin.${problem}` as TKey));
    setBusy(true);
    try {
      await onDone(pin);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={t("sec.pinTitle")}
      description={t("sec.pinHint")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="button" className="btn btn-primary" disabled={busy || !pin} onClick={() => void save()} data-pin-save>
            {busy && <Spinner />}
            <span>{t("sec.pinSave")}</span>
          </button>
        </>
      }
    >
      <label className="stack-field">
        <span className="field-label">{t("sec.pin")}</span>
        <input className="input" type="password" autoFocus autoComplete="new-password" value={pin} onChange={(e) => setPin(e.target.value)} data-pin-new />
      </label>
      <label className="stack-field">
        <span className="field-label">{t("sec.pinRepeat")}</span>
        <input
          className="input"
          type="password"
          autoComplete="new-password"
          value={repeat}
          onChange={(e) => setRepeat(e.target.value)}
          onKeyDown={(e) => isKey(e, "Enter") && void save()}
          data-pin-repeat
        />
      </label>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}

/** The recovery key: shown before encrypting (confirmed as kept) and again on request. */
function RecoveryDialog({ code, confirm, onConfirm, onClose }: { code: string; confirm: boolean; onConfirm?: () => void; onClose: () => void }) {
  const t = useT();
  const [kept, setKept] = useState(false);
  const copy = () => {
    void navigator.clipboard?.writeText(code).then(
      () => showToast({ title: t("sec.recoveryCopied"), duration: 2500 }),
      () => {},
    );
  };
  const save = async () => {
    const path = await pickSavePath(t("sec.recoveryFile"));
    if (!path) return;
    try {
      await security.saveRecovery(path, code);
      showToast({ title: t("sec.recoverySaved"), duration: 3000, tone: "success" });
    } catch (e) {
      showToast({ title: String(e), tone: "danger" });
    }
  };
  return (
    <Dialog
      title={t("sec.recoveryTitle")}
      description={t("sec.recoveryHint")}
      onClose={onClose}
      wide
      footer={
        confirm ? (
          <>
            <button type="button" className="btn" onClick={onClose}>
              {t("common.cancel")}
            </button>
            <button type="button" className="btn btn-primary" disabled={!kept} onClick={onConfirm} data-encrypt-confirm>
              <Lock size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("sec.encryptNow")}</span>
            </button>
          </>
        ) : (
          <button type="button" className="btn btn-primary" onClick={onClose}>
            {t("common.close")}
          </button>
        )
      }
    >
      <code className="sec-recovery" data-recovery-code>
        {code.split("-").map((g, i, all) => (
          <span key={i}>{i < all.length - 1 ? `${g}-` : g}</span>
        ))}
      </code>
      <div className="sec-actions">
        <button type="button" className="btn btn-sm" onClick={copy}>
          <Copy size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("sec.copy")}</span>
        </button>
        {inApp() && (
          <button type="button" className="btn btn-sm" onClick={() => void save()}>
            <Download size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("sec.saveFile")}</span>
          </button>
        )}
      </div>
      {confirm && (
        <div className="lock-switch" data-recovery-kept>
          <Switch label={t("sec.recoveryKept")} checked={kept} onChange={setKept} />
          <span>{t("sec.recoveryKept")}</span>
        </div>
      )}
    </Dialog>
  );
}

/** „Schlüssel mit Passwort schützen“: twice, at least 10 characters. */
function PasswordDialog({ onClose }: { onClose: () => void }) {
  const t = useT();
  const [pw, setPw] = useState("");
  const [repeat, setRepeat] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    const problem = passwordProblem(pw, repeat);
    if (problem) return setError(t(`sec.password.${problem}` as TKey));
    setBusy(true);
    try {
      await security.setPassword(pw);
      showToast({ title: t("sec.passwordSet"), duration: 3000, tone: "success" });
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={t("sec.passwordTitle")}
      description={t("sec.passwordHint")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="button" className="btn btn-primary" disabled={busy || !pw} onClick={() => void save()}>
            {busy && <Spinner />}
            <span>{t("sec.passwordSave")}</span>
          </button>
        </>
      }
    >
      <label className="stack-field">
        <span className="field-label">{t("sec.password")}</span>
        <input className="input" type="password" autoFocus autoComplete="new-password" value={pw} onChange={(e) => setPw(e.target.value)} />
      </label>
      <label className="stack-field">
        <span className="field-label">{t("sec.passwordRepeat")}</span>
        <input className="input" type="password" autoComplete="new-password" value={repeat} onChange={(e) => setRepeat(e.target.value)} />
      </label>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}

/** Settings → Sicherheit. */
export function SecuritySection({ settings, onChange }: { settings: Settings; onChange: (patch: Partial<Settings>) => void }) {
  const t = useT();
  const [lock, setLock] = useState<LockStatus | null>(null);
  const [cipher, setCipher] = useState<CipherStatus | null>(null);
  const [dialog, setDialog] = useState<null | { kind: "pin"; next: LockConfig } | { kind: "recovery"; code: string; confirm: boolean } | { kind: "password" } | { kind: "decrypt" }>(null);
  const [switching, setSwitching] = useState<null | "encrypt" | "decrypt">(null);
  const load = useCallback(() => {
    void security.lockStatus().then(setLock, () => {});
    void security.cipherStatus().then(
      (c) => {
        setCipher(c);
        if (c.notice) showToast({ title: c.notice, duration: 6000 });
      },
      () => {},
    );
  }, []);
  useEffect(load, [load]);
  if (!lock || !cipher) return <Spinner />;

  const config = lock.config;
  const bioName = lock.biometric ? t(BIOMETRIC[lock.biometric]) : null;
  const configure = async (next: LockConfig, pin: string | null = null) => {
    try {
      setLock(await security.configureLock(next, pin));
    } catch (e) {
      showToast({ title: t("sec.failed"), detail: String(e), tone: "danger" });
      throw e;
    }
  };
  const setMode = (mode: LockMode) => {
    const next = { ...config, mode };
    if (mode !== "off" && !lock.has_pin) return setDialog({ kind: "pin", next });
    void configure(next).then(() => {
      if (mode === "off") showToast({ title: t("sec.lockOff"), duration: 3000 });
    }, () => {});
  };
  const encrypt = async () => {
    try {
      const code = await security.recoveryKey(true);
      setDialog({ kind: "recovery", code, confirm: true });
    } catch (e) {
      showToast({ title: t("sec.failed"), detail: String(e), tone: "danger" });
    }
  };
  const doSwitch = async (on: boolean) => {
    setDialog(null);
    setSwitching(on ? "encrypt" : "decrypt");
    try {
      setCipher(await security.switchCipher(on));
      showToast({ title: on ? t("sec.encrypted") : t("sec.decrypted"), tone: "success", duration: 5000 });
    } catch (e) {
      showToast({ title: t("sec.switchFailed"), detail: String(e), tone: "danger" });
    } finally {
      setSwitching(null);
      load();
    }
  };
  const showRecovery = async () => {
    try {
      setDialog({ kind: "recovery", code: await security.recoveryKey(false), confirm: false });
    } catch (e) {
      showToast({ title: t("sec.failed"), detail: String(e), tone: "danger" });
    }
  };
  const encrypted = cipher.state === "encrypted";

  return (
    <div className="set-section" data-security>
      <div className="set-card">
        <div className="set-card-title">{t("sec.lock")}</div>
        <Row label={t("sec.lockMode")} description={lock.phone ? t("sec.lockModeHintPhone") : t("sec.lockModeHint")}>
          <Segmented
            label={t("sec.lockMode")}
            value={config.mode}
            options={[
              ["off", t("sec.mode.off")],
              ["start", t("sec.mode.start")],
              ["idle", lock.phone ? t("sec.mode.idlePhone") : t("sec.mode.idle")],
            ]}
            onChange={setMode}
          />
        </Row>
        {config.mode === "idle" && (
          <Row label={lock.phone ? t("sec.idleAfterPhone") : t("sec.idleAfter")}>
            <Select className="select" aria-label={t("sec.idleAfter")} value={config.idle_minutes} onChange={(e) => void configure({ ...config, idle_minutes: Number(e.target.value) }).catch(() => {})}>
              {IDLE_MINUTES.map((n) => (
                <option key={n} value={n}>
                  {t("sec.minutes", { n })}
                </option>
              ))}
            </Select>
          </Row>
        )}
        {config.mode !== "off" && (
          <>
            <Row label={lock.phone ? t("sec.onLeave") : t("sec.onSleep")} description={lock.phone ? t("sec.onLeaveHint") : t("sec.onSleepHint")}>
              <Switch label={lock.phone ? t("sec.onLeave") : t("sec.onSleep")} checked={config.on_sleep} onChange={(on_sleep) => void configure({ ...config, on_sleep }).catch(() => {})} />
            </Row>
            {bioName && (
              <Row label={t("sec.biometric", { name: bioName })} description={t("sec.biometricHint")}>
                <Switch label={t("sec.biometric", { name: bioName })} checked={config.biometric} onChange={(biometric) => void configure({ ...config, biometric }).catch(() => {})} />
              </Row>
            )}
            <Row label={t("sec.pin")} description={t("sec.privateHint")}>
              <div className="sec-actions">
                <button type="button" className="btn btn-sm" onClick={() => setDialog({ kind: "pin", next: config })}>
                  {t("sec.pinChange")}
                </button>
                <button type="button" className="btn btn-sm" onClick={() => void security.lockNow().catch(() => {})} data-lock-now>
                  <Lock size={14} strokeWidth={1.75} aria-hidden />
                  <span>{t("sec.lockNow")}</span>
                </button>
              </div>
            </Row>
          </>
        )}
      </div>

      <div className="set-card" data-encryption>
        <div className="set-card-title">{t("sec.atRest")}</div>
        <div className={`notice${encrypted ? "" : " notice-plain"}`} role="status">
          {encrypted ? <ShieldCheck size={16} strokeWidth={1.75} aria-hidden /> : <ShieldAlert size={16} strokeWidth={1.75} aria-hidden />}
          <span data-cipher-state={cipher.state}>{encrypted ? t("sec.stateOn") : t("sec.stateOff")}</span>
        </div>
        <p className="set-desc">{t("sec.atRestHint")}</p>
        {cipher.store_file && <p className="set-desc">{t("sec.storeFile")}</p>}
        {!encrypted ? (
          <div className="set-actions-left">
            <button type="button" className="btn btn-primary" disabled={!!switching} onClick={() => void encrypt()} data-encrypt>
              <Lock size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("sec.encrypt")}</span>
            </button>
          </div>
        ) : (
          <>
            <Row label={t("sec.recovery")} description={t("sec.recoveryRowHint")}>
              <button type="button" className="btn btn-sm" onClick={() => void showRecovery()}>
                {t("sec.recoveryShow")}
              </button>
            </Row>
            <Row label={t("sec.passwordRow")} description={cipher.password ? t("sec.passwordOn") : t("sec.passwordRowHint")}>
              <div className="sec-actions">
                <button type="button" className="btn btn-sm" onClick={() => setDialog({ kind: "password" })}>
                  {cipher.password ? t("sec.passwordChange") : t("sec.passwordSetUp")}
                </button>
                {cipher.password && (
                  <button type="button" className="btn btn-sm btn-quiet" onClick={() => void security.setPassword(null).then(load)}>
                    {t("sec.passwordRemove")}
                  </button>
                )}
              </div>
            </Row>
            <div className="set-actions-left">
              <button type="button" className="btn btn-danger-quiet" disabled={!!switching} onClick={() => setDialog({ kind: "decrypt" })} data-decrypt>
                <span>{t("sec.decrypt")}</span>
              </button>
            </div>
          </>
        )}
        {cipher.old_left && (
          <Row label={t("sec.oldLeft")} description={t("sec.oldLeftHint")}>
            <button type="button" className="btn btn-sm" onClick={() => void security.dropOld().then(load)}>
              <Trash2 size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("sec.dropOld")}</span>
            </button>
          </Row>
        )}
      </div>

      <div className="set-card">
        <div className="set-card-title">{t("sec.phishing")}</div>
        <Row label={t("sec.phishingWarn")} description={t("sec.phishingHint")}>
          <Switch label={t("sec.phishingWarn")} checked={settings.phishing_warning} onChange={(phishing_warning) => onChange({ phishing_warning })} />
        </Row>
      </div>

      {dialog?.kind === "pin" && (
        <PinDialog
          onDone={(pin) => configure(dialog.next.mode === "off" ? { ...dialog.next, mode: "start" } : dialog.next, pin)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === "recovery" && <RecoveryDialog code={dialog.code} confirm={dialog.confirm} onConfirm={() => void doSwitch(true)} onClose={() => setDialog(null)} />}
      {dialog?.kind === "password" && (
        <PasswordDialog
          onClose={() => {
            setDialog(null);
            load();
          }}
        />
      )}
      {dialog?.kind === "decrypt" && (
        <ConfirmDialog title={t("sec.decryptTitle")} text={t("sec.decryptText")} confirm={t("sec.decrypt")} danger onConfirm={() => void doSwitch(false)} onClose={() => setDialog(null)} />
      )}
      {switching && (
        <div className="overlay" role="alertdialog" aria-modal="true" aria-label={switching === "encrypt" ? t("sec.encrypting") : t("sec.decrypting")} data-switching>
          <div className="dialog sec-busy">
            <Spinner />
            <p>{switching === "encrypt" ? t("sec.encrypting") : t("sec.decrypting")}</p>
          </div>
        </div>
      )}
    </div>
  );
}
