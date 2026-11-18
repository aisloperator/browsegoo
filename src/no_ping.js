// Removes the `ping` attribute from links, so that following one doesn't also
// send a request to tell the `ping` URLs that it was followed ("hyperlink
// auditing").
//
// It runs in a JavaScript world of BrowseGoo's own, from the start of each
// frame. The observer's callback runs as soon as an attribute is added or
// changed, even by a page's script during a click, which is before the link
// is followed, so the page can't keep one.
(() => {
  "use strict";

  const strip = (element) => {
    if (element.hasAttribute("ping")) {
      element.removeAttribute("ping");
    }
  };

  const stripWithin = (node) => {
    if (node.nodeType !== Node.ELEMENT_NODE) {
      return;
    }
    strip(node);
    for (const element of node.querySelectorAll("[ping]")) {
      strip(element);
    }
  };

  new MutationObserver((mutations) => {
    for (const mutation of mutations) {
      if (mutation.type === "attributes") {
        strip(mutation.target);
      } else {
        mutation.addedNodes.forEach(stripWithin);
      }
    }
  }).observe(document, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ["ping"],
  });

  // Whatever is already there, such as when a frame's document was replaced.
  stripWithin(document.documentElement || document);
})();
