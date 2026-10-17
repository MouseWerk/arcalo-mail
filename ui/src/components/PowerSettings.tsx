// Settings → KI, Regeln, Vorlagen and Kontakte: the AI providers and which account's mail each
// may see; the rule editor for local and server rules; templates and snippets; address books
// and profile pictures.

import { ArrowDown, ArrowUp, Cloud, Eye, Lock, Pencil, Play, Plus, RefreshCw, Trash2, TriangleAlert, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { api } from "../lib/api";
import { currentLang, useT, type TKey } from "../lib/i18n";
import { listDate, now, displayName } from "../lib/format";
import { showToast } from "../lib/toasts";
import type {
  AccountInfo,
  AddressBook,
  AiKind,
  AiProvider,
  AiSettings,
  Mailbox,
  MessageSummary,
  Rule,
  RuleAction,
  RuleCondition,
  RuleField,
  RuleOp,
  RuleServer,
  Settings,
  Template,
  Tone,
} from "../lib/types";
import { ConfirmDialog, Dialog, Menu, Segmented, Spinner, Switch, type MenuItem } from "./ui";
import { forgetPictureCache } from "./Avatar";

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

const accountLabel = (a: AccountInfo) => a.name || a.email;

// ---------------------------------------------------------------- KI

const KIND_LABEL: Record<AiKind, TKey> = {
  ollama: "ai.kindOllama",
  lm_studio: "ai.kindLmStudio",
  openai: "ai.kindOpenai",
  anthropic: "ai.kindAnthropic",
  mistral: "ai.kindMistral",
  azure: "ai.kindAzure",
  compatible: "ai.kindCompatible",
};

const DEFAULT_URL: Record<AiKind, string> = {
  ollama: "http://localhost:11434",
  lm_studio: "http://localhost:1234/v1",
  openai: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com/v1",
  mistral: "https://api.mistral.ai/v1",
  azure: "",
  compatible: "",
};

/** Runs on this computer (the backend says the same, `AiProvider::local`). */
export function isLocal(url: string): boolean {
  try {
    const h = new URL(url).hostname.replace(/^\[|\]$/g, "").toLowerCase();
    return h === "localhost" || h.endsWith(".localhost") || h === "::1" || /^127\./.test(h);
  } catch {
    return false;
  }
}

/** `[a-z0-9-]` from a name, as the backend makes provider ids. */
export function slug(s: string): string {
  const out = s
    .trim()
    .toLowerCase()
    .replace(/ä/g, "a")
    .replace(/ö/g, "o")
    .replace(/ü/g, "u")
    .replace(/ß/g, "ss")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return out.slice(0, 40);
}

const needsKey = (p: AiProvider) => p.kind !== "ollama" && p.kind !== "lm_studio" && !isLocal(p.base_url);

function ProviderCard({ p, onChange, onRemove }: { p: AiProvider; onChange: (p: AiProvider) => void; onRemove: () => void }) {
  const t = useT();
  const [draft, setDraft] = useState(p);
  const [key, setKey] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [models, setModels] = useState<string[]>([]);
  const [check, setCheck] = useState<{ busy: boolean; ok: boolean | null; text: string }>({ busy: false, ok: null, text: "" });
  useEffect(() => setDraft(p), [p]);
  useEffect(() => {
    void api.aiHasKey(p.id).then(setHasKey);
  }, [p.id]);
  const local = isLocal(draft.base_url);
  const load = async () => {
    setCheck({ busy: true, ok: null, text: "" });
    try {
      if (key.trim()) {
        await api.aiSetKey(p.id, key);
        setHasKey(true);
        setKey("");
      }
      const list = await api.aiModels(draft, null);
      setModels(list);
      if (!draft.model && list[0]) onChange({ ...draft, model: list[0] });
      setCheck({ busy: false, ok: true, text: list.length ? t("ai.models", { n: list.length }) : t("ai.connected") });
    } catch (e) {
      setCheck({ busy: false, ok: false, text: String(e) });
    }
  };
  return (
    <div className="set-card ai-provider" data-kind={p.kind}>
      <div className="set-card-head">
        <input className="input set-card-name" aria-label={t("ai.providerName")} value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} onBlur={() => onChange(draft)} />
        <span className="set-badge" data-where={local ? "local" : "cloud"}>
          {local ? <Lock size={12} strokeWidth={1.75} aria-hidden /> : <Cloud size={12} strokeWidth={1.75} aria-hidden />}
          <span>{local ? t("ai.onDevice") : t("ai.cloud")}</span>
        </span>
        <button type="button" className="btn btn-danger-quiet btn-sm" onClick={onRemove}>
          <Trash2 size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("ai.remove")}</span>
        </button>
      </div>
      <div className="set-fields">
        <label className="stack-field">
          <span className="field-label">{t("ai.address")}</span>
          <input className="input" value={draft.base_url} spellCheck={false} placeholder={p.kind === "azure" ? "https://…openai.azure.com" : "https://…/v1"} onChange={(e) => setDraft({ ...draft, base_url: e.target.value.trim() })} onBlur={() => onChange(draft)} />
          <span className="field-hint">{t(KIND_LABEL[p.kind])}</span>
        </label>
        {needsKey(draft) && (
          <label className="stack-field">
            <span className="field-label">{t("ai.key")}</span>
            <input
              className="input"
              type="password"
              value={key}
              placeholder={hasKey ? t("ai.keyStored") : t("ai.keyPlaceholder")}
              onChange={(e) => setKey(e.target.value)}
              onBlur={() => {
                if (!key.trim()) return;
                void api.aiSetKey(p.id, key).then(() => {
                  setHasKey(true);
                  setKey("");
                  showToast({ title: t("ai.keySaved"), tone: "success", duration: 2500 });
                });
              }}
            />
            <span className="field-hint">{t("ai.keyHint")}</span>
          </label>
        )}
        {p.kind === "azure" && (
          <label className="stack-field">
            <span className="field-label">{t("ai.apiVersion")}</span>
            <input className="input" value={draft.api_version} placeholder="2024-10-21" onChange={(e) => setDraft({ ...draft, api_version: e.target.value })} onBlur={() => onChange(draft)} />
          </label>
        )}
        <label className="stack-field">
          <span className="field-label">{p.kind === "azure" ? t("ai.deployment") : t("ai.model")}</span>
          {models.length > 0 ? (
            <select className="select" value={draft.model} onChange={(e) => onChange({ ...draft, model: e.target.value })}>
              {!models.includes(draft.model) && draft.model && <option value={draft.model}>{draft.model}</option>}
              {models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          ) : (
            <input className="input" value={draft.model} spellCheck={false} placeholder={p.kind === "ollama" ? "llama3.2" : ""} onChange={(e) => setDraft({ ...draft, model: e.target.value })} onBlur={() => onChange(draft)} />
          )}
        </label>
      </div>
      <div className="set-actions">
        {check.text && (
          <span className={check.ok ? "field-ok" : "field-error"} role={check.ok ? "status" : "alert"}>
            {check.text}
          </span>
        )}
        <button type="button" className="btn" disabled={check.busy} onClick={() => void load()}>
          {check.busy ? <Spinner /> : <RefreshCw size={14} strokeWidth={1.75} aria-hidden />}
          <span>{t("ai.check")}</span>
        </button>
      </div>
    </div>
  );
}

