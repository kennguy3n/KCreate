import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { ThemeProvider } from "./styles/ThemeProvider";

const container = document.getElementById("root");
if (!container) {
  throw new Error("missing #root mount point");
}

createRoot(container).render(
  <StrictMode>
    <ThemeProvider>
      <App />
    </ThemeProvider>
  </StrictMode>,
);
