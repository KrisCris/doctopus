---
name: doctopus
description: Read, search, extract from and convert Word, Excel, PowerPoint, Visio and PDF files (.docx .doc .xlsx .xls .xlsb .pptx .ppt .vsdx .vsd .pdf and their macro and template variants) with the doctopus command-line tool, including everything embedded in them, such as a workbook inside a slide, files attached to a PDF, and images. Use this skill whenever a task needs what is inside an Office or PDF file. That covers reading or summarizing it, answering questions about it, finding something in it, and pulling out a table, sheet, slide, page, picture or attachment. It also covers turning one or many documents into Markdown files. Use it even when the user does not mention doctopus, and use it instead of unzipping the file, parsing its XML, or writing python-docx, openpyxl, python-pptx or PDF-library code.
---

# doctopus

doctopus is one self-contained program that turns Office and PDF files into
Markdown. It handles everything nested inside them, at any depth. It is
faster and more reliable than taking a file apart yourself. It keeps
headings, lists, tables, sheets, slides, diagrams and the documents
embedded in each other. Its output is the same every time. So let doctopus
do the extraction, and spend your effort on what the user actually asked.

## 1. Find the program

Look for the binary in this order, and use the first one that exists:

1. A path the user gave you, in the conversation or in project
   instructions such as `AGENTS.md` or `CLAUDE.md`.
2. `$DOCTOPUS_HOME/bin/doctopus`, if the `DOCTOPUS_HOME` environment
   variable is set.
3. `~/.doctopus/bin/doctopus`, the default install location. On Windows
   this is `%USERPROFILE%\.doctopus\bin\doctopus.exe`.
4. `doctopus` on the `PATH`.

On macOS and Linux, this checks 2, 3 and 4 in one command:

```sh
for d in "${DOCTOPUS_HOME:+$DOCTOPUS_HOME/bin/doctopus}" "$HOME/.doctopus/bin/doctopus" "$(command -v doctopus)"; do [ -n "$d" ] && [ -x "$d" ] && { echo "$d"; "$d" --version; break; }; done
```

It prints the path and the version. If it prints nothing, doctopus is not
installed.

In PowerShell:

```powershell
$home_dir = if ($env:DOCTOPUS_HOME) { $env:DOCTOPUS_HOME } else { Join-Path $HOME '.doctopus' }
@((Join-Path $home_dir 'bin\doctopus.exe'), (Get-Command doctopus -ErrorAction SilentlyContinue).Source) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
```

Once you have the path, write it out in full in every command, for example
`~/.doctopus/bin/doctopus info report.docx`. Each shell command may run in
a fresh shell, so a variable set in one command is gone in the next. The
examples below write `doctopus` for short.

**If doctopus is not found**, tell the user. Ask whether you may install
it into `~/.doctopus/bin`; this downloads a program from GitHub. If they
agree, follow [references/install.md](references/install.md). If they
decline, say what you could not do without it. Do not quietly switch to
parsing the file some other way.

## 2. Pick the command for the task

| The user wants | Run |
|---|---|
| To know what a file holds, or where something is | `doctopus info FILE` |
| To have it read, summarized or answered from | `doctopus read FILE`, page by page |
| One sheet, slide or page, or some cells | `doctopus read FILE --sheet 2` (or `--slide`, `--page`; numbers like `1,3-5`, or names like `Summary`, `"Q*"`), plus `--range A1:F50` for cells |
| Spreadsheet formulas | `doctopus read FILE --formulas` |
| The document kept as Markdown files | `doctopus convert FILE` (writes next to FILE) or `doctopus convert FILE -o DIR` |
| Pictures or attachments saved | `doctopus extract FILE` (to `NAME.parts/` next to FILE) or `-o DIR` |
| A survey of several files | `doctopus info a.docx b.xlsx c.pdf` |
| Many files converted | `doctopus convert docs/*.docx docs/*.pdf -o converted` |
| Anything not covered here | `doctopus guide`, which prints the full usage guide |

For a file you have not seen yet, `info` is a cheap first step. It lists the
sheets, slides or pages and every embedded part, and gives the size of the
full output in tokens. It ends with suggested next commands. Use it to
decide whether to read everything or only the part you need.

## 3. Read the output correctly

**Paging.** `read` prints at most 25,000 characters per call. When there is
more, a note on stderr gives the exact command for the next page, which
adds `--offset N`. Run that command to continue. Don't guess offsets, and
don't stop early and summarize from the first page as if it were the whole
document. For a long document, two other ways work better:

- `doctopus read FILE -o OUT.md` writes everything to one file. It
  prints the line where each sheet, slide or page starts, so you can open
  that file in chunks.
- `doctopus convert FILE -o DIR`, then read the Markdown files with your
  normal file tools.

To search a document, print it whole and pipe it:
`doctopus read FILE --max-chars 0 | grep -n -i "term"`.

**Embedded documents.** A document inside another one appears in place,
between `<embedded id=... ref=...>` and `</embedded>`. To read one of them
on its own, use its address: the file, `#`, then the id, with nested ids
joined by `/`. Always quote addresses, because `#` starts a comment in
some shells:

```sh
doctopus info "deck.pptx#emb1"
doctopus read "deck.pptx#emb1/emb2" --sheet 1
```

**The YAML header.** Each Markdown output starts with a header holding the
source, format, properties, counts, parts and warnings. Tell the user about
warnings that matter for their question. For example, a PDF page that is
only a scanned picture has no text. The warning lists its picture ids; run
`doctopus extract "FILE#imgN"` and look at the saved image if you can view
images.

**Pictures.** `extract` saves images as ordinary files and lists each file
with its address. If you can view images, open the saved files to see them.

## 4. Converting to Markdown files

`doctopus convert FILE` writes `NAME.md` and a `NAME.parts/` folder next to
FILE. Embedded documents become their own Markdown files inside the parts
folder, and every file links to the others with relative links. `-o DIR`
writes somewhere else. The report it prints starts with the directory it
wrote to, as an absolute path. It then lists every file, relative to that
directory, and ends with `next: read <file>`, the file to open first.
Pass those locations on to the user exactly as reported.

Running `convert` again replaces the earlier conversion. It never replaces
a file it did not make. If a `NAME.md` of the user's own is already there,
the report lists it under `left alone` and writes `NAME-2.md` instead.
Mention this to the user so they are not surprised by the new name.

## 5. When something fails

Errors go to stderr. Each one says what exists, for example the sheet
names or the part ids, and suggests a command. Follow the suggestion,
don't guess. The exit codes:

| Code | Meaning | What to do |
|---|---|---|
| 0 | Success | |
| 1 | The file could not be read: missing or damaged | Check the path; tell the user if the file is damaged |
| 2 | Invalid arguments: an unknown option, or no such part, sheet or range | Read the error, which lists what exists, and retry |
| 3 | Unsupported file type | `doctopus formats` lists what it reads; tell the user |
| 4 | Encrypted file | Ask the user for a copy without a password |

## 6. For people who want to look themselves

If the user wants to browse a conversion with their own eyes, point them
to the web app at <https://kriscris.github.io/doctopus/>. It converts a
dropped file in the browser, uploads nothing, and shows the files as a
tree beside a viewer that follows the links between them.
