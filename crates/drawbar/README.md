# drawbar

An [egui](https://github.com/emilk/egui) app over
[`nord-format`](../nord-format) and [`nord-usb`](../nord-usb) — everything
[`nord-cli`](../nord-cli) can do that is worth a window, reachable from a browser
tab or a desktop one. Same job as the CLI: **dogfood the libraries**, not ship a
product. The engineer's verbs stay in the CLI.

It is named for the nine-drawbar organ register widget in its editor.

> [!CAUTION]
> # ⚠️ 🧱 USE AT YOUR OWN RISK — THIS TOOL CAN BRICK YOUR DEVICE 🧱 ⚠️
>
> Until this project reaches 1.0, **do not use this tool to speak to a real Nord
> device** unless you are using it to actively develop these libraries.
>
> **I am not responsible for what you do with this tool.**

## What this build does

The window is a file browser over the places your sounds live:

| region | what it holds |
|---|---|
| **Storage** (left) | **This computer** — dropped or opened files, copies pulled off the instrument, fresh defaults. **Connect instrument** sits beside Open… and New; until an instrument answers, this is the whole sidebar. Attached, the **instrument** takes a column of its own alongside — folders (Programs, Set lists, Samples, Pianos, Live, Settings) filling in by themselves — so moving a sound between the two is a short drag. Each column scrolls on its own, and the divider between them is dragged to give one more room than the other; double-click it for an even split, and where it was left is kept between sessions |
| **Tabs** (centre) | one document per thing you opened. Double-click anything in the sidebar to open it; something on the instrument is copied here first. Each has a **Basic** and an **Advanced** face |
| **Status strip** (bottom) | one line about what just happened, and a spinner while something is running. Click it for the full activity log, protocol detail and all |

**Light or dark, whichever the machine is using.** The button at the top right says
which — *auto* follows the desktop's or the browser's own setting and changes with it
mid-session; clicking cycles to a held light, a held dark, and back to auto. The choice
is kept between sessions.

### This computer

Files arrive by drag-and-drop or **Open…**, and leave through **Export…** on a
row's context menu or the document header. Every file is decoded and immediately
re-encoded to check the bytes come back identical; a file that fails to decode
still gets a row, because reporting a bad file is the point of opening it.
**New** makes a fresh program, live or settings document.

**The list is kept between sessions.** What is on this computer is written to the
browser's own storage (or a small file beside the app natively) and comes back on
the next start, re-decoded and re-checked on the way in. Edits are kept without
being asked for. Two limits, both said out loud when they bite: a single sound
over about a megabyte is not kept — a sample runs to megabytes and one would fill
the store — and the whole list is capped well under the browser's quota. Removing
a row removes it from the store.

### The instrument

**Connect instrument**, beside Open… and New, opens the device (a chooser in the
browser, the first attached Clavia natively) and reads what it holds; the instrument's
own column appears with it, and goes away again when it is released. Each folder is read in **one session**,
the way Nord Sound Manager does it — the session is opened once and every slot
read inside it — so connecting no longer walks the instrument's display through
an open and a close per bank. Names fill in as each bank lands, and the folder
heading says how far it has got. Nothing asks for a bank number. **Read again**
re-walks one folder; anything you change is re-read on its own, and a change made
on the panel while the app is attached drops every cached name and reads them
again.

A folder is **one flat list**, each row labelled the way the panel and the CLI
label a slot — `7:4  Africa Split`. Empty slots are rows, not absences: they are
places something can be dropped. Pianos are listed and nothing more — a piano
library is hundreds of megabytes, and the folder offers no way to pull one down.

Writing into the slot the instrument currently has loaded leaves the panel
playing what it read before the write, so after a put or a rename that touches
it, the app asks the instrument to load that slot again. It can only do this for
a slot **it** selected: a selection made on the panel itself is invisible to the
host.

**An instrument that goes away says so.** Pull the cable and the column goes with
it — in a browser the moment the tab is told, and on either target the first time
an operation finds nothing on the other end of the pipe. A refusal is not a
disconnection: the instrument answering "no" is an instrument that is still
there. What was waiting to be sent stays waiting, so plugging back in and
pressing **Send all** picks up where you were.

### Moving things

Drag between the two places, or within one:

- **instrument → This computer** copies it here.
- **This computer → a slot** sends it. An occupied slot asks once — *Replace
  “Squabble B” in Programs 7:4 with “Africa Split”?* — and an empty one just
  does it.
- **slot → slot, inside one folder** rearranges them. The instrument swaps the
  two, so nothing is lost and nothing asks.

A target that cannot take what you are dragging does not light up; drop on it
anyway and the status strip says why.

Clicking anywhere in a row selects it — the whole row is the target, not the
words in it.

Right-click a row for Open, Copy to this computer / Export…, Load on instrument,
Rename, Duplicate, Delete… and Remove from list. Click the **name** of a row that
is already selected — or press F2 — to rename it in place; **Enter** renames and
anything else leaves it alone. Renaming on the instrument happens straight away,
because renaming it back is its own undo.

Progress is painted on the *instrument's own display* by the operations
themselves — there is no host-side progress callback, so the app shows a spinner
and does not invent one.

### Safety

- **Read-only until you say otherwise.** There is no armed mode: a destructive
  session exists only for the single operation the app released.
- **Replacing and deleting ask first, by name.** Nothing else does — a move
  loses nothing and a rename undoes itself.
- **Move is a swap, not an overwrite** — the destination's occupant ends up in
  the source slot, byte-identical. Confirmed on hardware.
- **A put is a delete followed by a write** — the instrument refuses to overwrite
  in place. The occupant is read into memory first and written back if the write
  fails; if the restore fails too, its bytes land on This computer as a rescued
  entity rather than being lost.
- **Live and settings refuse a write.** Whether either survives a delete of its
  own class is unconfirmed on hardware, so an edit of either stops at a file.
- **Every session closes, including on the error path.** An abandoned transaction
  strands the instrument on its progress screen with no way out but a power
  cycle.
- **One operation at a time.** What you asked for always goes ahead of the
  background read of the tree, so a click never waits on it. Reading a whole
  folder is one operation, and its progress arrives while it is still running.

### The document

A tab holds a working copy. Its header says where it came from; **Revert** goes back to
the bytes the tab opened with — the only undo there is — and **Export…** writes the file.

A name is this app's own: a Nord file stores none. It is set when the bytes arrive —
from the slot the instrument read it out of, from the filename, or from the New menu —
and nothing after that re-derives it. It is kept **verbatim**: "Big strings" is
"Big strings" in the list, in the tab, and on the slot a send renames. Only the
filename an **Export…** suggests is made path-safe, because that is the one place a
name meets a filesystem.

Looking at a document and changing it are the same thing. There is no Apply: a control
you move is set on the working copy that instant, the bytes are re-encoded and
re-checked, and the tab picks up a dot.

**Basic** is the front panel, and it is laid out like one: **knobs** for the values that
sweep, **lamps** for the switches, names printed *under* what they name, and controls
running left to right in strips that wrap rather than down a column of form rows.
Sections are titled boxes — Keyboard & split, then Organ, Piano, Sample, Effects, EQ —
and none of them folds away, because a control you cannot see is a control you do not
know you have. **Only the engines a part is actually playing
are there**: set both parts to piano and the organ section goes, and the part pickers
that bring it back are in Keyboard & split, which leads because it never goes.

**A document shows what the keyboard would be showing.**

- The organ section carries the model picker and **only the selected model's**
  registrations: two nine-drawbar presets for the B3 with its vibrato and percussion,
  the Vox's bars and vibrato, the Farfisa's registers (which the instrument reads as
  on/off tabs at 5 and above), the pipe organ's bars and nothing else. The preset the
  instrument is playing is marked, and clicking the other one switches it.
- In **b3+bass**, preset 1 is the bass manual: two drawbars, in their own fields. The
  nine nibbles they shadow hold stale leftovers and are not shown at all.
- **Transpose is one control**, a lamp and a number written together, because that is
  what the panel's button is. The instrument ignores the amount while the lamp is dark,
  and moving the number lights it.
- **The piano section names the piano** where the instrument can name it. A program
  stores its piano as an id and the panel's own dial position, and no file anywhere
  carries the name — so with an instrument attached the document reads the program's
  dependencies as it opens and the name appears on its own; *Ask again* re-reads it, and
  *Ask the instrument* is there for a read that found nothing. Without an instrument the
  id is shown and says as much.
- **The Model dial is a list of pianos** once the instrument's Pianos folder has been
  read: the scanned names of the document's category, each at its dial position. The
  bank ↔ category reading is inferred rather than hardware-confirmed, so where the
  dependency reply and the list disagree, the document says so and trusts the
  instrument. Unscanned or unattached, the dial stays a number.
- A picker offers only values the library can name. A file holding one it cannot reads
  as *unrecognized value (6)* and keeps it in the list, so changing away from it can be
  undone.
- Reserved bits, unexplained fields and the library ids that name a program's piano and
  sample are not offered here. They are all in Advanced.

**Drawbars** are the widget the crate is named after: pull down to draw out, with the
positions in digits underneath (`88 8800 000`). No hex anywhere in Basic.

**Every control answers the keyboard.** Tab reaches them; a knob steps on the arrows,
jumps ten on Page Up/Down and goes to its stops on Home/End; a lamp switches on Space or
Enter. A knob is dragged up to open it out, and double-clicking one opens its number for
typing — what is typed is clamped to the stops, because a knob cannot be turned past
them.

Sample instruments show their name and the stretch of keyboard each zone covers, with
root key and top note as note names (`C4` is middle C). Only v2 `.nsmp` content can be
changed; nsmp3/nsmp4 is carried verbatim.

### Sending changes back

Editing something you copied off the instrument does not write to it. The document is
marked **pending** instead — the tab and its row say *will be sent to Programs 7:4* —
and it goes nowhere until you say so.

- **Send all (n)**, beside the instrument's name, writes everything waiting. It asks
  once, listing every destination and naming what each one replaces, and then writes
  each folder's worth inside a single session. Progress arrives item by item.
- **Send to Programs 7:4** in a document's header sends that one, for when a batch is
  more ceremony than the job needs.
- If an item is refused the batch stops there. What was already written stays written,
  the report says how far it got, and the rest stay pending.
- **Cmd/Ctrl+S** marks the open document pending. For something that only lives on this
  computer it says so and does nothing, because those edits are already kept. It is
  never a file-export shortcut — that is Export….

**The name goes with the bytes.** A write does not carry one — the instrument names
the slot itself, from an argument this app does not choose — so every write is
followed by a rename, inside the same session, to whatever the sound is called
here. The format tag comes off on the way: `Africa-Split.ne5p` on this computer is
`Africa-Split` on the panel. A rename that will not go is said out loud and
nothing more; the bytes are in the slot either way.

### Advanced

The other face of every document, and the only face for something with no friendly view
(a file that did not decode, a set list).

It is the whole body as a table: one row per field the library declares — label, path,
bit placement, decoded value, and the stored spelling in a cell you can type into.
Nothing is hidden and nothing is prettied up. Reserved bits, both halves of the
transpose pair, the library ids and the wide hex registers are all here, in declaration
order, with a filter box over the names. A value the library refuses says so beside the
cell and leaves what you typed where it is. `unknown (9)` is a legal spelling here —
this is the engineer's view.

Under the table sits the record: the verify badge, the CBIN header numbers, the bytes
that moved since the tab opened (with the checksum rows annotated so they do not read as
a second edit nobody made), the `{:#?}` dump, and — for something read off the
instrument — the slot's own info and dependency list, on request.

### What lives in `nord-cli` instead

Raw browsing by class number and the sweep-capture tool are an engineer's
`nord raw` and `nord --sweep`; they are not in this window.

## Build and run — native

```sh
nix develop            # from the lab root: cargo, rustc, lld
cd crates
cargo run -p drawbar
```

## Build and serve — web

Everything below runs from `crates/`. The `--target wasm32-unknown-unknown`
builds **must** be run from here or below: `crates/.cargo/config.toml` supplies
`--cfg=web_sys_unstable_apis`, without which WebUSB does not exist in `web-sys`,
and Cargo only finds that file by walking up from the working directory.

```sh
cd crates
nix develop /path/to/lab

cargo build -p drawbar --lib --target wasm32-unknown-unknown --release

nix run nixpkgs#wasm-bindgen-cli -- \
  --target web \
  --out-dir drawbar/pkg \
  target/wasm32-unknown-unknown/release/drawbar.wasm

cd drawbar && python3 -m http.server 8000
```

Then open <http://localhost:8000/>. `localhost` counts as a secure context, so
WebUSB is available without TLS; `file://` is **not** — the module import and
WebUSB both fail there.

> [!IMPORTANT]
> `--lib` is not optional. The crate also has a binary target of the same name,
> so building both for wasm writes two different modules to the same
> `target/wasm32-unknown-unknown/*/drawbar.wasm`. Cargo warns about the collision
> and does not define which one survives; when it is the binary's stub `main`,
> `wasm-bindgen` emits a package that exports nothing.

⚠️ **`wasm-bindgen-cli` and the `wasm-bindgen` crate must be the same version.**
The CLI refuses a module built by any other one. nixpkgs currently ships
**0.2.121**, which is what this crate pins and therefore what the whole
workspace lock holds:

```sh
grep -A2 'name = "wasm-bindgen"' Cargo.lock
nix run nixpkgs#wasm-bindgen-cli -- --version
```

If those ever disagree, move the pin in both `nord-web-demo/Cargo.toml` and
`drawbar/Cargo.toml` to whatever the CLI reports and re-run `cargo build`. eframe
resolves against the pin rather than forcing it, so the pin is the one to follow.

## Browser support

| Browser | Works |
|---|---|
| Chrome | yes |
| Edge | yes |
| Firefox | no — WebUSB declined |
| Safari | no — WebUSB declined |

**Close Nord Sound Manager before connecting.** It claims the vendor interface
exclusively, and nothing else — this app, `nord-cli`, or Chrome — can attach
alongside it.

## Disclaimer

Not affiliated with, authorized, or endorsed by Clavia DMI AB. "Nord", "Clavia",
and "Electro" are trademarks of Clavia DMI AB, used here only to identify the
hardware these formats come from.
