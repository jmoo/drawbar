# Start with the nord command

Use `nord` to inspect a file, make the same edit across several patches, or
copy sounds to and from a connected keyboard. [Install nord-cli](../getting-started/install.md#the-nord-command)
to get the command.

## Try it without a keyboard

Run these commands in an empty folder. The first makes a new program file with
its gain set to 64. The next two read the file you just made.

```sh
nord program edit --set center_panel.gain=64 -o practice.ne5p
nord inspect practice.ne5p
nord verify practice.ne5p
```

`inspect` describes the program. `verify` checks that decoding and writing it
back reproduces the original bytes. You can also drop `practice.ne5p` into
drawbar to see its panel controls.

## Choose a task

| To… | Use |
|---|---|
| See what's in a file, check it or unpack a bundle | [Inspect files and unpack bundles](files.md) |
| List the editable settings and try an edit on a copy | [Edit a file or slot](editing.md) |
| List the keyboard's banks, copy a program or send one back | [Work with an instrument](instrument.md) |
| Extract a WAV, build a sample instrument or trim a piano | [Work with samples and pianos](libraries.md) |

Add `--help` at the level you're using. For example, `nord piano --help`
lists the piano commands, and `nord piano build --help` explains the inputs for
building one library.

## Slots

A slot address is `BANK:SLOT`, counted from 1 as on the panel. For example,
`nord program get 7:4` reads bank 7, slot 4 from the keyboard.
`nord program get practice.ne5p` reads a file on your computer.
`live` and `settings` have fewer commands than `program`. Use their own
`--help` to list them.

## Output

Results go to standard output. Progress, warnings and confirmation prompts
go to standard error, so you can redirect the results to a file. Piped output
uses plain ASCII. Use `--color=always`, `--color=never` or the `NO_COLOR`
environment variable to control color.

For an operation that asks for confirmation, Enter cancels. Outside an
interactive terminal, supply `--yes` to authorize the change in a script.
