# Nyx App Style

How Nyx apps look and behave. Most of it is built into the shared crates, so a new
app gets it by using them rather than by copying another app:

| Crate | What it gives an app |
| --- | --- |
| `nyx-core` | Its identity (`app::init`), XDG paths, the preferences store (`prefs_store!`), running commands, atomic writes, moving files from old names (`migrate`) |
| `nyx-theme` | Palettes, the bundled and user themes, Follow Omarchy, the suite-wide theme, the base stylesheet (`base.css`) |
| `nyx-ui` | Widgets and option rows, the settings card, the window frame (`chrome`), the command bar and Inspector, the common keys |
| `nyx-icons` | The app icons (`make_icons.py`) |

**Music** (`apps/music`) is the reference app with a sidebar, **Settings**
(`apps/settings`) the reference for an app whose pages are all settings, and **Write**
(`apps/office`) the reference for a document editor (command bar and Inspector). If a detail isn't
covered here, do what they do.

**Read the reference source before building or changing a layout.** Open Music's
`src/window.rs` and the relevant shared `nyx-ui` widget and `nyx-theme/base.css`
implementation. Copy the existing construction, sizing and resize behavior; do not
approximate it from screenshots or invent another sidebar implementation. Contacts
also demonstrates the corrected non-expanding navigation structure.

---

## 1. Principles

- **One flat surface.** The whole window is a single `bg` surface: sidebar,
  page and status bar alike. Areas are separated only by 1 px hairlines, never by
  different fills or shadows.
- **Monospace throughout.** Every label, button and readout uses JetBrains Mono.
- **Calm and dense.** Low contrast, one accent used sparingly for state (where you
  are, on, selected). No glows, no big titles.
- **Everything comes from semantic color tokens.** Never hard-code a color in a
  component. Every color is one of the tokens below, or one of the derived mixes.
- **Themeable, and follows Omarchy by default.** Every app ships the bundled themes,
  accepts user themes, and has a *Follow Omarchy* mode that restyles live.
- **Native, keyboard-first, no client-side chrome.** No title bar (Hyprland manages
  windows), and no top bar either: the window starts straight with the sidebar and
  the page. <kbd>Ctrl</kbd>+<kbd>F</kbd> searches,
  <kbd>Esc</kbd> clears or closes, <kbd>Ctrl</kbd>+<kbd>B</kbd> collapses the sidebar,
  <kbd>F1</kbd> lists the shortcuts, <kbd>Ctrl</kbd>+<kbd>Q</kbd>/<kbd>W</kbd> closes.
- **Works tiled at half-screen.** Layouts must fit an 800 px wide window with no
  horizontal scroll (see Responsive).
- **No visible scrollbars anywhere.** Every scrollable area — pages, sidebars,
  horizontal strips, tables, dialogs, popovers and dropdown menus — must scroll with
  hidden scrollbars (`gtk::PolicyType::External` or `Never`, and zero-size/transparent
  `scrollbar` rules in CSS). Never show a visible scrollbar track or thumb.

---

## 2. Color tokens

Every theme defines exactly these 11 colors, plus a `light` flag. `on_accent` is
derived: text drawn on an accent fill.

| Token | Role |
| --- | --- |
| `bg` | Window and content background |
| `surface` | Popovers and toasts: one step off `bg` (the window itself is all `bg`) |
| `text` | Primary text |
| `dim_text` | Secondary text: descriptions, group headings, readouts |
| `accent` | Focus, "on" switches, sort arrows, slider fill; mixed into `@nav_active` for selection |
| `border` | Base of `@hairline`: every line between areas and every card outline |
| `muted` | Neutral fills: slider troughs, off switches, the settings card's `@lift` |
| `highlight` | A spare selection tone (hover and selection use `@row_hover` and `@nav_active`) |
| `danger` | Destructive actions, errors, warnings |
| `glow` | Kept for themes and the Glow preference (usually = accent); the flat look draws no glows |
| `shadow` | Drop shadows, code-block backgrounds (darkest color) |
| `on_accent` | Derived: `#ffffff` for light themes, otherwise `bg` |

