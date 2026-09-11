# PART 1 — STYLE GUIDE

## 1.1 Design thesis

XOS is a system you supervise. Its visual language is **instrumentation** — studio
hardware, lab equipment, control surfaces — where every marking reports state rather than
decorates.

**The governing constraint: XOS has no decorative colour.** Three hues exist, each means
exactly one thing, and nothing else is ever coloured. This is what makes the interface
readable at a glance and what makes it look unlike every other AI product.

Deliberately rejected: near-black backgrounds with a single acid accent, glowing gradients,
purple AI-brand tropes, rounded card kits, all-caps eyebrow labels.

## 1.2 Palette

| Token | Hex | Use |
|---|---|---|
| `--xos-base` | `#262523` | Root background. Warm graphite — a faceplate, not a void. |
| `--xos-surface` | `#302E2B` | Panels, cards, raised elements |
| `--xos-surface-hi` | `#3A3835` | Hover, selected, input fields |
| `--xos-line` | `#454340` | Hairlines, dividers, borders |
| `--xos-text` | `#E4E1DB` | Primary text. Warm off-white, never pure white. |
| `--xos-text-dim` | `#8F8B84` | Labels, secondary, metadata |
| `--xos-text-faint` | `#66625C` | Disabled, placeholder |

### Semantic colours — the only colours permitted

| Token | Hex | Means, and only means |
|---|---|---|
| `--xos-local` | `#7A9B6E` | Running on local model. Free. Private. |
| `--xos-api` | `#C8934A` | Cloud API in use. Costing money. |
| `--xos-stop` | `#A85445` | Blocked, failed, needs you, over budget. |

**Rules:**
- Never use a semantic colour decoratively
- Never introduce a fourth hue
- Never colour a heading, border or icon for emphasis alone
- Emphasis is done with weight and `--xos-text` versus `--xos-text-dim`

### Light variant (optional, ships as alternate)

`--xos-base #E8E6E1` · `--xos-surface #F2F0EC` · `--xos-line #C9C5BE` ·
`--xos-text #2A2926` · `--xos-text-dim #6E6A64`. Semantic hues darken ~15% for contrast.

## 1.3 Typography

| Role | Face | Notes |
|---|---|---|
| UI, labels, prose | **IBM Plex Sans** | 400 and 500 only. Never 600+. |
| Machine data | **IBM Plex Mono** | Numbers, paths, code, IDs, tokens/sec |

Both are open source and share a design language, which is why they're chosen over the
usual Inter default.

**Mono is functional, never decorative.** It appears when content is machine-produced. A
label reading "vram" is sans; the value "3.1 / 8.0 GB" is mono. Getting this backwards is
the commonest generated-design tell.

**Scale** — tight, instrument-panel density:

| Use | Size / line-height |
|---|---|
| Status bar | 11px / 1.2 |
| Dense data rows | 12px / 1.4 |
| Body | 14px / 1.55 |
| Section heading | 16px / 1.3, weight 500 |
| Screen title | 20px / 1.25, weight 500 |

Sentence case everywhere. No all-caps labels. No tracked-out eyebrows.

## 1.4 Layout

**Density over airiness.** Generous whitespace is a consumer-app value; this is a control
surface. Information packs tightly and aligns to a grid.

- Base unit 4px. Gaps 4 / 8 / 12 / 16. Section spacing 24.
- Corner radius **3px** everywhere. Not 12px. This is a faceplate, not an app.
- Borders `1px solid var(--xos-line)`. No shadows anywhere — flat surfaces only.
- Numeric columns right-aligned and tabular (`font-variant-numeric: tabular-nums`) so
  digits don't jitter as values update.
- Labels left, values right, hairline between rows.

## 1.5 Motion

Almost none. **Motion is information, same as colour.**

The only permitted animations:
- Tier flip (local ↔ api): 180ms colour crossfade
- Node state change in a graph: 150ms
- Panel expand/collapse: 160ms ease-out

No entrance animations. No hover transitions on cards. No loading shimmer — show the
actual state instead.

## 1.6 Where boldness is spent

**The status bar.** It is the signature surface and the thing people screenshot.
Everything else stays quiet and disciplined.

## 1.7 Applying the palette per surface

| Surface | Implementation |
|---|---|
| Plymouth splash | `--xos-base` background, wordmark in `--xos-text`, no spinner — a single hairline progress rule |
| Hyprland | `--xos-line` inactive border, `--xos-local` active border, 3px radius, 6px gaps |
| waybar | `--xos-base`, 11px Plex Sans, semantic hues per §1.2 |
| Terminal | `--xos-base` bg, Plex Mono, ANSI palette derived from the three semantic hues plus greys |
| TUI (`xos chat`) | Same ANSI palette; box-drawing rules in `--xos-line` |
| Open WebUI | Custom CSS overriding their variables. **Open WebUI name and logo stay** — licence requires it above 50 aggregate users. |
| Mission Control | Full token set; dense data rows |

## 1.8 Copy voice

Plain verbs, sentence case, active voice. Errors state what happened and what to do, with
no apology. Empty states are an invitation, not a shrug. A button that says "Approve"
produces a state that says "Approved."

Never: "successfully", "please", "simply", exclamation marks, → appended to buttons.
