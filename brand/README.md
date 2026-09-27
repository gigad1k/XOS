# XOS brand

Two files, one rule.

| File | Use |
|---|---|
| `xos-mark.svg` | The mark alone. Favicons, window icons, anywhere 24px or smaller. |
| `xos-logo.svg` | Mark plus wordmark. Headers, the README, documentation. |

## The rule

**The mark is monochrome and stays that way.**

XOS has exactly three colours and each one means a single thing:

| | Means |
|---|---|
| `#7A9B6E` | running on the local model. Free, private. |
| `#C8934A` | a cloud API is in use. Costing money. |
| `#A85445` | blocked, failed, needs you, over budget. |

A logo painted in any of them would be making one of those statements
permanently, which is exactly what `STYLE.md` forbids: *never use a semantic
colour decoratively*. So both files draw in `currentColor` and take the colour
of whatever they sit on. One file works on the dark theme, the light theme, a
terminal, and a README on a white background.

## The mark

A registration mark — the thing an instrument is lined up against. It fits the
rest of the interface, which is built to look like lab equipment rather than a
consumer app, and it survives being shrunk to a favicon because the middle is
left empty.

An X in a rounded square was drawn first and thrown away: every interface in
the world uses that to mean *close*.

## Don't

- Don't recolour it, including "just for the logo".
- Don't add a gradient, a glow or a shadow. `STYLE.md` bans all three.
- Don't rotate it. It is a registration mark; its axes are the point.
- Don't put the mark and the wordmark next to each other by hand — use
  `xos-logo.svg`, which has the spacing in it.
- Don't set the wordmark in anything but IBM Plex Sans 500.

## Sizes

The mark is drawn on a 24px grid and is legible down to 14px. Below that, use
the word.
