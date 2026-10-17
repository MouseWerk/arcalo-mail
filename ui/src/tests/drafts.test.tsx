// A stored message opened again in the compose sheet (a scheduled one taken back, a draft from
// Drafts) keeps its attachments, forwarded files, signing and encryption, Bcc, formatting and
// the message it answers.
import { describe, expect, it, vi } from "vitest";
import { fromComposeDraft, toDraft } from "../lib/compose";
import { api } from "../lib/api";
import type { Attachment, ComposeDraft, FileInfo } from "../lib/types";
import { click, flush, key, mountApp } from "./harness";

const pdf: FileInfo = { path: "/home/me/Vertrag.pdf", name: "Vertrag.pdf", size: 1200, mime_type: "application/pdf" };
const fwd: Attachment = { id: 42, message_id: 7, part_index: 0, filename: "Plan.png", mime_type: "image/png", size: 800, content_id: null, inline: false, section: null };
const stored: ComposeDraft = {
  account_id: 1,
  to: ["a@example.org"],
  cc: [],
  bcc: ["chef@example.org"],
  subject: "Vertrag",
  text: "",
  html: "<p>Anbei <b>der Vertrag</b></p>",
  attachments: [pdf.path],
  in_reply_to: "orig@x",
  references: ["orig@x"],
  message_id: "m1@x",
  reply_to_id: 3,
  forwarded: [42],
  protect: { method: "pgp", sign: true, encrypt: true },
};

describe("fromComposeDraft", () => {
  it("keeps attachments, forwarded files, protection, Bcc and the replied-to message", () => {
    const again = toDraft(fromComposeDraft(stored, [pdf], [fwd]));
    expect(again.attachments).toEqual([pdf.path]);
    expect(again.forwarded).toEqual([42]);
    expect(again.protect).toEqual({ method: "pgp", sign: true, encrypt: true });
    expect(again.bcc).toEqual(["chef@example.org"]);
    expect(again.reply_to_id).toBe(3);
    expect(again.html).toContain("<b>der Vertrag</b>");
  });

  it("opens a rich message as rich text whatever the default is", () => {
    const state = fromComposeDraft({ ...stored, html: "<p>Der ganze Bericht</p>" });
    expect(state.rich).toBe(true);
    expect(state.html).toContain("Der ganze Bericht");
  });
});

describe("Geplant → Bearbeiten", () => {
  it("sends the message again with its files and protection", async () => {
    vi.spyOn(api, "fileInfos").mockResolvedValue([pdf]);
    vi.spyOn(api, "attachments").mockResolvedValue([fwd]);
    vi.spyOn(api, "pgpLocked").mockResolvedValue(null);
    const send = vi.spyOn(api, "send");
    await api.sendLater(stored, Math.floor(Date.now() / 1000) + 86400, null);
    const { el } = await mountApp();
    await flush(50);
    await click(el.querySelector('.sidebar [data-view="scheduled"]')!);
    await flush(50);
    await click(el.querySelector(`.scheduled [aria-label="Bearbeiten"], [aria-label="Bearbeiten"]`)!);
    await flush(100);
    expect(el.querySelector(".compose")).not.toBeNull();
    expect(el.querySelector(".compose-attachments")?.textContent).toContain("Vertrag.pdf");
    expect(el.querySelector(".compose-attachments")?.textContent).toContain("Plan.png");
    await key(el.querySelector("#compose-subject")!, "Enter", { ctrlKey: true });
    await flush(50);
    expect(send).toHaveBeenCalledTimes(1);
    const d = send.mock.calls[0][0];
    expect(d.attachments).toEqual([pdf.path]);
    expect(d.forwarded).toEqual([42]);
    expect(d.protect).toEqual(stored.protect);
    expect(d.bcc).toEqual(["chef@example.org"]);
  });
});

describe("Entwurf bearbeiten", () => {
  it("takes the stored draft with Bcc, formatting and the replied-to message", async () => {
    const draftOf = vi.spyOn(api, "draftOf").mockResolvedValue({ ...stored, attachments: [], forwarded: [42], protect: null });
    vi.spyOn(api, "attachments").mockResolvedValue([fwd]);
    const send = vi.spyOn(api, "send");
    const real = api.message;
    vi.spyOn(api, "message").mockImplementation(async (id) => ({ ...(await real(id)), draft: true }));
    const { el } = await mountApp();
    await click(el.querySelector('.row[data-id="1"]')!);
    await flush(100);
    const edit = Array.from(el.querySelectorAll<HTMLButtonElement>(".reading-pane .notice .btn")).find((b) => b.textContent === "Weiterschreiben")!;
    await click(edit);
    await flush(100);
    expect(draftOf).toHaveBeenCalledWith(1);
    expect(el.querySelector(".compose .editor")?.innerHTML).toContain("<b>der Vertrag</b>");
    await key(el.querySelector("#compose-subject")!, "Enter", { ctrlKey: true });
    await flush(50);
    const d = send.mock.calls[0][0];
    expect(d.bcc).toEqual(["chef@example.org"]);
    expect(d.reply_to_id).toBe(3);
    expect(d.forwarded).toEqual([42]);
  });
});
