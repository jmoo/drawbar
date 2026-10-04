// Draws each ```mermaid block, which mdbook-mermaid leaves as <pre class="mermaid">,
// in mermaid's dark theme under mdBook's dark themes and its default theme otherwise.
// mdBook switches themes by changing the classes on <html>, so a switch between light
// and dark draws every diagram again from its source.

"use strict";

(() => {
  const DARK = ["ayu", "coal", "navy"];
  const html = document.documentElement;
  const diagrams = [...document.querySelectorAll("pre.mermaid")];
  if (diagrams.length === 0) {
    return;
  }
  // Mermaid replaces each block's text with its drawing.
  const sources = diagrams.map((pre) => pre.textContent);
  let drawn = null;

  const draw = () => {
    const dark = DARK.some((theme) => html.classList.contains(theme));
    if (dark === drawn) {
      return;
    }
    drawn = dark;
    diagrams.forEach((pre, at) => {
      pre.removeAttribute("data-processed");
      pre.textContent = sources[at];
    });
    mermaid.initialize({ startOnLoad: false, theme: dark ? "dark" : "default" });
    mermaid.run({ nodes: diagrams });
  };

  draw();
  new MutationObserver(draw).observe(html, { attributes: true, attributeFilter: ["class"] });
})();
