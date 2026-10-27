// A small DOM for the synthetic X fixtures, so Node tests run extension/xdom.js with no package:
// the HTML the fixtures use, and the element calls xdom.js makes. Required by extension.cjs;
// not a test file.
const VOID = new Set(['meta', 'img', 'br', 'input', 'link', 'hr', 'source']);
const TOKEN = /<!--[\s\S]*?-->|<!doctype[^>]*>|<\/([a-z][a-z0-9]*)\s*>|<([a-z][a-z0-9]*)((?:\s+[a-z][\w:-]*(?:\s*=\s*"[^"]*")?)*)\s*\/?>|[^<]+/gi;
const ATTRIBUTE = /([a-z][\w:-]*)(?:\s*=\s*"([^"]*)")?/gi;

const decode = (text) => text.replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');

// `a[b="c"][d]` compounds, joined by descendant spaces, in a comma list.
function parseSelector(selector) {
  return selector.split(',').map((one) =>
    one.trim().split(/\s+/).map((text) => {
      const match = /^([a-z][a-z0-9]*)?((?:\[[^\]]+\])*)$/i.exec(text);
      if (!match) throw new Error(`x_dom: unsupported selector ${text}`);
      const attrs = [...match[2].matchAll(/\[([\w-]+)(?:="([^"]*)")?\]/g)].map(([, name, value]) => ({ name, value }));
      return { tag: match[1]?.toLowerCase(), attrs };
    }),
  );
}

const matchesCompound = (el, { tag, attrs }) =>
  (!tag || el.localName === tag) && attrs.every(({ name, value }) => el.attributes.has(name) && (value === undefined || el.attributes.get(name) === value));

function matchesChain(el, chain) {
  if (!matchesCompound(el, chain[chain.length - 1])) return false;
  let i = chain.length - 2;
  for (let up = el.parentElement; i >= 0 && up; up = up.parentElement) if (matchesCompound(up, chain[i])) i -= 1;
  return i < 0;
}

class Node {
  constructor() {
    this.parentNode = null;
    this.childNodes = [];
  }

  get parentElement() {
    return this.parentNode instanceof Element ? this.parentNode : null;
  }

  get children() {
    return this.childNodes.filter((node) => node instanceof Element);
  }

  get textContent() {
    return this.childNodes.map((node) => node.textContent).join('');
  }

  append(node) {
    node.parentNode = this;
    this.childNodes.push(node);
  }

  contains(node) {
    for (let at = node; at; at = at.parentNode) if (at === this) return true;
    return false;
  }

  *descendants() {
    for (const child of this.children) {
      yield child;
      yield* child.descendants();
    }
  }

  querySelectorAll(selector) {
    const chains = parseSelector(selector);
    return [...this.descendants()].filter((el) => chains.some((chain) => matchesChain(el, chain)));
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }
}

class Text extends Node {
  constructor(text) {
    super();
    this.text = text;
  }

  get textContent() {
    return this.text;
  }
}

class Element extends Node {
  constructor(localName, attributes) {
    super();
    this.localName = localName;
    this.tagName = localName.toUpperCase();
    this.attributes = attributes;
  }

  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }

  matches(selector) {
    return parseSelector(selector).some((chain) => matchesChain(this, chain));
  }

  closest(selector) {
    for (let at = this; at; at = at.parentElement) if (at.matches(selector)) return at;
    return null;
  }
}

class Document extends Node {}

// The fixture's document: elements, attributes and text; comments and the doctype dropped.
function parseHTML(html) {
  const document = new Document();
  let open = document;
  for (const [token, closing, opening, attrs] of html.matchAll(TOKEN)) {
    if (token.startsWith('<!')) continue;
    if (closing) {
      for (let at = open; at instanceof Element; at = at.parentNode) {
        if (at.localName === closing.toLowerCase()) {
          open = at.parentNode;
          break;
        }
      }
    } else if (opening) {
      const attributes = new Map([...(attrs ?? '').matchAll(ATTRIBUTE)].map(([, name, value]) => [name.toLowerCase(), decode(value ?? '')]));
      const el = new Element(opening.toLowerCase(), attributes);
      open.append(el);
      if (!VOID.has(el.localName) && !token.endsWith('/>')) open = el;
    } else {
      open.append(new Text(decode(token)));
    }
  }
  return document;
}

// What `event.composedPath()` gives for a click on `el`: it, its ancestors, the document, the window.
function composedPath(el) {
  const path = [];
  for (let at = el; at; at = at.parentNode) path.push(at);
  return [...path, { window: true }];
}

module.exports = { parseHTML, composedPath };
