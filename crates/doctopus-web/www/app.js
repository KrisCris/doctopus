// The doctopus web app: drop a document, then browse and download its
// conversion. The converter runs in worker.js; this file is the page.
//
// The address of the page names the file shown: `#/report.parts/emb1/Budget.md`.
// Links between converted files point at such addresses (see the doctopus-web
// crate's html module), so following them, and going back, needs no code here.

const MERMAID = {
  src: 'https://cdn.jsdelivr.net/npm/mermaid@12.2.0/dist/mermaid.min.js',
  integrity: 'sha384-8xUhuLNMjW/cI2Dacg3DR4WliYznvIg0LVG1stctThKD/YVrJFI9TNCFxft5NYgp',
};

// Pictures every browser shows, by extension.
const PICTURES = {
  avif: 'image/avif',
  bmp: 'image/bmp',
  gif: 'image/gif',
  ico: 'image/x-icon',
  jpeg: 'image/jpeg',
  jpg: 'image/jpeg',
  png: 'image/png',
  svg: 'image/svg+xml',
  webp: 'image/webp',
};
const TEXT = new Set(['csv', 'htm', 'html', 'json', 'log', 'md', 'rtf', 'tsv', 'txt', 'xml', 'yaml', 'yml']);
// Larger text files are offered for download instead of shown.
const TEXT_LIMIT = 2_000_000;

const ICONS = {
  chevron: '<path d="M9 6l6 6-6 6"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  markdown: '<path d="M6 3h9l4 4v14H6z"/><path d="M15 3v4h4"/><path d="M9 13h7M9 17h5"/>',
  picture: '<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="2"/><path d="M21 17l-5-5-9 8"/>',
  file: '<path d="M6 3h9l4 4v14H6z"/><path d="M15 3v4h4"/>',
  download: '<path d="M12 4v11M7 10l5 5 5-5M5 20h14"/>',
};

const $ = (id) => document.getElementById(id);
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
const darkScheme = matchMedia('(prefers-color-scheme: dark)');

// ---------------------------------------------------------------------------
// The converter, in a worker

class Converter {
  constructor() {
    this.pending = new Map();
    this.next = 1;
    this.start();
  }

  start() {
    this.worker = new Worker(new URL('./worker.js?v=dev', import.meta.url), { type: 'module' });
    this.worker.onmessage = ({ data }) => {
      const call = this.pending.get(data.id);
      if (!call) return;
      this.pending.delete(data.id);
      // After a trap the module may be broken: start afresh.
      if (data.fatal) this.restart();
      if ('error' in data) call.reject(new Error(data.error));
      else call.resolve(data.result);
    };
    this.worker.onerror = (event) => {
      event.preventDefault();
      const message = event.message || 'the converter could not be loaded';
      const hadCalls = this.pending.size > 0;
      for (const call of this.pending.values()) call.reject(new Error(message));
      this.pending.clear();
      if (hadCalls) this.restart();
    };
  }

  restart() {
    this.worker.terminate();
    this.start();
  }

  call(type, args = {}, transfer = []) {
    return new Promise((resolve, reject) => {
      const id = this.next++;
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ id, type, ...args }, transfer);
    });
  }
}

const converter = new Converter();

// ---------------------------------------------------------------------------
// State

// The conversion shown: { manifest, entries, bytes, html, urls }.
let current = null;
let selected = null;
let mode = 'preview';
let busy = false;

converter
  .call('hello')
  .then(({ version, formats }) => {
    $('engine').textContent = `doctopus ${version}, running in this tab`;
    const extensions = [...new Set(formats.flatMap((f) => f.extensions))];
    const list = $('formats');
    list.replaceChildren(
      'Reads ',
      ...extensions.flatMap((ext, i) => {
        const code = document.createElement('code');
        code.textContent = `.${ext}`;
        return i === 0 ? [code] : [' ', code];
      }),
    );
  })
  .catch((err) => {
    $('engine').textContent = `The converter could not be loaded: ${err.message}`;
    $('engine').classList.add('error');
  });

