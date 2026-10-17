import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/app.css";
import { App } from "./App";
import { startSplash } from "./lib/splash";
import { readSystemBars } from "./lib/viewport";

// The opening animation runs while the app loads and goes once it is ready (lib/splash.ts).
startSplash();
// Android: the system bars' insets before the first paint (lib/viewport.ts).
readSystemBars();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
