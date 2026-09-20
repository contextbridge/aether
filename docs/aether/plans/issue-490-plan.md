# Issue #490 — Plan: HTML artifact review with browser annotation overlay

## Overview

### Problem statement

Aether ships a first-party Review MCP (`review_artifact` in `crates/mcp-servers/src/review/`) that only supports Markdown artifacts. The agent sends Markdown (inline `content` or a `file` path); the server emits an MCP **Form elicitation** carrying `ArtifactReviewElicitationMeta { ui: "artifactReview", format: Markdown, markdown }`; wisp renders it in-terminal via `ArtifactReviewScreen` (clankerdiff-ratatui `MarkdownReviewState`) and answers with `decision: approved | feedback` + `feedback` text.

There is no way to review an **HTML artifact** (e.g. a generated landing page, email, report). A terminal Markdown renderer cannot render HTML, and there is no element-level annotation workflow ("click this heading, leave a comment").

### Goal

Extend `review_artifact` via a discriminated union to accept HTML artifacts, and implement a browser-based annotation flow:

1. Agent passes an HTML string (or HTML file path) to the tool, specifying `html` as the type/format.
2. The user's **default browser opens** a page showing the HTML artifact plus an injected annotation overlay (element picker à la devtools, Linear/dark-mode styled, style-isolated from the artifact).
3. The user selects elements, leaves comments, clicks **Submit review**.
4. The agent receives **structured feedback** with a token-efficient, human-readable **anchor** per comment (short injected element id + tag/text excerpt) so it can locate the element.

### Success / acceptance criteria

- `review_artifact` accepts Markdown and HTML via a required discriminated-union discriminator (`format: "markdown" | "html"` alongside the existing `source: {type: file|content}` tag). `format` is required with no default — backwards compatibility with the old format-less input shape is explicitly not preserved (breaking change, accepted per review).
- Passing HTML opens the OS default browser (macOS `open`, Linux `xdg-open`) on a loopback URL; the artifact renders faithfully and the overlay does not leak styles into (or from) the artifact. Windows support is out of scope — on unsupported platforms wisp shows the copy-URL fallback instead of opening a browser.
- The overlay supports: hover highlight, click-to-select element, comment box per selection, comment list/edit/delete, Submit / Cancel.
- On submit, the tool returns structured output containing per-annotation anchors, e.g.:
  ```yaml
  status: feedback
  feedback: |
    ## 2 comments on Landing page
    ### 1. Hero heading ([e12] H1 "Ship faster")
    > Make this larger on mobile.
  annotations:
    - id: a1
      target: "e12"
      tag: H1
      excerpt: "Ship faster"
      comment: "Make this larger on mobile."
  ```
- `approved` (no comments, explicit approve), `cancelled` / `declined` continue to work for HTML.
- Existing Markdown tests (`crates/mcp-servers/tests/integration/review_mcp.rs`, `crates/aether-core/tests/mcp/server_round_trip_tests.rs`, wisp `artifact_review` TUI tests) are updated to pass `format` explicitly; Markdown runtime behavior is unchanged.
- New unit + integration tests cover: input parsing (including missing-`format` rejection), meta round-trip, elicitation schema with annotations, output parsing, annotation target-id format + id-injection, browser-open invocation, HTTP submit → responder answer, cancel path.
- Docs updated (`description.md`, `review_mcp.md` doc comment, website `review.mdx`).

### Non-goals

- General screenshot / visual-diff review.
- Multi-user / remote-hosted review links (loopback only).
- Executing arbitrary artifact `<script>` by default (sandboxed; see trade-offs).
- A full in-terminal HTML renderer in wisp.
- Windows support (macOS/Linux browser-open only; other platforms fall back to copy-URL).

---

## Technical Approach

### Architectural decisions

**1. Keep the server stateless; the browser flow lives in wisp (the client).**

This mirrors the existing Markdown design and is the key structural decision:

- `ReviewMcp::execute_review_artifact` stays a thin translator: validate input → read file/inline content → emit **one Form elicitation** with meta + schema → on the MRTR response round, parse `ElicitResult` into `ReviewArtifactOutput`.
- wisp (`App::on_acp_event` → `artifact_review_meta`) owns all UI. For `format == html` it starts a loopback HTTP server, opens the browser, and answers the pending ACP elicitation when the browser POSTs the review.

Why not have the MCP server open the browser / serve HTTP itself?

- The server runs on the **agent side**. For in-memory servers this is coincidentally the same machine, but for remote/HTTP MCP servers the browser must open where the **human sits** (the TUI host). Client-side handling is correct in both topologies.
- It reuses the entire existing elicitation pipeline with zero protocol changes: MCP Form → `map_mcp_elicitation_request_to_acp` → ACP Form → wisp route → `ElicitationResponder::accept` → `map_acp_elicitation_response_to_mcp` → MRTR `parse_response`. No new elicitation modes, no `ElicitationComplete` notification involvement (that path is OAuth/URL-specific).
- wisp already has the injectable `BrowserOpener` seam (`crates/wisp/src/session/platform.rs`, faked in `crates/wisp/src/testing.rs`), so the "open browser" step is unit-testable. The MCP server has no such seam.

**2. Discriminated-union input design (invariant by construction, no backwards compat).**

Backwards compatibility is explicitly not a concern (per review), so the types make the markdown/html exclusion invariant instead of bolting an optional `format` field onto a struct that can hold both. `format` is the serde tag — required, exactly one variant present:

```rust
#[derive(JsonSchema, Deserialize)]
#[serde(tag = "format", rename_all = "lowercase", deny_unknown_fields)]
pub enum ReviewArtifactInput {
    Markdown { source: ReviewArtifactSource, title: Option<String> },
    Html { source: ReviewArtifactSource, title: Option<String> },
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum ReviewArtifactSource {
    File { path: PathBuf },
    Content { content: String },
}
```

- Wire shape is `{"format": "markdown", "source": {...}}` / `{"format": "html", "source": {...}}`. The old format-less shape (`{"source": {...}}` with no `format`) is rejected with `invalid_params` — existing tests and callers are updated to pass `format` explicitly. No `Default for ArtifactFormat`, no `default_format()` shim.
- Tool JSON-schema (via `schemars`) exposes the `format` tag plus per-variant properties, so the model sees that exactly one of Markdown/Html is required.
- `format` is authoritative: the server does not sniff content to second-guess it (a Markdown-looking HTML fragment and vice versa are both accepted). File reading is unchanged (`read_to_string`, regular-file check) plus a shared size cap (~5 MiB → tool error, not elicitation).
- Alternative considered and rejected: keeping a flat struct with `format: ArtifactFormat` defaulting to Markdown plus two payload strings. That permits `format: markdown` with a non-empty `html` field and needs a runtime `validate()` to recover the invariant the enum gives for free.

**3. Meta design (enum — exactly one payload by construction).**