// ---------------------------------------------------------------------------
// Opening files

const input = $('file-input');
for (const id of ['open', 'choose', 'failure-open']) $(id).addEventListener('click', () => input.click());
input.addEventListener('change', () => {
  const [file] = input.files;
  input.value = '';
  if (file) convert(file);
});

const hasFiles = (event) => [...(event.dataTransfer?.types ?? [])].includes('Files');
let dragDepth = 0;
addEventListener('dragenter', (event) => {
  if (!hasFiles(event)) return;
  event.preventDefault();
  dragDepth += 1;
  $('dropzone').hidden = false;
});
addEventListener('dragover', (event) => {
  if (!hasFiles(event)) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = 'copy';
});
addEventListener('dragleave', (event) => {
  if (!hasFiles(event)) return;
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) $('dropzone').hidden = true;
});
addEventListener('drop', (event) => {
  if (!hasFiles(event)) return;
  event.preventDefault();
  dragDepth = 0;
  $('dropzone').hidden = true;
  const [file] = event.dataTransfer.files;
  if (event.dataTransfer.files.length > 1) toast(`Converting ${file.name}; drop one file at a time.`);
  if (file) convert(file);
});

async function convert(file) {
  if (busy) return;
  busy = true;
  $('busy-text').textContent = `Converting ${file.name}…`;
  show('busy');
  try {
    const bytes = await file.arrayBuffer();
    load(await converter.call('convert', { name: file.name, bytes }, [bytes]));
  } catch (err) {
    current = null;
    selected = null;
    $('source').textContent = '';
    $('download-all').hidden = true;
    document.title = 'doctopus';
    history.replaceState(null, '', location.pathname + location.search);
    $('failure-title').textContent = `${file.name} could not be converted`;
    $('failure-text').textContent = err.message;
    show('failure');
  } finally {
    busy = false;
  }
}

function load({ manifest, files, html }) {
  for (const url of current?.urls.values() ?? []) URL.revokeObjectURL(url);
  current = {
    manifest,
    entries: new Map(manifest.files.map((entry) => [entry.path, entry])),
    bytes: new Map(files.map((file) => [file.path, file.bytes])),
    html,
    urls: new Map(),
  };
  $('source').textContent = manifest.source;
  $('source').title = manifest.source;
  document.title = `${manifest.source} · doctopus`;
  $('download-all').hidden = false;
  buildTree();
  show('files');
  const wanted = pathFromHash();
  const first = current.entries.has(wanted) ? wanted : manifest.files[0].path;
  history.replaceState(null, '', addressOf(first));
  showFile(first);
}

function show(state) {
  $('main').dataset.state = state;
  $('welcome').hidden = state !== 'welcome';
  $('busy').hidden = state !== 'busy';
  $('failure').hidden = state !== 'failure';
  $('tree').hidden = state !== 'files';
  $('viewer').hidden = state !== 'files';
}

// ---------------------------------------------------------------------------
// The page's address

function addressOf(path) {
  return `#/${path.split('/').map(encodeURIComponent).join('/')}`;
}

function pathFromHash() {
  if (!location.hash.startsWith('#/')) return null;
  try {
    return decodeURIComponent(location.hash.slice(2));
  } catch {
    return null;
  }
}

addEventListener('hashchange', () => {
  const path = pathFromHash();
  if (current?.entries.has(path) && path !== selected) showFile(path);
});

function go(path) {
  if (path === selected) return;
  location.hash = addressOf(path);
}

// ---------------------------------------------------------------------------
// The file tree

