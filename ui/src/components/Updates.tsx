// Updates, as in Arcalo: the checks (shortly after the start, then every few hours), the notice of
// a downloaded update („Update 1.0.1 bereit – wird beim Beenden installiert“, „Jetzt neu
// starten“), of a new version in the mode „Nur benachrichtigen“ („Installieren“, with the download
// to cancel), „Später erinnern“ and „Diese Version überspringen“, the reason and the release page
// for a copy that cannot install itself, „Neu in Arcalo Mail“ after an update, and the updates
// card of Settings → Über. On Android the APK of the GitHub release gets a notice with
// „Herunterladen“; the Play build never checks. Builds without an update key never check.

import { useEffect, useState, useSyncExternalStore, type MouseEvent, type ReactNode } from "react";
import { Download, ExternalLink, MoreHorizontal, RefreshCw, Sparkles, X } from "lucide-react";
import { api, inApp, onEvent, onUpdateProgress, openExternal } from "../lib/api";
import { currentLang, t, useT } from "../lib/i18n";
import { showToast } from "../lib/toasts";
import { apkNotice, CHECK_EVERY_MS, FIRST_CHECK_DELAY_MS, megabytes, notesBlocks, REMIND_DAYS, updateNotice, whatsNew } from "../lib/updates";
import type { ApkStatus, UpdateMode, UpdateProgress, UpdateStatus } from "../lib/types";
import { Dialog, Menu, Segmented, Spinner, type MenuItem } from "./ui";

/** `preparing`: an open message is saved before the download or the restart. */
type Phase = "idle" | "checking" | "preparing" | "downloading" | "installing";

type State = {
  status: UpdateStatus | null;
  apk: ApkStatus | null;
  phase: Phase;
  progress: UpdateProgress | null;
  /** The version whose notice was closed this session. */
  dismissed: string | null;
  /** „Neuigkeiten“ of the offered version is open. */
  notesOpen: boolean;
  /** „Neu in Arcalo Mail“ after an update. */
  whatsNew: { version: string; notes: string; url: string } | null;
};

let state: State = { status: null, apk: null, phase: "idle", progress: null, dismissed: null, notesOpen: false, whatsNew: null };
const listeners = new Set<() => void>();
const set = (patch: Partial<State>) => {
  state = { ...state, ...patch };
  listeners.forEach((f) => f());
};
const subscribe = (f: () => void) => {
  listeners.add(f);
  return () => listeners.delete(f);
};
const useUpdates = () => useSyncExternalStore(subscribe, () => state, () => state);

// End-to-end runs read the state (their builds have a test feed).
if (typeof navigator !== "undefined" && navigator.webdriver) (window as unknown as { __arcaloMailUpdates?: () => State }).__arcaloMailUpdates = () => state;

/** Saves what must not be lost before the app restarts for an update (the open message as a
 *  draft); App registers it. */
let beforeRestart: () => Promise<void> = async () => {};
export function setBeforeRestart(f: () => Promise<void>) {
  beforeRestart = f;
}

const fail = (title: string, e: unknown) => showToast({ title, detail: String(e), tone: "danger", duration: 8000 });

export async function refreshStatus(): Promise<UpdateStatus | null> {
  try {
    const status = await api.updateStatus();
    set({ status });
    return status;
  } catch {
    return null;
  }
}

/** The status at the start: the first start after an update says so, once. */
async function loadStatus(): Promise<void> {
  const status = await refreshStatus();
  const r = status?.restarted;
  if (!status || !r) return;
  if (r.installed) {
    const wn = whatsNew(status);
    showToast({ title: t("upd.restarted", { version: r.version }), tone: "success", action: wn ? { label: t("upd.whatsNew"), run: () => set({ whatsNew: wn }) } : undefined });
    if (wn) set({ whatsNew: wn });
  } else showToast({ title: t("upd.notInstalled", { version: r.version }), detail: t("upd.notInstalledDetail", { current: status.current_version }), duration: 0 });
}

let running: Promise<void> | null = null;

/** Looks for a newer release. `manual` tells every outcome; automatic checks stay quiet. */
export function checkForUpdates(manual: boolean): Promise<void> {
  if (running) return manual ? running.then(() => checkForUpdates(true)) : running;
  const p = (async () => {
    const status = state.status ?? (await refreshStatus());
    if (!status?.enabled) {
      if (manual) showToast({ title: t("upd.notConfigured") });
      return;
    }
    if (state.phase !== "idle") return;
    set({ phase: "checking" });
    try {
      const found = await api.updateCheck(manual);
      if (manual) set({ dismissed: null });
      const fresh = await refreshStatus();
      if (!found && manual) showToast({ title: t("upd.upToDate"), detail: t("upd.upToDateDetail", { version: fresh?.current_version ?? status.current_version }), tone: "success" });
    } catch (e) {
      if (manual) fail(t("upd.checkFailed"), e);
    } finally {
      set({ phase: "idle" });
    }
  })();
  running = p;
  void p.finally(() => {
    if (running === p) running = null;
  });
  return p;
}

