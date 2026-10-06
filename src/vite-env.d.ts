/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "1" in the end-to-end test build: allows the SAMPLE mock outside Tauri. */
  readonly VITE_SAMPLE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