function buildTree() {
  const root = { dirs: new Map(), files: [] };
  for (const entry of current.manifest.files) {
    const names = entry.path.split('/');
    let node = root;
    for (const name of names.slice(0, -1)) {
      if (!node.dirs.has(name)) node.dirs.set(name, { dirs: new Map(), files: [] });
      node = node.dirs.get(name);
    }
    node.files.push(entry);
  }
  $('tree-root').replaceChildren(...treeItems(root, 0));
  const { files, skipped, warnings } = current.manifest;
  const documents = files.filter((f) => f.markdown).length;
  $('tree-count').textContent = `${plural(documents, 'document')}, ${plural(files.length - documents, 'file')}`;

  const foot = $('tree-foot');
  foot.replaceChildren();
  if (warnings > 0) {
    const p = document.createElement('p');
    p.textContent = `${plural(warnings, 'warning')}, listed in the YAML header of each Markdown file.`;
    foot.append(p);
  }
  if (skipped.length > 0) {
    const details = document.createElement('details');
    const summary = document.createElement('summary');
    summary.textContent = `${plural(skipped.length, 'part')} with nothing to save`;
    const list = document.createElement('ul');
    for (const { address, reason } of skipped) {
      const li = document.createElement('li');
      li.textContent = `${address}: ${reason}`;
      list.append(li);
    }
    details.append(summary, list);
    foot.append(details);
  }
}

function treeItems(node, depth) {
  const files = [...node.files].sort(
    (a, b) => b.markdown - a.markdown || collator.compare(baseName(a.path), baseName(b.path)),
  );
  const dirs = [...node.dirs].sort(([a], [b]) => collator.compare(a, b));
  return [
    ...files.map((entry) => fileItem(entry, depth)),
    ...dirs.map(([name, child]) => dirItem(name, child, depth)),
  ];
}

function fileItem(entry, depth) {
  const li = document.createElement('li');
  li.setAttribute('role', 'treeitem');
  li.setAttribute('aria-selected', 'false');
  li.dataset.path = entry.path;
  const row = document.createElement('div');
  row.className = 'row';
  row.style.setProperty('--depth', depth);

  const main = document.createElement('button');
  main.type = 'button';
  main.className = 'row-main';
  main.title = `${entry.path}\n${entry.summary}`;
  main.append(icon(entry.markdown ? 'markdown' : isPicture(entry) ? 'picture' : 'file'), label(baseName(entry.path)));
  const hint = document.createElement('span');
  hint.className = 'row-hint';
  hint.textContent = humanSize(entry.size);
  main.append(hint);
  main.addEventListener('click', () => {
    go(entry.path);
    closeDrawer();
  });

  const save = document.createElement('button');
  save.type = 'button';
  save.className = 'row-save';
  save.setAttribute('aria-label', `Download ${baseName(entry.path)}`);
  save.title = 'Download';
  save.append(icon('download'));
  save.addEventListener('click', () => download(entry.path));

  row.append(main, save);
  li.append(row);
  return li;
}

function dirItem(name, node, depth) {
  const li = document.createElement('li');
  li.setAttribute('role', 'treeitem');
  li.setAttribute('aria-expanded', 'true');
  li.className = 'dir';
  const row = document.createElement('div');
  row.className = 'row';
  row.style.setProperty('--depth', depth);
  const main = document.createElement('button');
  main.type = 'button';
  main.className = 'row-main';
  const chevron = icon('chevron');
  chevron.classList.add('chevron');
  main.append(chevron, icon('folder'), label(name));
  // An embedded document's directory: say what it holds.
  const document_ = node.files.find((f) => f.markdown);
  if (document_) {
    const hint = document.createElement('span');
    hint.className = 'row-hint';
    hint.textContent = document_.summary;
    main.append(hint);
  }
  main.addEventListener('click', () => {
    const open = li.getAttribute('aria-expanded') === 'true';
    li.setAttribute('aria-expanded', String(!open));
  });
  row.append(main);
  const group = document.createElement('ul');
  group.setAttribute('role', 'group');
  group.append(...treeItems(node, depth + 1));
  li.append(row, group);
  return li;
}

