//! The stylesheet. One spacing scale (4/8/12/16/24 px), three radii (4 thumbs,
//! 6 rows and controls, 8 panels), three icon sizes (16 nav, 18 controls,
//! 20 primary). Colors come only from Tern's theme variables.

pub const SHEET: &str = r#"
.tt-app, .tt-bar, .tt-mp, .tt-tabs, .sf-overlay[data-role='tt.dialog'], .sf-overlay[data-role='tt.edit'] { font-family: var(--sans, system-ui, sans-serif); font-size: 13px;
  /* A ratio, not Tern's fixed region line height (17 px): larger text would
     overflow its line box and truncated text would clip its descenders. */
  line-height: 1.3; }
.tt-trunc { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.tt-dim { opacity: .62; }
.tt-num { font-variant-numeric: tabular-nums; }

/* ---------- icons and buttons ---------- */
/* Every picture here has its own click (no zoom viewer), but Tern still marks
   blob images `.sf-zoom`: drop its zoom-in cursor, hover ring and press dim so
   the cursor and feedback are the parent's (button, row, card). */
.sf-image.sf-zoom { cursor: inherit; }
.sf-image.sf-zoom .sf-img-frame, .sf-image.sf-zoom:hover .sf-img-frame { box-shadow: none; }
.sf-image.sf-zoom:active .sf-img-frame { opacity: 1; }
[data-role="tt.icon"] { box-shadow: none !important; border-radius: 0 !important; display: block; flex: 0 0 auto; }
[data-role="tt.icon"] svg { display: block; }
.tt-btn { position: relative; display: inline-flex; align-items: center; justify-content: center; flex: 0 0 auto;
  width: 32px; height: 32px; padding: 0; border-radius: 50%; color: inherit; opacity: .72; cursor: pointer;
  transition: opacity .12s, background-color .12s, transform .12s; }
.tt-btn:hover { opacity: 1; }
.tt-btn:active { transform: scale(.94); }
.tt-btn.tt-on { color: var(--accent); opacity: 1; }
/* The dot marks a mode that's on (shuffle, repeat, an open panel); a saved ✓ is a state, not a mode. */
.tt-btn.tt-on:not(.tt-like)::after { content: ""; position: absolute; bottom: 0; left: 50%; width: 4px; height: 4px; margin-left: -2px; border-radius: 50%; background: currentColor; }
.tt-btn.tt-primary { width: 36px; height: 36px; background: var(--t1); color: var(--card); opacity: 1; }
.tt-btn.tt-primary:hover { transform: scale(1.05); }
.tt-btn.tt-big { width: 52px; height: 52px; background: var(--accent-fill); color: #fff; opacity: 1; }
.tt-btn.tt-big:hover { transform: scale(1.04); filter: brightness(1.08); }
.tt-pill { display: inline-flex; align-items: center; justify-content: center; height: 32px; padding: 0 16px; border-radius: 16px;
  font-weight: 600; cursor: pointer; background: var(--accent-fill); color: #fff; }
.tt-pill:hover { filter: brightness(1.08); }
.tt-pill.tt-quiet { background: transparent; color: inherit; box-shadow: inset 0 0 0 1px var(--l2); }

/* ---------- app frame ---------- */
.tt-app { display: grid; grid-template-columns: 240px minmax(0, 1fr); gap: 8px; height: 100%; min-height: 0; padding: 8px 8px 0; box-sizing: border-box; }
.tt-app.tt-setup-only { grid-template-columns: minmax(0, 1fr); padding-bottom: 8px; }
.tt-app.tt-with-queue { grid-template-columns: 240px minmax(0, 1fr) 300px; }
.tt-panel { background: var(--card); border-radius: 8px; min-height: 0; overflow-y: auto; }

/* ---------- sidebar: a navigation card above the library card ---------- */
.tt-sidecol { display: flex; flex-direction: column; gap: 8px; min-height: 0; }
.tt-navcard { flex: 0 0 auto; padding: 8px; display: flex; flex-direction: column; gap: 2px; overflow: visible; }
.tt-side { flex: 1 1 auto; display: flex; flex-direction: column; padding: 8px; gap: 2px; }
.tt-nav { flex: 0 0 auto; display: flex; align-items: center; gap: 16px; height: 40px; padding: 0 12px; border-radius: 6px;
  font-size: 14px; font-weight: 700; opacity: .66; cursor: pointer; transition: opacity .12s, background-color .12s; }
.tt-nav:hover { opacity: 1; }
.tt-nav.tt-cur { opacity: 1; }
.tt-side-head { flex: 0 0 auto; display: flex; align-items: center; justify-content: space-between; height: 40px; padding: 0 12px;
  font-size: 14px; font-weight: 700; opacity: .66; }
.tt-side-empty { padding: 4px 12px; line-height: 1.5; }
.tt-pl { display: flex; align-items: center; gap: 12px; height: 64px; padding: 0 8px; border-radius: 6px; cursor: pointer; flex: 0 0 auto; }
.tt-pl:hover { background: var(--l1); }
.tt-pl.tt-cur { background: var(--l2); }
.tt-pl .tt-art { width: 48px; height: 48px; }
.tt-pl-text { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.tt-pl-name { font-size: 16px; }
.tt-pl-sub { font-size: 12px; }
.tt-pl .tt-pl-sub { font-size: 14px; }
.tt-side-title { display: flex; align-items: center; cursor: pointer; transition: opacity .12s; }
.tt-side-title:hover { opacity: 1; }
.tt-side-ico { flex: 0 0 auto; display: flex; width: 22px; height: 22px; margin-right: 12px; overflow: hidden; }
.tt-side-ico > div { display: flex; }
.tt-side-ico > .tt-ico-hover, .tt-side-title:hover .tt-ico-rest { display: none; }
.tt-side-title:hover .tt-ico-hover { display: flex; }
.tt-side-ico.tt-slide { width: 0; margin-right: 0; opacity: 0; transition: width .2s, margin .2s, opacity .2s; }
.tt-side:hover .tt-side-ico.tt-slide { width: 22px; margin-right: 12px; opacity: 1; }
.tt-collapsed .tt-side-ico { margin-right: 0; }

/* ---------- collapsed sidebar: icons and covers only (names on hover) ---------- */
.tt-app.tt-collapsed { grid-template-columns: 72px minmax(0, 1fr); }
.tt-app.tt-collapsed.tt-with-queue { grid-template-columns: 72px minmax(0, 1fr) 300px; }
.tt-collapsed .tt-nav { justify-content: center; padding: 0; }
.tt-collapsed .tt-nav > span, .tt-collapsed .tt-side-title > span, .tt-collapsed .tt-pl-text, .tt-collapsed .tt-side-empty { display: none; }
.tt-collapsed .tt-side-head { flex-direction: column; justify-content: flex-start; gap: 8px; height: auto; padding: 8px 0; }
.tt-collapsed .tt-side-add { margin: 0; width: 32px; height: 32px; border-radius: 50%; background: var(--l1); }
.tt-collapsed .tt-side-add:hover { background: var(--l2); }
.tt-collapsed .tt-side { padding: 8px 4px; align-items: center; }
.tt-collapsed .tt-pl { justify-content: center; padding: 0; width: 56px; height: 56px; }
.tt-collapsed .tt-pl .tt-art-40 { width: 48px; height: 48px; border-radius: 6px; }

/* ---------- Your Library: kind chips, a search button and the order, then the rows ---------- */
.tt-lib-tools { flex: 0 0 auto; display: flex; flex-direction: column; gap: 4px; padding: 4px 8px; }
.tt-lib-tools .tt-chips { gap: 6px; }
.tt-lib-tools .tt-chip { height: 32px; padding: 0 10px; border-radius: 16px; }
.tt-lib-bar { display: flex; align-items: center; gap: 8px; height: 40px; }
.tt-lib-find { margin-left: -4px; }
/* The search icon opens into the field, which takes the room of the order's label. */
.tt-search.tt-lib-filter { flex: 1 1 auto; min-width: 0; height: 32px; margin: 0; padding: 0 4px 0 12px; border-radius: 6px; }
.tt-lib-filter .sf-input { font-size: 13px; }
/* Tern draws a dim caret in every field, focused or not: show it only in the field being typed in. */
.tt-app .sf-input:not(.focused) .sf-caret, .sf-overlay[data-role^='tt.'] .sf-input:not(.focused) .sf-caret,
.tt-app .sf-editor:not(.focused) .sf-caret, .sf-overlay[data-role^='tt.'] .sf-editor:not(.focused) .sf-caret { opacity: 0; }
.tt-lib-clear { width: 24px; height: 24px; }
.tt-lib-sort { display: inline-flex; align-items: center; gap: 8px; margin-left: auto; height: 32px; padding: 0 4px 0 8px; border-radius: 6px;
  font-size: 14px; color: inherit; opacity: .7; cursor: pointer; transition: opacity .12s; }
.tt-lib-sort:hover { opacity: 1; }
.tt-lib-bar.tt-open .tt-lib-sort > span { display: none; }
.tt-pl.tt-line { height: 36px; gap: 8px; }
.tt-line .tt-pl-name { flex: 0 1 auto; font-size: 14px; }
.tt-line .tt-pl-sub { flex: 1 1 0; min-width: 0; font-size: 12px; }
.tt-lib-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 4px; padding-bottom: 8px; }
.tt-lib-grid.tt-large { grid-template-columns: repeat(2, minmax(0, 1fr)); }
.tt-lib-grid .tt-card { gap: 2px; padding: 6px; border-radius: 6px; }
.tt-lib-grid .tt-card .tt-art { margin-bottom: 4px; box-shadow: none; }
.tt-lib-grid .tt-card-title { font-size: 13px; }
.tt-lib-grid:not(.tt-large) .tt-pl-sub { display: none; }
.tt-side .tt-side-more { align-self: flex-start; margin: 4px 12px; font-size: 12px; }
.tt-collapsed .tt-side-more { display: none; }
/* On the compact layout the card is the page: edge to edge, with a page title. */
.tt-compact .tt-side { border-radius: 0; padding: 16px; }
.tt-compact .tt-side-head { height: auto; padding: 0 0 8px; font-size: 28px; font-weight: 800; letter-spacing: -.01em; opacity: 1; }
.tt-compact .tt-side-title { cursor: default; }
.tt-compact .tt-lib-tools { padding: 4px 0 8px; }
.tt-compact .tt-pl { padding: 0; }

/* ---------- Your Library's sort and view menu (a popup under the tools row) ---------- */
.sf-overlay[data-role='tt.menu'] { font-family: var(--sans, system-ui, sans-serif); font-size: 13px; }
.sf-overlay[data-role='tt.menu'] .sf-ov-card { width: 224px !important; }
.tt-menu { display: flex; flex-direction: column; padding: 4px 0; }
.tt-menu-head { padding: 8px 12px 4px; font-size: 12px; font-weight: 700; opacity: .7; }
.tt-menu-item { display: flex; align-items: center; justify-content: space-between; height: 36px; padding: 0 12px; border-radius: 6px;
  font-size: 14px; color: inherit; text-align: left; cursor: pointer; }
.tt-menu-item:hover { background: var(--l1); }
.tt-menu-item.tt-on { color: var(--ok); }
.tt-menu-rule { height: 1px; margin: 4px 8px; background: var(--l2); }
.tt-menu-views { display: flex; gap: 2px; margin: 4px 8px 8px; padding: 2px; border-radius: 8px; background: var(--l1); }
.tt-view { flex: 1 1 0; display: flex; align-items: center; justify-content: center; height: 36px; border-radius: 6px; color: inherit; opacity: .7; cursor: pointer; }
.tt-view:hover { opacity: 1; }
.tt-view.tt-on { background: var(--l2); color: var(--ok); opacity: 1; }

/* ---------- the devices menu (Spotify Connect) and the "Playing on" strip ---------- */
.sf-overlay[data-role='tt.devices'] { font-family: var(--sans, system-ui, sans-serif); font-size: 13px; }
.sf-overlay[data-role='tt.devices'] .sf-ov-card { width: 300px !important; max-width: calc(100vw - 16px) !important; }
.tt-dev { display: flex; align-items: center; gap: 12px; min-height: 48px; padding: 6px 12px; border-radius: 6px; box-sizing: border-box; }
.tt-menu-item.tt-dev { justify-content: flex-start; width: 100%; height: auto; }
.tt-dev-text { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1 1 auto; }
.tt-dev-name { font-size: 14px; font-weight: 600; }
.tt-dev-sub { font-size: 12px; font-weight: 400; }
.tt-dev-listening { font-size: 11px; font-weight: 700; color: var(--accent); }
.tt-dev-now { color: var(--accent); }
.tt-dev-now .tt-dev-name { color: var(--t1); }
.tt-menu-note { padding: 4px 12px 12px; font-size: 13px; }
.tt-dev-retry { margin: 0 4px 8px; font-weight: 600; }
.tt-btn.tt-cast { color: var(--accent); opacity: 1; }
.tt-playing-on { display: flex; align-items: center; justify-content: center; gap: 6px; height: 22px; margin: 0 8px; padding: 0 12px; border-radius: 6px;
  background: var(--accent-fill); color: #fff; font-family: var(--sans, system-ui, sans-serif); font-size: 12px; line-height: 1.3; font-weight: 600; box-sizing: border-box; }

/* ---------- artwork ---------- */
.tt-art { position: relative; flex: 0 0 auto; overflow: hidden; background: var(--l2); border-radius: 4px; }
.tt-art.tt-loaded { background: none; }
.tt-art > *, .tt-art img { width: 100% !important; height: 100% !important; border-radius: 0 !important; box-shadow: none !important; object-fit: cover; }
/* Tern rounds every raster frame 6 px; the box (.tt-art) owns the corners here,
   so a frame's own rounding shows the box's colour in the corners. */
.tt-art .sf-img-frame, .tt-art .sf-img-svg { border-radius: 0 !important; box-shadow: none !important; }
.tt-art-40 { width: 40px; height: 40px; }
.tt-art-56 { width: 56px; height: 56px; }
.tt-art-180 { width: 180px; height: 180px; border-radius: 8px; box-shadow: 0 8px 24px rgba(0,0,0,.35); }
.tt-art-liked { background: linear-gradient(135deg, #450af5, #c4efd9); color: #fff; }
.tt-art-fallback { display: flex; align-items: center; justify-content: center; }
.tt-art-fallback > [data-role="tt.icon"] { width: 40% !important; height: 40% !important; }
/* The glyph keeps its own px size inside the scaled box; fill the box so it stays centred. */
.tt-art-fallback > [data-role="tt.icon"] .sf-img-svg { width: 100% !important; height: 100% !important; }

/* ---------- content ---------- */
.tt-content { padding: 24px; }
/* A page's header fades from its cover's color (--tint, in the dynamic sheet) into the card. */
.tt-content.tt-tinted { background-image: linear-gradient(180deg, color-mix(in srgb, var(--tint) 32%, var(--card)), var(--card) 380px);
  background-repeat: no-repeat; background-attachment: local; }
.tt-hero { display: flex; align-items: flex-end; gap: 24px; margin-bottom: 24px; }
.tt-hero-text { display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.tt-kicker { font-size: 11px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; opacity: .7; }
.tt-hero-title { font-size: 40px; font-weight: 800; line-height: 1.08; letter-spacing: -.02em; }
.tt-hero-edit-cover { position: relative; width: 180px; height: 180px; flex: 0 0 180px; padding: 0; background: none; border-radius: 4px; cursor: pointer; }
.tt-hero-edit-cover > * { pointer-events: none; }
.tt-hero-edit-overlay { position: absolute; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 12px;
  background: rgba(0,0,0,.65); color: #fff; font-weight: 700; border-radius: 4px; opacity: 0; transition: opacity .15s; }
.tt-hero-edit-cover:hover .tt-hero-edit-overlay { opacity: 1; }
.tt-hero-description { line-height: 1.5; overflow-wrap: anywhere; }
.tt-actions { display: flex; align-items: center; gap: 16px; margin-bottom: 16px; }
.tt-h2 { font-size: 20px; font-weight: 700; margin: 24px 0 12px; }
.tt-h2:first-child { margin-top: 0; }
.tt-note { padding: 12px 16px; border-radius: 6px; background: var(--l1); line-height: 1.5; }
.tt-empty { padding: 48px 0; text-align: center; }

/* ---------- track table ---------- */
.tt-tracks { display: flex; flex-direction: column; }
.tt-tr { display: grid; grid-template-columns: 32px minmax(0, 5fr) minmax(0, 3fr) 56px; gap: 16px; align-items: center;
  height: 56px; padding: 0 16px; border-radius: 6px; flex: 0 0 auto; }
.tt-noalbum .tt-tr { grid-template-columns: 32px minmax(0, 1fr) 56px; }
.tt-tr:not(.tt-th):hover { background: var(--l1); }
.tt-tr.tt-sel { background: var(--l2); }
.tt-th { height: 36px; font-size: 11px; font-weight: 600; letter-spacing: .06em; text-transform: uppercase; opacity: .55;
  border-bottom: 1px solid var(--l1); border-radius: 0; margin-bottom: 8px; }
.tt-idx { text-align: right; opacity: .55; }
.tt-tcell { display: flex; align-items: center; gap: 12px; min-width: 0; }
.tt-tname-wrap { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.tt-tname { font-size: 14px; }
.tt-tsub { font-size: 12px; }
.tt-tdur { text-align: right; opacity: .55; }
.tt-tr.tt-now .tt-tname, .tt-tr.tt-now .tt-idx { color: var(--accent-ink); opacity: 1; }
.tt-explicit { display: inline-flex; align-items: center; justify-content: center; height: 14px; padding: 0 4px; margin-right: 6px;
  border-radius: 2px; font-size: 9px; font-weight: 700; background: currentColor; vertical-align: 1px; }
.tt-explicit > span { color: var(--card); }
.tt-more { margin: 12px 16px; }
/* Release date and © / ℗ lines under an album's songs, aligned with the rows' text. */
.tt-release { display: flex; flex-direction: column; gap: 2px; margin: 24px 16px 8px; font-size: 11px; opacity: .6; }
.tt-release-date { font-size: 13px; margin-bottom: 2px; }

/* ---------- cards (albums, playlists); column count comes from the dynamic sheet ---------- */
.tt-cards { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; }
.tt-card { display: flex; flex-direction: column; gap: 4px; padding: 12px; border-radius: 8px; cursor: pointer; min-width: 0; transition: background-color .12s; }
.tt-card:hover { background: var(--l1); }
.tt-card .tt-art { width: 100%; height: 0; padding-top: 100%; border-radius: 4px; margin-bottom: 8px; box-shadow: 0 4px 16px rgba(0,0,0,.18); }
.tt-card .tt-art.tt-round { border-radius: 50%; }
.tt-card-foot { display: flex; align-items: center; gap: 8px; min-width: 0; }
.tt-card-text { flex: 1 1 auto; display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.tt-card .tt-art > * { position: absolute; inset: 0; }
.tt-card-title { font-size: 14px; font-weight: 600; }

/* ---------- home ---------- */
.tt-greet { font-size: 28px; font-weight: 800; letter-spacing: -.01em; margin-bottom: 16px; }
.tt-quick-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; }
.tt-quick { position: relative; display: flex; align-items: center; gap: 12px; height: 48px; padding-right: 12px; border-radius: 4px;
  overflow: hidden; background: var(--l1); cursor: pointer; min-width: 0; transition: background-color .12s; }
.tt-quick:hover { background: var(--l2); }
.tt-art-48 { width: 48px; height: 48px; border-radius: 0; }
.tt-quick-title { font-size: 13px; font-weight: 700; min-width: 0; }
/* Beside the artwork, never on it (Spotify's design guidelines). */
.tt-tile-play { flex: 0 0 auto; display: flex; align-items: center; justify-content: center; width: 32px; height: 32px; padding: 0;
  border-radius: 50%; background: var(--accent-fill); color: #fff; cursor: pointer; opacity: 0; transition: opacity .15s, filter .15s; }
.tt-tile-play:hover { filter: brightness(1.1); }
.tt-quick:hover .tt-tile-play, .tt-card:hover .tt-tile-play { opacity: 1; }
.tt-quick .tt-tile-play { position: absolute; right: 8px; top: 8px; }
/* On cards the button rises into the footer's right edge; nothing slides or
   reflows, the text just gives way to it while the pointer is over the card. */
.tt-card-foot { position: relative; }
.tt-card .tt-tile-play { position: absolute; right: 0; top: 50%; margin-top: -16px; transform: translateY(6px);
  transition: opacity .2s ease-out, transform .2s ease-out, filter .15s; }
.tt-card:hover .tt-tile-play, .tt-card.tt-now .tt-tile-play { opacity: 1; transform: translateY(0); }
.tt-card:hover .tt-card-text, .tt-card.tt-now .tt-card-text { padding-right: 40px; }
.tt-quick:hover { padding-right: 48px; }
.tt-compact .tt-quick:hover { padding-right: 12px; }
.tt-compact .tt-quick .tt-tile-play { display: none; }
/* Narrow tiles wrap to two lines, as Spotify's phone shortcuts do, instead of
   cutting most names to a few letters. */
.tt-compact .tt-quick { gap: 8px; padding-right: 8px; }
.tt-compact .tt-quick-title { font-size: 12px; white-space: normal; overflow: hidden; display: -webkit-box;
  -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow-wrap: anywhere; }
.tt-shelf { margin-top: 28px; }
.tt-shelf-head { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; }
.tt-shelf-head .tt-h2 { margin: 0 0 4px; }
.tt-shelf-head .tt-link { font-size: 13px; font-weight: 700; color: inherit; opacity: .7; }
.tt-shelf-head .tt-link:hover { opacity: 1; text-decoration: underline; }
.tt-next .tt-qrow { padding: 0; }
/* Cards pad themselves 12 px: pull the row out so the artwork lines up with the heading. */
.tt-shelf > .tt-cards { margin: 0 -12px; }
.tt-card.tt-now .tt-card-title { color: var(--accent-ink); }
.tt-reconnect { margin-top: 28px; }

/* ---------- now-playing view (right-hand panel) ---------- */
.tt-nowview { padding: 16px; display: flex; flex-direction: column; gap: 16px; }
/* A scrolling flex column: items keep their height (by default they shrink,
   clipping the heading's top and the last card's bottom). */
.tt-nowview > *, .tt-queue > *, .tt-settings > *, .tt-eq > *, .tt-side > * { flex: 0 0 auto; }
.tt-nowview-head { font-size: 15px; font-weight: 700; line-height: 1.4; }
.tt-nowview-cover { width: 100%; height: 0; padding-top: 100%; border-radius: 8px; box-shadow: 0 8px 24px rgba(0,0,0,.3); }
.tt-nowview-cover > * { position: absolute; inset: 0; }
.tt-nowview-title { display: flex; align-items: center; gap: 8px; }
.tt-nowview-title .tt-tname-wrap { flex: 1 1 auto; gap: 4px; }
.tt-nowview-name { font-size: 22px; font-weight: 800; letter-spacing: -.01em; line-height: 1.3; }
.tt-nowview-card { background: var(--l1); border-radius: 8px; overflow: hidden; }
.tt-about-link { cursor: pointer; }
.tt-about-link:hover { text-decoration: underline; }
.tt-about-top { position: relative; }
.tt-about-top::before { content: ""; position: absolute; inset: 0 0 auto 0; height: 64px; z-index: 1; pointer-events: none;
  background: linear-gradient(rgba(0,0,0,.55), transparent); }
.tt-about-img { width: 100%; height: 0; padding-top: 66%; border-radius: 0; }
.tt-about-img > * { position: absolute; inset: 0; }
.tt-about-label { position: absolute; top: 0; left: 0; z-index: 2; padding: 14px 16px; color: #fff; }
.tt-about-info { display: flex; flex-direction: column; gap: 12px; padding: 16px; }
.tt-about-nameline { display: flex; align-items: center; gap: 8px; min-width: 0; }
.tt-about-name { font-size: 18px; font-weight: 800; letter-spacing: -.01em; }
.tt-verified { flex: 0 0 auto; display: inline-flex; align-items: center; justify-content: center; width: 16px; height: 16px; border-radius: 50%;
  background: var(--accent-fill); color: #fff; font-size: 10px; font-weight: 800; }
.tt-about-statrow { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.tt-about-stat { display: flex; flex-direction: column; min-width: 0; font-size: 14px; line-height: 1.35; }
.tt-about-bio { font-size: 12px; line-height: 1.5; display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 3; overflow: hidden; }
.tt-about-genres { font-size: 12px; }
.tt-credits { padding: 16px; display: flex; flex-direction: column; gap: 14px; }
.tt-credit { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.tt-credit-text { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.tt-credit-name { font-size: 14px; font-weight: 600; }
.tt-next { padding: 14px 16px; display: flex; flex-direction: column; gap: 10px; }
.tt-nowview-sub { font-size: 15px; font-weight: 700; }

/* ---------- search ---------- */
.tt-search { display: flex; align-items: center; gap: 8px; height: 44px; padding: 0 16px; margin-bottom: 24px; max-width: 480px;
  border-radius: 22px; background: var(--l1); box-shadow: inset 0 0 0 1px var(--l2); transition: box-shadow .12s, background-color .12s; }
.tt-search:hover { background: var(--l2); }
/* The pill is the field: the input inside draws no box of its own. */
.tt-search .sf-input { flex: 1 1 auto; background: transparent; box-shadow: none; border-radius: 0; padding: 0; font-size: 14px; }
.tt-search .sf-input.focused { box-shadow: none; }
.tt-search.tt-focus { box-shadow: inset 0 0 0 2px var(--t1); }

/* ---------- queue panel ---------- */
.tt-queue { padding: 16px; display: flex; flex-direction: column; gap: 4px; }
.tt-queue .tt-h2 { margin: 0; font-size: 16px; }
.tt-qtop { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; margin: 0 0 8px; }
.tt-qtop .tt-link { font-size: 12px; }
.tt-qhead { font-size: 12px; font-weight: 700; margin: 12px 0 4px; opacity: .7; }
.tt-qrow { position: relative; display: flex; align-items: center; gap: 12px; height: 48px; padding: 0 8px; border-radius: 6px; flex: 0 0 auto; }
.tt-qrow:hover { background: var(--l1); }
.tt-qrow.tt-now .tt-tname { color: var(--accent-ink); }
.tt-qrow.tt-auto .tt-tname { font-style: italic; }
.tt-qnote { font-size: 12px; line-height: 1.4; margin: 0 0 4px; }
/* The move and remove buttons slide in over the row's right end on hover. */
.tt-qact { position: absolute; right: 4px; top: 0; bottom: 0; display: flex; align-items: center; gap: 2px; padding-left: 28px;
  background: linear-gradient(90deg, transparent, var(--l1) 28px); opacity: 0; transition: opacity .12s; }
.tt-qrow:hover .tt-qact { opacity: 1; }
.tt-btn.tt-q { width: 26px; height: 26px; }
.tt-btn.tt-flip { transform: rotate(180deg); }
.tt-queue.tt-busy .tt-qrow { opacity: .5; }
.tt-queue.tt-busy .tt-qact { display: none; }

/* ---------- lyrics panel: head and credit stay, the words scroll between ---------- */
.tt-lyrics { display: flex; flex-direction: column; overflow: hidden; padding-top: 16px; }
.tt-ly-head { flex: 0 0 auto; display: flex; align-items: center; gap: 8px; padding: 0 16px 8px; }
.tt-ly-head .tt-h2 { margin: 0; font-size: 16px; }
.tt-ly-body { flex: 1 1 0; min-height: 0; overflow-y: auto; padding: 8px 16px 24px; scroll-padding-top: 35%;
  mask-image: linear-gradient(to bottom, transparent, #000 6%, #000 94%, transparent); }
/* A line rests at .45; the lyrics sheet fades it up when it is sung and down to .3 after. */
.tt-ly { padding: 6px 0; font-size: 22px; font-weight: 800; line-height: 1.25; letter-spacing: -.01em; opacity: .45; }
.tt-ly:empty { height: 16px; padding: 0; }
.tt-ly-plain { padding-bottom: 20px; font-size: 15px; line-height: 1.6; white-space: pre-line; }
.tt-ly-note { padding: 24px 0 8px; line-height: 1.5; }
.tt-ly-foot { flex: 0 0 auto; padding: 8px 16px 12px; }

/* ---------- settings page: grouped rows, a label and its description left, the control right ---------- */
.tt-settings { padding: 24px; display: flex; flex-direction: column; align-items: center; overflow-y: auto; }
.tt-set-page { width: 100%; max-width: 720px; display: flex; flex-direction: column; gap: 28px; }
.tt-set-head { font-size: 28px; font-weight: 800; letter-spacing: -.01em; }
.tt-set-sec { display: flex; flex-direction: column; gap: 8px; }
.tt-set-title { font-size: 16px; font-weight: 700; }
.tt-set-card { border-radius: 8px; background: var(--l1); overflow: hidden; }
.tt-set-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; min-height: 56px; padding: 10px 16px; box-sizing: border-box; }
.tt-set-row + .tt-set-row { box-shadow: inset 0 1px 0 var(--l2); }
.tt-set-text { flex: 1 1 auto; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.tt-set-label { font-size: 14px; font-weight: 600; }
.tt-set-desc { font-size: 12px; line-height: 1.4; }
.tt-set-control { flex: 0 0 auto; display: flex; align-items: center; justify-content: flex-end; gap: 8px; }
.tt-set-note { padding: 12px 16px; font-size: 12px; line-height: 1.5; opacity: .7; }
.tt-set-note + .tt-set-row, .tt-set-row + .tt-set-note { box-shadow: inset 0 1px 0 var(--l2); }
.tt-set-value { min-width: 64px; text-align: right; font-size: 12px; font-variant-numeric: tabular-nums; opacity: .8; }
.tt-set-line { display: flex; align-items: baseline; gap: 8px; line-height: 1.45; }
.tt-set-line > .tt-dot { flex: 0 0 auto; align-self: flex-start; margin-top: .5em; }
.tt-set-keys { padding: 12px 16px; }
.tt-sw { position: relative; width: 40px; height: 22px; padding: 0; border-radius: 11px; background: var(--l2); cursor: pointer; transition: background-color .15s; }
.tt-sw.tt-on { background: var(--accent-fill); }
.tt-sw-knob { position: absolute; top: 3px; left: 3px; width: 16px; height: 16px; border-radius: 50%; background: #fff;
  box-shadow: 0 1px 2px rgba(0,0,0,.3); transition: transform .15s; }
.tt-sw.tt-on .tt-sw-knob { transform: translateX(18px); }
.tt-seg { display: flex; gap: 2px; padding: 2px; border-radius: 8px; background: var(--l2); }
.tt-seg-opt { height: 28px; padding: 0 12px; border-radius: 6px; font-size: 12px; font-weight: 600; color: inherit; white-space: nowrap; cursor: pointer; opacity: .75; }
.tt-seg-opt:hover { opacity: 1; }
.tt-seg-opt.tt-cur { background: var(--t1); color: var(--card); opacity: 1; }
.tt-select { display: inline-flex; align-items: center; justify-content: space-between; gap: 8px; min-width: 150px; max-width: 260px; height: 32px; padding: 0 12px;
  border-radius: 6px; background: var(--l2); color: inherit; font-size: 13px; cursor: pointer; }
.tt-select:hover { filter: brightness(1.1); }
.tt-select > span:first-child { min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
/* A drawn chevron (the ▾ character renders as a dot at this size). */
.tt-chev { flex: 0 0 auto; width: 7px; height: 7px; margin: -4px 2px 0 0; border-right: 1.5px solid currentColor; border-bottom: 1.5px solid currentColor;
  transform: rotate(45deg); opacity: .7; }
.tt-cells { display: flex; align-items: center; gap: 2px; }
.tt-pcell { width: 7px; height: 20px; border-radius: 2px; background: var(--l2); cursor: pointer; }
.tt-pcell:hover { filter: brightness(1.25); }
.tt-pcell.tt-lit { background: var(--accent); }
.tt-pcell.tt-zero { background: var(--t3, var(--l2)); }
.tt-set-field { display: flex; align-items: center; gap: 8px; }
.tt-set-field .sf-input { width: 180px; height: 32px; padding: 0 12px; border-radius: 6px; background: var(--card); box-shadow: inset 0 0 0 1px var(--l2); }
.tt-set-field .sf-input.focused { box-shadow: inset 0 0 0 2px var(--accent); }
.tt-set-links { display: flex; flex-wrap: wrap; gap: 4px 16px; justify-content: flex-end; }
.tt-compact .tt-settings { padding: 16px; }
.tt-compact .tt-set-head { font-size: 24px; }
.tt-compact .tt-set-row.tt-wide { flex-direction: column; align-items: stretch; gap: 8px; }
.tt-compact .tt-set-row.tt-wide .tt-set-control { justify-content: flex-start; flex-wrap: wrap; }
.tt-compact .tt-set-row.tt-wide .tt-set-links { justify-content: flex-start; }
.tt-compact .tt-seg { flex-wrap: wrap; max-width: 100%; }
.tt-compact .tt-set-row.tt-wide .tt-select { max-width: 100%; }
.tt-set-note + .tt-set-note { padding-top: 0; box-shadow: none; }
.tt-compact .tt-set-field .sf-input { flex: 1 1 140px; width: auto; min-width: 0; }
.tt-sleep { display: inline-flex; flex: 0 0 auto; align-items: center; gap: 6px; height: 28px; padding: 0 10px; border-radius: 14px; background: var(--l1);
  color: inherit; font-size: 12px; font-weight: 600; white-space: nowrap; cursor: pointer; }
.tt-sleep:hover { background: var(--l2); }
.tt-dense .tt-tr { height: 40px; gap: 12px; }
.tt-dense .tt-tr .tt-art-40 { width: 28px; height: 28px; }
.tt-dense .tt-pl { height: 48px; }
.tt-dense .tt-pl.tt-line { height: 32px; }
.tt-dense .tt-qrow { height: 40px; }
.tt-dense .tt-set-row { min-height: 44px; padding-top: 6px; padding-bottom: 6px; }
.tt-tr.tt-blocked { opacity: .4; }
.tt-keys { display: flex; flex-direction: column; gap: 6px; }
.tt-key-row { display: flex; align-items: center; gap: 12px; }
.tt-key-row > .tt-kbd { flex: 0 0 84px; text-align: center; box-sizing: border-box; }
.tt-kbd { padding: 1px 6px; border-radius: 4px; background: var(--l1); font-size: 11px; font-weight: 600;
  font-family: var(--mono, ui-monospace, monospace); white-space: nowrap; }

/* ---------- chips: Your Library's kinds and an artist's discography ---------- */
.tt-chips { display: flex; flex-wrap: wrap; gap: 8px; }
.tt-chip { height: 28px; padding: 0 12px; border-radius: 14px; font-size: 12px; font-weight: 600; cursor: pointer;
  background: var(--l1); color: inherit; transition: background-color .12s; }
.tt-chip:hover { background: var(--l2); }
.tt-chip.tt-cur { background: var(--t1); color: var(--card); }

/* ---------- equalizer panel ---------- */
.tt-eq { position: relative; padding: 16px; display: flex; flex-direction: column; gap: 16px; }
.tt-eq-head { display: flex; align-items: center; justify-content: space-between; }
.tt-eq-head .tt-h2 { margin: 0; font-size: 16px; }
.tt-switch { position: relative; width: 36px; height: 20px; padding: 0; border-radius: 10px; background: var(--l2); cursor: pointer; transition: background-color .15s; }
.tt-switch.tt-on { background: var(--accent-fill); }
.tt-knob { position: absolute; top: 2px; left: 2px; width: 16px; height: 16px; border-radius: 50%; background: #fff;
  box-shadow: 0 1px 2px rgba(0,0,0,.3); transition: transform .15s; }
.tt-switch.tt-on .tt-knob { transform: translateX(16px); }
.tt-eq-note { display: flex; gap: 12px; font-size: 12px; line-height: 1.5; }
.tt-eq-note-text { flex: 1 1 0; min-width: 0; }
.tt-eq-i { flex: 0 0 auto; width: 16px; height: 16px; border-radius: 50%; box-shadow: inset 0 0 0 1.5px currentColor;
  font-size: 10px; font-weight: 800; line-height: 16px; text-align: center; opacity: .7; }

/* The preset: a button naming the current one. Its menu is an overlay like Your Library's (.tt-menu),
   over a scrim that takes the click that closes it. */
.tt-eq .tt-select { display: flex; align-items: center; gap: 8px; width: 100%; max-width: none; height: 40px; padding: 0 8px 0 12px; border-radius: 6px; background: var(--l1);
  color: inherit; text-align: left; cursor: pointer; transition: background-color .12s; }
.tt-eq .tt-select:hover, .tt-eq .tt-select.tt-open { background: var(--l2); filter: none; }
.tt-select-cap { font-size: 12px; opacity: .6; }
.tt-select-val { flex: 1 1 auto; font-size: 14px; font-weight: 700; }
.tt-select-chev { display: flex; opacity: .7; transition: transform .15s; }
.tt-select.tt-open .tt-select-chev { transform: rotate(180deg); }
.sf-overlay[data-role^='tt.eqmenu'] { font-family: var(--sans, system-ui, sans-serif); font-size: 13px; }
.sf-overlay[data-role='tt.eqmenu'] .sf-ov-card { width: 268px !important; }
.sf-overlay[data-role='tt.eqmenu-page'] .sf-ov-card { width: calc(100% - 32px) !important; }
.tt-eq-scrim { position: absolute; inset: 0; pointer-events: auto; }

/* The graph takes the panel's height, within bounds (`.tt-eq > .tt-eq-graph` outranks `.tt-eq > *`'s
   `flex: 0 0 auto`). Its plot has a gutter for the dB scale, then the field of ten bands: each a track
   with a thumb at its gain (--y, how far down the track, from .tt-g0 … .tt-g24) and a click cell per
   2 dB over it. The frequencies run under the plot. */
.tt-eq > .tt-eq-graph { --gut: 32px; flex: 1 1 0; min-height: 180px; max-height: 640px; display: flex; flex-direction: column; gap: 8px;
  user-select: none; transition: opacity .15s; }
.tt-eq-graph.tt-off { --accent: var(--t3); --accent-ink: var(--t3); opacity: .5; }
.tt-eq-plot { position: relative; flex: 1 1 0; min-height: 0; }
.tt-eq-axis { position: absolute; left: 0; top: 28px; bottom: 8px; width: calc(var(--gut) - 4px); }
.tt-eq-field { position: absolute; left: var(--gut); right: 0; top: 28px; bottom: 8px; }
.tt-eq-unit { font-size: 9px; font-weight: 600; letter-spacing: .04em; line-height: 12px; opacity: .45; }
.tt-eq-axis > .tt-eq-unit { position: absolute; right: 0; top: -24px; }
.tt-eq-tick { position: absolute; right: 0; margin-top: -6px; font-size: 10px; line-height: 12px; opacity: .55; }
/* Guides in the text colour at low opacity: neutral in any theme (the surface layers can carry a tint). */
.tt-eq-line { position: absolute; left: 0; right: 0; height: 1px; margin-top: -.5px; background: var(--t1); opacity: .06; }
.tt-eq-line.tt-at50 { opacity: .22; }
.tt-at0 { top: 0; } .tt-at25 { top: 25%; } .tt-at50 { top: 50%; } .tt-at75 { top: 75%; } .tt-at100 { top: 100%; }
.tt-eq-curve { position: absolute; inset: 0; color: var(--accent); pointer-events: none; }
.tt-eq-curve > *, .tt-eq-curve .sf-img-svg { position: absolute; inset: 0; width: 100% !important; height: 100% !important; }
.tt-eq-bands { position: absolute; inset: 0; display: flex; }
.tt-band { position: relative; flex: 1 1 0; min-width: 0; }
.tt-eq-rail { position: absolute; left: 50%; top: 0; bottom: 0; width: 3px; margin-left: -1.5px; border-radius: 2px; background: var(--l2); transition: background-color .12s; }
.tt-band:hover .tt-eq-rail { background: var(--l3); }
.tt-eq-act { position: absolute; left: 50%; width: 3px; margin-left: -1.5px; border-radius: 2px; background: var(--accent);
  top: min(var(--y), 50%); bottom: calc(100% - max(var(--y), 50%)); }
/* Knobs sit on the curve, Spotify-style: small, no ring cutting the line either side of them. */
.tt-eq-thumb { position: absolute; left: 50%; top: var(--y); width: 12px; height: 12px; margin: -6px 0 0 -6px; border-radius: 50%; background: var(--t1);
  box-shadow: 0 1px 4px rgba(0,0,0,.45); transition: transform .12s; }
.tt-band:hover .tt-eq-thumb { transform: scale(1.12); }
.tt-eq-val { position: absolute; left: 0; right: 0; top: -24px; text-align: center; font-size: 10px; font-weight: 700; line-height: 12px; color: var(--accent-ink); }
.tt-eq-cells { position: absolute; inset: 0; display: flex; flex-direction: column; }
.tt-eq-cell { position: relative; flex: 1 1 0; cursor: pointer; }
.tt-eq-cell.tt-first, .tt-eq-cell.tt-last { flex-grow: .5; }
.tt-eq-cell:hover::before { content: ""; position: absolute; left: 50%; top: 50%; width: 12px; height: 12px; margin: -6px 0 0 -6px; border-radius: 50%;
  background: var(--t1); opacity: .3; }
.tt-eq-cell.tt-first:hover::before { top: 0; }
.tt-eq-cell.tt-last:hover::before { top: 100%; }
.tt-eq-cell.tt-cur:hover::before { display: none; }
.tt-eq-cell:hover::after { content: attr(data-db); position: absolute; left: 50%; bottom: 100%; z-index: 3; padding: 2px 6px; border-radius: 4px;
  background: var(--t1); color: var(--card); font-size: 10px; font-weight: 700; line-height: 12px; white-space: nowrap; transform: translateX(-50%); }
.tt-eq-hz { display: flex; font-size: 10px; line-height: 12px; }
.tt-eq-hz > * { flex: 1 1 0; min-width: 0; text-align: center; opacity: .62; }
.tt-eq-hz > .tt-eq-unit { flex: 0 0 calc(var(--gut) - 4px); margin-right: 4px; text-align: right; }
/* The gain of each band, in dB from -12 (bottom) to +12: where its thumb sits as a share of the track. */
.tt-g0 { --y: 100% } .tt-g1 { --y: 95.8333% } .tt-g2 { --y: 91.6667% } .tt-g3 { --y: 87.5% } .tt-g4 { --y: 83.3333% } .tt-g5 { --y: 79.1667% } .tt-g6 { --y: 75% } .tt-g7 { --y: 70.8333% } .tt-g8 { --y: 66.6667% } .tt-g9 { --y: 62.5% } .tt-g10 { --y: 58.3333% } .tt-g11 { --y: 54.1667% } .tt-g12 { --y: 50% } .tt-g13 { --y: 45.8333% } .tt-g14 { --y: 41.6667% } .tt-g15 { --y: 37.5% } .tt-g16 { --y: 33.3333% } .tt-g17 { --y: 29.1667% } .tt-g18 { --y: 25% } .tt-g19 { --y: 20.8333% } .tt-g20 { --y: 16.6667% } .tt-g21 { --y: 12.5% } .tt-g22 { --y: 8.3333% } .tt-g23 { --y: 4.1667% } .tt-g24 { --y: 0% }

/* ---------- setup ---------- */
.tt-setup { max-width: 640px; margin: 0 auto; padding-top: 8px; display: flex; flex-direction: column; gap: 12px; }
.tt-setup-title { font-size: 28px; font-weight: 800; letter-spacing: -.01em; }
.tt-setup-lead { margin-bottom: 12px; line-height: 1.5; }
.tt-account { display: flex; align-items: center; gap: 8px; margin-top: 12px; padding-top: 16px; border-top: 1px solid var(--l1); font-size: 12px; }
.tt-link { padding: 0; background: none; color: var(--accent-ink); font: inherit; font-weight: 600; cursor: pointer; }
.tt-link:hover { text-decoration: underline; }
.tt-step { display: grid; grid-template-columns: 28px minmax(0, 1fr); gap: 16px; padding: 16px; border-radius: 8px; background: var(--l1); }
.tt-step.tt-todo { opacity: .5; }
.tt-step-n { display: flex; align-items: center; justify-content: center; width: 28px; height: 28px; border-radius: 50%;
  font-weight: 700; box-shadow: inset 0 0 0 1.5px currentColor; }
.tt-step.tt-done .tt-step-n { background: var(--accent-fill); color: #fff; box-shadow: none; }
.tt-step-body { display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.tt-step-title { font-size: 15px; font-weight: 700; line-height: 28px; }
.tt-step-text { line-height: 1.5; }
.tt-step-text code { font-family: var(--mono); font-size: 12px; padding: 1px 4px; border-radius: 4px; background: var(--l2); overflow-wrap: anywhere; }
.tt-field { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.tt-field .sf-input { flex: 1 1 180px; min-width: 0; height: 32px; padding: 0 12px; border-radius: 6px; background: var(--card); box-shadow: inset 0 0 0 1px var(--l2); }
.tt-field .sf-input.focused { box-shadow: inset 0 0 0 2px var(--accent); }
/* Edit details: Spotify's two-column form; artwork is not an upload control. */
.sf-overlay[data-role='tt.edit'] .sf-ov-card { width: 520px !important; max-width: calc(100vw - 32px) !important; background: var(--card); box-sizing: border-box; }
.sf-overlay[data-role='tt.edit'] .sf-ov-body { padding: 24px; gap: 0; }
.tt-edit-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 24px; gap: 16px; }
.tt-edit-title { font-size: 24px; font-weight: 800; line-height: 32px; }
.tt-edit-close { margin-right: -8px; color: var(--t2); }
.tt-edit { display: flex; gap: 16px; align-items: flex-start; }
.tt-edit > .tt-art { border-radius: 4px; pointer-events: none; }
.tt-edit-fields { flex: 1 1 auto; display: flex; flex-direction: column; gap: 16px; min-width: 0; }
.tt-edit .tt-field .sf-input { height: 40px; border-radius: 4px; background: var(--l1); box-shadow: none; font-size: 14px; }
.tt-edit .tt-field .sf-input.focused { box-shadow: inset 0 0 0 1px var(--t3); }
.tt-edit-description .sf-editor { height: 124px; min-height: 124px; max-height: 124px; border-radius: 4px; padding: 12px;
  background: var(--l1); font-family: var(--sans, system-ui, sans-serif); font-size: 14px; line-height: 24px; overflow: auto; box-sizing: border-box; }
.tt-edit-description .sf-editor.focused { box-shadow: inset 0 0 0 1px var(--t3); }
.tt-edit-foot { display: flex; justify-content: space-between; align-items: center; gap: 16px; margin-top: 16px; }
.tt-edit-public { display: inline-flex; align-items: center; gap: 8px; font-weight: 700; }
.tt-save { height: 48px; padding: 0 32px; border-radius: 24px; background: #fff; color: #000; font-size: 16px; font-weight: 800;
  cursor: pointer; transition: transform .1s; }
.tt-save:hover { transform: scale(1.04); }
.tt-dialog-cancel { font-weight: 600; }
.tt-step-status { display: flex; align-items: center; gap: 8px; font-size: 12px; }
.tt-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--t4); flex: 0 0 auto; }
.tt-dot.tt-live { background: var(--ok); animation: tt-pulse 1.6s ease-in-out infinite; }
.sf-still .tt-dot.tt-live, .sf-paused .tt-dot.tt-live { animation: none; }
@keyframes tt-pulse { 50% { opacity: .35; } }

/* ---------- now-playing bar (dock) ---------- */
.tt-bar { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1.6fr) minmax(0, 1fr); align-items: center; gap: 16px;
  height: 72px; padding: 0 16px;
  /* Tern pads the dock region ~24 px above its content; Spotify keeps the bar
     ~8 px under the panels. The negative margin takes the padding back. */
  margin-top: -22px; }
.tt-np { display: flex; align-items: center; gap: 12px; min-width: 0; }
.tt-np-text { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.tt-np-title { font-size: 14px; font-weight: 600; }
.tt-np-sub { font-size: 12px; }
.tt-center { display: flex; flex-direction: column; align-items: center; gap: 4px; min-width: 0; }
.tt-inert { opacity: .35; pointer-events: none; }
.tt-controls { display: flex; align-items: center; justify-content: center; gap: 8px; }
.tt-right { display: flex; align-items: center; justify-content: flex-end; gap: 4px; }
.tt-bar-m { grid-template-columns: minmax(0, 1fr) minmax(0, 1.6fr) minmax(max-content, 1fr); }
.tt-bar-m .tt-vrail { display: none; }

/* ---------- seek and volume ---------- */
.tt-seek { display: grid; grid-template-columns: 40px minmax(0, 1fr) 40px; align-items: center; gap: 8px; width: 100%; max-width: 640px; font-size: 11px; }
.tt-seek > :first-child { text-align: right; }
.tt-seek > :last-child { text-align: left; }
.tt-rail { position: relative; height: 16px; display: flex; align-items: center; }
.tt-track { position: absolute; left: 0; right: 0; height: 4px; border-radius: 2px; background: var(--l2); overflow: hidden; }
.tt-fill { position: absolute; inset: 0; background: var(--t2); transform-origin: left center; transform: scaleX(0); }
.tt-rail:hover .tt-fill { background: var(--accent); opacity: 1; }
.tt-hit { position: absolute; inset: 0; display: flex; }
.tt-cell { flex: 1 1 0; height: 100%; cursor: pointer; }
.tt-vol { display: flex; align-items: center; gap: 4px; }
.tt-vrail { position: relative; width: 96px; height: 16px; display: flex; align-items: center; }
.tt-vfill { position: absolute; left: 0; height: 4px; border-radius: 2px; background: var(--t2); }
.tt-vrail:hover .tt-vfill { background: var(--accent); opacity: 1; }

/* ---------- compact layout (phone-like): one column, mini player, tab bar ---------- */
.tt-app.tt-compact { grid-template-columns: minmax(0, 1fr); padding: 0; gap: 0; }
.tt-compact > .tt-panel { border-radius: 0; }
.tt-compact .tt-content { padding: 16px; }
.tt-compact .tt-hero { flex-direction: column; align-items: flex-start; gap: 16px; }
.tt-compact .tt-hero-title { font-size: 28px; }
.tt-compact .tt-tr { grid-template-columns: minmax(0, 1fr) 48px; padding: 0 4px; }
.tt-compact .tt-tr > .tt-idx, .tt-compact .tt-tr > :nth-child(3):not(.tt-tdur) { display: none; }
.tt-compact .tt-quick-grid { grid-template-columns: repeat(2, minmax(0, 1fr)) !important; }
.tt-mp { position: relative; margin: -22px 8px 0; border-radius: 8px; background: var(--l2); overflow: hidden; }
.tt-mp-row { display: flex; align-items: center; gap: 4px; height: 56px; padding: 0 8px; }
.tt-mp-info { flex: 1 1 auto; display: flex; align-items: center; gap: 10px; min-width: 0; cursor: pointer; }
.tt-mp-line { position: absolute; left: 8px; right: 8px; bottom: 0; height: 2px; border-radius: 1px; background: var(--l1); overflow: hidden; }
.tt-mp-line .tt-fill { background: var(--t1); }
.tt-btn.tt-mp-play { width: 40px; height: 40px; color: var(--t1); opacity: 1; }
.tt-tabs { display: grid; grid-template-columns: repeat(3, 1fr); height: 56px; }
.tt-tab { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 2px; font-size: 11px; font-weight: 600;
  opacity: .6; cursor: pointer; transition: opacity .12s; }
.tt-tab:hover, .tt-tab.tt-cur { opacity: 1; }

/* The now-playing page of the compact layout. */
.tt-nowview-full { padding: 12px 16px 24px; }
.tt-nowview-top { display: flex; align-items: center; gap: 8px; min-width: 0; }
/* Close and devices buttons are the same size either side, so the heading centres on the page. */
.tt-nowview-full .tt-nowview-head { flex: 1 1 auto; text-align: center; }
.tt-nowview-full .tt-nowview-cover { max-width: 420px; padding-top: min(100%, 420px); align-self: center; }
.tt-nowview-full .tt-seek { max-width: none; }
.tt-nowview-full .tt-controls { gap: 16px; }
.tt-nowview-full .tt-btn.tt-primary { width: 56px; height: 56px; }
.tt-nowview-tools { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.tt-nowview-tools .tt-vrail { width: 120px; }

/* Too short for anything but the player: the mini player fills the pane (a
   full-size overlay with its card look stripped). */
.sf-overlay[data-role='tt.strip'] .sf-ov-card { background: none !important; backdrop-filter: none !important; box-shadow: none !important;
  border: 0 !important; border-radius: 0 !important; inset: 0 !important; width: auto !important; height: auto !important;
  max-width: none !important; max-height: none !important; margin: 0 !important; padding: 0 !important; }
.sf-overlay[data-role='tt.strip'] .sf-ov-card::before, .sf-overlay[data-role='tt.strip'] .sf-ov-card::after { display: none !important; }
.sf-overlay[data-role='tt.strip'] .sf-ov-body { padding: 0 !important; height: 100%; display: flex; flex-direction: column; justify-content: center; }
.sf-overlay[data-role='tt.strip'] .tt-mp { margin: 0 8px; }

/* ---------- artist page ---------- */
.tt-see-more { margin: 8px 16px 0; font-size: 13px; font-weight: 700; }
.tt-shelf > .tt-chips { margin: 4px 0 12px; }
.tt-artist-about { display: flex; flex-direction: column; border-radius: 8px; background: var(--l1); overflow: hidden; }
.tt-about-body { display: flex; gap: 24px; align-items: flex-start; padding: 16px; }
.tt-about-photo { width: 160px; height: 160px; border-radius: 4px; }
.tt-about-text { line-height: 1.55; max-width: 72ch; }
.tt-source { font-size: 12px; font-weight: 600; color: inherit; cursor: pointer; }
.tt-source:hover { text-decoration: underline; }
.tt-plays { text-align: right; }
.tt-about-facts { display: flex; flex-direction: column; gap: 10px; min-width: 0; line-height: 1.5; }
.tt-about-facts .tt-about-name { padding: 0; font-size: 20px; }
.tt-compact .tt-about-body { flex-direction: column; }
.tt-compact .tt-about-photo { width: 100%; height: 0; padding-top: 100%; }
.tt-compact .tt-about-photo > * { position: absolute; inset: 0; }
/* Artist pages from the web player: the About card's banner, cities and photos, the artist's pick, the
   header's listeners, and the rows Popular shows until its songs arrive. */
.tt-about-banner { width: 100%; height: 0; padding-top: 32%; border-radius: 0; }
.tt-about-banner > * { position: absolute; inset: 0; }
.tt-about-gallery { display: flex; gap: 8px; padding: 0 16px 16px; overflow: hidden; }
.tt-about-shot { width: 96px; height: 96px; border-radius: 4px; }
.tt-about-sub { font-size: 11px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; opacity: .7; }
.tt-cities { display: flex; flex-direction: column; gap: 4px; }
.tt-city { display: flex; justify-content: space-between; gap: 16px; max-width: 360px; }
.tt-stats { font-size: 14px; font-weight: 600; }
.tt-tr.tt-skel { background: var(--l1); opacity: .5; margin-bottom: 2px; }
.tt-top-row { display: grid; grid-template-columns: minmax(0, 1fr) 280px; gap: 24px; align-items: start; }
/* The songs share the row with the pick: give the title the slack, the counts their own width. */
.tt-top-row .tt-tr { grid-template-columns: 32px minmax(0, 1fr) auto 48px; gap: 12px; padding: 0 8px; }
.tt-apick { display: flex; align-items: center; gap: 16px; margin: 0 -8px; padding: 8px; border-radius: 6px; cursor: pointer; }
.tt-apick:hover { background: var(--l1); }
.tt-art-pick { width: 80px; height: 80px; border-radius: 4px; }
.tt-apick-text { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.tt-apick-by { display: flex; align-items: center; gap: 6px; min-width: 0; font-size: 12px; }
.tt-art-16 { width: 16px; height: 16px; border-radius: 50%; }
.tt-apick-title { font-size: 15px; font-weight: 700; }
.tt-apick-comment { font-size: 12px; font-style: italic; opacity: .8; }
.tt-actions .tt-pill.tt-on { box-shadow: inset 0 0 0 1px var(--t1); }
.tt-hero-title.sf-act { cursor: pointer; }
.tt-side-head { gap: 8px; }
.tt-side-add { margin-left: auto; }

/* ---------- dialogs ---------- */
.tt-pick { display: flex; flex-direction: column; gap: 2px; max-height: 360px; overflow-y: auto; min-width: 300px; }
.tt-pick-row { display: flex; align-items: center; gap: 12px; height: 48px; padding: 0 8px; border-radius: 6px; cursor: pointer; flex: 0 0 auto; }
.tt-pick-row:hover { background: var(--l1); }
.tt-pick-row > .tt-trunc { flex: 1 1 auto; }
.tt-pick-state { flex: 0 0 auto; display: flex; margin-left: auto; }
.tt-dialog-foot { display: flex; justify-content: flex-end; padding-top: 8px; }

/* ---------- attribution ---------- */
.tt-open-spotify { display: inline-flex; align-items: center; gap: 8px; padding: 6px 12px 6px 8px; border-radius: 16px;
  font-size: 11px; font-weight: 700; letter-spacing: .08em; cursor: pointer; box-shadow: inset 0 0 0 1px var(--l2); }
.tt-open-spotify:hover { background: var(--l1); }
.tt-attrib { display: flex; justify-content: center; }
"#;
