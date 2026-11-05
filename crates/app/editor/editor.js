"use strict";
const body = document.getElementById("body");
const bar = document.getElementById("bar");
const tip = document.getElementById("tip");

/// The most a picture may add to a message, before encoding.
const MOST_IMAGE_BYTES = 8 * 1024 * 1024;
/// A file past this is refused before it is read, as the app would only refuse it after.
const MOST_FILE_BYTES = 18 * 1024 * 1024;

const FONTS = [["Sans", "Arial, Helvetica, sans-serif"], ["Serif", "Georgia, 'Times New Roman', serif"], ["Fixed width", "Consolas, 'Courier New', monospace"]];
const SIZES = [["Small", "12px"], ["Normal", ""], ["Large", "18px"], ["Huge", "24px"]];
const ALIGNS = [["Left", "justifyLeft"], ["Centre", "justifyCenter"], ["Right", "justifyRight"], ["More indent", "indent"], ["Less indent", "outdent"]];
/// A colour no one would choose, standing for "no colour" while the browser applies it.
const NO_INK = "#010203";
const INKS = ["", "#7d6a6e", "#c4122f", "#d9730d", "#2e7d32", "#1565c0", "#6a1b9a", "#ad1457"];
const MARKS = ["", "#fff3b0", "#ffd6a5", "#ffc9c9", "#c8f0c0", "#bde0fe", "#e0c8ff", "#e6e6e6"];

/// Talks to the app. The page only ever reports; the app decides what anything means.
function tell(message) {
  if (window.ipc) window.ipc.postMessage(JSON.stringify(message));
  else (window.told ??= []).push(message);
}

// ---- What may stay in the message: applies to anything pasted, dropped or loaded. ----

const TAGS = new Set(["A", "B", "BLOCKQUOTE", "BR", "CODE", "DIV", "EM", "FONT", "H1", "H2", "H3", "HR", "I", "IMG", "LI", "OL", "P", "PRE", "S", "SPAN", "STRIKE", "STRONG", "SUB", "SUP", "TABLE", "TBODY", "TD", "TH", "THEAD", "TR", "U", "UL"]);
const DROPPED_WHOLE = new Set(["SCRIPT", "STYLE", "IFRAME", "OBJECT", "EMBED", "LINK", "META", "TITLE", "HEAD", "SVG", "MATH", "TEMPLATE", "FORM", "INPUT", "BUTTON", "TEXTAREA", "SELECT", "VIDEO", "AUDIO"]);
const STYLES = ["color", "background-color", "font-family", "font-size", "font-weight", "font-style", "text-decoration-line", "text-align", "margin-left", "padding-left"];
const SAFE_LINK = /^(https?:|mailto:)/i;
const SAFE_IMAGE = /^(data:image\/(png|jpeg|gif|webp);base64,|cid:)/i;

function clean(html) {
  const parsed = new DOMParser().parseFromString(html, "text/html");
  const out = document.createDocumentFragment();
  for (const node of [...parsed.body.childNodes]) cleanInto(node, out);
  return out;
}

