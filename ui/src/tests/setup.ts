// The app's tests render the real <App /> against the demo backend in happy-dom, which has no
// real iframe: the sandboxed mail frame is a stub that keeps its exported event name.
import { vi } from "vitest";

vi.mock("../components/MailFrame", () => ({ MAILTO_EVENT: "arcalo-mail:mailto", MailFrame: () => null }));
