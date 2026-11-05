# Installing doctopus and its skill

These steps are written for a coding agent that installs doctopus for its
user. Run them as written. They put the program in `~/.doctopus/bin`, or in
`$DOCTOPUS_HOME/bin` when the user sets `DOCTOPUS_HOME` or names another
folder. They don't need administrator rights and change nothing outside
that folder and the agent's skills folder.

Before you start, make sure the user asked for the install, or agreed to
it, since it downloads a program from GitHub. If doctopus is already
installed (step 1 of the skill finds it), skip to part B.

## A. The program

### A1. Download the latest release (preferred)

Releases are at <https://github.com/KrisCris/doctopus/releases>. Each one
has an archive per platform and a `SHA256SUMS` file. The commands below:

1. pick the archive for this machine;
2. check it against `SHA256SUMS`;
3. unpack the program into the bin folder;
4. run it once.

**macOS and Linux.** Run it as one command; it runs in `sh`, whatever
the current shell is:

```sh
sh <<'EOF'
set -eu
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
  *) echo "no prebuilt doctopus for $(uname -sm)" >&2; exit 1 ;;
esac
bin="${DOCTOPUS_HOME:-$HOME/.doctopus}/bin"
api=https://api.github.com/repos/KrisCris/doctopus/releases/latest
url=$(curl -fsSL "$api" | grep -o "https://[^\"]*-$target\.tar\.gz" | head -n 1)
[ -n "$url" ] || { echo "no release archive for $target" >&2; exit 1; }
name=${url##*/}
tmp=$(mktemp -d)
curl -fsSL -o "$tmp/$name" "$url"
curl -fsSL -o "$tmp/SHA256SUMS" "${url%/*}/SHA256SUMS"
cd "$tmp"
if command -v sha256sum > /dev/null; then
  grep "  $name\$" SHA256SUMS | sha256sum -c -
else
  grep "  $name\$" SHA256SUMS | shasum -a 256 -c -
fi
tar -xzf "$tmp/$name" -C "$tmp"
mkdir -p "$bin"
mv "$tmp/${name%.tar.gz}/doctopus" "$bin/doctopus"
cd /
rm -rf "$tmp"
"$bin/doctopus" --version
echo "installed: $bin/doctopus"
EOF
```

On Linux these are the statically linked builds, which run on any
distribution.

**Windows** (PowerShell):

```powershell
$ErrorActionPreference = 'Stop'
$target = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$root = if ($env:DOCTOPUS_HOME) { $env:DOCTOPUS_HOME } else { Join-Path $HOME '.doctopus' }
$bin = Join-Path $root 'bin'
$release = Invoke-RestMethod 'https://api.github.com/repos/KrisCris/doctopus/releases/latest'
$asset = $release.assets | Where-Object { $_.name -like "*-$target.zip" } | Select-Object -First 1
if (-not $asset) { throw "no release archive for $target" }
$sums = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS' } | Select-Object -First 1
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
$zip = Join-Path $tmp $asset.name
Invoke-WebRequest $asset.browser_download_url -OutFile $zip -UseBasicParsing
Invoke-WebRequest $sums.browser_download_url -OutFile (Join-Path $tmp 'SHA256SUMS') -UseBasicParsing
$line = Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { $_ -like "*  $($asset.name)" } | Select-Object -First 1
$expected = ($line -split '\s+')[0]
if ((Get-FileHash $zip -Algorithm SHA256).Hash -ne $expected.ToUpper()) { throw 'checksum mismatch' }
Expand-Archive $zip -DestinationPath $tmp
New-Item -ItemType Directory -Force $bin | Out-Null
$folder = $asset.name -replace '\.zip$', ''
Move-Item -Force (Join-Path $tmp "$folder\doctopus.exe") (Join-Path $bin 'doctopus.exe')
Remove-Item -Recurse -Force $tmp
& (Join-Path $bin 'doctopus.exe') --version
"installed: $(Join-Path $bin 'doctopus.exe')"
```

If either script says there is no release archive, there is no published
release yet, or none for this platform. Use A2.

### A2. Build it from source (when there is no release)

This needs Rust 1.88 or newer (`cargo --version`) and takes a few minutes.
`--root` puts the program in `bin/` under the folder you name:

```sh
cargo install --locked --git https://github.com/KrisCris/doctopus doctopus --root "${DOCTOPUS_HOME:-$HOME/.doctopus}"
```

In PowerShell, pass `--root "$HOME\.doctopus"` instead, or the
`DOCTOPUS_HOME` folder. If Rust is missing, tell the user. Don't install
Rust without asking them.

### A3. Check it

Run the program by its full path:

```sh
~/.doctopus/bin/doctopus --version
~/.doctopus/bin/doctopus formats
```

Adding the bin folder to `PATH` is optional, because the skill finds the
program without it. Offer it to the user, and change their shell profile
only if they agree.

## B. The skill

The skill is the folder `skills/doctopus` in the repository: `SKILL.md`
and `references/`. Install it the way the user's agent loads skills.

**Claude Code**, with the `claude` command available:

```sh
claude plugin marketplace add KrisCris/doctopus
claude plugin install doctopus@doctopus
```

This installs the skill as a plugin, which `claude plugin update` keeps up
to date. Without the `claude` command, copy the folder instead, as below,
into `~/.claude/skills/doctopus`.

**Other agents, or a plain copy.** Put the folder where the agent loads
skills from:
- Claude Code: `~/.claude/skills/` for every project, or `.claude/skills/`
  in one project.
- Codex: `~/.codex/skills/`.
- Other agents: see their documentation on Agent Skills.

On macOS and Linux, this copies the skill from the repository's `main`
branch into `DEST`:

```sh
DEST=~/.claude/skills   # or the folder for your agent
tmp=$(mktemp -d)
curl -fsSL https://codeload.github.com/KrisCris/doctopus/tar.gz/refs/heads/main | tar -xz -C "$tmp"
mkdir -p "$DEST"
rm -rf "$DEST/doctopus"
cp -R "$tmp/doctopus-main/skills/doctopus" "$DEST/doctopus"
rm -rf "$tmp"
ls "$DEST/doctopus"
```

In PowerShell:

```powershell
$dest = Join-Path $HOME '.claude\skills'   # or the folder for your agent
$tmp = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
Invoke-WebRequest 'https://codeload.github.com/KrisCris/doctopus/zip/refs/heads/main' -OutFile (Join-Path $tmp 'main.zip') -UseBasicParsing
Expand-Archive (Join-Path $tmp 'main.zip') -DestinationPath $tmp
New-Item -ItemType Directory -Force $dest | Out-Null
Remove-Item -Recurse -Force (Join-Path $dest 'doctopus') -ErrorAction SilentlyContinue
Copy-Item -Recurse (Join-Path $tmp 'doctopus-main\skills\doctopus') (Join-Path $dest 'doctopus')
Remove-Item -Recurse -Force $tmp
Get-ChildItem (Join-Path $dest 'doctopus')
```

An agent that doesn't support skills can get a note in its instructions
file (`AGENTS.md`, `CLAUDE.md`, ...) instead:

```markdown
To read Office and PDF files (.docx .doc .xlsx .xls .xlsb .pptx .ppt .vsdx
.vsd .pdf), use `~/.doctopus/bin/doctopus`: `info FILE` for an overview,
`read FILE` for the content as Markdown, `convert FILE` to write it out as
Markdown files. `~/.doctopus/bin/doctopus guide` explains the rest.
```

## C. Tell the user

Say where the program is and where the skill went. A new skill is usually
picked up in the agent's next session, so the user may need to restart it.