function cleanInto(node, parent) {
  if (node.nodeType === Node.TEXT_NODE) {
    parent.append(node.textContent);
    return;
  }
  if (node.nodeType !== Node.ELEMENT_NODE || DROPPED_WHOLE.has(node.tagName.toUpperCase())) return;
  if (!TAGS.has(node.tagName)) {
    for (const child of [...node.childNodes]) cleanInto(child, parent);
    return;
  }
  const element = document.createElement(node.tagName);
  if (node.tagName === "A" && SAFE_LINK.test(node.getAttribute("href") ?? "")) element.href = node.getAttribute("href");
  if (node.tagName === "IMG") {
    if (!SAFE_IMAGE.test(node.getAttribute("src") ?? "")) return;
    element.src = node.getAttribute("src");
    element.alt = node.getAttribute("alt") ?? "";
    if (/^[\w-]+$/.test(node.dataset.cid ?? "")) element.dataset.cid = node.dataset.cid;
  }
  if (node.tagName === "FONT") {
    for (const name of ["color", "face", "size"]) if (node.hasAttribute(name)) element.setAttribute(name, node.getAttribute(name));
  }
  for (const name of ["signature", "quoted"]) if (node.classList.contains(name)) element.classList.add(name);
  for (const name of STYLES) {
    const value = node.style.getPropertyValue(name);
    if (value && !/url\(|expression/i.test(value)) element.style.setProperty(name, value);
  }
  for (const child of [...node.childNodes]) cleanInto(child, element);
  parent.append(element);
}

// ---- Telling the app ----

/// The message as it is kept: pictures are files beside it, so that this stays small enough to
/// send on every key, and nothing is ever waiting to be reported when the user leaves.
function serialise() {
  const copy = document.implementation.createHTMLDocument("").importNode(body, true);
  for (const image of copy.querySelectorAll("img")) {
    image.removeAttribute("class");
    if (image.dataset.cid) {
      image.setAttribute("src", `cid:${image.dataset.cid}`);
      image.removeAttribute("data-cid");
    }
  }
  return copy.innerHTML;
}

/// The text for those who want no formatting. The browser trims the space a signature needs.
function plainText() {
  return body.innerText.replace(/^--$/m, "-- ");
}

function changed(kind = "changed") {
  tell({ kind, html: serialise(), text: plainText() });
}

/// Hands the app every picture it has not seen, each once, as a file with an id.
function adoptPictures() {
  for (const image of body.querySelectorAll("img:not([data-cid])")) {
    const [head, data] = image.src.split(",");
    const mime = /^data:(image\/[a-z]+);base64$/.exec(head)?.[1];
    if (!mime) continue;
    image.dataset.cid = crypto.randomUUID();
    tell({ kind: "picture", cid: image.dataset.cid, name: image.alt || "picture", mime, data });
  }
}

// ---- Commands ----

let saved = null;

function remember() {
  const selection = getSelection();
  if (selection.rangeCount && body.contains(selection.anchorNode)) saved = selection.getRangeAt(0).cloneRange();
}

function restore() {
  body.focus();
  if (!saved) return;
  const selection = getSelection();
  selection.removeAllRanges();
  selection.addRange(saved);
}

function run(command, value = null) {
  restore();
  document.execCommand("styleWithCSS", false, true);
  document.execCommand(command, false, value);
  closeMenus();
  remember();
  refresh();
  changed();
}

/// Wraps the selection in a span with this style, which the browser has no command for.
function setFontSize(size) {
  restore();
  const selection = getSelection();
  if (!selection.rangeCount || selection.isCollapsed) {
    // Nothing to wrap, so it applies to what comes next, through a span with a place to type.
    const span = document.createElement("span");
    if (size) span.style.fontSize = size;
    span.append("​");
    selection.getRangeAt(0).insertNode(span);
    selection.collapse(span.firstChild, 1);
  } else {
    // A marker the browser can apply through its own undo history, swapped for the real size.
    document.execCommand("styleWithCSS", false, false);
    document.execCommand("fontSize", false, "7");
    for (const font of [...body.querySelectorAll('font[size="7"]')]) {
      const span = document.createElement("span");
      if (size) span.style.fontSize = size;
      span.append(...font.childNodes);
      font.replaceWith(span);
      span.querySelectorAll("span").forEach((inner) => (inner.style.fontSize = ""));
    }
  }
  closeMenus();
  remember();
  changed();
}

function toggleBlock(tag) {
  const inside = enclosing(tag);
  run("formatBlock", inside ? "div" : tag);
}

function enclosing(tag) {
  const selection = getSelection();
  let node = selection.anchorNode;
  while (node && node !== body) {
    if (node.nodeName === tag.toUpperCase()) return node;
    node = node.parentNode;
  }
  return null;
}

// ---- The toolbar ----

function refresh() {
  for (const button of bar.querySelectorAll("[data-do]")) {
    let on = false;
    try { on = document.queryCommandState(button.dataset.do); } catch {}
    button.classList.toggle("on", on);
    button.setAttribute("aria-pressed", on);
  }
  const quote = bar.querySelector("[data-block]");
  quote.classList.toggle("on", !!enclosing("blockquote"));
  bar.querySelector("[data-link]").classList.toggle("on", !!enclosing("a"));
  const colour = document.queryCommandValue("foreColor");
  document.getElementById("swatch").style.background = colour || "currentColor";
}

function closeMenus() {
  document.querySelectorAll(".menu.open").forEach((menu) => menu.classList.remove("open"));
}

function openMenu(menu, anchor) {
  const wasOpen = menu.classList.contains("open");
  closeMenus();
  if (wasOpen) return;
  menu.classList.add("open");
  const box = anchor.getBoundingClientRect();
  const left = Math.min(box.left, innerWidth - menu.offsetWidth - 8);
  menu.style.left = `${Math.max(left, 8)}px`;
  // From the bottom, so that it stays on the bar as the window changes size.
  menu.style.bottom = `${innerHeight - box.top + 6}px`;
}

function fillMenu(id, items, act) {
  const menu = document.getElementById(id);
  for (const [name, value] of items) {
    const button = document.createElement("button");
    button.textContent = name;
    button.setAttribute("role", "menuitem");
    if (id === "fonts") button.style.fontFamily = value;
    if (id === "sizes" && value) button.style.fontSize = value;
    button.addEventListener("click", () => act(value));
    menu.append(button);
  }
}

fillMenu("fonts", FONTS, (font) => run("fontName", font));
fillMenu("sizes", SIZES, setFontSize);
fillMenu("aligns", ALIGNS, (command) => run(command));

/// Takes the selection back to the reader's own text colour, which is no colour at all.
function clearColour() {
  run("foreColor", NO_INK);
  for (const element of body.querySelectorAll("[style*='color']")) {
    if (element.style.color === "rgb(1, 2, 3)") element.style.removeProperty("color");
    if (!element.getAttribute("style")) element.removeAttribute("style");
  }
  changed();
}

function colourRow(title, colours, act) {
  const menu = document.getElementById("colours");
  const label = document.createElement("div");
  label.className = "title";
  label.textContent = title;
  const row = document.createElement("div");
  row.className = "row";
  for (const colour of colours) {
    const button = document.createElement("button");
    button.style.background = colour || (title === "Text" ? "var(--text)" : "transparent");
    button.ariaLabel = `${title} ${colour || "none"}`;
    if (!colour && title !== "Text") button.innerHTML = '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2"><path d="M5 5l14 14"/></svg>';
    button.addEventListener("click", () => act(colour));
    row.append(button);
  }
  menu.append(label, row);
}

colourRow("Text", INKS, (colour) => (colour ? run("foreColor", colour) : clearColour()));
colourRow("Highlight", MARKS, (colour) => run("hiliteColor", colour || "transparent"));

bar.addEventListener("mousedown", (event) => {
  // The selection stays where it is, so that the button acts on it.
  remember();
  event.preventDefault();
});
document.querySelectorAll(".menu:not(#link)").forEach((menu) => menu.addEventListener("mousedown", (event) => event.preventDefault()));

bar.addEventListener("click", (event) => {
  const button = event.target.closest("button");
  if (!button) return;
  hideTip();
  const { dataset } = button;
  if (dataset.tell) tell({ kind: dataset.tell });
  else if (dataset.do) run(dataset.do);
  else if (dataset.block) toggleBlock(dataset.block);
  else if (dataset.menu) openMenu(document.getElementById(dataset.menu), button);
  else if ("link" in dataset) openLink(button);
  else if ("image" in dataset) document.getElementById("picker").click();
});

let tipTimer = 0;

function hideTip() {
  clearTimeout(tipTimer);
  tip.classList.remove("shown");
}

bar.addEventListener("mouseover", (event) => {
  const button = event.target.closest("[data-tip]");
  hideTip();
  if (!button) return;
  tipTimer = setTimeout(() => {
    tip.textContent = button.dataset.tip;
    const box = button.getBoundingClientRect();
    const left = box.left + box.width / 2 - tip.offsetWidth / 2;
    tip.style.left = `${Math.max(8, Math.min(left, innerWidth - tip.offsetWidth - 8))}px`;
    tip.style.top = `${box.top - tip.offsetHeight - 6}px`;
    tip.classList.add("shown");
  }, 400);
});
bar.addEventListener("mouseleave", hideTip);

// ---- Links ----

const linkMenu = document.getElementById("link");
const linkText = document.getElementById("link-text");
const linkUrl = document.getElementById("link-url");
let linkAnchor = null;

function openLink(button) {
  remember();
  linkAnchor = enclosing("a");
  linkText.value = linkAnchor ? linkAnchor.textContent : getSelection().toString();
  linkUrl.value = linkAnchor ? linkAnchor.getAttribute("href") : "";
  document.getElementById("link-remove").style.display = linkAnchor ? "" : "none";
  openMenu(linkMenu, button ?? bar.querySelector("[data-link]"));
  (linkText.value ? linkUrl : linkText).focus();
}

/// What was typed as an address, as one a mail client can follow.
function address(typed) {
  const url = typed.trim();
  if (!url) return "";
  if (SAFE_LINK.test(url)) return url;
  if (/^[^\s@]+@[^\s@]+$/.test(url)) return `mailto:${url}`;
  return /^[a-z][a-z0-9+.-]*:/i.test(url) ? "" : `https://${url}`;
}

function saveLink() {
  const url = address(linkUrl.value);
  if (!url) {
    linkUrl.focus();
    return;
  }
  const text = linkText.value || url.replace(/^mailto:/, "");
  restore();
  if (linkAnchor) {
    linkAnchor.href = url;
    linkAnchor.textContent = text;
  } else {
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.textContent = text;
    document.execCommand("insertHTML", false, anchor.outerHTML);
  }
  closeMenus();
  remember();
  refresh();
  changed();
}

document.getElementById("link-save").addEventListener("click", saveLink);
document.getElementById("link-remove").addEventListener("click", () => {
  if (!linkAnchor) return;
  linkAnchor.replaceWith(...linkAnchor.childNodes);
  closeMenus();
  body.focus();
  changed();
});
linkMenu.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    saveLink();
  } else if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    closeMenus();
    restore();
  }
});