/** „Jetzt neu starten“: the open message is saved, then the downloaded update installs. */
export async function restartNow() {
  if (state.phase !== "idle") return;
  set({ phase: "preparing" });
  try {
    await beforeRestart();
    set({ phase: "installing" });
    await api.updateRestartNow();
  } catch (e) {
    set({ phase: "idle" });
    fail(t("upd.failed"), e);
  }
}

let cancelled = false;

/** „Installieren“ (mode „Nur benachrichtigen“): saves the open message, downloads, installs and
 *  restarts. */
export async function installUpdate() {
  if (state.phase !== "idle" || !state.status?.available) return;
  set({ phase: "preparing", progress: null, notesOpen: false });
  cancelled = false;
  try {
    await beforeRestart();
    set({ phase: "downloading" });
    await api.updateInstall();
    set({ phase: "installing" });
  } catch (e) {
    set({ phase: "idle", progress: null });
    // „Abbrechen“: the download stopped as asked; its part stays for the next try.
    if (!cancelled) fail(t("upd.failed"), e);
    void refreshStatus();
  }
}

/** „Abbrechen“ while „Installieren“ downloads. */
async function cancelInstall() {
  cancelled = true;
  await api.updatePause().catch(() => {});
}

async function remindLater(days: number) {
  try {
    const at = await api.updateRemind(days);
    set({ notesOpen: false });
    await refreshStatus();
    showToast({ title: t("upd.reminded", { date: new Date(at).toLocaleDateString(currentLang() === "de" ? "de-DE" : "en-GB") }) });
  } catch (e) {
    fail(t("upd.failed"), e);
  }
}

async function skipVersion(version: string) {
  try {
    await api.updateSkip(version);
    set({ notesOpen: false });
    await refreshStatus();
    showToast({
      title: t("upd.skipped", { version }),
      action: {
        label: t("common.undo"),
        run: () => void api.updateUnskip().then(() => checkForUpdates(false)),
      },
    });
  } catch (e) {
    fail(t("upd.failed"), e);
  }
}

const laterItems = (version: string): MenuItem[] => [
  { label: t("upd.remindTomorrow"), onSelect: () => void remindLater(REMIND_DAYS.tomorrow) },
  { label: t("upd.remindNextWeek"), onSelect: () => void remindLater(REMIND_DAYS.nextWeek) },
  "separator",
  { label: t("upd.skip"), onSelect: () => void skipVersion(version) },
];

/** Starts the checks (desktop) or the Android notice; returns the cleanup. */
export function startUpdates(platform: string): () => void {
  if (!inApp()) return () => {};
  if (platform === "android") {
    const load = (apk: ApkStatus | null) => {
      set({ apk });
      if (apk?.updated_from && apk.notes) set({ whatsNew: { version: apk.current, notes: apk.notes, url: apk.offered?.page ?? "" } });
      else if (apk?.updated_from) void api.apkWhatsNewSeen();
    };
    void api.apkUpdateStatus().then(load, () => {});
    return onEvent<ApkStatus>("apkupdate://status", (apk) => set({ apk }));
  }
  if (platform === "ios") return () => {};
  void loadStatus();
  const first = setTimeout(() => void checkForUpdates(false), FIRST_CHECK_DELAY_MS);
  const every = setInterval(() => void checkForUpdates(false), CHECK_EVERY_MS);
  // The background download reports each step; the status follows (at most every 300 ms).
  let pending: ReturnType<typeof setTimeout> | undefined;
  const refresh = () => {
    if (pending) return;
    pending = setTimeout(() => {
      pending = undefined;
      void refreshStatus();
    }, 300);
  };
  const offState = onEvent("update://state", refresh);
  const offProgress = onUpdateProgress((progress) => {
    set({ progress });
    refresh();
  });
  return () => {
    clearTimeout(first);
    clearInterval(every);
    clearTimeout(pending);
    offState();
    offProgress();
  };
}

