#!/usr/bin/env bash
# Build the web app (crates/doctopus-web) into a directory of static files
# that any web server can serve; the conversion runs in the browser.
#
# Usage:
#   scripts/build-web.sh [OUT_DIR]     build into OUT_DIR (default: target/web)
#   python3 -m http.server -d target/web 8000
#
# Needs the wasm32-unknown-unknown target and the wasm-bindgen command at the
# version in Cargo.lock (WASM_BINDGEN may name the program):
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <version> --locked
#
# wasm-opt is not used: it makes the module 6% smaller, but no smaller
# once compressed, which is how web servers send it.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-$root/target/web}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
bindgen="${WASM_BINDGEN:-wasm-bindgen}"
cd "$root"

# The command must match the crate exactly, or the generated glue does not
# fit the module.
want="$(cargo pkgid --locked wasm-bindgen | sed -E 's/.*[#@]//')"
if ! have="$("$bindgen" --version 2>/dev/null | awk '{print $2}')"; then
  echo "error: $bindgen not found; install it with:" >&2
  echo "  cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi
if [ "$have" != "$want" ]; then
  echo "error: $bindgen is version $have, but Cargo.lock has wasm-bindgen $want; install it with:" >&2
  echo "  cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi

cargo build --release --locked --target wasm32-unknown-unknown -p doctopus-web

rm -rf "$out"
mkdir -p "$out"
"$bindgen" --target web --no-typescript --out-dir "$out/pkg" \
  "$target_dir/wasm32-unknown-unknown/release/doctopus_web.wasm"
cp -R crates/doctopus-web/www/. "$out/"
# The crates built into the module ask for their notices to go with it.
cp LICENSE-MIT LICENSE-APACHE NOTICE "$out/"
python="$(command -v python3 || command -v python)"
"$python" scripts/third-party-licenses.py --target wasm32-unknown-unknown --package doctopus-web \
  > "$out/THIRD-PARTY-LICENSES.txt"

# Browsers may keep the files of a web server for a while (GitHub Pages:
# ten minutes), and glue from one build does not fit the module of another.
# The page's references to its own files end in `?v=dev`: make that a stamp
# of this build, so that a browser fetches all of one build or all of
# another.
"$python" - "$out" <<'EOF'
import hashlib
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
digest = hashlib.sha256()
for path in sorted(p for p in out.rglob("*") if p.is_file()):
    digest.update(path.relative_to(out).as_posix().encode() + b"\0" + path.read_bytes())
stamp = "?v=" + digest.hexdigest()[:12]
for name in ("index.html", "app.js", "worker.js"):
    path = out / name
    text = path.read_text(encoding="utf-8")
    if "?v=dev" not in text:
        sys.exit(f"error: {name} has no ?v=dev to stamp")
    path.write_text(text.replace("?v=dev", stamp), encoding="utf-8")
EOF

wasm="$out/pkg/doctopus_web_bg.wasm"
echo "built $out ($(wc -c < "$wasm" | tr -d ' ') bytes of WebAssembly)"
echo "serve it with: python3 -m http.server -d '$out' 8000"
