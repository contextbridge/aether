(() => {
  "use strict";

  const HOVER = "data-aether-hover";
  const COMMENTED = "data-aether-commented";
  const MAX_EXCERPT = 120;
  const token = document.currentScript.dataset.token;
  const style = document.getElementById("aether-review-style");
  let annotations = [];
  let sequence = 0;
  let hovered = null;
  let finished = false;

  const host = document.createElement("div");
  const shadow = host.attachShadow({ mode: "open" });
  const list = el("ol", { className: "aether-list" });
  const empty = el("p", {
    className: "aether-empty",
    textContent: "Click any element to comment on it. Alt+click uses the page instead.",
  });
  const count = el("span", { className: "aether-count", textContent: "No comments" });
  const summaryBox = el("textarea", { className: "aether-input aether-summary", placeholder: "Summary (optional)" });
  const submitButton = el("button", { type: "button", className: "aether-btn aether-submit", textContent: "Approve" });
  const cancelButton = el("button", { type: "button", className: "aether-btn aether-cancel", textContent: "Cancel" });
  const status = el("div", { className: "aether-status" });
  shadow.append(
    style.cloneNode(true),
    el(
      "div",
      { className: "aether-panel" },
      el("div", { className: "aether-head" }, el("span", { className: "aether-title", textContent: "Comments" }), count),
      summaryBox,
      empty,
      list,
      el("div", { className: "aether-foot" }, submitButton, cancelButton),
    ),
    status,
  );
  document.body.append(host);

  document.addEventListener("mouseover", onHover, true);
  document.addEventListener("mouseout", onOut, true);
  document.addEventListener("click", onClick, true);
  submitButton.addEventListener("click", submit);
  cancelButton.addEventListener("click", cancel);

  function onHover(event) {
    const target = event.target;
    if (finished || !isCommentable(target)) return clearHover();
    if (hovered !== target) {
      clearHover();
      hovered = target;
      target.setAttribute(HOVER, "");
    }
  }

  function onOut(event) {
    if (event.target === hovered && !event.relatedTarget) clearHover();
  }

  function clearHover() {
    if (hovered) hovered.removeAttribute(HOVER);
    hovered = null;
  }

  // Alt+click reaches the page itself, so a live app stays usable mid-review.
  function onClick(event) {
    const target = event.target;
    if (finished || event.altKey || !isCommentable(target)) return;
    event.preventDefault();
    event.stopPropagation();
    const existing = annotations.find((annotation) => annotation.node === target);
    if (existing) return reveal(existing.id);
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
      : "No comments";
    submitButton.textContent = annotations.length ? "Submit review" : "Approve";
  }

  function card(annotation, index) {
    const comment = el("textarea", {
      className: "aether-input aether-comment",
      placeholder: "Leave a comment…",
      value: annotation.comment,
    });
    comment.setAttribute("aria-label", `Comment ${index + 1} on ${annotation.tag}`);
    comment.addEventListener("input", () => {
      annotation.comment = comment.value;
    });
    const remove = el("button", { type: "button", className: "aether-btn aether-delete", textContent: "Delete" });
    remove.addEventListener("click", () => {
      annotation.node.removeAttribute(COMMENTED);
      annotations = annotations.filter((candidate) => candidate !== annotation);
      render();
    });
    const item = el(
      "li",
      { className: "aether-card" },
      el("span", { className: "aether-anchor", textContent: `${index + 1}. ${annotation.tag} ${annotation.excerpt}` }),
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
      return finish({ status: "approved" }, "Approved. You can close this tab.");
    }
    const body = {
      status: "feedback",
      feedback: summary,
      annotations: annotations.map(({ element, excerpt, comment }) => ({ element, excerpt, comment })),
    };
    finish(body, "Review submitted. You can close this tab.");
  }

  function cancel() {
    finish({ status: "cancelled" }, "Review cancelled. You can close this tab.");
  }

  function finish(body, message) {
    if (finished) return;
    finished = true;
    clearHover();
    status.textContent = message;
    status.style.display = "flex";
    fetch(`/submit?token=${encodeURIComponent(token)}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }).catch(() => {
      status.textContent = "Could not reach the review server. Return to the terminal.";
    });
  }

  function isCommentable(node) {
    return Boolean(node) && node.nodeType === 1 && node !== host && node.tagName !== "HTML" && node.tagName !== "BODY";
  }

  function excerptFor(element) {
    const tag = element.tagName.toLowerCase();
    if (tag === "img") return element.getAttribute("alt") || "";
    if (tag === "input" || tag === "textarea") {
      return element.getAttribute("placeholder") || element.getAttribute("value") || "";
    }
    return (element.innerText || element.textContent || "").replace(/\s+/g, " ").trim().slice(0, MAX_EXCERPT);
  }

  function el(tag, props, ...children) {
    const node = document.createElement(tag);
    Object.assign(node, props);
    node.append(...children);
    return node;
  }
})();
