# Browser Events in the Timeline

**Status:** proposed  
**Date:** 2026-08-26  
**Deciders:** demostage maintainers

## Context

A browser pane can be revealed (`focus`), scrolled (`scroll`), and photographed one frame at a time (`src/export/browser.rs:365-425`, the per-frame capture introduced in `c816622`). There is no way to express what a viewer would *do* in it — click a link, hover a control, type into a field, submit a form. For a demo whose subject is a web UI, that is the whole demo.

The per-frame capture changes the premise: the headless browser is stepped through the demo's own clock, with an absolute scroll position set per frame. An event dispatched at a chosen frame index is therefore a small, local change to `capture_web_pane` — not a redesign of the capture loop.

This document decides the shape of that change so a later implementation spec can be written without reopening the questions.

## Forces

- **The score is portable and declarative.** Anything that only works on the machine where the score was written is not an option (`src/model/demo.rs:1-51` — the `Score` struct is serialized to TOML and shared).
- **Selectors are promises about pages we do not control.** A CSS selector that matches today may not match after the page's next deploy. Every targeting mechanism has a cost; the decision is which cost to pay.
- **Determinism is non-negotiable.** Two exports of the same score must produce the same video. Animations, network timing, focus rings, and text carets all threaten this.
- **Silent failure is worse than loud failure.** A video that omits a click is worse than an export that errors. The author must know when their score cannot be fulfilled.
- **The existing step surface is small and regular.** `Step` is a tagged enum with 12 variants (`src/model/demo.rs:225-285`); new variants should match its shape.

---

## 1. The Surface

**Decision:** `click`, `hover`, and `type_into` become timeline steps alongside `scroll`. They share its shape: an action tag, an optional `pane` (defaulting to the focused browser pane), and action-specific fields.

### Step definitions

```rust
// src/model/demo.rs — new Step variants alongside Scroll (line 274)

/// Click an element in a browser pane.
Click {
    /// How to find the element (see §2).
    target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane: Option<String>,
},

/// Hover an element in a browser pane (mouse enter, no click).
Hover {
    target: Target,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane: Option<String>,
},

/// Type text into a focused element in a browser pane.
TypeInto {
    target: Target,
    text: String,
    /// Clear the field before typing (default: true).
    #[serde(default = "default_true")]
    clear: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pane: Option<String>,
},
```

`Target` is a new enum (§2). `clear` defaults to `true` because the common case is "type this into the field," not "append to whatever is already there."

### Realistic example

```toml
# Open a page, click a nav link, type into search, submit.

[[timeline]]
action = "focus"
pane = "web"

[[timeline]]
action = "click"
target = { css = "nav a[href='/docs']" }

[[timeline]]
action = "wait"
duration_ms = 800          # let the navigation paint

[[timeline]]
action = "type_into"
target = { css = "input[name='q']" }
text = "getting started"

[[timeline]]
action = "click"
target = { css = "button[type='submit']" }

[[timeline]]
action = "wait"
duration_ms = 1200         # let results render
```

### Why not a single `interact` step with a `kind` field?

A single `interact` step with `kind = "click" | "hover" | "type_into"` would reduce the variant count but add a field that only some values use (`text` is meaningless for `click`). The existing `Step` enum uses one variant per action (`Type`, `Keypress`, `Scroll`); consistency with that shape matters more than variant count. Rejected: the cost would be a less idiomatic API and validation logic that switches on a nested field instead of the tag.

### Why not subsume these into `scroll` with an `event` field?

`scroll` is conceptually different — it moves the viewport, not the cursor. Merging them would make `scroll` a grab-bag of unrelated behaviors. Rejected: the cost would be a step that is hard to validate (which fields are legal with which `event`?) and hard to extend (adding `drag` later would require more special cases).

---

## 2. Targeting

**Decision:** The primary mechanism is **CSS selector**. A `data-demo-id` attribute is supported as a convention (documented, not enforced). Coordinates and visible text are rejected as primary mechanisms.

### The `Target` type

```rust
// src/model/demo.rs — new type

/// How to find an element in a browser pane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// A CSS selector. The first matching element is used.
    Css(String),
    /// Match by `data-demo-id="..."` attribute. Sugar for `[data-demo-id="..."]`.
    DemoId(String),
}
```