Names in code: GTK `@theme_bg`, `@theme_dim_text`, …; CSS/web `--nx-bg`,
`--nx-dim-text`, …; QML/Rust `palette.bg`, `palette.dimText`/`dim_text`.

### Derived colors

Define these once at the top of the stylesheet, loaded in the same provider as the
palette, and use them by name:

```css
@define-color hairline mix(@theme_bg, @theme_border, 0.82);
@define-color divider alpha(@theme_text, 0.08);
@define-color nav_active mix(@theme_bg, @theme_accent, 0.18);
@define-color row_hover mix(@theme_bg, @theme_accent, 0.07);
@define-color lift mix(@theme_bg, @theme_muted, 0.09);
@define-color lift_active mix(@lift, @theme_accent, 0.1);
@define-color veil alpha(mix(@theme_bg, @theme_shadow, 0.45), 0.82);
```

| Use | Recipe |
| --- | --- |
| Every line between areas, card and input borders | `@hairline` |
| Divider between rows inside a card | `@divider` |
| Active nav item, selected row, selected chip or segment, primary button | `@nav_active` |
| Hover on rows, nav items and flat buttons | `@row_hover` |
| Settings card's right column | `@lift` |
| Active item in the settings card's list | `@lift_active` |
| Dimmed window behind a card | `@veil` |
| Breadcrumb's page name, sort arrows, on-switch knob, icons on primary buttons | `accent` |
| Secondary text: descriptions, headings, readouts, status bar | `dim_text` |
| Danger: armed buttons, warnings, errors | `danger` (fills at 0.10–0.18 alpha) |

---

## 3. Themes

### Bundled themes (ship all of them)

| id | Name | bg | surface | text | dim_text | accent | border | muted | highlight | danger | glow | shadow | light |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| dracula | Dracula | #282a36 | #21222c | #f8f8f2 | #6272a4 | #bd93f9 | #44475a | #44475a | #343746 | #ff5555 | #bd93f9 | #191a21 | no |
| catppuccin-mocha | Catppuccin Mocha | #1e1e2e | #181825 | #cdd6f4 | #7f849c | #89b4fa | #313244 | #45475a | #313244 | #f38ba8 | #89b4fa | #11111b | no |
| catppuccin-macchiato | Catppuccin Macchiato | #24273a | #1e2030 | #cad3f5 | #8087a2 | #8aadf4 | #363a4f | #494d64 | #363a4f | #ed8796 | #8aadf4 | #181926 | no |
| catppuccin-frappe | Catppuccin Frappé | #303446 | #292c3c | #c6d0f5 | #838ba7 | #8caaee | #414559 | #51576d | #414559 | #e78284 | #8caaee | #232634 | no |
| catppuccin-latte | Catppuccin Latte | #eff1f5 | #e6e9ef | #4c4f69 | #8c8fa1 | #1e66f5 | #ccd0da | #bcc0cc | #dce0e8 | #d20f39 | #1e66f5 | #9ca0b0 | yes |
| tokyo-night | Tokyo Night | #1a1b26 | #16161e | #c0caf5 | #565f89 | #7aa2f7 | #292e42 | #3b4261 | #283457 | #f7768e | #7aa2f7 | #0f0f14 | no |
| tokyo-night-storm | Tokyo Night Storm | #24283b | #1f2335 | #c0caf5 | #565f89 | #7aa2f7 | #292e42 | #3b4261 | #2e3c64 | #f7768e | #7aa2f7 | #1b1e2d | no |
| tokyo-night-moon | Tokyo Night Moon | #222436 | #1e2030 | #c8d3f5 | #636da6 | #82aaff | #2f334d | #3b4261 | #2d3f76 | #ff757f | #82aaff | #191a2a | no |
| one-dark-pro | One Dark Pro | #282c34 | #21252b | #abb2bf | #5c6370 | #61afef | #3e4451 | #4b5263 | #2c313a | #e06c75 | #61afef | #181a1f | no |
| nord | Nord | #2e3440 | #292e39 | #eceff4 | #7b88a1 | #88c0d0 | #3b4252 | #434c5e | #3b4252 | #bf616a | #88c0d0 | #242933 | no |
| gruvbox-dark | Gruvbox Dark | #282828 | #1d2021 | #ebdbb2 | #928374 | #fabd2f | #3c3836 | #504945 | #3c3836 | #fb4934 | #fabd2f | #141414 | no |
| rose-pine | Rosé Pine | #191724 | #1f1d2e | #e0def4 | #6e6a86 | #c4a7e7 | #26233a | #403d52 | #26233a | #eb6f92 | #c4a7e7 | #111019 | no |
| everforest | Everforest | #2d353b | #272e33 | #d3c6aa | #859289 | #a7c080 | #3d484d | #475258 | #3d484d | #e67e80 | #a7c080 | #1e2326 | no |
| solarized-dark | Solarized Dark | #002b36 | #00232c | #93a1a1 | #586e75 | #268bd2 | #073642 | #0f4a56 | #073642 | #dc322f | #268bd2 | #001e26 | no |
| kanagawa | Kanagawa | #1f1f28 | #16161d | #dcd7ba | #727169 | #7e9cd8 | #2a2a37 | #363646 | #2d4f67 | #e82424 | #7e9cd8 | #0d0c0c | no |

