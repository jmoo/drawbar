# Edit a program

Programs, live slots, presets and settings open with controls laid out like the
keyboard's panel. A *program* remembers a playable setup, such as a piano and
organ split with effects.

Audio files have their own editors. See [sample instruments](samples.md),
[piano libraries](pianos.md) and [WAV recordings](recording.md).

## Open a program

1. Import a program file, or [copy one from your keyboard](sending.md#copy-a-sound-to-this-computer).
2. Double-click it under **This computer**.
3. In **Basic**, choose a section at the top to jump to its controls. Drag a
   knob up to raise its value, or double-click it to type one. A `*` beside the
   document name means you have unsaved edits.
4. Keep or discard your edits:

   | To | Choose |
   |---|---|
   | Change the library file | **File ▸ Save** |
   | Keep the new version as a separate file | **Export…** in the document header |
   | Go back to the last save | **Revert** |

5. To hear the program on the keyboard, [send it to a slot](sending.md#sending),
   then load that slot.

Saving a copy that came from a slot also queues it for that slot. Saving a
document you opened from an instrument slot writes the slot at once.

## Undo an edit

**Edit ▸ Undo** takes back one gesture: a drag of a knob, a click, or what you
typed in a box before you left it. It brings the sound to the front and sends
nothing to the instrument. A **Revert** is a step too.

Undo passes over a save and leaves the file as you saved it, so the sound reads
unsaved. When the save itself is next, the menu reads **Undo Save**, which puts
the file back. In a sample instrument or piano library edited in its file, a
save ends what Undo can take back.

## Work with the panel controls

Click a lamp to flip a switch, open a selector to choose a setting, or pull a
drawbar down. Inactive sections may be folded. An **unrecognized value** is
the value that was in the file. You can leave it as it is.

To edit from the computer keyboard, press Tab to reach a control.

| Key | Does |
|---|---|
| Arrow keys | Step a knob |
| Page Up, Page Down | Take larger steps |
| Home, End | Reach the limits |
| Space | Flip a switch |

**Choose a piano.** With the instrument connected, choose the piano **Type**,
then choose a **Model** from the pianos it holds. The program uses that piano
when you send it. Without the instrument, a program may identify a piano or
sample only by number.

**Change a morph.** A morph lets a playing control, such as a wheel, move a
setting toward a second value. Three dots mark a control with a morph. Pick the
source in the inspector's **Morph** card and set its target value. Choose
**Panel** or **Back to the panel** to edit the starting value again.

## The document header

The strip above the controls names the sound, where it is and whether it has
unsaved edits. Hover over an icon or status for details. **Basic** shows the
playing controls, and **Advanced** shows the file's fields.

<details>
<summary>Every field and raw values</summary>

In **Advanced**, **Every field** lists settings the panel view leaves out.
**Raw** shows the value as last saved. Type a new one under **Writes** to
change it. A pencil marks each changed field.

</details>

If drawbar recognizes a file but has no editor for it, **Advanced** lets you
inspect its contents. It can still be sent, copied and tagged, and goes back to
the instrument byte for byte.