`DemoId` is sugar: `target = { demo_id = "search-box" }` expands to `[data-demo-id="search-box"]`. It exists because authors write scores against pages they maintain, and a `data-demo-id` contract is more stable than a CSS selector that depends on class names chosen by a framework.

### Why CSS selector as primary

- **Ubiquity.** Every browser automation tool uses CSS selectors; authors already know them.
- **Expressiveness.** Selectors can target by attribute, position, state, hierarchy — anything the DOM can express.
- **CDP support.** `headless_chrome` resolves selectors via `DOM.querySelector`, so no custom matching logic is needed.

### Why not visible text as primary

Visible text matching (`text = "Click here"`) is fragile: the same text may appear in multiple elements, whitespace normalization is non-trivial, and localized pages break the score. Rejected: the cost would be ambiguous matches and locale-dependent scores.

### Why not coordinates as primary

Coordinates (`x = 450, y = 120`) are the least portable targeting mechanism: they break on any viewport resize, any layout change, any font metric difference. The score is meant to be portable (`src/model/demo.rs:1-51`); coordinates tie it to a specific render. Rejected: the cost would be scores that only work on the author's machine.

### Why not XPath

XPath is more powerful than CSS selectors but less widely known, and everything a demo needs (find by class, id, attribute, text content via `:has-text()` or a post-filter) is expressible in CSS. Rejected: the cost would be a steeper authoring curve for no additional capability in practice.

### When the selector matches nothing

**The export fails with an error.** The error names the step, the selector, and the pane:

```
timeline[3]: click target 'nav a[href="/docs"]' not found in pane 'web' after 5000ms
```

A warning-and-continue policy would produce a video that silently omits the click — the author would not know their demo is broken until a viewer points it out. Failing fast is the only honest choice.

### When the selector matches several

**The first match (in DOM order) is used.** This matches `document.querySelector` semantics and is what `headless_chrome`'s `find_element` does. No error, no warning — the author wrote a selector that is ambiguous, but the behavior is deterministic. If they want specificity, they refine the selector.

### The `data-demo-id` convention

Documented in `docs/demo-toml.md` as a recommendation for pages the author controls:

```html
<input data-demo-id="search-box" name="q" type="text" />
```

The score then uses `target = { demo_id = "search-box" }`, which is stable across class-name refactors and framework migrations. This is a convention, not an enforcement — the tool does not validate that `data-demo-id` attributes exist in the page.

---

## 3. Failure

**Decision:** An element that never appears **fails the export with an error.** The error includes the selector, the pane, and the timeout that was waited.

### Timeout before failure

Before failing, the exporter waits up to **5000ms** for the element to appear, polling every 100ms. This handles the common case where the page is still rendering (fonts loading, JS hydrating, animations settling). The timeout is not configurable in v1 — if authors need longer waits, they add an explicit `wait` step before the event.

```rust
// src/export/browser.rs — pseudocode for the wait loop

fn wait_for_element(tab: &Tab, selector: &str, timeout: Duration) -> Result<Element> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(el) = tab.find_element(selector)? {
            return Ok(el);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(Error::Export(format!(
        "target '{}' not found after {}ms",
        selector,
        timeout.as_millis()
    )))
}
```

### What the exported video shows

**Nothing.** The export fails before producing a video. A partial video with a missing click is worse than no video — the author cannot tell whether the click happened or was silently skipped. Failing the entire export forces the author to fix the selector or the page.

### Why not warn-and-continue

A warning-and-continue policy would let the export succeed with a video that omits the interaction. The author might not notice until a viewer asks "why didn't the search work?" — by which point the demo has already misled someone. Rejected: the cost would be silent broken demos.

### Why not retry with exponential backoff

A 100ms poll interval for 5000ms is 50 attempts — sufficient for any reasonable page load. Exponential backoff adds complexity for no benefit: the page is either going to render in the first second or it is not going to render at all. Rejected: the cost would be more code for the same outcome.

---

## 4. Time

**Decision:** An event sits at a **frame index** derived from the pane's window. The event is dispatched *before* the screenshot for that frame is taken, so the frame captures the post-event state.

### How this composes with `scroll`

