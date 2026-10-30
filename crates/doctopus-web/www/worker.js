// Runs the converter off the page's thread. A panic in WebAssembly traps
// instead of unwinding, which can leave the module in a broken state, so
// the page starts a new worker after a `fatal` error.
//
// scripts/build-web.sh stamps the `?v=` of the imports with each build's
// own, so the glue and the module always come from the same build.

import init, { Conversion, formats, last_panic, version } from './pkg/doctopus_web.js?v=dev';

const ready = init({ module_or_path: new URL('./pkg/doctopus_web_bg.wasm?v=dev', import.meta.url) });
let conversion = null;

self.onmessage = async ({ data: { id, type, ...args } }) => {
  try {
    await ready;
    if (type === 'hello') {
      reply(id, { version: version(), formats: JSON.parse(formats()) });
    } else if (type === 'convert') {
      conversion?.free();
      conversion = null;
      conversion = new Conversion(new Uint8Array(args.bytes), args.name);
      const manifest = JSON.parse(conversion.manifest());
      const files = [];
      const html = {};
      for (const entry of manifest.files) {
        files.push({ path: entry.path, bytes: conversion.file(entry.path) });
        if (entry.markdown) html[entry.path] = conversion.html(entry.path);
      }
      reply(id, { manifest, files, html }, files.map((f) => f.bytes.buffer));
    } else if (type === 'zip') {
      const bytes = conversion.zip();
      reply(id, { bytes }, [bytes.buffer]);
    }
  } catch (err) {
    const fatal = err instanceof WebAssembly.RuntimeError;
    let message = typeof err === 'string' ? err : String(err?.message ?? err);
    if (fatal) {
      let panic = null;
      try {
        panic = last_panic();
      } catch {
        // The module is past asking.
      }
      message = panic ? `doctopus stopped: ${panic}` : `doctopus stopped: ${message}`;
    }
    self.postMessage({ id, error: message, fatal });
  }
};

function reply(id, result, transfer = []) {
  self.postMessage({ id, result }, transfer);
}
