# Where these came from

Every image here is a real run. None is a mockup, and none has been retouched.

| File | What it is | How it was taken |
|---|---|---|
| `mission-control.jpg` | Mission Control, with a real completed goal, this machine's GPU and disks, and the routing table | A browser screenshot of `http://127.0.0.1:7783` served by a running `xosd`. |
| `chat-tui.jpg` | `xos chat`, answering a question on the local model at 119 tok/s | See the note below. |
| `installer.png` | The unattended install partitioning a disk, running pacstrap and applying the XOS layer to `/mnt` | A QEMU framebuffer dump, taken with QMP `screendump` while the install ran. |
| `installed-boot.png` | The disk that install produced, booting with no medium present | The same, on a second boot of the disk alone. |
| `boot-menu.png` | The medium's boot menu, including the unattended entry | The same, six seconds into the boot. |

## The one that needs a caveat

`chat-tui.jpg` is not a photograph of a terminal window, because nothing here
runs a desktop to photograph one in.

`xos chat` was run on a real pty of a fixed size with `script(1)`, which records
every byte it wrote. Those bytes were then replayed onto a grid — cursor moves,
colours and all — exactly as a terminal would, and the result rendered in a
browser. The characters, the layout and the colours are the TUI's own; only the
glyph rasteriser is the browser's rather than a terminal's.

The alternative was a photograph of a QEMU console at 640×480, which would have
been a worse picture of the same thing.