The meta mirrors the input enum so the "exactly one of markdown/html" invariant holds at the type level instead of as a runtime `validate()` over two co-present strings:

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "lowercase")]
pub struct ArtifactReviewElicitationMeta {
    pub ui: String, // "artifactReview"
    pub path: Option<PathBuf>,
    pub title: String,
    #[serde(flatten)]
    pub body: ArtifactReviewBody,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactReviewBody {
    Markdown { markdown: String },
    Html { html: String },
}
```

Serializes to `{"ui": "artifactReview", "format": "markdown", "markdown": "…"}` and `{"ui": "artifactReview", "format": "html", "html": "…"}`, so the tag doubles as the wisp routing discriminator. Constructors are per-variant (`new_markdown`/`inline_markdown`, `new_html`/`inline_html` — or one constructor per body variant); there is no cross-variant state to validate because the other payload cannot exist. `parse()` returns `None` on deserialization failure or `ui` mismatch (falling through to the generic form modal rather than crashing). `parse()` also rejects empty payloads (`markdown`/`html` must be non-empty) — a format/html emptiness mismatch is inexpressible, only emptiness needs a check.

No backwards-compat shims: old meta without a `format` tag fails to parse (falls through to the generic modal), and the Markdown/HTML screens each accept only their own body variant. `ArtifactReviewBody` accessors (`as_markdown()`, `as_html()`) keep call sites total without matching on strings.

**4. Elicitation schema: add an `annotations` string field for HTML.**

The MCP↔ACP form bridge only supports primitive fields, so structured annotations travel as a **JSON-encoded string**:

- Markdown schema (unchanged): `decision: enum[approved,feedback]` (required), `feedback: string` (optional).
- HTML schema: same two fields **plus** `annotations: string` (optional, JSON array of `HtmlAnnotation` with `target` ids — see §6). wisp fills it with `serde_json::to_string(&annotations)`; the server parses it into `Vec<HtmlAnnotation>` and returns structured output `ReviewArtifactOutput::Feedback { feedback, annotations }` (see §5). Markdown clients never send it; `#[serde(default)]` keeps parsing total.

**5. Output design.**

```rust
pub enum ReviewArtifactOutput {
    Approved,
    Feedback { feedback: String },
    HtmlFeedback { feedback: String, annotations: Vec<HtmlAnnotation> },
    Cancelled,
    Declined,
}
```

with `#[serde(tag = "status", rename_all = "lowercase")]` extended so the wire values are `approved | feedback | htmlFeedback | cancelled | declined`. Hmm — `htmlFeedback` as a separate status is noisy for the model. **Preferred alternative:** keep `Feedback { feedback, annotations: Option<Vec<HtmlAnnotation>> }` with `skip_serializing_if = None`, so Markdown yields today's exact JSON (`{"status":"feedback","feedback":"…"}`) and HTML yields `{"status":"feedback","feedback":"…","annotations":[…]}`. This keeps one `feedback` branch for the agent to handle and keeps Markdown output JSON byte-identical (`{"status":"feedback","feedback":"…"}` — existing assertions only need the new required input `format` added). **Adopt the `Option` variant.**

```rust
pub struct HtmlAnnotation {
    pub id: String,      // "a1", "a2" — comment number assigned by overlay JS
    pub target: String,  // injected element id, e.g. "e12" — resolves via `[data-aether-id="e12"]`
    pub tag: String,     // "H1"
    pub excerpt: String, // text/attribute excerpt ≤ 200 chars, e.g. "Ship faster"
    pub comment: String, // user comment
}
```

The `target` id is assigned by the tool, not the model: before serving, the HTML is post-processed to inject a short unique `data-aether-id="e{n}"` onto every element (see §6), so annotations always anchor on an explicit id and the model just looks the id up in the served HTML.

The server also builds a Markdown `feedback` rendering from the annotations (headings + blockquotes, same voice as the Markdown reviewer's `formatted` output) so models that ignore `annotations` still get a readable summary.

**6. Anchoring choice: tool-injected short element ids (`data-aether-id`) — no selectors, no XPath.**

LLMs reason over text, not selectors: a `body > main:nth-of-type(1) > h1:nth-of-type(2)` selector forces the model to re-derive DOM structure instead of pattern-matching a token. So the tool makes "every element has an id" an invariant by post-processing the HTML before serving:

- **Injection (Rust, at serve time in `server.rs`):** parse the artifact HTML and stamp every element with `data-aether-id="e{n}"` (`e1`, `e2`, … in document order). Ids are sequential, unique by construction, and short/token-efficient (no hashes, no UUIDs). A `data-*` attribute is used rather than `id` so model-authored `id`s (referenced by the artifact's own CSS/`label[for]`/JS) and page CSS selectors are never clobbered or re-targeted. Elements that already carry a `data-aether-id` (e.g. a re-review of tool output) keep a fresh id — the injector overwrites deterministically in document order so ids are stable for identical input. Injection runs on the served bytes only; the agent's original HTML is untouched.
- **Annotation anchor:** the picker reads the clicked element's `data-aether-id` and posts `{ target: "e12", tag, excerpt, rect }` — no selector generation in JS at all. `HtmlAnnotation.target` is that id; the model locates it with a text search for `data-aether-id="e12"` (or `[data-aether-id="e12"]` in `querySelector`). No XPath is ever emitted — verbose, unfamiliar, and strictly worse than the id.
- **Companion fields** `tag` and `excerpt` (trimmed `innerText` slice ≤ 120 chars, or `alt`/`placeholder`/`value` for media/inputs) keep each annotation self-describing in the transcript without re-reading the DOM.
- **Token efficiency:** `e12` is 3 chars vs ~40 for an equivalent unique selector; the per-element `data-aether-id` overhead (~16 chars/element) is paid once in the served page, never in the agent transcript (annotations carry only the referenced ids).
- **Validation:** server-side `HtmlAnnotation::target_is_plausible()` checks `target` matches `^e[0-9]+$` (bounded length); anything else (selectors, XPath, empty) is rejected with `invalid_params`. `excerpt`/`comment` length caps unchanged.

**7. Style isolation: artifact in a sandboxed `<iframe>`, overlay in the parent document.**

Two candidate mechanisms from the issue:

| | Shadow DOM | `<iframe srcdoc>` / `src=/artifact` |
|---|---|---|
| Style isolation | Good one-way (page styles don't enter shadow tree), but artifact styles **can still leak out** via inherited properties, and overlay positioning over slotted content is fiddly | **Full both-directions isolation** — artifact CSS cannot touch the overlay and vice versa |
| Element picking | Straightforward (same document) | Requires mapping `iframe.contentDocument` events + `getBoundingClientRect` offset math |
| Artifact JS | Runs in page context (risk of breaking overlay) | Contained by `sandbox` attribute |
| Full-document artifacts (`<html>/<head>`) | Awkward to graft into a div | Natural (`srcdoc` parses as a full document) |

**Adopt the iframe approach:** the served wrapper page contains (a) the Linear-inspired overlay UI (toolbar + sidebar + pins layer, all in the parent document with its own `<style>`), and (b) `<iframe sandbox="allow-same-origin" src="/artifact?token=…">` filling the viewport. The injected picker script runs **inside the iframe** (served as `/picker.js` same-origin so it can access `contentDocument`), draws hover outlines via a same-document highlight div, and `postMessage`s selections (`data-aether-id` target + tag/excerpt/rect) to the parent, which renders pins. No overlay CSS ever enters the artifact document; no artifact CSS ever enters the parent.

- `sandbox="allow-same-origin"` **without** `allow-scripts` blocks artifact scripts entirely (safest default; static mockups/previews render fine). Document the trade-off; a follow-up can add `allow-scripts` opt-in. Server-side injection (id-stamping + `<script src="/picker.js">`) parses the artifact HTML once at serve time — handle both full documents and fragments.
- Serve the user's HTML from memory (`/artifact`), never written to disk (avoids temp-file cleanup races); the wrapper/picker/sidebar assets are `include_str!`'d at compile time.

**8. Local HTTP server + browser open (wisp side).**

New wisp module `screens/html_review/` (parallel to `screens/artifact_review/`):

- On `ArtifactReviewScreen::new`-equivalent (`HtmlReviewScreen::new(meta, responder, browser_opener)`): bind `127.0.0.1:0` (OS-assigned port), generate a per-review secret token (`uuid`-free — use `rand`/counter or `std::time` nonce; check what workspace already deps on — `rand` is available), build an `axum` router (axum is already a workspace dependency):
  - `GET /?token=…` → wrapper page (overlay shell + iframe).
  - `GET /artifact?token=…` → artifact HTML with picker script tag injected before `</body>` (or appended for fragments).
  - `GET /picker.js` → picker script (no token needed; inert without parent).
  - `POST /submit?token=…` → JSON `{ decision, annotations }` → validate token → answer `ElicitationResponder::accept` with `{decision, feedback, annotations}` → shutdown server, close route.
  - `POST /cancel` equivalently answers cancel.
- Call `browser_opener(&url)` (the injected seam; production = `default_browser_opener`). On opener error, show the URL in a fallback wisp modal with copy-URL affordance (reuse `UrlModal` patterns: Enter retries, `c` copies, Esc cancels) instead of failing silently.
- While the browser review is pending, wisp shows a lightweight waiting route ("HTML review opened in browser — Submit or Cancel there; Esc cancels here") so the TUI is not stuck and double-Ctrl-C / connection-loss still cancels the responder (mirrors `ArtifactReviewScreen::cancel` semantics; `ElicitationResponder::drop` cancels by default).
- Elicitation completion: reuse the existing Form-response mapping — no `CompleteElicitationNotification` needed (that is URL-mode/OAuth-specific). The POST handler translates annotations → `feedback` Markdown + `annotations` JSON string → `responder.accept(...)`.
- Security: loopback bind only, unguessable token per review, `POST` validates token, artifact size cap (~5 MiB, return a TUI error otherwise), server shuts down on submit/cancel/route-exit. Non-localhost `Host` headers rejected.

**9. What happens for non-wisp clients?** Any MCP client with Form-elicitation support still gets a usable (if degraded) experience: it receives the HTML bytes in meta plus the same `decision/feedback/annotations` schema and can render/collect them however it likes. The browser overlay is a wisp enhancement, not a protocol requirement.

### Key trade-offs

- **Iframe + postMessage vs Shadow DOM**: iframe chosen for true bidirectional isolation and natural full-document support, at the cost of rect-mapping code (~100 lines JS). Shadow DOM would be less code but leaks inherited styles and runs artifact JS in-page.
- **Sandbox blocks artifact scripts by default**: safest (artifact JS can't hijack the picker or exfiltrate the token), but interactive prototypes won't be interactive during review. Call it out in tool description; add opt-in later.
- **Annotations as JSON-in-a-string**: required by the Form-primitive bridge; slightly awkward but consistent with how `feedback` already travels, and the server rehydrates it into first-class structured output for the model.
- **Serving from memory vs temp file**: memory avoids cleanup/`file://` CORS-sandbox issues and keeps `review --content` file-free (a property the Markdown tests explicitly assert).

---

## Implementation Steps

Each step is atomic, lands with tests, and keeps `just check / lint / fmt / test` green.

### Step 1 — Shared types: `ArtifactFormat::Html`, `HtmlAnnotation`, enum meta, id injector

**File:** `crates/utils/src/artifact_review.rs` (modify)

- Add `Html` variant to `ArtifactFormat` (`#[serde(rename_all="lowercase")]` gives `"html"`). `ArtifactFormat` remains for the meta tag (which flattens the body enum); no `Default` impl — `format` is required everywhere.
- Add `pub struct HtmlAnnotation { id, target, tag, excerpt, comment }` (Serialize/Deserialize/JsonSchema, `deny_unknown_fields`), where `target` is the injected `data-aether-id` (e.g. `"e12"`).
- Replace the flat `ArtifactReviewElicitationMeta { format, markdown }` with the tagged `ArtifactReviewElicitationMeta { ui, path, title, #[serde(flatten)] body: ArtifactReviewBody }` + `ArtifactReviewBody::Markdown { markdown } / Html { html }` per §3. Per-variant constructors (`new_markdown`/`inline_markdown`, `new_html`/`inline_html`); `parse()` returns `None` on deserialization failure, `ui` mismatch, or empty payload.
- Add `fn inject_element_ids(html) -> String` (pure, unit-testable): parse the artifact HTML, stamp every element with sequential `data-aether-id="e{n}"` in document order (overwriting any pre-existing value deterministically), serialize back. Fragments are wrapped in a minimal document first. Needs an HTML parser dependency (e.g. `tl`, `lol-html`, or `html5ever` — pick whatever is already in the workspace lockfile; prefer a streaming rewriter if available so large artifacts stay cheap).
- Add `fn render_html_feedback(title, annotations) -> String` producing the Markdown summary (same voice as clankerdiff's `formatted`: `## {title}`, `### {n}. {excerpt} ([{target}] {tag})`, `> comment`).
- Add `HtmlAnnotation::target_is_plausible()` validator (`^e[0-9]+$`, bounded length) for server-side defense; rejects selectors/XPath/empty.
- Unit tests (in-file `mod tests`): meta round-trips for both variants; `parse` rejects missing-`format` legacy JSON and empty payloads; id injector golden tests (full doc + fragment + overwrite-determinism + element count); feedback renderer golden test; `submission_fields_match_the_serialized_contract`-style test for the new `annotations` field.

### Step 2 — MCP tool: accept `format`, emit HTML elicitation, parse annotated responses

**Files:**
- `crates/mcp-servers/src/review/tools/review_artifact/mod.rs` (modify)
- `crates/mcp-servers/src/review/tools/mod.rs` (re-export `HtmlAnnotation` if needed)
- `crates/mcp-servers/src/review/tools/review_artifact/description.md` (modify)

Details:

- `ReviewArtifactInput` becomes the required-tag enum per §2 (`Markdown { source, title }` / `Html { source, title }`, `#[serde(tag = "format", deny_unknown_fields)]`). No default: requests without `format` fail deserialization → `invalid_params`. Update all existing Markdown tests/callers to include `"format": "markdown"`.
- `ReviewArtifactSource` unchanged (`File{path}` / `Content{content}`).
- `execute_review_artifact`: match on the input enum. Read payload as **bytes→string** (rename `read_file` binding from `markdown` to `content`); build the matching meta variant (`new_markdown`/`inline_markdown` vs `new_html`/`inline_html`). Enforce a size cap (e.g. 5 MiB string length → tool error, not elicitation). Message `"Review {subject} and approve or submit feedback."` is unchanged — fine for both.
- `build_elicitation_form`: always `decision` enum + optional `feedback`; for the `Html` variant, add `.optional_string("annotations")` with title/description ("JSON array of {id,target,tag,excerpt,comment} where target is the data-aether-id, e.g. e12").
- `ReviewArtifactOutput`: `Feedback { feedback: String, #[serde(default, skip_serializing_if="Option::is_none")] annotations: Option<Vec<HtmlAnnotation>> }`. `TryFrom<ElicitResult>`: on accept, deserialize to an internal `ReviewForm { decision, feedback: String, annotations: Option<String> }`; if `annotations` present, `serde_json::from_str::<Vec<HtmlAnnotation>>`, validate each (`target_is_plausible`, caps: ≤ 200 annotations, comment ≤ 5k chars), map errors to `McpError::invalid_params`. `Approved` with non-empty feedback still rejected (existing rule).
- Pseudo-code for the new parse branch:
  ```rust
  let form: HtmlCapableForm = serde_json::from_value(content)?;
  match form.decision {
      Approved if form.feedback.is_empty() && form.annotations.is_none() => Output::Approved,
      Feedback => {
          let annotations = form.annotations.map(parse_annotations).transpose()?;
          Output::Feedback { feedback: form.feedback, annotations }
      }
      _ => invalid_params(...),
  }
  ```
- Update `description.md`: document required `format`, HTML usage JSON (`{"format":"html","source":{"type":"content","content":"<main>…"}}`), anchor contract (`target` is the injected `data-aether-id`; look it up via text search for `data-aether-id="e12"`), return shape (`annotations` array), sandbox note (scripts disabled during review).
- Update `crates/mcp-servers/src/docs/review_mcp.md` (the `#[doc]` source for `ReviewMcp`): one paragraph on HTML.

### Step 3 — Overlay web assets: wrapper page, picker script, sidebar UI

**New files** (all static, `include_str!`'d — no build step, no npm):
- `crates/wisp/src/screens/html_review/assets/wrapper.html` — shell: toolbar (title, "Pick element" toggle, count, Submit, Cancel), `<iframe id="artifact" sandbox="allow-same-origin" src="/artifact?token=…">`, `<div id="pins">` overlay layer, `<aside id="sidebar">` comment list. Tokens injected server-side via placeholder replacement (`{{REVIEW_TITLE}}`, `{{TOKEN}}`).
- `crates/wisp/src/screens/html_review/assets/picker.js` — runs **inside the iframe document**: hover outline (single reused `div[data-aether-highlight]`), click capture (`preventDefault/stopPropagation`, skip our own highlight node), reads the clicked element's `data-aether-id` (injected per §6 — no selector generation), `postMessage({type:"aether-select", target, tag, excerpt, rect})` to parent. Also handles `aether-highlight-annotation` messages from parent to outline already-commented elements by id.
- `crates/wisp/src/screens/html_review/assets/overlay.js` — runs in parent: listens for `message` events, positions pins from iframe offset + rect, manages sidebar CRUD, `POST /submit` with `{decision, annotations}` + renders the Markdown `feedback` client-side? No — server (Rust) renders feedback from annotations; the page just sends annotations + decision. Keep rendering in Rust (`render_html_feedback`) so headless/test clients share it.
- `crates/wisp/src/screens/html_review/assets/overlay.css` — Linear/dark-inspired: `#0A0A0F` sidebar, `#5E6AD2` accent, Inter/system font stack, 12px radius cards. **Only styles the parent document** — never injected into the artifact.

Design constraints to encode: all overlay element IDs/classes prefixed `aether-`; picker highlight uses inline styles on one node (no global CSS in artifact); `postMessage` origin-checked against `location.origin`.

### Step 4 — wisp: `HtmlReviewScreen` + loopback server + browser open

**New files:**
- `crates/wisp/src/screens/html_review/mod.rs` — `pub use screen::HtmlReviewScreen;`
- `crates/wisp/src/screens/html_review/screen.rs` — `HtmlReviewScreen { title, html: String, responder: ElicitationResponder, browser_opener: BrowserOpener, server_handle, state: Waiting|Submitted|Failed(String), opened_url: Option<String> }`.
  - `HtmlReviewScreen::new(meta, responder, browser_opener) -> Result<Self, String>`: accept only the `Html` body variant (reject `Markdown` — caller routes it to the Markdown screen); validate non-empty html + size cap; run `inject_element_ids` once and serve the stamped bytes; spawn server (see below) on a background `tokio` task; call `browser_opener(&url)`; on error store `launch_error` and stay on a fallback view (show URL + `c` copies via `clipboard_writer`? — screen needs the writer too; thread it like `ElicitationModal` does). The copy-URL fallback is also the whole story on platforms without a browser opener (Windows and other non-macOS/Linux targets return `Err` from `open_url`): no Windows-specific open path is implemented.
  - `on_ui_event`: `Esc` → cancel (POST-equivalent: answer cancel, kill server, emit `ArtifactReviewOutput::Outcome(Cancelled)`); `Enter`/`o` → re-open browser; `c` → copy URL.
  - `render`: waiting view (title, "Opened in browser" + URL, hints, error line) — deliberately simple; the rich UI is in the browser.
  - `cancel()`, `is_done()`, server shutdown on `Drop`.
- `crates/wisp/src/screens/html_review/server.rs` — axum router (axum 0.8 workspace dep; add to wisp `Cargo.toml` + `tokio::net::TcpListener`, `rand`/`uuid` — check workspace: `uuid` exists; use `uuid::Uuid::new_v4`):
  - Shared state: `Arc<ReviewState> { token, title, html, result_tx: Mutex<Option<oneshot::Sender<SubmitPayload>>> }`.
  - `inject_picker(html) -> String`: run `inject_element_ids` (id-stamp every element), then insert `<script src="/picker.js"></script>` before last `</body>` (case-insensitive search) else append; if input is a fragment (no `<html>`), wrap in `<!doctype html><html><head><meta charset=utf-8><meta viewport></head><body>…` first.
  - Routes as in §8; `POST /submit` body cap (axum `DefaultBodyLimit` / explicit 2 MiB JSON cap); token check → `result_tx.send` → `200 {ok:true}`; wrong token → `403`.
  - A `tokio::task` in `screen.rs` awaits `result_rx`, renders feedback via `utils::artifact_review::render_html_feedback`, calls `responder.accept(...)`, marks done.
- **Modify:** `crates/wisp/src/screens/mod.rs` (add `pub mod html_review;`), `crates/wisp/src/app/navigation.rs` (`Route::HtmlReview(Box<HtmlReviewScreen>)`), `crates/wisp/src/app/acp_reducer.rs` (match on `meta.body`: `Markdown` → existing `ArtifactReviewScreen` → `Route::ArtifactReview`, `Html` → `HtmlReviewScreen::new` → `Route::HtmlReview`), `crates/wisp/src/app/mod.rs` (`CommandResult::ReviewThemesListed` arm: ignore for HtmlReview), `crates/wisp/Cargo.toml` (add `axum`, `uuid`/`rand` as needed).
- Reuse `BrowserOpener`/`ClipboardWriter` injection; tests observe opened URLs via the existing `opened_urls` fake in `testing.rs`.

### Step 5 — Response plumbing: annotations string → structured output

Covered mostly in Step 2 (server) + Step 4 (wisp POST handler builds `{decision:"feedback", feedback, annotations: "<json>"}` and calls `responder.accept_strings`-equivalent with three fields — note `accept_strings<const T: usize>` is generic over count, so 3 fields work). Verify end-to-end mapping:

- wisp accept content `{decision, feedback, annotations}` → ACP `ElicitationContentValue::String` → `map_acp_elicitation_response_to_mcp` → JSON → MRTR `InputResponses["review"]` → `ReviewArtifactOutput::try_from` → structured `CallToolResponse` → `tool_bridge` YAML → agent sees `status: feedback` + `annotations:` list.

No changes needed in `acp-utils` elicitation mapping or `aether-cli` actor (generic string pass-through). Confirm with a round-trip test through `McpTestBuilder` (Step 6).

### Step 6 — Tests

**Unit (fast, no browser):**
- `crates/utils/src/artifact_review.rs`: meta round-trips (md + html), `parse` rejects legacy format-less JSON and empty payloads, id-injector cases (full doc / fragment / overwrite determinism), `render_html_feedback` golden, annotation `target` validator cases.
- `crates/mcp-servers`: new `#[cfg(test)]` or integration cases in `tests/integration/review_mcp.rs` + builder extension (input gains required `format`):
  - Missing `format` is rejected (`invalid_params`); HTML content request emits tagged meta with `format: html` + `html` payload, schema containing `annotations`.
  - HTML feedback round-trip: `{"decision":"feedback","feedback":"…","annotations":"[{\"id\":\"a1\",\"target\":\"e12\",…}]"}` → `{"status":"feedback","feedback":"…","annotations":[…]}`.
  - Malformed annotations JSON → `invalid_params` error; selector/XPath-shaped `target` → `invalid_params`; `approved` + annotations → error; Markdown output JSON unchanged (input now requires explicit `"format": "markdown"`).
  - HTML file source resolves against root; missing file → tool error (mirrors Markdown).
- wisp `screens/html_review/server.rs` tests: `inject_picker` full-doc + fragment + id-injection cases; token rejection (403); oversized POST rejected.
- Picker contract: the picker reads `data-aether-id` (no selector generation), so no JS unit tests are needed — cover the Rust-side `inject_element_ids` + `target_is_plausible` instead, plus a `node`-free assertion that `wrapper.html`/`overlay.js`/`picker.js` contain required placeholders (`data-aether-id`, `aether-select`) to guard against template drift. Don't introduce a JS toolchain.

**Integration (wisp TUI harness, `crates/wisp/tests/tui/`):**
- New `html_review.rs` module (register in `main.rs`): drive `App` with a fake ACP HTML elicitation (extend `support.rs` with `html_artifact_elicitation(html, title)` mirroring `url_elicitation`), assert `Route::HtmlReview` opens, `browser_opener` fake captures a `http://127.0.0.1:{port}/?token=…` URL, page `GET /` + `GET /artifact` return 200 with picker injected, `POST /submit` with 2 annotations → responder answers `feedback` + `annotations`, route closes. Cancel via `Esc` → `Cancel` response, server port released.
- `BrowserOpener`-failure case: opener returns `Err` → waiting view shows error + URL, `c` copies (assert via clipboard fake), `Enter` retries (assert second open attempt).

**Round-trip (production executor):**
- Extend `crates/aether-core/tests/mcp/server_round_trip_tests.rs` with `artifact_review_round_trips_html_annotations_through_the_production_executor`: HTML content (with explicit `"format": "html"`) + accepted response carrying annotations JSON → YAML output contains `status: feedback` and both `target` ids.

### Step 7 — Docs + tool copy

- `description.md`: HTML section (usage JSON with required `"format": "html"`, anchor semantics with `data-aether-id` example, `annotations` return shape, sandbox/scripts note, size cap).
- `crates/mcp-servers/src/docs/review_mcp.md` (+ `ReviewMcp` doc comment by inclusion): HTML paragraph.
- `packages/website/src/content/docs/aether/built-in-servers/review.mdx`: HTML flow + example + anchor example.
- `crates/mcp-servers/src/review/README.md`: one line on HTML.

---

## Testing Plan

| Layer | What's covered | Where |
|---|---|---|
| Unit | meta round-trip/type-level invariant; id injector; feedback renderer; annotation `target` validation | `crates/utils/src/artifact_review.rs` `mod tests` |
| Unit | input requires `format` (missing → `invalid_params`); schema contains `annotations` only for html; output parsing incl. `target` error cases | `crates/mcp-servers` review_artifact tests + `tests/integration/review_mcp.rs` |
| Unit | id-stamping picker injection (full doc/fragment), token auth, body limits | `crates/wisp/src/screens/html_review/server.rs` `#[cfg(test)]` |
| Integration (TUI) | route branching, browser-open URL capture, GET/POST flow, Esc-cancel, opener-failure fallback | `crates/wisp/tests/tui/html_review.rs` (+ `support.rs` helper) |
| Round-trip | HTML annotations through the real MRTR executor to YAML | `crates/aether-core/tests/mcp/server_round_trip_tests.rs` |
| Regression | Markdown runtime behavior unchanged; existing tests updated for required `format` | existing suites (updated to pass `"format": "markdown"` explicitly; output JSON unchanged modulo the additive `annotations: Option` field with `skip_serializing_if`) |
| Manual | generate a sample HTML artifact, run `review_artifact` with `format: html`, verify browser opens, overlay styling (Linear/dark), style isolation (artifact with aggressive global CSS `* { color: red !important }` doesn't touch sidebar; sidebar CSS doesn't touch artifact), select/comment/submit, agent transcript readability | dev loop with `wisp` + review MCP |

Edge cases to verify explicitly:

- Empty HTML (`""`) → tool-side `invalid_params`/tool error, never opens a browser.
- Oversize HTML (> 5 MiB inline; large file) → clean tool error.
- HTML file that doesn't exist / is a directory / isn't UTF-8 → same error shape as Markdown today.
- `format: html` + Markdown-looking content (and vice versa) → accepted (no sniffing rejection); `format` is authoritative.
- Annotations `[]` + `decision: feedback` → treated as feedback with empty list (valid; feedback text may still carry the summary) — decide: allow, since user may write a general comment without anchors. `decision: approved` + any annotations → error.
- Duplicate annotation ids / >200 annotations / over-long comments → server rejects with `invalid_params` naming the problem.
- Hostile content (`<script>` in comment, `target` with selector/XPath smuggling) → `target` rejected unless `^e[0-9]+$`; comment HTML-escaped in served page; JSON round-trip preserves it; Markdown renderer escapes/fences it.
- Browser already closed / POST after cancel → second answer is no-op (`ElicitationResponder` takes-once semantics; server returns 200/410 without panicking).
- Port conflicts: OS-assigned port (`bind 0`) — no fixed-port logic, no retries needed.
- Non-wisp MCP clients (e.g. headless tests with `ElicitationScript`): HTML elicitation still answerable with plain `{decision, feedback}` and no `annotations` (server treats missing as `None`).
- Clients without elicitation support → existing `ELICITATION_UNSUPPORTED` error (unchanged; note: URL-mode capability is *not* required since we use Form mode).

---

## Files to Modify/Create

| Path | Change | Kind |
|---|---|---|
| `crates/utils/src/artifact_review.rs` | Add `ArtifactFormat::Html`, `HtmlAnnotation{target}`, enum meta (`ArtifactReviewBody`), `inject_element_ids`, `render_html_feedback`, `target_is_plausible`, unit tests | Modify |
| `crates/mcp-servers/src/review/tools/review_artifact/mod.rs` | `ReviewArtifactInput` required-tag enum (no default), per-variant meta construction, HTML-aware schema (`annotations`), `ReviewArtifactOutput::Feedback` gains `annotations: Option<Vec<HtmlAnnotation>>`, `target` parsing/validation | Modify |
| `crates/mcp-servers/src/review/tools/mod.rs` | Re-export `HtmlAnnotation` | Modify |
| `crates/mcp-servers/src/review/tools/review_artifact/description.md` | Document `format: html`, usage examples, anchor contract, return shape, sandbox note | Modify |
| `crates/mcp-servers/src/docs/review_mcp.md` | HTML paragraph (feeds `ReviewMcp` doc comment) | Modify |
| `crates/mcp-servers/src/review/README.md` | Mention HTML support | Modify |
| `crates/mcp-servers/tests/integration/review_mcp.rs` | Builder `format` support; HTML meta/schema/feedback/malformed-annotations/file-source tests | Modify |
| `crates/aether-core/tests/mcp/server_round_trip_tests.rs` | HTML annotations round-trip through production executor | Modify |
| `crates/wisp/src/screens/html_review/mod.rs` | New module root | **Create** |
| `crates/wisp/src/screens/html_review/screen.rs` | `HtmlReviewScreen`: responder + browser open + waiting UI + cancel/retry/copy | **Create** |
| `crates/wisp/src/screens/html_review/server.rs` | Axum loopback server: routes, token auth, picker injection, submit handling, unit tests | **Create** |
| `crates/wisp/src/screens/html_review/assets/wrapper.html` | Overlay shell (toolbar/sidebar/iframe/pins) | **Create** |
| `crates/wisp/src/screens/html_review/assets/picker.js` | In-iframe hover/select/`data-aether-id` read/postMessage | **Create** |
| `crates/wisp/src/screens/html_review/assets/overlay.js` | Parent-doc pins/sidebar/submit/cancel | **Create** |
| `crates/wisp/src/screens/html_review/assets/overlay.css` | Linear/dark overlay styles (parent doc only) | **Create** |
| `crates/wisp/src/screens/mod.rs` | Register `html_review` | Modify |
| `crates/wisp/src/app/navigation.rs` | `Route::HtmlReview(Box<HtmlReviewScreen>)` | Modify |
| `crates/wisp/src/app/acp_reducer.rs` | Match `meta.body` → `HtmlReviewScreen::new` → `Route::HtmlReview` (Markdown → existing screen); opener/clipboard threading | Modify |
| `crates/wisp/src/app/mod.rs` | `ReviewThemesListed` arm handles `HtmlReview` (ignore) | Modify |
| `crates/wisp/src/app/input.rs` (check; actually `submission.rs`/`input/` dir) | Route `HtmlReview` into existing artifact-review input/output plumbing (`ArtifactReviewOutput`) if routed through the same handlers — confirm call sites of `Route::ArtifactReview` and mirror them | Modify |
| `crates/wisp/Cargo.toml` | Add `axum` (+ `uuid` or `rand` for tokens if not already transitively available) | Modify |
| `crates/wisp/tests/tui/html_review.rs` | TUI integration tests (browser-open capture, HTTP flow, cancel, opener failure) | **Create** |
| `crates/wisp/tests/tui/main.rs` | Register `html_review` module | Modify |
| `crates/wisp/tests/tui/support.rs` | `html_artifact_elicitation(...)` helper | Modify |
| `packages/website/src/content/docs/aether/built-in-servers/review.mdx` | HTML flow docs | Modify |
| `docs/aether/plans/issue-490-plan.md` | This plan | **Create** (this file) |

> Note: `crates/wisp/src/app/input.rs` in the table refers to whichever input-dispatch module matches on `Route::ArtifactReview` — grep `ArtifactReview` under `crates/wisp/src` at implementation time (`submission.rs`, renderer, `close_elicitation_owner` paths) and mirror each arm for `HtmlReview`. The Markdown screen's `surfaces/input.rs::ArtifactReviewOutput` enum is reused as-is (Approved / Outcome(Submitted|Cancelled) / SetTheme-ignored).

### Files explicitly NOT touched

- `crates/acp-utils/src/elicitation.rs`, `crates/aether-cli/src/acp/session/actor.rs`, `crates/mcp-utils/src/{server/mrtr,client/*}` — the generic Form bridge already passes the new fields through; no protocol changes.
- `crates/wisp/src/screens/artifact_review/screen.rs` — Markdown path stays byte-identical.
- `crates/aether-auth/src/browser.rs` — wisp uses its own `BrowserOpener`; no change needed.

---

## Additional Notes

### Documentation updates needed

- Tool description (`description.md`) is model-facing — keep it terse, example-led, and explicit that `annotations[].target` is the injected `data-aether-id` (e.g. `e12`, found via text search) plus a human `excerpt`.
- Website `review.mdx` is human-facing — include the 5-step flow from the issue and a screenshot/GIF follow-up (record during manual verification; not part of this plan's deliverable but recommended).
- Changelog entries follow repo convention (release-plz per-crate `CHANGELOG.md` — auto-generated; do not hand-edit).

### Recommended follow-ups (separate issues, not this plan)

1. **Opt-in artifact scripts**: `format: html` + e.g. `"scripts": true` flag → `sandbox="allow-same-origin allow-scripts"` for interactive prototypes, with a warning in the overlay.
2. **Viewport presets** (mobile/desktop widths) + full-page screenshot attachment to the elicitation for vision-capable models.
3. **Multi-round persistence**: keep the loopback server alive across feedback rounds so the user's pins survive "address feedback and call again" loops (requires stable review-id threading through the tool input).
4. **External-client rendering guidance**: document how non-wisp MCP hosts should render `format: html` meta (the meta already carries everything they need).
5. ~~`uniqueSelector` vitest~~ — dropped: no selector generation exists anymore (picker reads `data-aether-id`); no JS toolchain.

### Risks & mitigations

- **axum version skew**: workspace already pins `axum 0.8.9` — reuse it; the review server needs only routing + JSON + static strings, so API churn risk is minimal.
- **iframe `srcdoc` size limits**: large artifacts via `srcdoc` attribute can hit browser URL/attribute limits → serve artifact at `GET /artifact` (separate same-origin document) instead of inlining into `srcdoc`. This plan already does that.
- **Model confusion between two feedback shapes**: mitigated by keeping a single `status: feedback` envelope and always rendering the Markdown summary even when `annotations` is present.
- **Flaky TUI/HTTP tests**: bind `127.0.0.1:0`, never assert on timing (no timeouts — poll channels / await oneshots), shut the server down in every test path (`Drop` guard).

### Review resolutions (addressed per PR #491 feedback)

1. Anchoring: **tool-injected `data-aether-id` + excerpt** (changed per review — CSS selectors rejected as non-ideal for models; XPath never considered). The model looks ids up by text search; no selector generation anywhere.
2. `format` design: **required serde-tag enum, invariant by construction** (changed per review — no `Default`, no `#[serde(default)]`, no backwards compat; a flat struct with optional fields was explicitly rejected in favor of types that make mixed markdown/html states inexpressible).
3. Scripts in reviewed HTML are **disabled** in v1 (sandbox without `allow-scripts`).
4. Windows support is **out of scope** (changed per review — macOS `open` / Linux `xdg-open` only; unsupported platforms use the copy-URL fallback; no `cmd /C start` path).
5. No persistence of reviews to disk; everything in-memory. The tool remains side-effect-free (consistent with "never creates or modifies an artifact").