export function AiSection({ settings, accounts, onChange }: { settings: Settings; accounts: AccountInfo[]; onChange: (patch: Partial<Settings>) => void }) {
  const t = useT();
  const ai = settings.ai;
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);
  const [cloudFor, setCloudFor] = useState<{ account: AccountInfo; provider: AiProvider } | null>(null);
  const [tones, setTones] = useState<Record<number, Tone | null>>({});
  const set = (patch: Partial<AiSettings>) => onChange({ ai: { ...ai, ...patch } });
  useEffect(() => {
    for (const a of accounts) void api.aiTone(a.id).then((tn) => setTones((x) => ({ ...x, [a.id]: tn })));
  }, [accounts]);
  const add = (kind: AiKind) => {
    const label = t(KIND_LABEL[kind]);
    let id = slug(label) || "provider";
    const ids = new Set(ai.providers.map((p) => p.id));
    for (let n = 2; ids.has(id); n++) id = `${slug(label)}-${n}`;
    const p: AiProvider = { id, name: label, kind, base_url: DEFAULT_URL[kind], model: "", api_version: "" };
    const providers = [...ai.providers, p];
    set({ providers, default_provider: ai.default_provider || (isLocal(p.base_url) ? p.id : "") });
  };
  const local = ai.providers.filter((p) => isLocal(p.base_url));
  const choiceOf = (a: AccountInfo) => ai.accounts[String(a.id)] ?? "";
  return (
    <div className="set-section">
      <p className="muted set-intro">{t("ai.intro")}</p>
      {ai.providers.length === 0 && (
        <div className="set-card ai-empty">
          <p>{t("ai.noneSetUp")}</p>
        </div>
      )}
      {ai.providers.map((p, i) => (
        <ProviderCard
          key={p.id}
          p={p}
          onChange={(np) => set({ providers: ai.providers.map((x, j) => (j === i ? np : x)) })}
          onRemove={() => {
            void api.aiSetKey(p.id, "");
            const accountsLeft = Object.fromEntries(Object.entries(ai.accounts).filter(([, v]) => v !== p.id));
            set({ providers: ai.providers.filter((_, j) => j !== i), accounts: accountsLeft, default_provider: ai.default_provider === p.id ? "" : ai.default_provider });
          }}
        />
      ))}
      <div className="set-actions set-actions-left">
        <button
          type="button"
          className="btn"
          aria-haspopup="menu"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            setMenu({
              at: { x: r.left, y: r.bottom + 4 },
              items: [
                { heading: t("ai.onThisDevice") },
                { label: t("ai.kindOllama"), onSelect: () => add("ollama") },
                { label: t("ai.kindLmStudio"), onSelect: () => add("lm_studio") },
                { label: t("ai.kindCompatible"), onSelect: () => add("compatible") },
                "separator",
                { heading: t("ai.inTheCloud") },
                { label: t("ai.kindOpenai"), onSelect: () => add("openai") },
                { label: t("ai.kindAnthropic"), onSelect: () => add("anthropic") },
                { label: t("ai.kindMistral"), onSelect: () => add("mistral") },
                { label: t("ai.kindAzure"), onSelect: () => add("azure") },
              ],
            });
          }}
        >
          <Plus size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("ai.add")}</span>
        </button>
      </div>
      {ai.providers.length > 0 && (
        <>
          <div className="set-card">
            <div className="set-card-title">{t("ai.whoSees")}</div>
            <p className="set-desc set-card-desc">{t("ai.whoSeesHint")}</p>
            {local.length > 0 && (
              <Row label={t("ai.default")} description={t("ai.defaultHint")}>
                <select className="select" aria-label={t("ai.default")} value={ai.default_provider} onChange={(e) => set({ default_provider: e.target.value })}>
                  <option value="">{t("ai.nobody")}</option>
                  {local.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
                </select>
              </Row>
            )}
            {accounts.map((a) => (
              <Row key={a.id} label={accountLabel(a)} description={a.name ? a.email : undefined}>
                <select
                  className="select"
                  aria-label={t("ai.accountChoice", { name: accountLabel(a) })}
                  value={choiceOf(a)}
                  onChange={(e) => {
                    const v = e.target.value;
                    const p = ai.providers.find((x) => x.id === v);
                    if (p && !isLocal(p.base_url)) return setCloudFor({ account: a, provider: p });
                    const accounts2 = { ...ai.accounts };
                    if (v === "") delete accounts2[String(a.id)];
                    else accounts2[String(a.id)] = v;
                    set({ accounts: accounts2 });
                  }}
                >
                  <option value="">{t("ai.useDefault")}</option>
                  <option value="off">{t("ai.nobody")}</option>
                  {ai.providers.map((p) => (
                    <option key={p.id} value={p.id}>
                      {isLocal(p.base_url) ? t("ai.choiceLocal", { name: p.name }) : t("ai.choiceCloud", { name: p.name, host: hostOf(p.base_url) })}
                    </option>
                  ))}
                </select>
              </Row>
            ))}
          </div>
          <div className="set-card">
            <Row label={t("ai.autoSummary")} description={t("ai.autoSummaryHint")}>
              <Switch label={t("ai.autoSummary")} checked={ai.auto_summary} onChange={(auto_summary) => set({ auto_summary })} />
            </Row>
            <Row label={t("ai.priorityLabel")} description={t("ai.priorityHint")}>
              <Switch label={t("ai.priorityLabel")} checked={ai.priority} onChange={(priority) => set({ priority })} />
            </Row>
            <Row label={t("ai.tone")} description={t("ai.toneHint")}>
              <Switch label={t("ai.tone")} checked={ai.tone} onChange={(tone) => set({ tone })} />
            </Row>
            {ai.tone &&
              accounts.map((a) => {
                const tn = tones[a.id];
                return (
                  <Row key={a.id} label={accountLabel(a)} description={tn ? t("ai.toneLearned", { n: tn.learned_from }) : t("ai.toneNot")}>
                    <span className="set-control-row">
                      <button type="button" className="btn btn-sm" onClick={() => void api.aiLearnTone(a.id).then((x) => (setTones((s) => ({ ...s, [a.id]: x })), x ? undefined : showToast({ title: t("ai.toneNothing"), duration: 3000 })))}>
                        {t("ai.learn")}
                      </button>
                      {tn && (
                        <button type="button" className="btn btn-sm btn-quiet" onClick={() => void api.aiForgetTone(a.id).then(() => setTones((s) => ({ ...s, [a.id]: null })))}>
                          {t("ai.forget")}
                        </button>
                      )}
                    </span>
                  </Row>
                );
              })}
            <Row label={t("ai.kept")} description={t("ai.keptHint")}>
              <button type="button" className="btn btn-sm btn-quiet" onClick={() => void api.aiForgetResults().then(() => showToast({ title: t("ai.keptForgotten"), duration: 2500 }))}>
                {t("ai.forget")}
              </button>
            </Row>
          </div>
        </>
      )}
      {menu && <Menu items={menu.items} at={menu.at} onClose={() => setMenu(null)} label={t("ai.add")} />}
      {cloudFor && (
        <ConfirmDialog
          title={t("ai.cloudTitle", { name: cloudFor.provider.name })}
          text={t("ai.cloudText", { account: accountLabel(cloudFor.account), host: hostOf(cloudFor.provider.base_url), name: cloudFor.provider.name })}
          confirm={t("ai.cloudConfirm")}
          onConfirm={() => set({ accounts: { ...ai.accounts, [String(cloudFor.account.id)]: cloudFor.provider.id } })}
          onClose={() => setCloudFor(null)}
        />
      )}
    </div>
  );
}

