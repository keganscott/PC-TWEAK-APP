// Evidence for when the real-app end-to-end test cannot attach: can this
// process start peaktweaks.exe directly (CreateProcess, as msedgedriver does),
// and is the process still alive a few seconds later?
import { spawn } from "node:child_process";

const exe = process.argv[2];
const child = spawn(exe, [], { stdio: "ignore", env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9222" } });
let exited = null;
child.on("error", (e) => console.log(`spawn error: ${e.code ?? ""} ${e.message}`));
child.on("exit", (code) => (exited = code));
await new Promise((r) => setTimeout(r, 20000));
console.log(exited === null ? `still running after 20 s (pid ${child.pid})` : `exited with code ${exited}`);
try {
  const res = await fetch("http://127.0.0.1:9222/json/version");
  console.log(`WebView2 debugging endpoint answered: ${(await res.text()).slice(0, 300)}`);
} catch (e) {
  console.log(`WebView2 debugging endpoint did not answer: ${e.message}`);
}
// Where did WebView2 put its profile, and did it write the port file?
import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";
const roots = [process.env.LOCALAPPDATA, process.env.APPDATA].filter(Boolean);
for (const root of roots) {
  for (const name of readdirSync(root)) {
    if (!/peaktweaks/i.test(name)) continue;
    const walk = (dir, depth) => {
      if (depth > 3) return;
      for (const n of readdirSync(dir)) {
        const p = join(dir, n);
        if (n === "DevToolsActivePort") console.log(`port file: ${p}`);
        try {
          if (statSync(p).isDirectory()) walk(p, depth + 1);
        } catch {}
      }
    };
    console.log(`profile folder: ${join(root, name)}`);
    walk(join(root, name), 0);
  }
}
child.kill();