function selectInTree(path) {
  for (const li of $('tree-root').querySelectorAll('[aria-selected="true"]')) li.setAttribute('aria-selected', 'false');
  const li = [...$('tree-root').querySelectorAll('li[data-path]')].find((item) => item.dataset.path === path);
  if (!li) return;
  li.setAttribute('aria-selected', 'true');
  for (let dir = li.parentElement.closest('li.dir'); dir; dir = dir.parentElement.closest('li.dir')) {
    dir.setAttribute('aria-expanded', 'true');
  }
  li.querySelector('.row').scrollIntoView({ block: 'nearest' });
}

// ---------------------------------------------------------------------------
// The viewer

const content = $('content');

function showFile(path) {
  selected = path;
  const entry = current.entries.get(path);
  selectInTree(path);

  const crumbs = $('crumbs');
  crumbs.replaceChildren();
  path.split('/').forEach((name, i, names) => {
    if (i > 0) {
      const sep = document.createElement('span');
      sep.className = 'crumb-sep';
      sep.textContent = '/';
      crumbs.append(sep);
    }
    const crumb = document.createElement('span');
    crumb.className = i === names.length - 1 ? 'crumb current' : 'crumb';
    crumb.textContent = name;
    crumbs.append(crumb);
  });
  crumbs.title = path;

  const facts = [entry.summary, entry.address];
  if (entry.tokens != null) facts.push(`about ${entry.tokens.toLocaleString()} tokens`);
  facts.push(humanSize(entry.size));
  $('meta').textContent = facts.join(' · ');

  $('mode').hidden = !entry.markdown;
  render();
  $('scroll').scrollTop = 0;
}

function render() {
  const entry = current.entries.get(selected);
  const bytes = current.bytes.get(selected);
  for (const button of $('mode').querySelectorAll('button')) {
    button.setAttribute('aria-pressed', String(button.dataset.mode === mode));
  }
  content.replaceChildren();
  if (entry.markdown && mode === 'preview') {
    content.className = 'content markdown';
    content.innerHTML = current.html[selected];
    for (const img of content.querySelectorAll('img[data-path]')) {
      const path = img.dataset.path;
      if (isPicture(current.entries.get(path))) {
        img.src = urlFor(path);
        continue;
      }
      // EMF, WMF, TIFF...: link to the file instead of a broken picture.
      const link = document.createElement('a');
      link.href = addressOf(path);
      link.className = 'picture-file';
      link.textContent = `${img.alt || 'Picture'}: ${baseName(path)}`;
      link.title = 'Browsers cannot show this kind of picture; open it to download it';
      img.replaceWith(link);
    }
    drawDiagrams(content);
  } else if (entry.markdown || (isText(entry) && bytes.length <= TEXT_LIMIT)) {
    content.className = 'content source';
    const pre = document.createElement('pre');
    pre.textContent = new TextDecoder().decode(bytes);
    content.append(pre);
  } else if (isPicture(entry)) {
    content.className = 'content picture';
    const img = document.createElement('img');
    img.src = urlFor(selected);
    img.alt = baseName(selected);
    content.append(img);
  } else {
    content.className = 'content card';
    const title = document.createElement('h2');
    title.textContent = baseName(selected);
    const what = document.createElement('p');
    what.textContent = `${entry.summary}. The browser cannot show this file; download it to open it in another program.`;
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'button primary';
    button.textContent = `Download ${baseName(selected)}`;
    button.addEventListener('click', () => download(selected));
    content.append(title, what, button);
  }
}

for (const button of $('mode').querySelectorAll('button')) {
  button.addEventListener('click', () => {
    mode = button.dataset.mode;
    render();
  });
}

// Links inside a file to places in it: the page's address names the file,
// so scroll instead of changing it.
content.addEventListener('click', (event) => {
  const link = event.target.closest('a[href^="#"]');
  if (!link || link.getAttribute('href').startsWith('#/')) return;
  event.preventDefault();
  const id = decodeURIComponent(link.getAttribute('href').slice(1));
  content.querySelector(`#${CSS.escape(id)}`)?.scrollIntoView({ block: 'start', behavior: 'smooth' });
});