export const hostOf = (url: string) => {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
};

// ---------------------------------------------------------------- rules

const FIELDS: [RuleField, TKey][] = [
  ["from", "rules.from"],
  ["to", "rules.to"],
  ["cc", "rules.cc"],
  ["subject", "rules.subject"],
  ["list", "rules.list"],
  ["has_attachment", "rules.hasAttachment"],
  ["size", "rules.size"],
  ["age", "rules.age"],
  ["category", "rules.category"],
];

const TEXT_OPS: [RuleOp, TKey][] = [
  ["contains", "rules.contains"],
  ["not_contains", "rules.notContains"],
  ["is", "rules.is"],
  ["starts_with", "rules.startsWith"],
];
const NUM_OPS: [RuleOp, TKey][] = [
  ["over", "rules.over"],
  ["under", "rules.under"],
];

const ACTIONS: [RuleAction["kind"], TKey][] = [
  ["move", "rules.move"],
  ["label", "rules.label"],
  ["mark_read", "rules.markRead"],
  ["flag", "rules.flag"],
  ["pin", "rules.pin"],
  ["forward", "rules.forward"],
  ["delete", "rules.delete"],
  ["skip_inbox", "rules.skipInbox"],
  ["category", "rules.toCategory"],
];

const CATEGORIES: [string, TKey][] = [
  ["people", "cat.people"],
  ["newsletters", "cat.newsletters"],
  ["notifications", "cat.notifications"],
  ["receipts", "cat.receipts"],
  ["other", "cat.other"],
];

const numeric = (f: RuleField) => f === "size" || f === "age";

function newAction(kind: RuleAction["kind"]): RuleAction {
  switch (kind) {
    case "move":
      return { kind, folder: "" };
    case "label":
      return { kind, keyword: "" };
    case "forward":
      return { kind, to: "" };
    case "category":
      return { kind, category: "newsletters" };
    default:
      return { kind } as RuleAction;
  }
}

/** One line about a rule: „Betreff enthält „Rechnung“ → In „Rechnungen“ verschieben“. */
export function ruleSummary(r: Rule, t: ReturnType<typeof useT>): string {
  const cond = r.body.conditions
    .map((c) => {
      const field = t(FIELDS.find(([f]) => f === c.field)?.[1] ?? "rules.subject");
      if (c.field === "has_attachment") return c.value === "false" ? t("rules.noAttachment") : t("rules.withAttachment");
      if (c.field === "category") return `${field} ${t(CATEGORIES.find(([k]) => k === c.value)?.[1] ?? "cat.other")}`;
      const op = t([...TEXT_OPS, ...NUM_OPS].find(([o]) => o === c.op)?.[1] ?? "rules.contains");
      const unit = c.field === "size" ? " KB" : c.field === "age" ? ` ${t("rules.days")}` : "";
      return numeric(c.field) ? `${field} ${op} ${c.value}${unit}` : `${field} ${op} „${c.value}“`;
    })
    .join(r.body.match_all ? ` ${t("rules.and")} ` : ` ${t("rules.or")} `);
  const act = r.body.actions
    .map((a) => {
      const label = t(ACTIONS.find(([k]) => k === a.kind)?.[1] ?? "rules.move");
      if (a.kind === "move") return `${label} „${a.folder}“`;
      if (a.kind === "label") return `${label} „${a.keyword}“`;
      if (a.kind === "forward") return `${label} ${a.to}`;
      if (a.kind === "category") return `${label} ${t(CATEGORIES.find(([k]) => k === a.category)?.[1] ?? "cat.other")}`;
      return label;
    })
    .join(", ");
  return `${cond} → ${act}`;
}

