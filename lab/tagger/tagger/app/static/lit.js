/* knowmoretabs · tagger app · lit-html for the classic scripts
   The vendored lit-html (an ES module; version, source and hash in the
   README) handed to the views as K.lit. Every script is deferred, so this
   runs in document order: after core.js, before the views that use it. */
import { html, nothing, render, repeat, unsafeHTML } from "./lit-html.js";

window.KMT.lit = { html, nothing, render, repeat, unsafeHTML };
