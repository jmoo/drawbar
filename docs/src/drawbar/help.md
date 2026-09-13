# Help and About

The **Help** menu holds four items: **User guide**, **What's new**, **Copy activity
log** and **About drawbar**.

## User guide

**User guide** opens this guide in a new browser tab. In a browser, it opens the
guide published beside the copy of drawbar you are using; the desktop app opens
<https://drawbar.app/docs/>.

## What's new

In a browser, drawbar opens a notice the first time you run each new version. It
shows the version, the alpha notice, a list of what to expect of this build, and the
release notes for this version.

The alpha notice reads *Use at your own risk, this is alpha software.* Each
expectation is marked supported, not yet tested on a real instrument, or not
supported. They cover:

- which piano, sample and Electro 5 files drawbar can create, edit and transfer;
- the Electro 5's USB support;
- what has not been tested on hardware: playback of edited nsmp3 and nsmp4 files,
  and Nord Stage 2, 3 and 4 programs and presets;
- that USB support for instruments other than the Electro 5 cannot be guaranteed;
- that long-term storage in drawbar is not guaranteed while it is in alpha, so back
  up your files elsewhere.

The release notes are read from the version's GitHub release. Links in them open in
a new tab, and **Release page** opens the release itself. If the notes cannot be
read, the notice says so and links to **Releases**. On a wide window the notes sit
beside the notices; on a narrow one they sit below.

**Continue**, or Escape, closes the notice. drawbar remembers the version you have
seen, so the notice does not open again until the next version. **Help ▸ What's
new** opens it again at any time.

The desktop app has no notice. There, **What's new** opens this version's release
page in your browser.

## Copy activity log

**Copy activity log** puts the whole activity log on the clipboard, for a bug report.

## About drawbar

**About drawbar** shows, on every target:

- the version running, and what drawbar is;
- links to **Source on GitHub**, the **User guide** and **Releases**;
- **Licences**: every licence whose notice has to travel with a copy of drawbar.
  drawbar's own BSD 3-Clause licence comes first, then the bundled fonts (Ubuntu,
  Hack, Noto Emoji and egui's emoji icon font), the Lucide icons, and the Rust crates
  compiled in, grouped by licence. Click a row to read its full text;
- the trademark disclaimer.

The licence texts are compiled into the app, so a copy of drawbar carries the terms
it is under. **Close**, or Escape, closes the box.