function RuleEditor({ rule, accounts, onSaved, onClose }: { rule: Rule; accounts: AccountInfo[]; onSaved: () => void; onClose: () => void }) {
  const t = useT();
  const lang = currentLang();
  const [r, setR] = useState<Rule>(rule);
  const [folders, setFolders] = useState<Mailbox[]>([]);
  const [server, setServer] = useState<RuleServer | null>(null);
  const [checking, setChecking] = useState(false);
  const [unsupported, setUnsupported] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ total: number; items: MessageSummary[] } | null>(null);
  const [runFolder, setRunFolder] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const account = accounts.find((a) => a.id === r.account_id) ?? null;
  useEffect(() => {
    const ids = r.account_id != null ? [r.account_id] : accounts.map((a) => a.id);
    void Promise.all(ids.map((id) => api.mailboxes(id))).then((lists) => {
      const all = lists.flat().filter((m) => m.selectable);
      setFolders(all);
      setRunFolder((f) => f ?? all.find((m) => m.role === "inbox")?.id ?? null);
    });
  }, [r.account_id]);
  useEffect(() => {
    if (r.place !== "server" || r.account_id == null) return setServer(null);
    setChecking(true);
    void api
      .rulesServer(r.account_id, null)
      .then(setServer)
      .catch((e) => setServer({ kind: "none", server: null, error: String(e), checked: true }))
      .finally(() => setChecking(false));
  }, [r.place, r.account_id]);
  useEffect(() => {
    const timer = setTimeout(() => void api.ruleUnsupported(r).then(setUnsupported).catch(() => setUnsupported(null)), 150);
    return () => clearTimeout(timer);
  }, [r]);
  const body = r.body;
  const setBody = (patch: Partial<Rule["body"]>) => setR({ ...r, body: { ...body, ...patch } });
  const setCond = (i: number, c: RuleCondition) => setBody({ conditions: body.conditions.map((x, j) => (j === i ? c : x)) });
  const setAct = (i: number, a: RuleAction) => setBody({ actions: body.actions.map((x, j) => (j === i ? a : x)) });
  const folderNames = [...new Set(folders.filter((m) => m.role !== "inbox").map((m) => m.name))];
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.ruleSave(r);
      if (res.server_error) showToast({ title: t("rules.serverFailed"), detail: res.server_error, tone: "danger", duration: 8000 });
      else showToast({ title: t("rules.saved"), tone: "success", duration: 2500 });
      onSaved();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title={rule.id ? t("rules.edit") : t("rules.new")}
      onClose={onClose}
      wide
      className="rule-editor"
      footer={
        <>
          {error && (
            <span className="field-error" role="alert">
              {error}
            </span>
          )}
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void save()}>
            {busy && <Spinner />}
            <span>{t("rules.save")}</span>
          </button>
        </>
      }
    >
      <div className="rule-grid">
        <label className="stack-field">
          <span className="field-label">{t("rules.name")}</span>
          <input className="input" value={r.name} autoFocus placeholder={t("rules.namePlaceholder")} onChange={(e) => setR({ ...r, name: e.target.value })} />
        </label>
        <label className="stack-field">
          <span className="field-label">{t("rules.account")}</span>
          <select
            className="select"
            value={r.account_id ?? ""}
            onChange={(e) => {
              const id = e.target.value ? Number(e.target.value) : null;
              setR({ ...r, account_id: id, place: id == null ? "local" : r.place });
            }}
          >
            <option value="">{t("rules.allAccounts")}</option>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {accountLabel(a)}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div className="rule-where">
        <span className="field-label">{t("rules.where")}</span>
        <Segmented<"local" | "server">
          label={t("rules.where")}
          value={r.place}
          onChange={(place) => setR({ ...r, place })}
          options={[
            ["local", t("rules.local")],
            ["server", t("rules.server")],
          ]}
        />
        <span className="muted rule-where-note">
          {r.place === "local"
            ? t("rules.localHint")
            : r.account_id == null
              ? t("rules.pickAccount")
              : checking
                ? t("rules.checking")
                : server?.kind === "sieve"
                  ? t("rules.sieve", { host: server.server ? `${server.server.host}:${server.server.port}` : "" })
                  : server?.kind === "graph"
                    ? t("rules.graph")
                    : t("rules.noServer")}
        </span>
      </div>
      {r.place === "server" && server?.error && (
        <div className="notice notice-warning" role="status">
          <TriangleAlert size={16} strokeWidth={1.75} aria-hidden />
          <span>{server.error}</span>
        </div>
      )}
      {unsupported && (
        <div className="notice notice-warning" role="status">
          <TriangleAlert size={16} strokeWidth={1.75} aria-hidden />
          <span>{t("rules.unsupported", { why: unsupported })}</span>
        </div>
      )}
      <fieldset className="rule-block">
        <legend>
          <span>{t("rules.when")}</span>
          <Segmented<"all" | "any">
            label={t("rules.when")}
            value={body.match_all ? "all" : "any"}
            onChange={(v) => setBody({ match_all: v === "all" })}
            options={[
              ["all", t("rules.matchAll")],
              ["any", t("rules.matchAny")],
            ]}
          />
        </legend>
        {body.conditions.map((c, i) => (
          <div key={i} className="rule-line">
            <select
              className="select"
              aria-label={t("rules.field")}
              value={c.field}
              onChange={(e) => {
                const field = e.target.value as RuleField;
                setCond(i, { field, op: numeric(field) ? "over" : field === "has_attachment" || field === "category" ? "is" : "contains", value: field === "has_attachment" ? "true" : field === "category" ? "newsletters" : "" });
              }}
            >
              {FIELDS.map(([f, label]) => (
                <option key={f} value={f}>
                  {t(label)}
                </option>
              ))}
            </select>
            {c.field === "has_attachment" ? (
              <select className="select" aria-label={t("rules.value")} value={c.value} onChange={(e) => setCond(i, { ...c, value: e.target.value })}>
                <option value="true">{t("rules.yes")}</option>
                <option value="false">{t("rules.no")}</option>
              </select>
            ) : c.field === "category" ? (
              <>
                <select className="select" aria-label={t("rules.op")} value={c.op} onChange={(e) => setCond(i, { ...c, op: e.target.value as RuleOp })}>
                  <option value="is">{t("rules.is")}</option>
                  <option value="not_contains">{t("rules.isNot")}</option>
                </select>
                <select className="select" aria-label={t("rules.value")} value={c.value} onChange={(e) => setCond(i, { ...c, value: e.target.value })}>
                  {CATEGORIES.map(([k, label]) => (
                    <option key={k} value={k}>
                      {t(label)}
                    </option>
                  ))}
                </select>
              </>
            ) : (
              <>
                <select className="select" aria-label={t("rules.op")} value={c.op} onChange={(e) => setCond(i, { ...c, op: e.target.value as RuleOp })}>
                  {(numeric(c.field) ? NUM_OPS : TEXT_OPS).map(([o, label]) => (
                    <option key={o} value={o}>
                      {t(label)}
                    </option>
                  ))}
                </select>
                <input
                  className="input"
                  aria-label={t("rules.value")}
                  inputMode={numeric(c.field) ? "numeric" : undefined}
                  value={c.value}
                  placeholder={c.field === "size" ? "KB" : c.field === "age" ? t("rules.days") : ""}
                  onChange={(e) => setCond(i, { ...c, value: numeric(c.field) ? e.target.value.replace(/\D/g, "") : e.target.value })}
                />
              </>
            )}
            <button type="button" className="icon-btn icon-btn-sm" aria-label={t("rules.removeCondition")} onClick={() => setBody({ conditions: body.conditions.filter((_, j) => j !== i) })}>
              <X size={14} strokeWidth={1.75} aria-hidden />
            </button>
          </div>
        ))}
        <button type="button" className="btn btn-sm btn-quiet" onClick={() => setBody({ conditions: [...body.conditions, { field: "from", op: "contains", value: "" }] })}>
          <Plus size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("rules.addCondition")}</span>
        </button>
      </fieldset>
      <fieldset className="rule-block">
        <legend>
          <span>{t("rules.then")}</span>
        </legend>
        {body.actions.map((a, i) => (
          <div key={i} className="rule-line">
            <select className="select" aria-label={t("rules.action")} value={a.kind} onChange={(e) => setAct(i, newAction(e.target.value as RuleAction["kind"]))}>
              {ACTIONS.map(([k, label]) => (
                <option key={k} value={k}>
                  {t(label)}
                </option>
              ))}
            </select>
            {a.kind === "move" &&
              (folderNames.length ? (
                <select className="select" aria-label={t("rules.folder")} value={a.folder} onChange={(e) => setAct(i, { ...a, folder: e.target.value })}>
                  <option value="">{t("rules.chooseFolder")}</option>
                  {folderNames.map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              ) : (
                <input className="input" aria-label={t("rules.folder")} value={a.folder} onChange={(e) => setAct(i, { ...a, folder: e.target.value })} />
              ))}
            {a.kind === "label" && <input className="input" aria-label={t("rules.keyword")} placeholder="$Rechnung" value={a.keyword} onChange={(e) => setAct(i, { ...a, keyword: e.target.value.replace(/\s/g, "") })} />}
            {a.kind === "forward" && <input className="input" aria-label={t("rules.forwardTo")} placeholder="name@example.org" value={a.to} onChange={(e) => setAct(i, { ...a, to: e.target.value })} />}
            {a.kind === "category" && (
              <select className="select" aria-label={t("rules.category")} value={a.category} onChange={(e) => setAct(i, { ...a, category: e.target.value })}>
                {CATEGORIES.map(([k, label]) => (
                  <option key={k} value={k}>
                    {t(label)}
                  </option>
                ))}
              </select>
            )}
            <button type="button" className="icon-btn icon-btn-sm" aria-label={t("rules.removeAction")} onClick={() => setBody({ actions: body.actions.filter((_, j) => j !== i) })}>
              <X size={14} strokeWidth={1.75} aria-hidden />
            </button>
          </div>
        ))}
        <button type="button" className="btn btn-sm btn-quiet" onClick={() => setBody({ actions: [...body.actions, { kind: "mark_read" }] })}>
          <Plus size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("rules.addAction")}</span>
        </button>
        <label className="rule-stop">
          <Switch label={t("rules.stop")} checked={body.stop} onChange={(stop) => setBody({ stop })} />
          <span>{t("rules.stop")}</span>
        </label>
      </fieldset>
      <div className="rule-tools">
        <button
          type="button"
          className="btn btn-sm"
          disabled={!body.conditions.length}
          onClick={() =>
            void api
              .rulePreview(r)
              .then(setPreview)
              .catch((e) => setError(String(e)))
          }
        >
          <Eye size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("rules.preview")}</span>
        </button>
        <span className="toolbar-gap" />
        <select className="select select-sm" aria-label={t("rules.runFolder")} value={runFolder ?? ""} onChange={(e) => setRunFolder(Number(e.target.value))}>
          {folders.map((m) => (
            <option key={m.id} value={m.id}>
              {(accounts.length > 1 && r.account_id == null ? `${accountLabel(accounts.find((a) => a.id === m.account_id) ?? accounts[0])} · ` : "") + (m.role === "inbox" ? t("folder.inbox") : m.name)}
            </option>
          ))}
        </select>
        <button
          type="button"
          className="btn btn-sm"
          disabled={runFolder == null || !body.conditions.length || !body.actions.length}
          onClick={() =>
            runFolder != null &&
            void api
              .ruleRun(r, runFolder)
              .then((rep) => showToast({ title: t("rules.ran", { n: rep.matched }), tone: "success", duration: 4000 }))
              .catch((e) => setError(String(e)))
          }
        >
          <Play size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("rules.runNow")}</span>
        </button>
      </div>
      {preview && (
        <div className="rule-preview" aria-label={t("rules.preview")}>
          <p className="muted">{t("rules.matches", { n: preview.total })}</p>
          {preview.items.length > 0 && (
            <ul className="contact-list">
              {preview.items.map((m) => (
                <li key={m.id} className="contact-row rule-preview-row">
                  <span className="contact-row-subject">{m.subject || t("list.noSubject")}</span>
                  <span className="muted">{displayName(m.from, lang)}</span>
                  <span className="muted contact-row-date">{listDate(m.date, now(), lang)}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {account && r.place === "server" && <p className="muted rule-account-note">{t("rules.serverNote", { account: accountLabel(account) })}</p>}
    </Dialog>
  );
}

export function RulesSection({ accounts }: { accounts: AccountInfo[] }) {
  const t = useT();
  const [rules, setRules] = useState<Rule[] | null>(null);
  const [editing, setEditing] = useState<Rule | null>(null);
  const [removing, setRemoving] = useState<Rule | null>(null);
  const load = () => void api.rules().then(setRules);
  useEffect(load, []);
  const warn = (e: string | null) => e && showToast({ title: t("rules.serverFailed"), detail: e, tone: "danger", duration: 8000 });
  const move = (i: number, dir: -1 | 1) => {
    if (!rules) return;
    const list = [...rules];
    const [x] = list.splice(i, 1);
    list.splice(i + dir, 0, x);
    setRules(list);
    void api.rulesReorder(list.map((r) => r.id)).then(warn);
  };
  const blank = (): Rule => ({
    id: 0,
    account_id: accounts.length === 1 ? accounts[0].id : null,
    name: "",
    place: "local",
    enabled: true,
    position: 0,
    body: { match_all: true, conditions: [{ field: "from", op: "contains", value: "" }], actions: [{ kind: "move", folder: "" }], stop: false },
    server_ref: null,
  });
  return (
    <div className="set-section">
      <p className="muted set-intro">{t("rules.intro")}</p>
      {rules === null ? (
        <Spinner />
      ) : rules.length === 0 ? (
        <div className="set-card">
          <p className="muted set-empty">{t("rules.none")}</p>
        </div>
      ) : (
        <ul className="set-card rule-list" aria-label={t("settings.rules")}>
          {rules.map((r, i) => {
            const a = accounts.find((x) => x.id === r.account_id);
            return (
              <li key={r.id} className="rule-item" data-off={r.enabled ? undefined : ""}>
                <span className="rule-order">
                  <button type="button" className="icon-btn icon-btn-xs" disabled={i === 0} aria-label={t("rules.up", { name: r.name })} onClick={() => move(i, -1)}>
                    <ArrowUp size={12} strokeWidth={2} aria-hidden />
                  </button>
                  <button type="button" className="icon-btn icon-btn-xs" disabled={i === rules.length - 1} aria-label={t("rules.down", { name: r.name })} onClick={() => move(i, 1)}>
                    <ArrowDown size={12} strokeWidth={2} aria-hidden />
                  </button>
                </span>
                <Switch
                  label={t("rules.enabled", { name: r.name })}
                  checked={r.enabled}
                  onChange={(on) => {
                    setRules(rules.map((x) => (x.id === r.id ? { ...x, enabled: on } : x)));
                    void api.ruleEnable(r.id, on).then(warn);
                  }}
                />
                <button type="button" className="rule-text" onClick={() => setEditing(r)}>
                  <span className="rule-name">{r.name}</span>
                  <span className="muted rule-summary">{ruleSummary(r, t)}</span>
                </button>
                <span className="set-badge" data-where={r.place}>
                  {r.place === "server" ? t("rules.onServer") : t("rules.inApp")}
                </span>
                <span className="muted rule-account">{a ? accountLabel(a) : t("rules.allAccounts")}</span>
                <button type="button" className="icon-btn icon-btn-sm" aria-label={t("rules.editNamed", { name: r.name })} onClick={() => setEditing(r)}>
                  <Pencil size={14} strokeWidth={1.75} aria-hidden />
                </button>
                <button type="button" className="icon-btn icon-btn-sm" aria-label={t("rules.deleteNamed", { name: r.name })} onClick={() => setRemoving(r)}>
                  <Trash2 size={14} strokeWidth={1.75} aria-hidden />
                </button>
              </li>
            );
          })}
        </ul>
      )}
      <div className="set-actions set-actions-left">
        <button type="button" className="btn" onClick={() => setEditing(blank())}>
          <Plus size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("rules.add")}</span>
        </button>
      </div>
      {editing && <RuleEditor rule={editing} accounts={accounts} onSaved={load} onClose={() => setEditing(null)} />}
      {removing && (
        <ConfirmDialog
          title={t("rules.deleteTitle", { name: removing.name })}
          text={removing.place === "server" ? t("rules.deleteServerText") : t("rules.deleteText")}
          confirm={t("rules.deleteConfirm")}
          danger
          onConfirm={() => void api.ruleDelete(removing.id).then((e) => (warn(e), load()))}
          onClose={() => setRemoving(null)}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------- templates

const VARS = "{name} {vorname} {datum} {uhrzeit} {mein_name} {betreff} {signatur}";
const VARS_EN = "{name} {first_name} {date} {time} {my_name} {subject} {signature}";

function TemplateEditor({ template, accounts, onSaved, onClose }: { template: Template; accounts: AccountInfo[]; onSaved: () => void; onClose: () => void }) {
  const t = useT();
  const [x, setX] = useState(template);
  const [error, setError] = useState<string | null>(null);
  const snippet = x.kind === "snippet";
  return (
    <Dialog
      title={template.id ? (snippet ? t("tpl.editSnippet") : t("tpl.editTemplate")) : snippet ? t("tpl.newSnippet") : t("tpl.newTemplate")}
      onClose={onClose}
      wide
      className="template-editor"
      footer={
        <>
          {error && (
            <span className="field-error" role="alert">
              {error}
            </span>
          )}
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() =>
              void api
                .templateSave(x)
                .then(() => {
                  onSaved();
                  onClose();
                })
                .catch((e) => setError(String(e)))
            }
          >
            {t("tpl.save")}
          </button>
        </>
      }
    >
      <div className="rule-grid">
        <label className="stack-field">
          <span className="field-label">{t("tpl.name")}</span>
          <input className="input" autoFocus value={x.name} onChange={(e) => setX({ ...x, name: e.target.value })} />
        </label>
        {snippet ? (
          <label className="stack-field">
            <span className="field-label">{t("tpl.trigger")}</span>
            <span className="input-affix">
              <span className="muted" aria-hidden>
                ;;
              </span>
              <input className="input" value={x.trigger ?? ""} spellCheck={false} placeholder={t("tpl.triggerPlaceholder")} onChange={(e) => setX({ ...x, trigger: e.target.value.replace(/[^\p{L}\p{N}_-]/gu, "").toLowerCase() })} />
            </span>
          </label>
        ) : (
          <label className="stack-field">
            <span className="field-label">{t("tpl.account")}</span>
            <select className="select" value={x.account_id ?? ""} onChange={(e) => setX({ ...x, account_id: e.target.value ? Number(e.target.value) : null })}>
              <option value="">{t("rules.allAccounts")}</option>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {accountLabel(a)}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>
      {!snippet && (
        <label className="stack-field">
          <span className="field-label">{t("tpl.subject")}</span>
          <input className="input" value={x.subject} onChange={(e) => setX({ ...x, subject: e.target.value })} />
        </label>
      )}
      <label className="stack-field">
        <span className="field-label">{t("tpl.text")}</span>
        <textarea className="input textarea" rows={8} value={x.body} onChange={(e) => setX({ ...x, body: e.target.value })} />
        <span className="field-hint">{t("tpl.vars", { vars: currentLang() === "de" ? VARS : VARS_EN })}</span>
      </label>
    </Dialog>
  );
}

export function TemplatesSection({ accounts }: { accounts: AccountInfo[] }) {
  const t = useT();
  const [list, setList] = useState<Template[] | null>(null);
  const [editing, setEditing] = useState<Template | null>(null);
  const load = () => void api.templates().then(setList);
  useEffect(load, []);
  const part = (kind: Template["kind"]) => (list ?? []).filter((x) => x.kind === kind);
  const blank = (kind: Template["kind"]): Template => ({ id: 0, kind, name: "", trigger: kind === "snippet" ? "" : null, subject: "", body: "", account_id: null });
  const block = (kind: Template["kind"], title: TKey, hint: TKey, empty: TKey, add: TKey) => (
    <div className="set-card">
      <div className="set-card-title">{t(title)}</div>
      <p className="set-desc set-card-desc">{t(hint)}</p>
      {part(kind).length === 0 ? (
        <p className="muted set-empty">{t(empty)}</p>
      ) : (
        <ul className="rule-list template-list">
          {part(kind).map((x) => (
            <li key={x.id} className="rule-item">
              <button type="button" className="rule-text" onClick={() => setEditing(x)}>
                <span className="rule-name">
                  {kind === "snippet" && <code className="set-code">;;{x.trigger}</code>} {x.name}
                </span>
                <span className="muted rule-summary">{(x.subject ? `${x.subject} – ` : "") + x.body.replace(/\s+/g, " ").slice(0, 120)}</span>
              </button>
              <button type="button" className="icon-btn icon-btn-sm" aria-label={t("tpl.editNamed", { name: x.name })} onClick={() => setEditing(x)}>
                <Pencil size={14} strokeWidth={1.75} aria-hidden />
              </button>
              <button type="button" className="icon-btn icon-btn-sm" aria-label={t("tpl.deleteNamed", { name: x.name })} onClick={() => void api.templateDelete(x.id).then(load)}>
                <Trash2 size={14} strokeWidth={1.75} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="set-actions set-actions-left">
        <button type="button" className="btn btn-sm" onClick={() => setEditing(blank(kind))}>
          <Plus size={14} strokeWidth={1.75} aria-hidden />
          <span>{t(add)}</span>
        </button>
      </div>
    </div>
  );
  return (
    <div className="set-section">
      <p className="muted set-intro">{t("tpl.intro")}</p>
      {list === null ? (
        <Spinner />
      ) : (
        <>
          {block("template", "tpl.templates", "tpl.templatesHint", "tpl.noTemplates", "tpl.addTemplate")}
          {block("snippet", "tpl.snippets", "tpl.snippetsHint", "tpl.noSnippets", "tpl.addSnippet")}
        </>
      )}
      {editing && <TemplateEditor template={editing} accounts={accounts} onSaved={load} onClose={() => setEditing(null)} />}
    </div>
  );
}

// ---------------------------------------------------------------- contacts

const CARDDAV: [string, string][] = [
  ["iCloud", "https://contacts.icloud.com"],
  ["Fastmail", "https://carddav.fastmail.com"],
  ["mailbox.org", "https://dav.mailbox.org"],
  ["Posteo", "https://posteo.de:8843"],
  ["Nextcloud", "https://{host}/remote.php/dav"],
];

function CardDavDialog({ onAdded, onClose }: { onAdded: () => void; onClose: () => void }) {
  const t = useT();
  const [preset, setPreset] = useState(0);
  const [url, setUrl] = useState(CARDDAV[0][1]);
  const [host, setHost] = useState("");
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [state, setState] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  const nextcloud = CARDDAV[preset]?.[0] === "Nextcloud";
  const address = nextcloud ? url.replace("{host}", host.trim() || "{host}") : url;
  return (
    <Dialog
      title={t("contacts.addCarddav")}
      description={t("contacts.addCarddavHint")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={state.busy || !user || !password || address.includes("{host}")}
            onClick={() => {
              setState({ busy: true, error: null });
              void api.carddavAdd(preset < CARDDAV.length ? CARDDAV[preset][0] : "", address, user, password).then(
                () => {
                  onAdded();
                  onClose();
                },
                (e) => setState({ busy: false, error: String(e) }),
              );
            }}
          >
            {state.busy && <Spinner />}
            <span>{t("contacts.connect")}</span>
          </button>
        </>
      }
    >
      <label className="stack-field">
        <span className="field-label">{t("contacts.service")}</span>
        <select
          className="select"
          value={preset}
          onChange={(e) => {
            const i = Number(e.target.value);
            setPreset(i);
            setUrl(i < CARDDAV.length ? CARDDAV[i][1] : "https://");
          }}
        >
          {CARDDAV.map(([name], i) => (
            <option key={name} value={i}>
              {name}
            </option>
          ))}
          <option value={CARDDAV.length}>{t("contacts.otherServer")}</option>
        </select>
      </label>
      {nextcloud ? (
        <label className="stack-field">
          <span className="field-label">{t("contacts.host")}</span>
          <input className="input" value={host} spellCheck={false} placeholder="cloud.example.org" onChange={(e) => setHost(e.target.value.trim())} />
        </label>
      ) : (
        <label className="stack-field">
          <span className="field-label">{t("contacts.address")}</span>
          <input className="input" value={url} spellCheck={false} onChange={(e) => setUrl(e.target.value.trim())} />
        </label>
      )}
      <label className="stack-field">
        <span className="field-label">{t("setup.username")}</span>
        <input className="input" value={user} spellCheck={false} autoComplete="off" onChange={(e) => setUser(e.target.value)} />
      </label>
      <label className="stack-field">
        <span className="field-label">{t("contacts.appPassword")}</span>
        <input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
        <span className="field-hint">{t("contacts.appPasswordHint")}</span>
      </label>
      {state.error && (
        <p className="field-error" role="alert">
          {state.error}
        </p>
      )}
    </Dialog>
  );
}

export function ContactsSection({ settings, accounts, onChange }: { settings: Settings; accounts: AccountInfo[]; onChange: (patch: Partial<Settings>) => void }) {
  const t = useT();
  const lang = currentLang();
  const [books, setBooks] = useState<AddressBook[] | null>(null);
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState<number | null>(null);
  const load = () => void api.addressBooks().then(setBooks);
  useEffect(load, []);
  const microsoft = accounts.filter((a) => a.transport.kind === "imap" && a.transport.auth.kind === "o_auth2");
  const sync = (id: number) => {
    setBusy(id);
    void api
      .addressBookSync(id)
      .catch((e) => showToast({ title: t("contacts.syncFailed"), detail: String(e), tone: "danger" }))
      .finally(() => {
        setBusy(null);
        load();
      });
  };
  return (
    <div className="set-section">
      <p className="muted set-intro">{t("contacts.intro")}</p>
      <div className="set-card">
        <div className="set-card-title">{t("contacts.books")}</div>
        {books === null ? (
          <Spinner />
        ) : books.filter((b) => b.kind === "carddav").length === 0 ? (
          <p className="muted set-empty">{t("contacts.noBooks")}</p>
        ) : (
          <ul className="rule-list">
            {books
              .filter((b) => b.kind === "carddav")
              .map((b) => (
                <li key={b.id} className="rule-item">
                  <span className="rule-text">
                    <span className="rule-name">{b.name || b.url}</span>
                    <span className={b.error ? "field-error rule-summary" : "muted rule-summary"}>
                      {b.error ?? t("contacts.bookInfo", { n: b.cards, when: b.last_sync ? listDate(b.last_sync, now(), lang) : "–" })}
                    </span>
                  </span>
                  <button type="button" className="icon-btn icon-btn-sm" disabled={busy === b.id} aria-label={t("contacts.syncNamed", { name: b.name })} onClick={() => sync(b.id)}>
                    {busy === b.id ? <Spinner /> : <RefreshCw size={14} strokeWidth={1.75} aria-hidden />}
                  </button>
                  <button type="button" className="icon-btn icon-btn-sm" aria-label={t("contacts.removeNamed", { name: b.name })} onClick={() => void api.addressBookRemove(b.id).then(load)}>
                    <Trash2 size={14} strokeWidth={1.75} aria-hidden />
                  </button>
                </li>
              ))}
          </ul>
        )}
        <div className="set-actions set-actions-left">
          <button type="button" className="btn btn-sm" onClick={() => setAdding(true)}>
            <Plus size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("contacts.addCarddav")}</span>
          </button>
        </div>
      </div>
      {microsoft.length > 0 && (
        <div className="set-card">
          {microsoft.map((a) => {
            const book = books?.find((b) => b.kind === "graph" && b.account_id === a.id);
            return (
              <Row key={a.id} label={t("contacts.microsoft", { email: a.email })} description={book?.error ?? (book ? t("contacts.bookInfo", { n: book.cards, when: book.last_sync ? listDate(book.last_sync, now(), lang) : "–" }) : t("contacts.microsoftHint"))}>
                <Switch
                  label={t("contacts.microsoft", { email: a.email })}
                  checked={!!book}
                  onChange={(on) => {
                    setBusy(a.id);
                    void api
                      .microsoftContacts(a.id, on)
                      .catch((e) => showToast({ title: t("contacts.syncFailed"), detail: String(e), tone: "danger" }))
                      .finally(() => {
                        setBusy(null);
                        load();
                      });
                  }}
                />
              </Row>
            );
          })}
        </div>
      )}
      <div className="set-card">
        <div className="set-card-title">{t("contacts.pictures")}</div>
        <Row label={t("contacts.bimi")} description={t("contacts.bimiHint")}>
          <Switch
            label={t("contacts.bimi")}
            checked={settings.bimi}
            onChange={(bimi) => {
              forgetPictureCache();
              onChange({ bimi });
            }}
          />
        </Row>
        <Row label={t("contacts.gravatar")} description={t("contacts.gravatarHint")}>
          <Switch
            label={t("contacts.gravatar")}
            checked={settings.gravatar}
            onChange={(gravatar) => {
              forgetPictureCache();
              onChange({ gravatar });
            }}
          />
        </Row>
        <Row label={t("contacts.forgetPictures")} description={t("contacts.forgetPicturesHint")}>
          <button
            type="button"
            className="btn btn-sm btn-quiet"
            onClick={() =>
              void api.forgetPictures("").then(() => {
                forgetPictureCache();
                showToast({ title: t("contacts.picturesForgotten"), duration: 2500 });
              })
            }
          >
            {t("ai.forget")}
          </button>
        </Row>
      </div>
      {adding && <CardDavDialog onAdded={load} onClose={() => setAdding(false)} />}
    </div>
  );
}

