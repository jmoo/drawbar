# Talking to an instrument

Close Nord Sound Manager first — it claims the vendor interface exclusively, and
`nord` cannot attach alongside it.

```sh
nord device status                      # inventory per class; --json for machines
nord device recover                     # release a session an interrupted run left open

nord program list                       # every occupied slot
nord program get 7:4 -o patch.ne5p      # write the .ne5p
nord program put patch.ne5p 7:4 --yes
nord program move 8:13 7:16 --yes
nord program deps 7:4                   # piano/sample dependencies, with names
nord program select 2:12                # load live on the instrument
```

The same verbs work on `nord setlist`, and on `nord raw --class N` for a class
with no noun of its own.

## Safety

- **Read-only until told otherwise.** Every mutating command reads the slot
  first, says what it will touch, then refuses without `--yes`.
- **Replacing and deleting name what is about to be lost.** `duplicate` names
  the destination's occupant; `move` names it as **SWAPPING WITH**, because the
  instrument exchanges the two slots and nothing is lost.
- **Off a terminal, a missing `--yes` is a real dry run.** There is no `--force`.
- **Back up your instrument first.** Nord Sound Manager makes a full backup.

> ⚠️ **A settings write reloads the selected program on the instrument.** Panel
> state that has not been stored is lost, so re-`select` and re-apply afterwards.

The full command set is in
[`crates/nord-cli/README.md`](https://github.com/jmoo/drawbar/blob/master/crates/nord-cli/README.md).