let mermaid = null;

function loadMermaid() {
  mermaid ??= new Promise((resolve, reject) => {
    const script = document.createElement('script');
    script.src = MERMAID.src;
    script.integrity = MERMAID.integrity;
    script.crossOrigin = 'anonymous';
    script.referrerPolicy = 'no-referrer';
    script.onload = () => {
      window.mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'strict',
        theme: darkScheme.matches ? 'dark' : 'default',
      });
      resolve(window.mermaid);
    };
    script.onerror = () => {
      mermaid = null;
      script.remove();
      reject(new Error('Mermaid could not be loaded from cdn.jsdelivr.net'));
    };
    document.head.append(script);
  });
  return mermaid;
}

async function drawDiagrams(article) {
  const blocks = [...article.querySelectorAll('pre.mermaid')];
  if (blocks.length === 0) return;
  try {
    const library = await loadMermaid();
    // The reader may have moved on while Mermaid loaded.
    const shown = blocks.filter((block) => block.isConnected);
    if (shown.length > 0) await library.run({ nodes: shown });
  } catch (err) {
    for (const block of blocks) {
      block.classList.add('not-drawn');
      block.title = `Diagram not drawn: ${err.message}`;
    }
  }
}

// ---------------------------------------------------------------------------
// Downloads

$('download').addEventListener('click', () => selected && download(selected));

$('download-all').addEventListener('click', async (event) => {
  const button = event.currentTarget;
  button.disabled = true;
  try {
    const { bytes } = await converter.call('zip');
    save(`${current.manifest.name}.zip`, bytes, 'application/zip');
  } catch (err) {
    toast(`The zip file could not be made: ${err.message}`);
  } finally {
    button.disabled = false;
  }
});

function download(path) {
  const entry = current.entries.get(path);
  save(baseName(path), current.bytes.get(path), entry.markdown ? 'text/markdown' : typeOf(entry));
}

function save(name, bytes, type) {
  const url = URL.createObjectURL(new Blob([bytes], { type }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}

// ---------------------------------------------------------------------------
// The file list as a drawer on narrow screens

const toggleTree = $('toggle-tree');
toggleTree.addEventListener('click', () => {
  const open = $('main').classList.toggle('drawer-open');
  toggleTree.setAttribute('aria-expanded', String(open));
});

function closeDrawer() {
  $('main').classList.remove('drawer-open');
  toggleTree.setAttribute('aria-expanded', 'false');
}

// ---------------------------------------------------------------------------
// Helpers

function urlFor(path) {
  const bytes = current.bytes.get(path);
  if (!bytes) return null;
  if (!current.urls.has(path)) {
    current.urls.set(path, URL.createObjectURL(new Blob([bytes], { type: typeOf(current.entries.get(path)) })));
  }
  return current.urls.get(path);
}

function extension(path) {
  const name = baseName(path);
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

function typeOf(entry) {
  return PICTURES[extension(entry.path)] ?? entry.content_type ?? 'application/octet-stream';
}

function isPicture(entry) {
  return extension(entry.path) in PICTURES;
}

function isText(entry) {
  const type = entry.content_type ?? '';
  return type.startsWith('text/') || /[/+](json|xml)$/.test(type) || TEXT.has(extension(entry.path));
}

function baseName(path) {
  return path.slice(path.lastIndexOf('/') + 1);
}

function plural(n, word) {
  return `${n.toLocaleString()} ${word}${n === 1 ? '' : 's'}`;
}

function humanSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let size = bytes / 1024;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(size < 10 ? 1 : 0)} ${units[unit]}`;
}

function icon(name) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  svg.classList.add('icon');
  svg.innerHTML = ICONS[name];
  return svg;
}

function label(text) {
  const span = document.createElement('span');
  span.className = 'row-name';
  span.textContent = text;
  return span;
}

let toastTimer = 0;
function toast(message) {
  const el = $('toast');
  el.textContent = message;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    el.hidden = true;
  }, 6000);
}
