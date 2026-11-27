# Work with an instrument

Make a full backup with Nord Sound Manager before writing to the keyboard.
Close Sound Manager to release the USB connection. On Linux, check the
[USB permission rule](../getting-started/install.md#connecting-on-linux).
[See which instruments have been tested](../getting-started/support.md#usb).

## Read a program onto your computer

```sh
nord device status
nord program list
nord program get 7:4 -o patch.ne5p
```

`status` shows what the instrument holds. Use `--json` for scripts.
Choose an occupied address from `list` in place of `7:4`. Without `-o`,
`get` prints a summary. With it, you have a file you can [inspect](files.md)
or [edit](editing.md) independently of the keyboard.

Use `nord device info` to identify the attached instrument, or `nord device
geometry` for its banks and slot counts. To see what's currently selected on
the panel, run `nord program focus`.

## Before anything changes

For example, to send an edited file back to bank 7, slot 4:

```sh
nord program put louder.ne5p 7:4
```

Read the destination and the name of the current occupant when prompted. Press
Enter to cancel or type `y` to continue. The slot takes its new name from the
file. Use `--yes` to answer in advance when scripting a write.

## Organize slots

| Task | Example | Effect |
|---|---|---|
| Load a program | `nord program select 2:12` | Selects it on the panel. |
| Move a program | `nord program move 8:13 7:16` | Swaps the two slots. The command lists songs that follow the move. |
| Copy to a second slot | `nord program duplicate 7:2 7:3` | Replaces the destination if occupied. |
| Rename a program | `nord program rename 6:13 "Warm Pad"` | Changes the name shown on the keyboard. |
| Delete programs | `nord program delete 7:50 7:49` | Removes the sounds from those slots. |

Use `get` and `put` with `sample`, `piano` or `setlist` to transfer those
sounds. For a song and all its dependencies, [read a bundle](files.md#bundles).
`live` provides `get`, `info`, `deps` and `edit`. For `settings`, use `get`,
`info` or `edit`.

## What a write does

Before replacing an occupied program, song, sample or piano, `nord` checks the
input and saves the old sound to the working directory. To choose a different
recovery folder, use `--rescue-dir DIR` or set `NORD_RESCUE_DIR`. If it can't
save the recovery copy, it leaves the slot alone.

If the write fails, `nord` tries to restore the old sound. After a successful
write or restore, it removes the recovery file. If both fail, or the command
was interrupted, look for a file such as `nord-rescued-7-50.ne5p`. Use it
with `nord program put` to restore the sound to the intended slot. A
`.partial` file means the recovery copy was unfinished and the slot hadn't
yet been changed. Live slots and settings are written in place.

If the next command says the instrument answered out of step after an interrupted
session, run `nord device recover` and try again.
