(() => {
  "use strict";

  const HOVER = "data-aether-hover";
  const COMMENTED = "data-aether-commented";
  const ARMED = "data-aether-armed";
  const MAX_EXCERPT = 120;
  const DWELL_MS = 120;
  const SCROLL_GRACE_MS = 150;
  const script = document.currentScript;
  const token = script.dataset.token;
  const style = document.getElementById("aether-review-style");
  const liveApp = script.dataset.mode === "app";
  let annotations = [];
  let sequence = 0;
  let armed = !liveApp;
  let hovered = null;
  let pending = null;
  let dwell = null;
  let scrollUntil = 0;
  let finished = false;

  const host = document.createElement("div");
  const shadow = host.attachShadow({ mode: "open" });
  const list = el("ol", { className: "aether-list" });
  const empty = el("p", { className: "aether-empty" });
  const count = el("span", { className: "aether-count", textContent: "No comments" });
  const armButton = el("button", { type: "button", className: "aether-btn aether-arm" });
  const summaryBox = el("textarea", { className: "aether-input aether-summary", placeholder: "Summary (optional)" });
  const submitButton = el("button", { type: "button", className: "aether-btn aether-submit", textContent: "Approve" });
  const cancelButton = el("button", { type: "button", className: "aether-btn aether-cancel", textContent: "Cancel" });
  const status = el("div", { className: "aether-status", hidden: true });
  shadow.append(
    style.cloneNode(true),
    el(
      "div",
      { className: "aether-panel" },
      el(
        "div",
        { className: "aether-head" },
        el("span", { className: "aether-title", textContent: "Review" }),
        el("div", { className: "aether-head-actions" }, count, armButton),
      ),
      summaryBox,
      empty,
      list,
      el("div", { className: "aether-foot" }, submitButton, cancelButton),
    ),
    status,
  );
  const REVIEW = "data-aether-review";
  document.documentElement.setAttribute(REVIEW, "");
  document.body.append(host);

  document.addEventListener("mouseover", onHover, true);
  document.addEventListener("mouseout", onOut, true);
  document.addEventListener("click", onClick, true);
  document.addEventListener("keydown", onKey, true);
  document.addEventListener("scroll", onScroll, true);
  document.addEventListener("wheel", onScroll, { capture: true, passive: true });
  armButton.addEventListener("click", () => setArmed(!armed));
  summaryBox.addEventListener("input", () => autoGrow(summaryBox));
  submitButton.addEventListener("click", submit);
  cancelButton.addEventListener("click", cancel);

  render();
  setArmed(armed);

  function setArmed(next) {
    armed = next;
    if (armed) document.documentElement.setAttribute(ARMED, "");
    else document.documentElement.removeAttribute(ARMED);
    if (!armed) clearHover();
    armButton.setAttribute("aria-pressed", String(armed));
    armButton.textContent = armed ? "Annotating" : "Annotate";
    armButton.title = armed ? "Stop annotating (C)" : "Comment on elements (C)";
    empty.textContent = armed
      ? "Click any element to comment on it. Press C to stop."
      : "Comment mode is off. Press C to annotate.";
  }

  function onHover(event) {
    const target = event.target;
    if (!canAnnotate() || performance.now() < scrollUntil || !isCommentable(target)) return clearHover();
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

  function onKey(event) {
    if (finished || event.repeat || isTyping(event)) return;
    if (event.key === "Escape") {
      if (armed) setArmed(false);
      return;
    }
    if (event.key.toLowerCase() !== "c" || event.metaKey || event.ctrlKey || event.altKey) return;
    event.preventDefault();
    setArmed(!armed);
  }

  // Alt+click reaches the page itself, so a live app stays usable mid-review.
  function onClick(event) {
    const target = event.target;
    if (!canAnnotate() || event.altKey || !isCommentable(target)) return;
    event.preventDefault();
    event.stopPropagation();
    const existing = annotations.find((annotation) => annotation.node === target);
    if (existing) return reveal(existing.id);
    const draft = annotations.find(isDraft);
    if (draft) removeAnnotation(draft);
    sequence += 1;
    const annotation = {
      id: `a${sequence}`,
      node: target,
      tag: target.tagName,
      element: target.cloneNode(false).outerHTML,
      excerpt: excerptFor(target),
      comment: "",
    };
    target.setAttribute(COMMENTED, annotation.id);
    annotations.push(annotation);
    render();
    reveal(annotation.id);
  }

  function render() {
    list.replaceChildren(...annotations.map(card));
    empty.style.display = annotations.length ? "none" : "block";
    count.textContent = annotations.length
      ? `${annotations.length} comment${annotations.length === 1 ? "" : "s"}`
      : "";
    submitButton.textContent = annotations.length ? "Request Changes" : "Approve";
    growInputs();
  }

  // Growth must wait until the inputs are mounted, or scrollHeight reads as zero.
  function growInputs() {
    autoGrow(summaryBox);
    for (const box of list.querySelectorAll("textarea")) autoGrow(box);
  }

  function card(annotation, index) {
    const comment = el("textarea", {
      className: "aether-input aether-comment",
      placeholder: "Leave a comment…",
      rows: 1,
      value: annotation.comment,
    });
    comment.setAttribute("aria-label", `Comment ${index + 1} on ${annotation.tag}`);
    comment.addEventListener("input", () => {
      annotation.comment = comment.value;
      autoGrow(comment);
    });
    const remove = el("button", { type: "button", className: "aether-btn aether-delete", textContent: "×", title: "Delete comment" });
    remove.addEventListener("click", () => {
      removeAnnotation(annotation);
      render();
    });
    const item = el(
      "li",
      { className: "aether-card" },
      el(
        "div",
        { className: "aether-card-head" },
        el("span", { className: "aether-badge", textContent: String(index + 1) }),
        el("span", { className: "aether-tag", textContent: annotation.tag.toLowerCase() }),
        el("span", { className: "aether-anchor", textContent: annotation.excerpt }),
      ),
      comment,
      el("div", { className: "aether-card-foot" }, remove),
    );
    item.dataset.card = annotation.id;
    item.addEventListener("mouseenter", () => annotation.node.setAttribute(HOVER, ""));
    item.addEventListener("mouseleave", () => annotation.node.removeAttribute(HOVER));
    return item;
  }

  function reveal(id) {
    const comment = list.querySelector(`[data-card="${id}"] .aether-comment`);
    if (comment) comment.focus();
  }

  function submit() {
    const summary = summaryBox.value.trim();
    if (annotations.length === 0 && !summary) {
      return finish({ status: "approved" }, "Approved.");
    }
    const body = {
      status: "feedback",
      feedback: summary,
      annotations: annotations.map(({ element, excerpt, comment }) => ({ element, excerpt, comment })),
    };
    finish(body, "Review submitted.");
  }

  function cancel() {
    finish({ status: "cancelled" }, "Review cancelled.");
  }

  function finish(body, message) {
    if (finished) return;
    finished = true;
    setArmed(false);
    document.documentElement.removeAttribute(REVIEW);
    status.textContent = message;
    status.hidden = false;
    fetch(`/submit?token=${encodeURIComponent(token)}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }).catch(() => {
      status.textContent = "Could not reach the review server. Return to the terminal.";
    });
  }

  function canAnnotate() {
    return !finished && armed;
  }

  function isCommentable(node) {
    return Boolean(node) && node.nodeType === 1 && node !== host && node.tagName !== "HTML" && node.tagName !== "BODY";
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

  // Only one un-commented draft may exist at a time; a fresh pick re-targets it.
  function isDraft(annotation) {
    return annotation.comment.trim() === "";
  }

  function removeAnnotation(annotation) {
    annotation.node.removeAttribute(COMMENTED);
    annotations = annotations.filter((candidate) => candidate !== annotation);
  }

  // scrollHeight excludes borders, so add them back (height is border-box).
  function autoGrow(textarea) {
    textarea.style.height = "auto";
    const borders = textarea.offsetHeight - textarea.clientHeight;
    textarea.style.height = `${textarea.scrollHeight + borders}px`;
  }

  function el(tag, props, ...children) {
    const node = document.createElement(tag);
    Object.assign(node, props);
    node.append(...children);
    return node;
  }
})();
