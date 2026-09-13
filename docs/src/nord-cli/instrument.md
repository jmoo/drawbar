# Talking to an instrument

Close Nord Sound Manager first — it claims the vendor interface exclusively, and
`nord` cannot attach alongside it.

> ⚠️ **Back up your instrument before writing to it.** Nord Sound Manager makes
> a full backup. This is alpha software driving real hardware over a
> reverse-engineered protocol; what is hardware-verified and what is not is
> listed under [USB protocol](../reference/usb-protocol.md).

```sh
nord device status                      # inventory per class; --json for machines
nord device info                        # what is attached, from the USB descriptors
nord device geometry                    # partitions, banks and slot capacity, from the device
nord device recover                     # release a session an interrupted run left open

nord program list                       # every occupied slot, walked with the device's own cursor
nord program focus                      # what the panel has loaded
nord program get 7:4                    # summary to stdout
nord program get 7:4 -o patch.ne5p      # write the .ne5p instead
nord program put patch.ne5p 7:4 --yes
nord program move 8:13 7:16 --yes
nord program delete 7:50 7:49 --yes
nord program info 7:4                   # size, format, version, name, crc32
nord program deps 7:4                   # piano/sample dependencies, with names
nord program select 2:12                # load live on the instrument
nord program rename 6:13 "foo" --yes
nord program duplicate 7:2 7:3 --yes
```

The same verbs work on `nord setlist`, and on `nord raw` with an explicit class:

```sh
nord setlist get 1:1
nord raw --class 1 info 1:1             # `nord piano info 1:1`, by class number
nord raw --class 5 get 1:1 --body -o setlist.body
```

`--body` saves the wire body verbatim instead of wrapping it in a CBIN header.
Use it for classes whose header layout is not known, where the wrapped file would
look plausible and be wrong.

## Safety

Every mutating command **reads the slot first, says what it will touch, then
refuses without `--yes`**. Off a terminal, running it without the flag is a real
dry run:

```
$ nord program duplicate 7:2 7:3
duplicating "Africa Split" from bank 7 slot 2 to bank 7 slot 3 — OVERWRITING "Squabble B"
error: refusing to proceed without --yes
```

`duplicate` names the *destination's* current occupant because that is what is
about to be lost. `move` names it too, but as **SWAPPING WITH**: the instrument
exchanges the two slots, so nothing is lost — and calling that an overwrite would
invite deleting the one copy the swap preserves. A `move` also lists the set
lists that reference the program, because the instrument rewrites them to follow
it, and a factory set list is migrated to the current version by that rewrite,
irreversibly.

`put` names the slot after the file's stem, the way Nord Sound Manager does, and
says so in pre-flight. Rename is refused on the library classes, so a sample's
name is fixed at write time.

Two other guards worth knowing about:

- An empty slot reports `bank 5 slot 42 is empty` rather than a raw status code.
- Every command closes its transaction **even when it fails**. An abandoned
  session leaves the instrument stuck on a progress screen with no way out but a
  power cycle.

`--yes` means *don't ask me*. There is no `--force`.

## Writing into an occupied slot

Writing into an occupied slot is a **delete followed by a write** — the
instrument refuses to overwrite in place. `nord` reads the occupant first and
puts it back if the write fails; if the restore fails too, the bytes are written
to a `nord-rescued-BANK-SLOT.ne5p` in the working directory.

`live` and `settings` are the exception: the instrument accepts a write at one of
their occupied slots, so nothing is deleted to make room. The occupant is still
read back first and put back if the write fails.