The fallback theme, when nothing else is available, is **Tokyo Night**.

### Follow Omarchy (the default mode)

Other apps (GTK, libadwaita, Qt, KDE) get the theme from hyprchroma, which
Settings → Theme → *Apps follow the theme* controls. New apps don't need to
theme anything outside their own window.

Read `~/.local/state/omarchy/current/theme/colors.toml` (respect `$XDG_STATE_HOME`)
and map it:

| Token | Omarchy key (fallbacks in order) |
| --- | --- |
| bg | `background` (required) |
| surface | `dark_background` → `background` |
| text | `foreground` (required) |
| dim_text | `dark_foreground` → `muted` → `foreground` |
| accent | `accent` → `blue` (required) |
| border | `lighter_background` → `selection` → `foreground` |
| muted | `muted` → `lighter_background` → `background` |
| highlight | `selection` → `lighter_background` → `background` |
| danger | `red` → `#e06c75` |
| glow | = accent |
| shadow | `darker_background` → `#000000` |
| light | `mode == "light"` |

- **Live updates:** poll once a second for changes to the modification time of
  `colors.toml` or the contents of `…/current/theme.name`, and restyle in place. Polling
  is more reliable than inotify, because Omarchy swaps the whole directory.
- If `colors.toml` is missing or invalid, hide the *Follow Omarchy* switch and use the
  chosen bundled theme.

### User themes

Each user theme is one file in `~/.config/nyx/themes/<id>.toml` (offered in every
app) or `~/.config/<app>/themes/<id>.toml` (this app only), with the same keys as a
bundled theme plus a display name:

```toml
name = "My Theme"
bg = "#1a1b26"
surface = "#16161e"
text = "#c0caf5"
dim_text = "#565f89"
accent = "#7aa2f7"
border = "#292e42"
muted = "#3b4261"
highlight = "#283457"
danger = "#f7768e"
glow = "#7aa2f7"
shadow = "#0f0f14"
light = false
```

List user themes after the bundled ones, sorted by name, with ids `custom:<file stem>`.

### One theme for the suite

