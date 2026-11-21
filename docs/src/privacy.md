# Privacy

Your sounds, tags, folders and edits stay in the library you chose. In the
browser, drawbar sends anonymous usage and error counts unless you turn them
off. On the desktop, the only reports sent are ones you choose to submit.

## Anonymous usage and error counts

To turn off counts at drawbar.app, uncheck **Help ▸ Share anonymous usage**.
drawbar also honors Do Not Track and Global Privacy Control. It keeps the
date of your last visit in your browser to count return visits. Turning
sharing off deletes that date.

Counts help the developer decide which problems to fix and which instruments
and languages to support. They contain no filenames, sounds, email addresses
or personal identifiers. There is no identifier to link one visit to an error
or another visit. Individual rows are kept for three months. Aggregate totals,
such as monthly visits, may be kept longer.

## Reports you send

**Help ▸ Report a problem…** and **Help ▸ Send feedback…** send what you write
and an email address if you give one. You decide whether to include recent
errors, details of the build and connected instrument, and an activity log
that names your files. A problem report starts with all three attachments
selected. Feedback starts with none. Review the preview before sending.
The form works even when usage sharing is off.

Reports are emailed privately to drawbar's developer and deleted after 90
days. To see your report or have it deleted sooner, give the report number
shown after sending, or the email address you supplied. For reports and other
privacy questions, contact John Moore at [contact@drawbar.app](mailto:contact@drawbar.app).

## What stays in your browser

- **Library and drafts.** The browser's private file storage holds its own
  library of sounds, folders, tags and change history. It also keeps drafts
  and the recovery copies made before overwriting instrument slots, including
  when you're working from a folder on disk.
- **Recent folders.** Your browser keeps references to folders you've opened
  in an IndexedDB database named `drawbar`. Unless you granted continuing
  access, it asks again before opening a folder on a later visit.
- **Settings and older libraries.** Local storage holds your settings. For
  libraries from drawbar 0.10 and earlier it can also hold sounds, folders and
  tags, and a record of which files and drafts were brought into a library.

Clearing browser storage can delete your work. [Keep a copy of your library](drawbar/backup.md#keeping-a-copy-in-the-browser).

## Fields in an anonymous count

Every row includes the drawbar `version`.

| Row | Sent when | Fields |
|---|---|---|
| `visit` | drawbar opens | `navigation` (a new page or a reload), `first_day` and `first_month` (whether this browser has visited today or this month), `webusb` (whether the browser can connect an instrument), `fits` (whether the window is large enough), `referrer` (the website that linked here, without the page), `language` (the browser's language, such as `de`) |
| `start_failed` | drawbar could not load | `step` (downloading or starting, or the browser lacking what drawbar needs), `error` (the kind of error, such as `TypeError`, or `WebGL` when the browser cannot provide it) |
| `panic` | drawbar crashed | `location` (the line of drawbar's code that crashed), `model` and `firmware` of the connected instrument |
| `error` | something went wrong outside an instrument operation, or you closed the browser's device list without choosing a device | `domain` (such as `usb`, `audio` or `library`), `kind` (such as `lost`, or the step at which connecting stopped), `model`, `firmware` |
| `op` | an instrument operation failed, or one you asked for finished | `op` (such as `put`), `class` (programs, samples and so on, by name), `outcome` (`ok` or the kind of failure), `took` (under one second, ten seconds, a minute, or longer), `model`, `firmware` |

The server reads your browser's user agent to add `browser` (such as
`chrome 141`) and `os` (such as `macos`), then discards the user agent. It
adds `country` from the network to `visit` rows only to help prioritize
translations. It does not store IP addresses or keep server logs.

## Who else is involved

[Cloudflare](https://www.cloudflare.com/) receives both and stores the counts.
Reports are delivered by email to [Fastmail](https://www.fastmail.com/), which holds
them. Both act for us under their data processing terms. Nothing is sold or shared with
anyone else.

If you are in the European Union or the United Kingdom and are not satisfied with our
answer, you may complain to your data protection authority.