`scroll` already operates per-frame: `capture_web_pane` (`src/export/browser.rs:365-425`) computes absolute scroll offsets for each output frame and applies them before the screenshot. Events follow the same pattern:

1. For each output frame, compute the scroll offset (existing logic).
2. If this frame has an event, dispatch it before the screenshot.
3. Take the screenshot.

The event and the scroll are independent — a frame can have both (scroll to a position, then click something visible at that position).

### Where events are scheduled

Events are assigned to frames by the exporter, not the author. The author writes:

```toml
[[timeline]]
action = "click"
target = { css = "nav a" }
```

The exporter maps this to a frame index based on the pane's `reveal_at` and `hide_at` times, and the order of steps in the timeline. The first event after a `focus` lands on the first frame of the pane's window; subsequent events are spaced by `wait` steps or by a default interval (500ms = ~7 frames at 15fps) if no `wait` separates them.

### What happens when a click causes navigation mid-pane

A click that triggers a navigation (e.g., clicking a link) invalidates the current page. The exporter must:

1. Detect the navigation (via `tab.wait_until_navigated()` or a URL change).
2. Wait for the new page to load (up to the 5000ms timeout).
3. Continue capturing frames of the new page for the remainder of the pane's window.

The scroll position resets to 0 on navigation (the new page has its own scroll height). Subsequent events in the timeline must target elements on the *new* page — the author is responsible for writing selectors that match the post-navigation DOM.

### Why not trigger events by element appearance

An alternative is to fire an event when its target element appears, regardless of frame index. This would make the score resilient to page-load timing but would break determinism: the same score could produce different videos depending on network speed. Rejected: the cost would be non-reproducible exports.

### Why not use wall-clock delays

Wall-clock delays (`wait 2s then click`) are what the current `wait` step does, and they work — but they couple the score to a specific machine speed. A frame-index approach is deterministic: the event always lands on the same frame, regardless of how long the page took to load (within the timeout). Rejected: the cost would be timing-dependent scores that produce different videos on different machines.

---

## 5. Determinism

**Decision:** The design neutralizes the four threats to determinism — animations, network, focus rings, and carets — by (a) disabling animations, (b) waiting for network idle, (c) suppressing focus styles, and (d) hiding the caret.

### Threat 1: Animations

CSS transitions and JS animations cause the page to look different on each render. The exporter injects a stylesheet before any interaction:

```css
*, *::before, *::after {
    animation-duration: 0s !important;
    transition-duration: 0s !important;
}
```

This kills all animations and transitions. The page may look slightly less polished, but it will look the same on every export.

### Threat 2: Network

Network timing affects when resources load and when the page is "ready." The exporter waits for network idle (no requests for 500ms) after navigation, in addition to the 5000ms element timeout. This is already done implicitly by `tab.wait_until_navigated()` in `src/export/browser.rs:332-334`, but should be made explicit for the event path.

### Threat 3: Focus rings

When an element is clicked or focused, browsers draw a focus ring (outline). The ring's appearance depends on the browser, OS, and theme — it is not deterministic across machines. The exporter injects a stylesheet to suppress focus rings:

```css
*:focus {
    outline: none !important;
}
```

This is a deliberate trade-off: the exported video will not show focus indicators, even though a real user would see them. The alternative — rendering focus rings deterministically — is not possible without controlling the browser's theme at a deeper level.

### Threat 4: Text carets

When an input is focused (by `type_into`), the browser draws a blinking caret. The caret's position in its blink cycle is non-deterministic. The exporter hides carets via stylesheet:

```css
caret-color: transparent !important;
```

This removes the caret entirely. The typed text is still visible; only the blinking cursor is suppressed.

### Why not accept non-determinism and document it

Non-determinism would make the score unreliable: two exports of the same score could produce different videos, breaking the promise of reproducibility. The entire point of `demo export` is that it is reproducible — unlike `demo open --view`, which is explicitly non-reproducible (`src/export/browser.rs:438-540`). Rejected: the cost would be the loss of reproducibility, which is the project's core value.

---

## 6. Scope of the First Implementation

**Decision:** The first implementation delivers `click` and `type_into` with CSS selector targeting, fails on missing elements, and suppresses animations/focus/carets. `hover` is deferred.