// ---- Pictures and files ----

function readAsUrl(file) {
  return new Promise((done, failed) => {
    const reader = new FileReader();
    reader.onload = () => done(reader.result);
    reader.onerror = failed;
    reader.readAsDataURL(file);
  });
}

async function insertPictures(files) {
  for (const file of files) {
    if (file.size > MOST_IMAGE_BYTES) {
      tell({ kind: "problem", text: `${file.name} is too large to put in the message` });
      continue;
    }
    const image = document.createElement("img");
    image.src = await readAsUrl(file);
    image.alt = file.name ?? "";
    restore();
    document.execCommand("insertHTML", false, image.outerHTML);
    remember();
  }
  adoptPictures();
  changed();
}

document.getElementById("picker").addEventListener("change", (event) => {
  insertPictures([...event.target.files]);
  event.target.value = "";
});

async function handleFiles(files) {
  const pictures = files.filter((file) => /^image\/(png|jpeg|gif|webp)$/.test(file.type));
  await insertPictures(pictures);
  for (const file of files.filter((file) => !pictures.includes(file))) {
    if (file.size > MOST_FILE_BYTES) {
      tell({ kind: "problem", text: `${file.name} is too large to attach` });
      continue;
    }
    tell({ kind: "file", name: file.name, mime: file.type, data: (await readAsUrl(file)).split(",")[1] ?? "" });
  }
}