The theme (`mode` and `theme`), density, text size and where apps open (`start`) live in
`~/.config/nyx/theme.toml`. Picking a theme in any app writes it there, and every open
app follows within a second (`nyx-theme` polls the file with Omarchy's). An app passes
`set_theme` to `nyx_theme::install` so the shared choice lands in its own preferences.

**Open on:** every app opens on its start page (`start = "home"`, the default) unless
the suite is set to reopen the page shown last (`start = "last"`). An app picks its
first page with `nyx_theme::start_page(home, last)` and still saves the last page it
showed. The choice is the **Open on** row (`nyx_ui::widgets::start_row`), in each
app's window group and in Settings → Nyx apps.

### Preferences (per app, `~/.config/<app>/settings.toml`)

`mode = "omarchy" | "theme"`, `theme = "<id>"`, `glow = true`,
`reduce_motion = false`, `sidebar_collapsed = false`. Picking a theme sets `mode = "theme"`. Every app exposes these
in its own settings, in a group called **<App> window** (e.g. **Settings window**): Follow Omarchy theme, a theme
dropdown (disabled while following), current-color swatches, Glow, Reduce motion, and Open on.

---

---

## 4. Typography

- **Font:** `"JetBrains Mono", "JetBrainsMono Nerd Font", monospace` on the window,
  weight 500, with tabular figures where numbers line up. Don't bundle it.
- **Base size:** the toolkit default (≈ 13–14 px). Everything else is relative:

| Element | Size | Weight | Color | Extra |
| --- | --- | --- | --- | --- |
| Sidebar head app name | 1em | 600 | `dim_text` | |
| Inspector segment | 0.92em | 500 | `dim_text`, `text` when open | contextual segments in `accent` |
| Inspector heading | 0.72em | 600 | `dim_text` | UPPERCASE, letter-spacing 0.1em |
| Group heading | 0.9em | 600 | `dim_text` | UPPERCASE, letter-spacing 2 px |
| Sidebar group label | 0.76em | 600 | `dim_text` | UPPERCASE, letter-spacing 1 px |
| Settings card heading | 0.92em | 600 | `dim_text` | `SETTINGS`, letter-spacing 2.5 px |
| Card header title | 1.2em | 600 | `text` | |
| Nav item, row title | 1em | 500 / 700 | `text` | |
| Row description | 0.88em | 400 | `dim_text` | wraps |
| Table header | 0.86em | 500 | `dim_text` | |
| Status bar | 0.86em | 500 | `dim_text` | `F1` key in `text` |

There are no page titles or page descriptions: the sidebar shows where you are, the
window title names the page (`App — Page`, `chrome::set_title`), and a page's one-line
description (if any) goes in the status bar.

GTK CSS can't transform text, so write uppercase headings with `to_uppercase()` in
code, and keep the original title for search.

**Copy style:** sentence case, American spelling. Descriptions are one short sentence
that says what the option *does* for the user, not what it's called internally.

---

## 5. Shape, spacing, motion

| Thing | Value |
| --- | --- |
| Radius scale | **4** small parts (tags, key caps, segment buttons) · **6** buttons, chips, inputs, nav items · **10** cards, banners, toasts · **14** the settings card. Only switches and sliders are pills. |
| Height scale | 30 bar buttons · 28 chips · 32 buttons and inputs · 35 table rows · 42 nav items · 46 toolbars · 44 command bar · 28 Inspector segments · 34 status bar |
| Spacing | multiples of 2 and 4 |
| Page padding | 12 px top · 24 px sides · 36 px bottom (narrow: 12 / 16 / 32) |
| Group heading margin | 26 px above, 12 px below; the page's first heading has none above, so the gap to the top edge equals the gap below it |
| Pages that fill the window | no padding at all: a toolbar row, then the table or grid edge to edge |
| Row padding | 10 px 20 px, 52 px tall at least |
| Transitions | 150 ms ease-out for hover/color; 160 ms crossfade between pages |
| Reduce motion | all durations 0; toolkit animations off |

---

## 6. Layout

```
┌──────────────┬───────────────────────────────────────────────┐
│ ▣ App     ⌕  │  [Filter…]  [Action] [Action]           [⋯]  │ toolbar row, 46 px
│──────────────├───────────────────────────────────────────────┤
│ ⌂ Home       │  Name ↑            │ Size   │ Modified       │ table header, 34 px
│              ├───────────────────────────────────────────────┤
│ LIBRARY      │  row                                          │ rows, 35 px
│ ▣ Item  ◀──  │  row                                          │
│ ▣ Item       │                                               │
├──────────────┴───────────────────────────────────────────────┤
│ F1 Shortcuts                          1,204 items   [eq]  ⚙ │ status bar, 34 px
└──────────────────────────────────────────────────────────────┘
```

- **Window:** `bg` everywhere. No title bar and no top bar: set an empty widget as the
  GTK titlebar. Default size 1120 × 800 (Notepad and Archive open floating, smaller;
  their Hyprland rules are in `hypr/nyx.lua`, written by the installer). The frame is
  a vertical box: body (sidebar and page column), any app bar (a player bar), status
  bar. The window's overlay holds toasts and cards.
- **Sidebar head** (`chrome::sidebar_head`): the first row of the sidebar holds the
  collapse toggle, the app's name and a search button. In the icon-only sidebar the
  name hides and the buttons stack (`chrome::set_head_compact`).
- **Search:** the search button or <kbd>Ctrl</kbd>+<kbd>F</kbd> shows the entry in a
  slim strip over the page (`chrome::search_strip`); <kbd>Esc</kbd> clears and hides
  it. Where search filters the sidebar itself (Chat's chats), the entry sits under the
  sidebar head instead. On a page with its own filter field (a process list),
  <kbd>Ctrl</kbd>+<kbd>F</kbd> goes there.
- **Sidebar** (`.settings-navigation`, 250 px, `bg`, hairline on the right): nav items
  are an icon and a label (and an optional count in `dim_text`); hover is
  `@row_hover`, the active item `@nav_active` with no ring. Group labels are small
  UPPERCASE with a hairline above each group (none above the first). A group label
  can carry a flat `+` at its right. Settings is not in the sidebar.
- **Apps without a sidebar** put what the sidebar head would hold where it fits:
  Notepad's file menu, find and settings sit in its tab bar; Archive's Back sits at the
  start of the status bar.
- **Pages:** a stack, crossfading, built on first visit. Content pages scroll
  vertically with the scrollbar hidden and stay under ~920 px wide, centered.
  Table and grid pages fill the window: a hairline toolbar row on top (controls
  vertically centered, 7 px above and below), then the content edge to edge.
- **Status bar** (`.status-bar`, 34 px, hairline above, 0.86em `dim_text`): a flat
  `F1 Shortcuts` button on the left; on the right a live readout that matters for the
  app, then app-wide actions as small icon buttons (`chrome::status_action`: the
  equalizer, a temporary chat) and the settings gear last (`chrome::gear`).
- **Document editors** (Write, Sheets, Present) put a **command bar** above the
  document and an **Inspector** at its right (see Components). The bar starts with the
  File menu and ends with Find and the Inspector toggle. View switches (the outline,
  the chart pane, the sorter), focus mode and full screen sit in the status bar beside
  the zoom.
- **Settings** open as a **card over the window**, not a page (see Components).
  `navigate("settings")` opens it, so old links and `--section settings` still work.
- **Fullscreen** (players, viewers) hides the sidebar, search strip, status bar and
  app bars.
- **Collapsible sidebar:** the head's toggle and <kbd>Ctrl</kbd>+<kbd>B</kbd> collapse
  it to icons (about 52 px: labels, group labels and counts hide, tooltips stay, icons
  centered in the 40 px column). The choice is saved as `sidebar_collapsed`. The
  sidebar is compact when *either* the window is under 980 px *or* the user collapsed
  it.
- **Responsive:** nothing may be cut off or force a horizontal scrollbar at 800 px.
  Keep every page's minimum width under about 580 px: titles wrap, dropdowns ellipsize,
  groups of buttons flow onto two lines, toolbar labels drop before icons.
- **Keys every app shares:** <kbd>Ctrl</kbd>+<kbd>,</kbd> settings, <kbd>F1</kbd> and
  <kbd>?</kbd> the shortcuts, <kbd>Ctrl</kbd>+<kbd>F</kbd> search,
  <kbd>Ctrl</kbd>+<kbd>B</kbd> the sidebar, <kbd>Esc</kbd> clears or closes,
  <kbd>Ctrl</kbd>+<kbd>Q</kbd> quits, <kbd>Ctrl</kbd>+<kbd>W</kbd> closes the document
  or tab in document apps and the window elsewhere, <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>
  the palette (`nyx_ui::palette`: every page and named button in the window, so give
  buttons tooltips with their keys, "Find (Ctrl+F)"). `nyx_ui::keys` adds any an app
  doesn't handle itself.
- **Deleting:** prefer doing it at once with an Undo toast (hide the item, delete when
  the toast has gone) to asking first. Keep a confirmation only for what can't come
  back, like deleting files from disk.

### Required GTK sidebar construction

The sidebar must stay at the shared stylesheet's width, never grow to fill spare
space or become a fraction of the window. Use Music's sidebar structure:

```rust
let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
sidebar.add_css_class("settings-navigation");
sidebar.set_hexpand(false); // Required: expanding labels must not expand the sidebar.

let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
list.set_hexpand(false);
let navigation = gtk::ScrolledWindow::builder()
    .hexpand(false)
    .vexpand(true)
    .hscrollbar_policy(gtk::PolicyType::Never)
    .vscrollbar_policy(gtk::PolicyType::External)
    .child(&list)
    .build();
sidebar.append(&head.root); // chrome::sidebar_head(...)
sidebar.append(&navigation);

let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
content.add_css_class("settings-content");
content.set_hexpand(true); // Only the page column receives spare horizontal space.
```

- Explicitly disable horizontal expansion on the sidebar, navigation list and
  navigation scroller. A label with `hexpand(true)` otherwise propagates expansion
  up through its parents and can make the sidebar take half the window.
- Let shared `.settings-navigation` and `.settings-navigation.compact` CSS own the
  widths (250 px expanded; about 52 px compact). `set_width_request` sets a minimum,
  not a maximum, and is not a substitute for `set_hexpand(false)`. Do not add
  per-app width overrides, percentage widths or a resizable split for navigation.
- Use Music's nav buttons: 12 px between icon and label, an expanding ellipsized
  label marked `.compact-hide`, and a tooltip. In compact mode, hide labels,
  counts and group labels, center the button content, and call
  `chrome::set_head_compact`.
- Copy Music's resize handling: use the allocated window width (default width
  before allocation), compact below 980 px or when `sidebar_collapsed` is set,
  and respond to compositor resizing through the overlay resize probe. Keep its
  slow fallback for missed resize events. Fit the shared settings card on resize.
- Before installing a layout change, check this sizing behavior at a normal
  window width and at 800 px. A sidebar taking spare space or remaining expanded
  in an 800 px window is a defect, not an alternative layout.

---

## 7. Components

**Group:** an UPPERCASE heading, an optional note in `dim_text`, then **one card holding
all the group's rows** (`.group-list`: hairline border, radius 10, no fill), rows split
by `@divider` hairlines. Content with its own layout (tiles, a grid, an editor) goes in
a group without the card. Name each group's wrapper after its title
(`set_widget_name`), so the settings card can list it.

**Option row (`.settings-option`):** title and description on the left (they expand
and wrap), the control on the right, vertically centered. A row on its own outside a
card draws a hairline border, radius 10. A *stacked* row puts a wide control under its
text.

**Buttons:**
- **No text buttons unless absolutely necessary.** An action is an icon button with a
  tooltip naming it and its key ("New item (Ctrl+N)"). Text is for the few places an
  icon can't say it plainly: a dialog's confirm and cancel pair, a banner's one action,
  the second click of a dangerous action. Not toolbars, item headers, rows or lists;
- default: transparent with a hairline border, `@row_hover` on hover;
- primary (`suggested-action`): `@nav_active` fill, no border, weight 600, icon in `accent`;
- destructive: `danger` text on `alpha(danger, 0.12)` with a `danger` border;
- flat and bar buttons: no border, `dim_text` icon, `text` on hover;
- buttons on a banner: solid `accent` with `on_accent` text (`danger` on a warning
  banner), so the banner's action stands out on its tint.

Dangerous actions need two clicks: the first arms the button ("Click again to …"),
the second acts; it disarms after 4 s.

**Chip** (filters, + Spacer): a 28 px rectangle, radius 6, hairline border; selected
is `@nav_active` with no border. Not a pill.

**Segmented control:** a hairline track, radius 6, 2 px padding; the selected segment
is `@nav_active` with `text`. Prefer it to a dropdown when every option fits on one line.

**Switch:** a 44 × 26 pill with a hairline border and `alpha(muted, 0.3)` fill; the
18 px knob is `dim_text` when off. On, the track is `mix(bg, accent, 0.25)` and the
knob `accent`.

**Slider:** 4 px trough in `alpha(muted, 0.8)` with an `accent` fill and a 16 px knob.
The value readout sits to the right in tabular figures. No scale marks.

**Table** (`ColumnView`): transparent; header 34 px, labels 0.86em `dim_text`, a hairline
under the header and between header cells, the sort arrow in `accent`. Rows 35 px, no
striping, no row dividers, no rounded corners; hover `@row_hover`, selected
`@nav_active`. Reset the header button's font size to `1em` before sizing its label,
or GTK's own smaller size compounds.

**Settings card** (`nyx_ui::settings_card`; the app's `settings_dialog.rs` passes its
group table and settings page): a card over the window on a `@veil` backdrop,
up to 1040 × 720 and 24 px clear of the window edges, radius 14, hairline border.
- Left column (280 px; 200 px when the card is under 900 px wide): `SETTINGS`, a
  search field, then one entry per group (or per settings page): icon, bold title and
  a one-line summary in `dim_text` ("Theme, glow, motion"). The active entry is
  `@lift_active` with its icon in `accent`.
- Right column on `@lift`: a 64 px header with the group's title, an edit button for
  the config file and ✕, a hairline below; then the group's rows, its own heading hidden.
- Search filters every row across groups, shows each matching group with its heading,
  hides entries with no matches, and shows "No settings match “…”" when nothing does.
- <kbd>Esc</kbd> clears the search, then closes; a click on the backdrop closes.
- Other app-wide panels (the equalizer) use the same card with just a header and the page.

**Toolbar row** (`.toolbar`, `.list-toolbar`): 46 px, `0 12px 0 14px` padding, hairline
below. A toolbar of two rows (filters over a search) uses `.toolbar.stacked`: 10 px
padding and 10 px between the rows. A line of help above a list uses `.page-hint`
(12 px 14 px, hairline below, `dim_text`).

**Inputs:** transparent with a hairline border, radius 6, 32 px; the border turns
`accent` on focus, with no glow.

**Dropdown / popover:** 160 px minimum width; the closed dropdown ellipsizes. Popovers
use `surface`, a border, radius 10, 4 px padding, rows of radius 6 that hover to
`alpha(accent, 0.18)`.

**Tag:** a small label after a title for status that affects the whole row
("Restart needed", "Read-only"): 0.72em, bold, radius 4.

**Banner:** a full-width strip, radius 10, `alpha(accent, 0.10)` with a
`alpha(accent, 0.40)` border (warning: the same with `danger`), an icon, wrapped text
and an optional solid action button. Use it for status the user should act on.

**Toast:** bottom-centered over the window (above the status bar and any app bar), on
`surface`, radius 10; it replaces any toast showing and closes itself after 3.5 s
(6 s with an Undo button). Use toasts to confirm actions and report errors, never
modal alerts.

**Dialog:** a transient modal window (`.dialog-card`): `bg`, radius 14, 24 px padding,
a bold title, an optional mono code block on `alpha(shadow, 0.35)`, and buttons
right-aligned with the safe choice first ("Not now", then the action).

**Shortcuts list** (<kbd>F1</kbd>, also <kbd>?</kbd> where it doesn't clash): a dialog
with one card of rows, each an action and its key caps. Keep the list in one
`SHORTCUTS` constant that the settings Keyboard group reuses. Every app builds it the
same way (Fonts' `show_shortcuts` is the template):

- `widgets::dialog("Keyboard shortcuts", 520)`: the `section-title` heading, 520 px wide.
- One `.group-list` card of `widgets::row(action, "", key_caps)` rows. When the
  shortcuts are grouped (by context), each group gets an UPPERCASE `group-title`
  heading and its own card; the first heading has no margin above it.
- The list sits in a `ScrolledWindow` that scrolls without a scrollbar
  (`vscrollbar_policy(External)`, natural height up to 560 px), so no bar covers the keys.
- A plain **Close** button at the bottom right; <kbd>Esc</kbd> also closes. No other
  text in the dialog.

**Graphs** (Cairo on a `DrawingArea`): panel `mix(bg, surface, 0.5)`, radius 10, a
hairline border. Gridlines are `border` at 0.3–0.45 alpha, axis labels 11 px
`dim_text`, the line 2.5 px `accent` with a 0.14-alpha fill below it.

**Command bar** (`nyx_ui::commandbar`): one 44 px row of icons with a hairline below,
holding what each editor uses most: the File menu, undo and redo (`lead`), then groups
(`group(priority)`) of 30 px icon tools, `dim_text` at rest, `text` on hover, `accent`
on `nav_active` when on. Groups are split by 18 px dividers with 6 px either side. A
control that also lives in the Inspector is mirrored (`toggle_of`, `button_of`): the
bar's copy shows its state and clicks it, so the two never disagree. The bar's main
action is an accent square (`primary`, at most one); a choice of ways to do it (Play)
is an accent-outlined split button (`.commandbar-split`). The file's name sits in the
middle, its extension dimmed and an accent dot while unsaved; clicking it is Save as.
The bar never wraps: when it runs out of room whole groups fold into the `⋯` menu,
least important first, where each tool shows its name and keys; tools added with
`more` always live there.

**Inspector** (`nyx_ui::inspector`): a 288 px panel at the document's right, in
`theme_surface`, with the formatting for what's selected. A segmented switch at its top
picks a segment (Write: Text · Page · Review); contextual segments (Picture, Drawing,
Object) appear in `accent` and are picked while something matching is selected. Each
segment is a column of sections: an UPPERCASE 0.72em heading, then controls that fill
the row (`add`, `row`, `field` with an 84 px label column, `segmented` for joined
buttons like B I U). Nothing sits stranded in empty space. It docks beside the
document when the document leaves it room, and floats over it otherwise (Office first
narrows the sidebar to icons to make room); Esc closes a floating one. Colour buttons in
it carry a label beside their swatch. <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>I</kbd> (or <kbd>Ctrl</kbd>+<kbd>F1</kbd>)
shows and hides it, per editor (`inspector` in the settings: open by default, closed in
Sheets). The palette opens it at the segment of the control it presses.

**App icons** (`nyx-icons`): a 128 px canvas with a 112 px tile at 8, radius 26, in a
diagonal `#2a2f45` → `#16182a` gradient with a `#7aa2f7` hairline at 0.4 opacity. The
glyph is drawn in 6 px round-capped lines in a `#7aa2f7` → `#c58bd6` gradient inside
the 32–96 square, one idea per app. Add a glyph to `make_icons.py` and run it.

