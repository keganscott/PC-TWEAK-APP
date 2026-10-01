// Evidence for when the real-app end-to-end test cannot attach: can this
// process start peaktweaks.exe directly (CreateProcess, as msedgedriver does),
// and is the process still alive a few seconds later?
import { spawn } from "node:child_process";

const exe = process.argv[2];
const child = spawn(exe, [], { stdio: "ignore", env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9222" } });
let exited = null;
child.on("error", (e) => console.log(`spawn error: ${e.code ?? ""} ${e.message}`));
child.on("exit", (code) => (exited = code));
await new Promise((r) => setTimeout(r, 8000));
console.log(exited === null ? `still running after 8 s (pid ${child.pid})` : `exited with code ${exited}`);
try {
  const res = await fetch("http://127.0.0.1:9222/json/version");
  console.log(`WebView2 debugging endpoint answered: ${(await res.text()).slice(0, 300)}`);
} catch (e) {
  console.log(`WebView2 debugging endpoint did not answer: ${e.message}`);
}
child.kill();
