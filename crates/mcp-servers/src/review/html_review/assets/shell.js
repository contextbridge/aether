(() => {
  "use strict";

  const SUBMIT = "/__aether__/submit";
  const script = document.currentScript;
  const token = script.dataset.token;
  const appOrigin = script.dataset.origin;
  const shellOrigin = location.origin;
  const app = document.querySelector(".aether-app");
  const cardTemplate = document.getElementById("aether-card-template");
  const sources = new Set();
  let annotations = [];
  let nextId = 0;
  let armed = !appOrigin;
  let finished = false;

  const skeleton = document.getElementById("aether-panel-template").content.cloneNode(true);
  const pick = (selector) => skeleton.querySelector(selector);
  const list = pick(".aether-list");
  const empty = pick(".aether-empty");
  const notice = pick(".aether-notice");
  const count = pick(".aether-count");
  const armButton = pick(".aether-arm");
  const summaryBox = pick(".aether-summary");
  const submitButton = pick(".aether-submit");
  const cancelButton = pick(".aether-cancel");
  const status = pick(".aether-status");
  document.body.append(skeleton);

  window.addEventListener("message", onMessage);
  app.addEventListener("load", () => {
    checkOutside();
    sources.add(app.contentWindow);
    send(app.contentWindow, { type: "state", armed });
  });
  armButton.addEventListener("click", () => setArmed(!armed));
  summaryBox.addEventListener("input", () => autoGrow(summaryBox));
  submitButton.addEventListener("click", submit);
  cancelButton.addEventListener("click", cancel);

  render();
  setArmed(armed);
  checkOutside();

  function send(source, message) {
    source.postMessage({ aether: "shell", ...message }, shellOrigin);
  }

  function onMessage(event) {
    if (event.origin !== shellOrigin) return;
    const message = event.data;
    if (!message || message.aether !== "picker") return;
    if (message.type === "hello") {
      sources.add(event.source);
      send(event.source, { type: "state", armed });
    } else if (message.type === "pick") {
      addPick(message, event.source);
    } else if (message.type === "reveal") {
      const annotation = annotations.find((item) => item.source === event.source && item.key === message.key);
      if (annotation) reveal(annotation.id);
    } else if (message.type === "toggle") {
      setArmed(Boolean(message.armed));
    }
  }

  function setArmed(next) {
    armed = next;
    armButton.setAttribute("aria-pressed", String(armed));
    armButton.textContent = armed ? "Annotating" : "Annotate";
    armButton.title = armed ? "Stop annotating (C)" : "Comment on elements (C)";
    empty.textContent = armed
      ? "Click any element to comment on it. Press C to stop."
      : "Comment mode is off. Press C to annotate.";
    for (const source of sources) send(source, { type: "state", armed });
  }

  function addPick(message, source) {
    if (!source) return;
    const draft = annotations.find((annotation) => !annotation.comment.trim());
    if (draft) {
      send(draft.source, { type: "unpick", key: draft.key });
      annotations = annotations.filter((annotation) => annotation !== draft);
    }
    nextId += 1;
    annotations.push({
      id: nextId,
      key: message.key,
      source,
      tag: message.tag,
      element: message.element,
      excerpt: message.excerpt,
      path: message.path,
      comment: "",
    });
    render();
    reveal(nextId);
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
    const item = cardTemplate.content.firstElementChild.cloneNode(true);
    const pick = (selector) => item.querySelector(selector);
    const comment = pick(".aether-comment");
    pick(".aether-badge").textContent = String(index + 1);
    pick(".aether-tag").textContent = annotation.tag.toLowerCase();
    pick(".aether-anchor").textContent = annotation.excerpt;
    comment.value = annotation.comment;
    comment.setAttribute("aria-label", `Comment ${index + 1} on ${annotation.tag}`);
    comment.addEventListener("input", () => {
      annotation.comment = comment.value;
      autoGrow(comment);
    });
    pick(".aether-delete").addEventListener("click", () => {
      annotations = annotations.filter((candidate) => candidate !== annotation);
      send(annotation.source, { type: "unpick", key: annotation.key });
      render();
    });
    item.dataset.card = annotation.id;
    item.addEventListener("mouseenter", () => highlight(annotation, true));
    item.addEventListener("mouseleave", () => highlight(annotation, false));
    return item;
  }

  function highlight(annotation, on) {
    send(annotation.source, { type: "highlight", key: annotation.key, on });
  }

  function reveal(id) {
    const comment = list.querySelector(`[data-card="${id}"] .aether-comment`);
    if (comment) comment.focus();
  }

  function submit() {
    const summary = summaryBox.value.trim();
    if (annotations.length === 0 && !summary) return finish({ status: "approved", url: appUrl() }, "Approved.");
    const body = {
      status: "feedback",
      feedback: summary,
      url: appUrl(),
      annotations: annotations.map((annotation) => ({
        element: annotation.element,
        excerpt: annotation.excerpt,
        comment: annotation.comment,
        url: absoluteUrl(annotation.path),
      })),
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
    status.textContent = message;
    status.hidden = false;
    fetch(`${SUBMIT}?token=${encodeURIComponent(token)}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }).catch(() => {
      status.textContent = "Could not reach the review server. Return to the terminal.";
    });
  }

  // A null path means the frame left the review behind: reading a cross-origin location throws.
  function appPath() {
    try {
      const { pathname, search, hash } = app.contentWindow.location;
      return pathname + search + hash;
    } catch {
      return null;
    }
  }

  function absoluteUrl(path) {
    return appOrigin ? appOrigin + path : undefined;
  }

  function appUrl() {
    const path = appPath();
    return path ? absoluteUrl(path) : undefined;
  }

  function checkOutside() {
    notice.hidden = appPath() !== null;
  }

  function autoGrow(textarea) {
    textarea.style.height = "auto";
    const borders = textarea.offsetHeight - textarea.clientHeight;
    textarea.style.height = `${textarea.scrollHeight + borders}px`;
  }
})();
