# doctopus-web

doctopus in the browser, at <https://kriscris.github.io/doctopus/>. A
static web page converts a dropped document in WebAssembly, exactly as
`doctopus convert` does, without a server: the file never leaves the
browser.

- The file tree on the left holds `NAME.md`, the pictures and files in
  `NAME.parts/`, and the Markdown of every embedded document at
  `NAME.parts/<id>/<Stem>.md`.
- The viewer renders the selected Markdown file with its pictures, and
  draws Visio diagrams from their Mermaid blocks. A link to another
  converted file opens it and selects it in the tree; the page's address
  (`#/NAME.parts/emb1/Budget.md`) names the file shown, so the back button
  and bookmarks work.
- Any file, or the whole tree as a zip file, can be downloaded.

## Building

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.127 --locked   # the version in Cargo.lock
scripts/build-web.sh                  # writes target/web
python3 -m http.server -d target/web 8000
node scripts/check-web.mjs            # loads the build in Node and converts a file
```

The page needs no build tools of its own: `www/` is plain HTML, CSS and
JavaScript, and `scripts/build-web.sh` adds the WebAssembly module and its
wasm-bindgen glue next to it. It also replaces the `?v=dev` that ends the
page's references to its own files with a stamp of the build, so that a
browser holding files of an earlier build in its cache never mixes them
with a newer one. CI publishes the build to GitHub Pages after each push
to `main`.

## How it works

- `src/lib.rs` is the module's interface: `Conversion::new(bytes, name)`
  reads the document with every doctopus reader and lays out its files with
  `doctopus_core::render::convert`, the same code the command line writes
  to disk. The page then asks for the list of files, their bytes, a
  Markdown file as HTML, and a zip file of everything.
- `src/html.rs` turns a Markdown file into HTML for the viewer. Documents
  are untrusted, so the HTML keeps only the renderer's own `<br>`, `<sup>`
  and `<sub>` tags and shows any other HTML as text. Links keep only web and
  mail addresses, and pictures load only from the conversion itself.
- `www/worker.js` runs the module in a Web Worker. In WebAssembly a panic
  aborts instead of unwinding, so a reader that panics fails the whole file
  rather than one embedded part, and the page starts a fresh worker.
- `www/app.js` is the page. Mermaid is the one thing loaded from elsewhere:
  a pinned release from jsdelivr, checked against its hash, and only when a
  file has a diagram. The page's Content Security Policy allows nothing
  else.

Licensed under either of Apache-2.0 or MIT, at your option.
