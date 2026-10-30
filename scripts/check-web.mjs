// Load the web app that scripts/build-web.sh built and convert a small
// document with it in Node, to check that the WebAssembly module and the
// JavaScript wasm-bindgen generated for it fit together.
//
// Usage: node scripts/check-web.mjs [OUT_DIR]     (default: target/web)

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const out = resolve(process.argv[2] ?? join(root, 'target', 'web'));
for (const file of ['index.html', 'app.js', 'worker.js', 'style.css', 'THIRD-PARTY-LICENSES.txt']) {
  const text = readFileSync(join(out, file), 'utf8');
  assert(!text.includes('?v=dev'), `${file} still refers to files without the build's stamp`);
}

const doctopus = await import(pathToFileURL(join(out, 'pkg', 'doctopus_web.js')).href);
await doctopus.default({ module_or_path: readFileSync(join(out, 'pkg', 'doctopus_web_bg.wasm')) });

assert(JSON.parse(doctopus.formats()).some((f) => f.extensions.includes('docx')));
const conversion = new doctopus.Conversion(new TextEncoder().encode('# Notes\n'), 'notes.txt');
const manifest = JSON.parse(conversion.manifest());
assert.equal(manifest.files[0].path, 'notes.md');
const markdown = new TextDecoder().decode(conversion.file('notes.md'));
assert.match(markdown, /^---\nsource: notes\.txt\n/);
assert.match(conversion.html('notes.md'), /<pre><code># Notes/);
const zip = conversion.zip();
assert.equal(String.fromCharCode(zip[0], zip[1]), 'PK');
assert.throws(() => new doctopus.Conversion(new Uint8Array([0xd0, 0xcf, 0x11, 0xe0]), 'x.doc'));

console.log(`doctopus ${doctopus.version()} web app in ${out}: works`);
