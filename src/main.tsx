import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "./index.css";
import { App } from "./App";
import { defaultBackend } from "./services/backend";
import { StoreProvider } from "./store/hooks";
import { createAppStore } from "./store/store";

const root = createRoot(document.getElementById("root")!);

defaultBackend()
  .then((backend) => {
    const store = createAppStore(backend);
    root.render(
      <StrictMode>
        <StoreProvider store={store}>
          <App />
        </StoreProvider>
      </StrictMode>,
    );
  })
  .catch((e: unknown) => {
    // Only reachable if the IPC module itself fails to load. Plain DOM, no React.
    const p = document.createElement("p");
    p.textContent = `PeakTweaks could not start its interface: ${e instanceof Error ? e.message : String(e)}`;
    document.getElementById("root")!.replaceChildren(p);
  });
