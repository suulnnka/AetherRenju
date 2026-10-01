/* Node 侧 Worker 宿主:把 src/worker.js 装进 worker_threads
 * (self / fetch 打底,消息走 parentPort)。仅供测试。 */
import { parentPort } from 'node:worker_threads';
import { readFileSync } from 'node:fs';

const bytes = readFileSync(new URL('../wasm/aether_renju.wasm', import.meta.url));

globalThis.self = {
  postMessage: (m) => parentPort.postMessage(m),
};
globalThis.fetch = async () =>
  new Response(bytes, { headers: { 'Content-Type': 'application/wasm' } });

await import('../src/worker.js');

parentPort.on('message', (d) => {
  const h = globalThis.self.onmessage;
  if (h) h({ data: d });
});
