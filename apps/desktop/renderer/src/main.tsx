import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { LocaleProvider } from "./i18n";
import { ThemeProvider } from "./styles/ThemeProvider";

const container = document.getElementById("root");
if (!container) {
  throw new Error("missing #root mount point");
}

createRoot(container).render(
  <StrictMode>
    <LocaleProvider>
      <ThemeProvider>
        <App />
      </ThemeProvider>
    </LocaleProvider>
  </StrictMode>,
);