/** A small menu below a button. */
function useMenuAt(): [ReactNode, (e: MouseEvent, items: MenuItem[]) => void] {
  const [menu, setMenu] = useState<{ at: { x: number; y: number }; items: MenuItem[] } | null>(null);
  const node = menu ? <Menu items={menu.items} at={menu.at} label={t("upd.later")} onClose={() => setMenu(null)} /> : null;
  const open = (e: MouseEvent, items: MenuItem[]) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    setMenu({ at: { x: r.left, y: r.bottom + 4 }, items });
  };
  return [node, open];
}

const percentText = (p: UpdateProgress | null) => (p?.percent != null ? `${p.percent} %` : t("upd.preparingDownload"));

/** The desktop's floating notice: a downloaded update, a version to install, or one to download by hand. */
function UpdateNotice() {
  const t = useT();
  const { status, phase, progress, dismissed } = useUpdates();
  const [menu, openMenu] = useMenuAt();
  const notice = updateNotice(status, dismissed);
  const busy = phase === "preparing" || phase === "downloading" || phase === "installing";
  if (notice.kind === "none" && !busy) return null;
  const version = notice.kind === "none" ? (status?.available?.version ?? "") : notice.version;
  if (busy && notice.kind !== "ready")
    return (
      <div className="update-card" role="status" data-state={phase}>
        <Download size={16} strokeWidth={1.75} aria-hidden className="update-card-icon" />
        <div className="update-card-body">
          <div className="update-card-title">{phase === "installing" ? t("upd.installing", { version }) : t("upd.downloading", { version })}</div>
          <div className="update-card-detail">{phase === "installing" ? t("upd.restartSoon") : phase === "preparing" ? t("upd.preparing") : percentText(progress)}</div>
          {phase === "downloading" && (
            <>
              <div className="update-bar" aria-hidden>
                <span style={{ width: `${progress?.percent ?? 0}%` }} />
              </div>
              <div className="update-card-actions">
                <button type="button" className="btn btn-sm" onClick={() => void cancelInstall()}>
                  {t("common.cancel")}
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    );
  const close = () => set({ dismissed: version });
  return (
    <div className="update-card" role="status" data-state={notice.kind}>
      <Sparkles size={16} strokeWidth={1.75} aria-hidden className="update-card-icon" />
      <div className="update-card-body">
        <div className="update-card-title">{notice.kind === "ready" ? t("upd.readyTitle", { version }) : t("upd.availableTitle", { version })}</div>
        <div className="update-card-detail">
          {notice.kind === "ready" ? t("upd.readyDetail") : notice.kind === "available" && notice.manual ? status?.manual_text : t("upd.availableDetail")}
        </div>
        {status?.waiting ? <div className="update-card-detail">{t("upd.waiting", { n: status.waiting })}</div> : null}
        <div className="update-card-actions">
          {notice.kind === "ready" && (
            <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => void restartNow()}>
              {busy ? <Spinner /> : <RefreshCw size={14} strokeWidth={1.75} aria-hidden />}
              <span>{t("upd.restartNow")}</span>
            </button>
          )}
          {notice.kind === "available" && !notice.manual && (
            <button type="button" className="btn btn-sm btn-primary" onClick={() => void installUpdate()}>
              <Download size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("upd.install")}</span>
            </button>
          )}
          {notice.kind === "available" && notice.manual && (
            <button type="button" className="btn btn-sm btn-primary" onClick={() => void openExternal(status?.available?.url ?? status?.release_page ?? "")}>
              <ExternalLink size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("upd.releasePage")}</span>
            </button>
          )}
          {status?.available?.notes && (
            <button type="button" className="btn btn-sm" onClick={() => set({ notesOpen: true })}>
              {t("upd.whatsNew")}
            </button>
          )}
          <button type="button" className="icon-btn icon-btn-sm" aria-haspopup="menu" aria-label={t("upd.laterOptions")} title={t("upd.laterOptions")} onClick={(e) => openMenu(e, laterItems(version))}>
            <MoreHorizontal size={14} strokeWidth={1.75} aria-hidden />
          </button>
        </div>
      </div>
      <button type="button" className="icon-btn icon-btn-sm" aria-label={t("upd.later")} title={t("upd.later")} onClick={close}>
        <X size={14} strokeWidth={1.75} aria-hidden />
      </button>
      {menu}
    </div>
  );
}

