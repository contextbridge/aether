(() => {
  "use strict";

  const HOVER = "data-aether-hover";
  const COMMENTED = "data-aether-commented";
  const ARMED = "data-aether-armed";
  const MAX_EXCERPT = 120;
  const DWELL_MS = 120;
  const SCROLL_GRACE_MS = 150;
  const UNCOMMENTABLE = new Set(["HTML", "BODY", "IFRAME", "FRAME"]);
  const origin = location.origin;
  let armed = false;
  let hovered = null;
  let pending = null;
  let dwell = null;
  let scrollUntil = 0;
  let sequence = 0;

  document.addEventListener("mouseover", onHover, true);
  document.addEventListener("mouseout", onOut, true);
  document.addEventListener("click", onClick, true);
  document.addEventListener("keydown", onKey, true);
  document.addEventListener("scroll", onScroll, true);
  document.addEventListener("wheel", onScroll, { capture: true, passive: true });
  window.addEventListener("message", onMessage);

  post({ type: "hello" });

  function post(message) {
    window.top.postMessage({ aether: "picker", ...message }, origin);
  }

  function onMessage(event) {
    if (event.origin !== origin) return;
    const message = event.data;
    if (!message || message.aether !== "shell") return;
    if (message.type === "state") setArmed(Boolean(message.armed));
    else if (message.type === "highlight") setHighlight(message.key, Boolean(message.on));
    else if (message.type === "unpick") unpick(message.key);
  }

  function setArmed(next) {
    armed = next;
    if (armed) document.documentElement.setAttribute(ARMED, "");
    else document.documentElement.removeAttribute(ARMED);
    if (!armed) clearHover();
  }

  function setHighlight(key, on) {
    const node = document.querySelector(`[${COMMENTED}="${key}"]`);
    if (node) node.toggleAttribute(HOVER, on);
  }

  function unpick(key) {
    document.querySelector(`[${COMMENTED}="${key}"]`)?.removeAttribute(COMMENTED);
  }

  function onHover(event) {
    const target = event.target;
    if (!armed || performance.now() < scrollUntil || !isCommentable(target)) return clearHover();
    if (target === hovered || target === pending) return;
    clearHover();
    pending = target;
    dwell = setTimeout(() => {
      dwell = null;
      pending = null;
      hovered = target;
      target.setAttribute(HOVER, "");
    }, DWELL_MS);
  }

  function onOut(event) {
    if (event.target === hovered || event.target === pending) clearHover();
  }

  function onScroll() {
    scrollUntil = performance.now() + SCROLL_GRACE_MS;
    clearHover();
  }

  function clearHover() {
    if (dwell !== null) clearTimeout(dwell);
    dwell = null;
    pending = null;
    if (hovered) hovered.removeAttribute(HOVER);
    hovered = null;
  }

  // The picker never arms itself: it asks the shell, which owns the state and broadcasts it back.
  function onKey(event) {
    if (event.repeat || isTyping(event)) return;
    if (event.key === "Escape") {
      if (armed) post({ type: "toggle", armed: false });
      return;
    }
    if (event.key.toLowerCase() !== "c" || event.metaKey || event.ctrlKey || event.altKey) return;
    event.preventDefault();
    post({ type: "toggle", armed: !armed });
  }

  // Alt+click reaches the page itself, so a live app stays usable mid-review.
  function onClick(event) {
    const target = event.target;
    if (!armed || event.altKey || !isCommentable(target)) return;
    event.preventDefault();
    event.stopPropagation();
    if (target.hasAttribute(COMMENTED)) {
      post({ type: "reveal", key: target.getAttribute(COMMENTED) });
      return;
    }
    sequence += 1;
    const key = `a${sequence}`;
    target.setAttribute(COMMENTED, key);
    post({
      type: "pick",
      key,
      tag: target.tagName,
      element: target.cloneNode(false).outerHTML,
      excerpt: excerptFor(target),
      path: location.pathname + location.search + location.hash,
    });
  }

  function isCommentable(node) {
    return node?.nodeType === 1 && !UNCOMMENTABLE.has(node.tagName);
  }

  function isTyping(event) {
    const target = event.composedPath()[0];
    if (!target || target.nodeType !== 1) return false;
    return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
  }

  function excerptFor(element) {
    const tag = element.tagName.toLowerCase();
    if (tag === "img") return element.getAttribute("alt") || "";
    if (tag === "input" || tag === "textarea") {
      return element.getAttribute("placeholder") || element.getAttribute("value") || "";
    }
    return (element.innerText || element.textContent || "").replace(/\s+/g, " ").trim().slice(0, MAX_EXCERPT);
  }
})();
