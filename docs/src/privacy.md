# Privacy

drawbar is made by John Moore. Write to
[contact@drawbar.app](mailto:contact@drawbar.app) with any question about this page.

drawbar at [drawbar.app](https://drawbar.app/) sends two things: anonymous usage and
error counts, which you can turn off, and reports you write and send yourself. The
desktop app sends only the reports.

## Anonymous usage and error counts

These tell us how many people use drawbar, what breaks, and which instruments and
languages to support next. Each is one row of short codes. A row never holds a name,
a file, a sound, your address or anything that identifies you, and no row can be tied
to another, so we cannot follow one person from a visit to an error.

**To turn them off**, uncheck **Help ▸ Share anonymous usage**. drawbar also sends
nothing if your browser has Do Not Track or Global Privacy Control switched on.

drawbar keeps one value in your browser for these counts: the date of your last visit,
so a second visit on the same day is not counted as a new visitor. It is deleted when
you turn sharing off.

Every row carries `version`, the drawbar version. The rows are:

| Row | Sent when | Fields |
|---|---|---|
| `visit` | drawbar opens | `navigation` (a new page or a reload), `first_day` and `first_month` (whether this browser has visited today or this month), `webusb` (whether the browser can connect an instrument), `fits` (whether the window is large enough), `referrer` (the website that linked here, without the page), `language` (the browser's language, such as `de`) |
| `start_failed` | drawbar could not load | `step` (downloading or starting, or the browser lacking what drawbar needs), `error` (the kind of error) |
| `panic` | drawbar crashed | `location` (the line of drawbar's code that crashed), `model` and `firmware` of the connected instrument |
| `error` | something went wrong outside an instrument operation | `domain` (such as `usb`, `audio` or `library`), `kind` (such as `lost`), `model`, `firmware` |
| `op` | an instrument operation failed, or one you asked for finished | `op` (such as `put`), `class` (programs, samples and so on, by name), `outcome` (`ok` or the kind of failure), `took` (under one second, ten seconds, a minute, or longer), `model`, `firmware` |

The server adds `browser` (such as `chrome 141`) and `os` (such as `macos`), which it
works out from your browser's user agent and then discards. To `visit` rows only, it
adds `country`, from the network, to help choose which languages to translate drawbar
into. It does not store your IP address, and it keeps no logs.

Rows are kept for three months. Totals made from them, such as visits per month, may be
kept longer; they describe everyone, not anyone.

## What stays in your browser

drawbar.app keeps your library in your browser, and sends none of it anywhere:

- The origin private file system holds the browser's own library: your sounds,
  their tags and folders, and the record of each change. For every library you
  open, a folder on your computer included, it also holds the unsaved edits, and
  the copy of a sound drawbar takes before writing over it on your instrument.
- An IndexedDB database named `drawbar` holds the folders on your computer you
  opened lately, as handles the browser gives drawbar. Unless you allowed a
  folder on every visit, the browser asks you again before drawbar can open it.
- Local storage holds drawbar's settings, and the sounds drawbar 0.10 and
  earlier kept under `drawbar.this_computer`, `drawbar.folders` and
  `drawbar.tags`. `drawbar.taking_in` and `drawbar.taken_in` record that a
  library took those sounds in: their hash, and where each sound that had an
  unsaved edit was put.

[Where your library lives](drawbar/this-computer.md#where-your-library-lives)
says what can delete it.

## Reports you send

**Help ▸ Report a problem** and **Help ▸ Send feedback** send what you write, and
an email address if you give one. You choose what to attach: the recent errors, what
build and instrument you are running, and the activity log, which names your files.
The form shows exactly what will be sent before you send it. These are sent even when
sharing is off, because you asked.

Reports are emailed to drawbar's developer and are never published. They are deleted
after 90 days. When a report is sent, drawbar shows its number. Write to
contact@drawbar.app with that number, or the email address you gave, to see what we
hold or to have it deleted sooner.

## Who else is involved

[Cloudflare](https://www.cloudflare.com/) receives both and stores the counts.
Reports are delivered by email to [Fastmail](https://www.fastmail.com/), which holds
them. Both act for us under their data processing terms. Nothing is sold or shared with
anyone else.

If you are in the European Union or the United Kingdom and are not satisfied with our
answer, you may complain to your data protection authority.
