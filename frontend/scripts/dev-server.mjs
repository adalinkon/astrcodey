/**
 * Industry-standard pattern: start the dev server, poll until the HTTP port
 * responds, then let Tauri's built-in devUrl polling see it as "ready"
 * immediately.  Avoids ERR_CONNECTION_REFUSED when Vite cold-starts slowly
 * on Windows.
 */

import { spawn } from 'node:child_process';
import { request } from 'node:http';

const PORT = 5173;
const MAX_RETRIES = 60;
const RETRY_MS = 500;

console.log('[dev-server] Starting Vite...');

const vite = spawn('npm', ['run', 'dev'], {
  stdio: 'inherit',
  shell: true,
});

function poll(retries = 0) {
  // 127.0.0.1 avoids IPv6 resolution issues on Windows; port is Number not String
  request({ hostname: '127.0.0.1', port: PORT, path: '/' }, (res) => {
    res.resume();
    console.log(`[dev-server] Vite ready on http://localhost:${PORT}`);
  })
    .on('error', () => {
      if (retries >= MAX_RETRIES) {
        console.error(`[dev-server] Timed out after ${(MAX_RETRIES * RETRY_MS) / 1000}s`);
        vite.kill();
        process.exit(1);
      }
      setTimeout(() => poll(retries + 1), RETRY_MS);
    })
    .end();
}

// Give Vite a head-start before the first poll
setTimeout(poll, 1000);

// Mirror Vite's exit code and forward signals
vite.on('exit', (code) => process.exit(code ?? 0));
process.on('SIGINT', () => { vite.kill(); });
process.on('SIGTERM', () => { vite.kill(); });