/** The Android notice: a newer APK of the GitHub release. */
function ApkNotice() {
  const t = useT();
  const { apk, dismissed } = useUpdates();
  const offered = apkNotice(apk, dismissed);
  if (!offered || !apk) return null;
  const size = megabytes(offered.size, currentLang() === "de" ? "de-DE" : "en-GB");
  return (
    <div className="update-card" role="status" data-state={apk.phase}>
      <Sparkles size={16} strokeWidth={1.75} aria-hidden className="update-card-icon" />
      <div className="update-card-body">
        <div className="update-card-title">{t("apk.title", { version: offered.version })}</div>
        <div className="update-card-detail">{apk.phase === "blocked" ? apk.blocked : t("apk.detail")}</div>
        <div className="update-card-actions">
          <button type="button" className="btn btn-sm btn-primary" onClick={() => void openExternal(apk.phase === "blocked" ? offered.page : offered.apk)}>
            {apk.phase === "blocked" ? <ExternalLink size={14} strokeWidth={1.75} aria-hidden /> : <Download size={14} strokeWidth={1.75} aria-hidden />}
            <span>{apk.phase === "blocked" ? t("upd.releasePage") : size ? t("apk.downloadSize", { size }) : t("apk.download")}</span>
          </button>
          {offered.notes && (
            <button type="button" className="btn btn-sm" onClick={() => set({ notesOpen: true })}>
              {t("upd.whatsNew")}
            </button>
          )}
        </div>
      </div>
      <button type="button" className="icon-btn icon-btn-sm" aria-label={t("upd.later")} title={t("upd.later")} onClick={() => set({ dismissed: offered.version })}>
        <X size={14} strokeWidth={1.75} aria-hidden />
      </button>
    </div>
  );
}

/** Release notes as plain text blocks (never HTML). */
function Notes({ markdown }: { markdown: string }) {
  return (
    <div className="update-notes">
      {notesBlocks(markdown).map((b, i) =>
        b.kind === "list" ? (
          <ul key={i}>
            {b.items.map((item, j) => (
              <li key={j}>{item}</li>
            ))}
          </ul>
        ) : b.kind === "heading" ? (
          <h3 key={i}>{b.text}</h3>
        ) : (
          <p key={i}>{b.text}</p>
        ),
      )}
    </div>
  );
}

/** „Neuigkeiten“ of the offered version. */
function OfferNotes() {
  const t = useT();
  const { status, apk, notesOpen } = useUpdates();
  const offer = status?.available ?? (apk?.offered ? { version: apk.offered.version, notes: apk.offered.notes, url: apk.offered.page } : null);
  if (!notesOpen || !offer?.notes) return null;
  return (
    <Dialog
      title={t("upd.newIn", { version: offer.version })}
      onClose={() => set({ notesOpen: false })}
      footer={
        <button type="button" className="btn" onClick={() => void openExternal(offer.url)}>
          <ExternalLink size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("upd.releasePage")}</span>
        </button>
      }
    >
      <Notes markdown={offer.notes} />
    </Dialog>
  );
}

/** „Neu in Arcalo Mail“ after an update. */
function WhatsNewDialog() {
  const t = useT();
  const { whatsNew: wn } = useUpdates();
  if (!wn) return null;
  const close = () => {
    set({ whatsNew: null });
    if (state.apk) void api.apkWhatsNewSeen().then((apk) => set({ apk }));
    else void api.updateWhatsNewSeen(wn.version).catch(() => {});
  };
  return (
    <Dialog
      title={t("wn.title", { version: wn.version })}
      onClose={close}
      footer={
        <>
          {wn.url && (
            <button type="button" className="btn" onClick={() => void openExternal(wn.url)}>
              {t("wn.allNotes")}
            </button>
          )}
          <button type="button" className="btn btn-primary" onClick={close}>
            {t("wn.done")}
          </button>
        </>
      }
    >
      <Notes markdown={wn.notes} />
    </Dialog>
  );
}

/** Everything of the updater that floats: the notice, the notes and „Neu in Arcalo Mail“. */
export function UpdateLayer() {
  return (
    <>
      <div className="update-layer">
        <UpdateNotice />
        <ApkNotice />
      </div>
      <OfferNotes />
      <WhatsNewDialog />
    </>
  );
}

/** The status line of Settings → Über. */
function statusLine(status: UpdateStatus, phase: Phase, progress: UpdateProgress | null): string {
  const v = status.available?.version ?? "";
  const d = status.download;
  if (phase === "checking") return t("upd.checking");
  if (status.ready && status.ready === v) return status.manual ? t("upd.statusAvailable", { version: v }) : t("upd.statusReady", { version: v });
  if (d.phase === "downloading") return t("upd.statusDownloading", { version: v, percent: percentText(progress ?? d) });
  if (d.phase === "paused") return t("upd.statusPaused", { version: v });
  if (d.phase === "failed") return t("upd.statusFailed", { version: v });
  if (v) return t("upd.statusAvailable", { version: v });
  return t("upd.statusCurrent", { version: status.current_version });
}

