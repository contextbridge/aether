const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const assets = path.join(__dirname, "../src/review/html_review/assets");
const origin = "http://127.0.0.1:1234";

class Element {
  constructor(tagName = "DIV") {
    this.tagName = tagName;
    this.nodeType = 1;
    this.dataset = {};
    this.attributes = new Map();
    this.listeners = new Map();
    this.style = {};
    this.clientHeight = 0;
    this.offsetHeight = 0;
    this.scrollHeight = 0;
  }
  addEventListener(type, listener) {
    this.listeners.set(type, listener);
  }
  emit(type, event = {}) {
    this.listeners.get(type)?.(event);
  }
  setAttribute(name, value) {
    this.attributes.set(name, value);
  }
  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }
  hasAttribute(name) {
    return this.attributes.has(name);
  }
  removeAttribute(name) {
    this.attributes.delete(name);
  }
  toggleAttribute(name, on) {
    if (on) this.setAttribute(name, "");
    else this.removeAttribute(name);
  }
  cloneNode() {
    return { outerHTML: `<${this.tagName.toLowerCase()}>` };
  }
  focus() {
    this.focused = true;
  }
}

function shell() {
  const elements = Object.fromEntries(
    [
      ".aether-list",
      ".aether-empty",
      ".aether-notice",
      ".aether-count",
      ".aether-arm",
      ".aether-summary",
      ".aether-submit",
      ".aether-cancel",
      ".aether-status",
    ].map((key) => [key, new Element()]),
  );
  const list = elements[".aether-list"];
  list.replaceChildren = (...children) => {
    list.children = children;
  };
  list.querySelector = (selector) =>
    list.children
      .find((item) => selector.includes(`[data-card="${item.dataset.card}"]`))
      ?.querySelector(".aether-comment");
  list.querySelectorAll = () =>
    list.children.map((item) => item.querySelector(".aether-comment"));
  const app = new Element("IFRAME");
  app.contentWindow = {
    location: { pathname: "/", search: "", hash: "" },
    postMessage(message) {
      messages.push(message);
    },
  };
  const messages = [];
  const panel = { querySelector: (selector) => elements[selector] };
  const document = {
    currentScript: { dataset: { token: "token" } },
    body: { append() {} },
    querySelector: () => app,
    getElementById(id) {
      if (id === "aether-panel-template")
        return { content: { cloneNode: () => panel } };
      return {
        content: {
          firstElementChild: {
            cloneNode: () => {
              const nodes = Object.fromEntries(
                [
                  ".aether-comment",
                  ".aether-badge",
                  ".aether-tag",
                  ".aether-anchor",
                  ".aether-delete",
                ].map((key) => [key, new Element()]),
              );
              return Object.assign(new Element("LI"), {
                querySelector: (selector) => nodes[selector],
              });
            },
          },
        },
      };
    },
  };
  const window = new Element();
  vm.runInNewContext(fs.readFileSync(path.join(assets, "shell.js"), "utf8"), {
    document,
    window,
    location: { origin },
    fetch() {},
  });
  const send = (type, key, extra = {}) =>
    window.emit("message", {
      origin,
      source: app.contentWindow,
      data: { aether: "picker", type, key, ...extra },
    });
  return { app, elements, list, messages, send };
}

function picker() {
  const messages = [];
  const elements = [new Element("BUTTON"), new Element("A")];
  const root = new Element("HTML");
  const document = new Element();
  document.documentElement = root;
  document.querySelector = (selector) =>
    elements.find(
      (element) =>
        selector.includes(
          `="${element.getAttribute("data-aether-commented")}"]`,
        ) && element.hasAttribute("data-aether-commented"),
    );
  const window = new Element();
  window.top = { postMessage: (message) => messages.push(message) };
  vm.runInNewContext(fs.readFileSync(path.join(assets, "picker.js"), "utf8"), {
    document,
    window,
    location: { origin, pathname: "/", search: "", hash: "" },
    performance,
  });
  const state = (armed) =>
    window.emit("message", {
      origin,
      data: { aether: "shell", type: "state", armed },
    });
  const click = (element) => {
    let intercepted = false;
    document.emit("click", {
      target: element,
      altKey: false,
      preventDefault() {
        intercepted = true;
      },
      stopPropagation() {},
    });
    return intercepted;
  };
  return { elements, messages, click, state };
}

test("shell synchronizes picker state on iframe load even if hello was missed", () => {
  const review = shell();
  review.app.emit("load");
  assert.deepEqual(
    { ...review.messages.at(-1) },
    { aether: "shell", type: "state", armed: true },
  );
});

test("clicking an existing pick focuses its comment without activating the app", () => {
  const review = shell();
  const page = picker();
  page.state(true);
  assert.equal(page.click(page.elements[0]), true);
  review.send("pick", page.messages.at(-1).key, page.messages.at(-1));
  const comment = review.list.children[0].querySelector(".aether-comment");
  assert.equal(page.click(page.elements[0]), true);
  const reveal = page.messages.at(-1);
  review.send(reveal.type, reveal.key);
  assert.equal(comment.focused, true);
  assert.equal(review.list.children.length, 1);
});

test("a new pick replaces the empty draft but preserves written comments", () => {
  const review = shell();
  review.send("pick", "a1", {
    tag: "BUTTON",
    element: "<button>",
    excerpt: "First",
    path: "/",
  });
  review.send("pick", "a2", {
    tag: "A",
    element: "<a>",
    excerpt: "Second",
    path: "/",
  });
  assert.equal(review.list.children.length, 1);
  assert.deepEqual(
    { ...review.messages.at(-1) },
    { aether: "shell", type: "unpick", key: "a1" },
  );

  const comment = review.list.children[0].querySelector(".aether-comment");
  comment.value = "Keep this";
  comment.emit("input");
  review.send("pick", "a3", {
    tag: "BUTTON",
    element: "<button>",
    excerpt: "Third",
    path: "/",
  });
  assert.equal(review.list.children.length, 2);
  assert.equal(
    review.list.children[0].querySelector(".aether-comment").value,
    "Keep this",
  );
});
