# paddock site

The landing page for [paddock](https://github.com/plopezlpz/paddock), a tmux popup for jumping between sessions,
windows and panes that also shows which AI coding agents are working, waiting or finished.

It is a static site: one `index.html`, two stylesheets and two small scripts. The demos are real paddock frames,
recorded from the actual program in a scratch tmux server and replayed by a small player. Nothing is built at deploy
time and nothing is deployed from here. The folder is meant to live in the paddock repository as `site/`; the tools
below find the paddock checkout one directory up (see "Where paddock is").

## View it

```sh
open index.html
# or
python3 -m http.server -d . 8765     # then http://localhost:8765/
```

The page makes no external requests (no fonts, scripts, analytics or images from elsewhere) and works from `file://`:
the frames are plain `<script>` files, not fetched, and the page uses no storage.

## Where paddock is

The recorder and the README-image builder need a paddock checkout. They look for it in this folder's parent (the
repository root, when this folder is `site/`). Elsewhere, set `PADDOCK_REPO=<repo>` or pass `--paddock <repo>`. The
recorder checks for `Cargo.toml` there before it builds anything.

## Regenerate the frames

```sh
cd tools
npm install
npm run record               # = node record.mjs all feat --conf demo/themes/gruber.conf
node record.mjs tour --conf demo/themes/gruber.conf      # one recording: tour | phone | status | feat-<name>
```

`all` is the tour, the phone and the status recordings; `feat` is the five feature recordings (preview, rename,
manage, group, notify). This writes `assets/frames/<name>.js`. The shipped recordings are made with the Gruber theme
(`demo/themes/gruber.conf`), so pass it with `--conf`. The scenarios check that its agent colours are set. It needs:

- a paddock checkout (see above) and a Rust toolchain: it builds the release binary there first;
- tmux 3.8 or later (`TMUX_BIN=<path>` picks a particular one);
- Node 20 or later and `npm install`, which pulls Playwright (used by the checks, not by the recorder). The checks need
  its browser: `npx playwright install chromium`.

The recorder runs paddock in its own `tmux -L paddock-demo-*` servers with fake sessions and fake agent states, and never
touches your default tmux server. `--keep` leaves the scratch servers running for inspection (kill them yourself with
`tmux -L <name> kill-server`). The scenarios are in `tools/demo/scenarios/`, the fake pane output in
`tools/demo/transcripts/*.ans`, and the pinned tmux config in `tools/demo/tmux.conf`.

## Rebuild the SVGs

```sh
cd tools
node build-svg.mjs                    # everything
node build-svg.mjs --only posters     # just the site posters
```

This turns the recorded frames into SVGs: the `assets/frames/*-poster.svg` stills the page shows when JavaScript is off,
and the demo SVGs in the paddock repo's `docs/brand/` that its README embeds. The posters build without the paddock
repo; the README images go to `docs/brand/` of the checkout (`--paddock <repo>`). The terminal palette comes from
`assets/css/player.css` and the `.page` block of `assets/css/site.css`.

## Checks

All from `tools/`:

| Command | What it does |
|---|---|
| `npm test` | unit tests for the ANSI converter, the frame format and the SVG builder (`node --test test/*.test.mjs`) |
| `node shoot.mjs` | serves the site (needs `python3`), screenshots it into `tools/.out/shots/` at 1440, 1024, 834, 390 and 320 px wide, with reduced motion and with no JavaScript, and audits it: console errors, requests outside localhost, horizontal overflow, alt text, button names, landmarks, the step-by-step demo, pause, the enlarged terminal on small screens, install tabs and copy buttons over http and `file://` |
| `node contrast.mjs` | WCAG contrast of the site tokens and the terminal palette. Site text is held to 4.5:1 (the red asking marker and the focus ring to 3:1). Terminal text is held to 4.5:1, apart from bright black, which has a floor of 4:1, and dim text, which has a floor of 2:1; anything under the usual target is reported as a note |
| `node readme-preview.mjs` | renders the paddock README through `gh api markdown` and screenshots it in light and dark into `tools/.out/` (needs `gh`, online) |
| `node shoot-svg.mjs` | screenshots the README SVGs from the paddock checkout, chapter by chapter, with reduced motion |
| `node brand/rasterize.mjs --sheet` | brand contact sheet, `tools/.out/brand-sheet@{1,2}x.png` |
| `node brand/rasterize.mjs --png` | regenerates `favicon-32.png` and `apple-touch-icon.png` |

`tools/.out/` is git-ignored.

## Layout

```
index.html              the page
assets/
  brand/                logo, mark, favicons (geometry in tools/brand/NOTES.md)
  css/site.css          page styles; the .page block holds the colour tokens and the terminal theme
  css/player.css        terminal palette defaults and player chrome
  fonts/                JetBrains Mono (latin subset) and its licence
  js/site.js            the scroll-driven demo, install tabs, copy buttons, margin notes
  js/player.js          the frame player
  frames/               recorded frames (status, tour, phone, feat-*.js) and the poster SVGs
tools/                  recorder, SVG builder, checks (Node, ES modules)
  demo/                 tmux config, theme, scenarios, fake pane output
  lib/                  ANSI conversion, frame format, scenario runner, tmux helpers
  brand/                brand rasteriser and notes
  test/                 unit tests
```

## Constraints

- No external requests: no CDNs, web fonts, trackers or frameworks.
- Works from `file://` as well as over http, and without JavaScript (each step shows a still).
- One dark theme, Gruber Darker; the terminal recordings are dark too. Responsive down to 320 px, and it respects
  `prefers-reduced-motion`.
- Copy stays factual: paddock doesn't run agents or infer their state, and the comparison to herdr describes it as
  herdr's own site does.

## Licence

The site's code is MIT, like paddock itself (see the repository's `LICENSE`). JetBrains Mono is licensed under the
SIL Open Font License 1.1: the text is in `assets/fonts/JetBrainsMono-OFL.txt`. The two woff2 files are the latin
subset from `@fontsource/jetbrains-mono` 5.3.0 (weights 400 and 700).