/** The updates card of Settings → Über (desktop and Android). */
export function UpdateSettings({ mode, onMode, platform, version }: { mode: UpdateMode; onMode: (m: UpdateMode) => void; platform: string; version: string }) {
  const t = useT();
  const { status, apk, phase, progress } = useUpdates();
  useEffect(() => {
    if (!inApp()) return;
    if (platform === "android") void api.apkUpdateStatus().then((a) => set({ apk: a }), () => {});
    else if (platform !== "ios") void refreshStatus();
  }, [platform]);
  if (platform === "ios") return null;
  if (platform === "android") {
    if (!apk) return null;
    return (
      <div className="set-card" data-updates="android">
        <div className="set-card-title">{t("upd.title")}</div>
        <div className="set-row">
          <div className="set-text">
            <div className="set-label">{apk.offered && apk.phase !== "idle" ? t("upd.statusAvailable", { version: apk.offered.version }) : t("upd.statusCurrent", { version: apk.current })}</div>
            <div className="set-desc">{apk.store ? t("apk.store") : !apk.enabled ? t("upd.notConfigured") : apk.phase === "blocked" ? apk.blocked : apk.phase === "error" ? apk.error : t("apk.hint")}</div>
          </div>
          {apk.enabled && (
            <div className="set-control">
              <button type="button" className="btn btn-sm" disabled={apk.phase === "checking"} onClick={() => void api.apkUpdateCheck(true).then((a) => set({ apk: a, dismissed: null }), (e) => fail(t("upd.checkFailed"), e))}>
                {apk.phase === "checking" && <Spinner />}
                <span>{t("upd.check")}</span>
              </button>
            </div>
          )}
        </div>
      </div>
    );
  }
  const d = status?.download;
  return (
    <div className="set-card" data-updates="desktop">
      <div className="set-card-title">{t("upd.title")}</div>
      <div className="set-row">
        <div className="set-text">
          <div className="set-label">{t("upd.modeLabel")}</div>
          <div className="set-desc">{t("upd.modeHint")}</div>
        </div>
        <div className="set-control">
          <Segmented
            label={t("upd.modeLabel")}
            value={mode}
            onChange={onMode}
            options={[
              ["auto", t("upd.modeAuto")],
              ["notify", t("upd.modeNotify")],
            ]}
          />
        </div>
      </div>
      <div className="set-row">
        <div className="set-text">
          <div className="set-label" data-update-status>
            {status ? statusLine(status, phase, progress) : t("upd.statusCurrent", { version })}
          </div>
          <div className="set-desc">{!status?.enabled ? t("upd.notConfigured") : status.manual ? status.manual_text : d?.phase === "failed" ? d.error : status.waiting ? t("upd.waiting", { n: status.waiting }) : t("upd.checkHint")}</div>
        </div>
        <div className="set-control set-actions-left">
          {status?.enabled && d?.phase === "downloading" && (
            <button type="button" className="btn btn-sm" onClick={() => void api.updatePause()}>
              {t("upd.pause")}
            </button>
          )}
          {status?.enabled && !status.manual && (d?.phase === "paused" || d?.phase === "failed") && (
            <button type="button" className="btn btn-sm" onClick={() => void api.updateDownload().catch((e) => fail(t("upd.failed"), e))}>
              {t("upd.resume")}
            </button>
          )}
          {status?.enabled && !status.manual && status.ready && status.ready === status.available?.version && (
            <button type="button" className="btn btn-sm btn-primary" disabled={phase !== "idle"} onClick={() => void restartNow()}>
              {t("upd.restartNow")}
            </button>
          )}
          {status?.enabled && status.manual && status.available && (
            <button type="button" className="btn btn-sm" onClick={() => void openExternal(status.available?.url ?? status.release_page)}>
              {t("upd.releasePage")}
            </button>
          )}
          {status?.enabled && !status.manual && status.mode === "notify" && status.available && !status.ready && d?.phase !== "downloading" && (
            <button type="button" className="btn btn-sm btn-primary" disabled={phase !== "idle"} onClick={() => void installUpdate()}>
              {t("upd.install")}
            </button>
          )}
          <button type="button" className="btn btn-sm" disabled={!status?.enabled || phase === "checking"} onClick={() => void checkForUpdates(true)}>
            {phase === "checking" && <Spinner />}
            <span>{t("upd.check")}</span>
          </button>
        </div>
      </div>
    </div>
  );
}
