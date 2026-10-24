/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "1" in the end-to-end test build: allows the SAMPLE mock outside Tauri. */
  readonly VITE_SAMPLE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

/** "0.1.0, build 1a2b3c4": the app version and the commit it was built from (vite.config.ts). */
declare const __APP_BUILD__: string;