### What is built

1. **`Step::Click` and `Step::TypeInto`** in `src/model/demo.rs:225-285`, with `Target::Css` only (no `DemoId` sugar yet).
2. **Validation** in `src/validate.rs:66-118`: events require a focused browser pane, and the `target` field must be present.
3. **Dispatch logic** in `src/export/browser.rs:365-425` (`capture_web_pane`): before each screenshot, check if the current frame has an event; if so, dispatch it via CDP.
4. **Element wait loop** (5000ms timeout, 100ms poll) in `src/export/browser.rs`.
5. **Determinism stylesheets** (animation/transition suppression, focus ring removal, caret hiding) injected once after navigation.
6. **TOML documentation** in `docs/demo-toml.md:130-160` for the new steps.

### Acceptance criterion

A score that opens a local HTML page, clicks a button, types into an input, and clicks a submit button produces a video where:

- The button's `:active` state is visible in the frame after the click.
- The input's value is visible in the frame after the `type_into`.
- Two exports of the same score produce byte-identical videos (modulo PNG compression variance, which is already accepted for `scroll`).

### What is deferred

- **`hover`:** No immediate use case in the current demo backlog. When needed, it is a small addition (same `Target` type, dispatches `mouseenter` instead of `click`).
- **`Target::DemoId`:** The `data-demo-id` convention is documented but not enforced in v1. Authors can use `target = { css = "[data-demo-id='search-box']" }` directly.
- **Navigation detection:** v1 assumes the page does not navigate mid-pane. If a click triggers a navigation, the exporter will capture the new page but will not reset scroll or re-validate subsequent selectors — the author must write selectors that work on the post-navigation DOM. Full navigation handling is deferred to v2.
- **Configurable timeout:** The 5000ms element timeout is hardcoded in v1. If authors need longer waits, they can add `wait` steps before the event.

### Why not build `hover` first

`hover` is the simplest event (no click, no typing, no form submission), but it has no immediate use case. Building it without a consumer would be speculative. Rejected: the cost would be untested code paths and no validation that the design works end-to-end.

### Why not build `Target::DemoId` first

`DemoId` is sugar for a CSS selector. Authors can write `target = { css = "[data-demo-id='x']" }` today. The sugar is a convenience, not a capability. Rejected: the cost would be a more complex `Target` enum before the basic path is proven.

---

## Consequences

- **The score gains three new step variants.** `src/model/demo.rs:225-285` grows by ~30 lines (the variants, the `Target` enum, and serde attributes).
- **Validation grows.** `src/validate.rs:66-118` adds checks for event steps: pane must be a browser, `target` must be present.
- **The exporter grows.** `src/export/browser.rs:365-425` adds the event dispatch loop, the element wait, and the determinism stylesheets. ~150 lines.
- **The editor grows.** `src/commands/edit.rs` and `src/commands/edit_reveal.rs` need to handle the new steps in any interactive editing flows. ~50 lines.
- **Documentation grows.** `docs/demo-toml.md:130-160` adds the new steps to the reference table and adds an example.

## Alternatives Rejected

| Alternative | Cost |
|---|---|
| Single `interact` step with `kind` field | Less idiomatic API; validation logic switches on nested field instead of tag |
| Subsume events into `scroll` with `event` field | Hard to validate (which fields legal with which `event`?); hard to extend |
| Visible text as primary targeting | Ambiguous matches; locale-dependent scores |
| Coordinates as primary targeting | Breaks on any viewport resize or layout change; ties score to author's machine |
| XPath as primary targeting | Steeper authoring curve for no additional capability in practice |
| Warn-and-continue on missing element | Silent broken demos; author does not know the click was skipped |
| Retry with exponential backoff | More code for the same outcome (50 attempts at 100ms is sufficient) |
| Trigger events by element appearance | Non-reproducible exports; timing-dependent scores |
| Wall-clock delays for event timing | Timing-dependent scores; different videos on different machines |
| Accept non-determinism and document it | Loss of reproducibility, the project's core value |
| Build `hover` first | Speculative code with no consumer; no end-to-end validation |
| Build `Target::DemoId` first | More complex `Target` enum before the basic path is proven |