body.addEventListener("paste", (event) => {
  event.preventDefault();
  const data = event.clipboardData;
  const files = [...data.files];
  if (files.length) {
    handleFiles(files);
    return;
  }
  const html = data.getData("text/html");
  if (html) {
    const holder = document.createElement("div");
    holder.append(clean(html));
    document.execCommand("insertHTML", false, holder.innerHTML);
    adoptPictures();
  } else {
    document.execCommand("insertText", false, data.getData("text/plain"));
  }
});

// Anything dropped anywhere stays in the app, rather than the page trying to open it.
addEventListener("dragover", (event) => event.preventDefault());
addEventListener("drop", (event) => {
  event.preventDefault();
  const position = document.caretRangeFromPoint(event.clientX, event.clientY);
  if (position && body.contains(position.startContainer)) saved = position;
  handleFiles([...event.dataTransfer.files]);
});

// ---- Keys ----

const SHORTCUTS = {
  "ctrl+shift+x": () => run("strikeThrough"),
  "ctrl+shift+7": () => run("insertOrderedList"),
  "ctrl+shift+8": () => run("insertUnorderedList"),
  "ctrl+shift+9": () => toggleBlock("blockquote"),
  "ctrl+[": () => run("outdent"),
  "ctrl+]": () => run("indent"),
  "ctrl+\\": () => run("removeFormat"),
  "ctrl+k": () => openLink(),
};

