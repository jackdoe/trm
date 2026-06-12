# trm

A zero-dependency terminal emulator for macOS, written entirely by Claude
(Anthropic's AI) in collaboration with its human operator, who set the
constraints: no third-party code, minimal attack surface, software rendering,
and the smallest codebase that can comfortably run modern TUI applications
like Claude Code, vim, htop, and tmux.

Roughly 3,100 lines of Rust. Zero crates. The only linked libraries are
macOS system frameworks (AppKit, QuartzCore, CoreFoundation, IOSurface) —
no CoreText, no CoreGraphics: font parsing and rasterization are done by
trm's own TrueType engine over an embedded font (3270 Nerd Font). The
binary is ~3 MB, of which 2.6 MB is the font.

## Architecture

The design principle: **everything that processes data is compiler-verified
safe Rust; `unsafe` exists only at the OS rim.**

| Module      | Role                                        | Safety |
|-------------|---------------------------------------------|--------|
| `vt.rs`     | escape-sequence parser, grid, scrollback    | `#[forbid(unsafe_code)]` |
| `render.rs` | cells → pixels, procedural glyphs           | `#[forbid(unsafe_code)]` |
| `input.rs`  | key/mouse → bytes, selection, paste filter  | `#[forbid(unsafe_code)]` |
| `font.rs`   | TrueType parser + scanline rasterizer       | `#[forbid(unsafe_code)]` |
| `main.rs`   | window, run loop, PTY, clipboard            | unsafe rim |
| `ffi.rs`    | hand-written extern declarations            | declarations only |

There are no Objective-C source files and no Swift. AppKit is driven through
raw `objc_msgSend` calls; the terminal view is an `NSView` subclass built at
runtime with `objc_allocateClassPair`.

## Data flow

```
shell writes ─► PTY master ─► CFFileDescriptor on the main run loop
                                  │
                                  ▼
                            vt::Term::feed()          (pure, fuzzed)
                                  │
                     grid mutations + damage flags
                                  ▼
                            render::frame()           (pure)
                                  │
                  pixels into one of two IOSurfaces   (zero-copy)
                                  ▼
                          layer.contents swap
keystrokes ─► input::key() ─► bytes ─► PTY master
```

Single process, single thread. Redraws coalesce on an 8 ms timer and respect
synchronized-output mode (2026). `vt::Term::feed` is the entire hostile-input
surface: it cannot open files or sockets because its module cannot contain
`unsafe` and receives nothing but bytes.

## Rendering

Pure software, one font. 3270 Nerd Font is embedded in the binary and
rendered by trm's own TrueType engine: a bounds-checked `glyf` parser
(simple and composite glyphs, format 4 and 12 cmaps) feeding a scanline
rasterizer that accumulates signed coverage per cell — all in safe Rust.
Glyphs rasterize lazily on first use and are cached, so the full Nerd Font
icon set (powerline, devicons) just works. Box drawing, block elements,
braille, and a few symbols the font lacks (⏺ ⎿) are drawn procedurally —
borders and spinners are pixel-perfect at any size. Wide CJK/emoji render
as double-width placeholder boxes by design: one font, no fallback stack.

## Security posture

- The PTY byte stream is the only untrusted input the process parses, and
  the code that parses it cannot contain `unsafe` (enforced at compile time).
- Paste is sanitized: ESC, C0, and UTF-8-encoded C1 controls are stripped,
  so clipboard contents cannot smuggle escape sequences.
- Programs can *set* the clipboard via OSC 52, but never silently: every
  write flashes the window title (`⧉ clipboard ← N bytes`) for 1.5 s.
  Payloads are strictly-validated base64; oversized OSC (>1.5 MB) is
  dropped whole rather than truncated. The OSC 52 *read* query is not
  implemented — programs can never read the clipboard. Titles are
  control-stripped and capped.
- `DYLD_*` variables are stripped from the child environment.
- No network code, no runtime config parsing, no plugins, no logging.
- Release builds: `panic = "abort"`, `overflow-checks = true`, hardened
  runtime, ad-hoc signed.
- Parser is fuzzed (structure-aware random streams under the test suite).

## Build and run

Requires Xcode Command Line Tools and a Rust toolchain.

```
make            # builds and signs Trm.app
make run        # builds and opens Trm.app
make test       # golden tests, fuzz smoke, benchmarks
```

Benchmarks print with
`cargo test --release bench -- --nocapture`
(~390 MB/s parse, ~2 ms worst-case full frame).

## Usage

- Cmd+C / Cmd+V — copy selection / paste (bracketed-paste aware)
- Cmd+= / Cmd− — font size
- Cmd+[ / Cmd+] — brightness (gamma)
- Cmd+P — phosphor: amber / green / white
- Cmd+Q — quit
- Scroll wheel — scrollback (10,000 lines); any keypress snaps to bottom
- Click/drag selects; double-click word, triple-click line; Shift overrides
  mouse reporting

Configuration is compile-time: the constants at the top of `src/main.rs`
(size, colors, padding, scrollback depth) and the embedded font at
`font/3270NerdFont-Regular.ttf`.

## Deliberate non-features

No tabs (use tmux), no scrollback reflow on resize, no GPU pipeline, no
color emoji or CJK glyphs, no X10 mouse fallback (SGR only), no IME
marked-text display, no config files, no telemetry of any kind.
