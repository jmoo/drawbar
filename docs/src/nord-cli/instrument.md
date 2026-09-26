# The instrument

Back up first. Nord Sound Manager makes a full backup, and it must be closed
before `nord` can connect, since it keeps the USB connection to itself.
[What is supported](../getting-started/support.md) says which instruments have
been tested.

```sh
nord device status                      # what the instrument holds; --json for scripts
nord device info                        # what is attached
nord device recover                     # release a session an interrupted run left open

nord program list                       # every occupied slot
nord program focus                      # what the panel has loaded
nord program get 7:4                    # print the summary
nord program get 7:4 -o patch.ne5p      # or save the file
nord program put patch.ne5p 7:4 --yes   # send a file to a slot
nord program move 8:13 7:16 --yes
nord program duplicate 7:2 7:3 --yes
nord program rename 6:13 "Warm Pad" --yes
nord program delete 7:50 7:49 --yes
nord program select 2:12                # load it on the panel
nord program info 7:4                   # size, format, name, checksum
nord program deps 7:4                   # the piano and sample it uses, by name
```

The same verbs work on `setlist`, `sample` and `piano`. `live` takes `get`,
`info`, `deps` and `edit`, and `settings` takes `get`, `info` and `edit`.

## Before anything changes

A command that changes the instrument reads the slot first, says what it is
about to do, and asks:

```
$ nord program duplicate 7:2 7:3
duplicating "Africa Split" from bank 7 slot 2 to bank 7 slot 3 — OVERWRITING "Squabble B"
proceed? [y/N]
```

`move` reports a swap rather than an overwrite, because the instrument exchanges
the two slots and nothing is lost. It also lists the set lists that point at the
program, since the instrument updates them to follow it. `put` names the slot
after the file.

`--yes` skips the question. Without a terminal to ask on, the command stops
with `refusing to proceed without --yes`. There is no `--force`, and nothing
skips the read.

## What a write does

The instrument does not overwrite an occupied slot in place, so a `put` into one
deletes the old sound first. `nord` reads the old sound before deleting it, and
puts it back if the write fails. If that fails too, the old bytes are saved in the
working directory as a file such as `nord-rescued-7-50.ne5p`, which `put` takes
straight back. Live slots and settings are the exception: the instrument
overwrites those in place.

Every command closes its session even when it fails, so an error cannot leave the
instrument stuck on its progress screen. If a run is interrupted, `nord device
recover` releases the session.
