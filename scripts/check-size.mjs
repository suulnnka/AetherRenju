/* 体积闸门:引擎 wasm gzip ≤ 70 KB(旧 JS 版预算 35 KB)。 */
import { gzipSync } from 'node:zlib';
import { readFileSync, statSync } from 'node:fs';

const BUDGET = 70 * 1024;
const files = ['wasm/aether_renju.wasm'];
let bad = 0;
for (const f of files) {
  const gz = gzipSync(readFileSync(f), { level: 9 }).length;
  const raw = statSync(f).size;
  const ok = gz <= BUDGET;
  console.log(`${ok ? '✓' : '✗'} ${f}: raw ${(raw / 1024).toFixed(1)} KB, gzip ${(gz / 1024).toFixed(1)} KB (预算 ${BUDGET / 1024} KB)`);
  if (!ok) bad++;
}
process.exit(bad ? 1 : 0);