function shortcut(event) {
  const code = event.code.startsWith("Digit") ? event.code.slice(5) : event.key.toLowerCase();
  return [event.ctrlKey && "ctrl", event.shiftKey && "shift", code].filter(Boolean).join("+");
}

addEventListener("keydown", (event) => {
  if (event.key === "Enter" && event.ctrlKey) {
    event.preventDefault();
    tell({ kind: "send" });
  } else if (event.key === "Escape") {
    event.preventDefault();
    hideTip();
    if (document.querySelector(".menu.open")) {
      closeMenus();
      restore();
    } else {
      tell({ kind: "close" });
    }
  } else if (event.ctrlKey && !event.shiftKey && /^Digit[1-9]$/.test(event.code)) {
    event.preventDefault();
    tell({ kind: "sender", index: Number(event.code.slice(5)) - 1 });
  } else if (SHORTCUTS[shortcut(event)]) {
    event.preventDefault();
    SHORTCUTS[shortcut(event)]();
  } else if (event.key === "Tab" && document.activeElement === body) {
    event.preventDefault();
    const list = enclosing("li");
    if (list) run(event.shiftKey ? "outdent" : "indent");
    else if (event.shiftKey) tell({ kind: "leave" });
    else document.execCommand("insertText", false, "\t");
  }
});

// ---- The message ----

body.addEventListener("input", () => changed());
body.addEventListener("keyup", () => { remember(); refresh(); });
body.addEventListener("mouseup", () => { remember(); refresh(); });
body.addEventListener("click", (event) => {
  // A link here is something being written, so a click edits it rather than leaving.
  body.querySelectorAll("img.picked").forEach((image) => image.classList.remove("picked"));
  if (event.target.tagName === "IMG") {
    event.target.classList.add("picked");
    const range = document.createRange();
    range.selectNode(event.target);
    const selection = getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    remember();
  }
  if (event.target.closest("a")) event.preventDefault();
});
document.addEventListener("mousedown", (event) => {
  if (!event.target.closest(".menu") && !event.target.closest("#bar")) closeMenus();
});

/// Called by the app.
const editor = {
  /// Puts a message in the editor, with the caret where writing starts.
  load(html) {
    body.replaceChildren(clean(html));
    if (!body.firstChild) body.append(document.createElement("div"));
    saved = null;
    adoptPictures();
    editor.focus();
    refresh();
    // The browser has its own way of writing what it was given, and that is the starting point.
    changed("loaded");
  },
  focus() {
    body.focus();
    if (!saved) getSelection().collapse(body, 0);
    else restore();
    remember();
  },
  theme({ dark, ...values }) {
    for (const [name, value] of Object.entries(values)) document.documentElement.style.setProperty(`--${name}`, value);
    document.documentElement.toggleAttribute("data-dark", dark);
  },
  /// Replaces the signature, or adds one above anything quoted.
  sign(html) {
    let signature = body.querySelector(".signature");
    if (!html) {
      signature?.remove();
    } else {
      if (!signature) {
        signature = document.createElement("div");
        signature.className = "signature";
        body.insertBefore(signature, body.querySelector(".quoted"));
      }
      signature.replaceChildren(...clean(html).firstChild.childNodes);
    }
    changed();
  },
};
window.editor = editor;
refresh();
tell({ kind: "ready" });
