# Edit a program

A *program* remembers a playable setup, such as a piano and organ split with
effects. A *song* in a set list remembers which programs you want to call up.
Changing the order of programs in a song leaves the programs themselves alone.

## Try a program change

1. Import a program file, or [copy one from your keyboard](sending.md#copy-a-sound-to-this-computer).
   Double-click the file under **This computer**.
2. In **Basic**, choose a section at the top to jump to its controls. Try
   dragging a knob up to increase its value, or double-click it to type a
   value. The `*` beside the document name means you have unsaved edits.
3. To keep the new version as a separate file, use **Export…** in the document
   header. To change the library copy, choose **File ▸ Save**. **Revert**
   discards the unsaved edits and returns to the last save.
4. To hear the edited program on the keyboard, [send it to a slot](sending.md#sending),
   then load that slot. **Load on instrument** plays whatever is already stored
   there.

Saving a library copy linked to the keyboard also queues it for its slot.
If you opened the instrument slot directly, **Save** writes back to the keyboard.

## Work with the panel controls

Programs, live slots, presets and settings open with the controls available
for that file. Click a lamp to flip a switch, open a selector to choose a
setting, or pull a drawbar down. Inactive sections may be folded. An
**unrecognized value** is the value that was in the file. You can leave it
as it is.

To edit with the keyboard, press Tab to reach a control. The arrow keys step
a knob, Page Up and Page Down take larger steps, Home and End reach the limits,
and Space flips a switch.

**Choose a piano.** With the instrument connected, choose the piano **Type**,
then choose a **Model** from the pianos it holds. The program will use that
piano when you send it. Without the instrument, a program may identify a piano
or sample only by number.

**Change a morph.** A morph lets a playing control, such as a wheel, move a
setting toward a second value. Three dots mark a control with a morph. Pick
the source in the inspector's **Morph** card and set its target value. Choose
**Panel** or **Back to the panel** when you're ready to edit the starting value
again.

## The document header

The strip above the controls identifies the sound, its location and its state.
Hover over an icon or status for details. **Basic** shows the playing controls.
In **Advanced**, **Every field** includes settings the panel view leaves out.
**Raw** shows the saved value. Type a new one under **Writes**. Changed fields
are marked with a pencil. If drawbar recognizes a file but has no editor for
it, **Advanced** lets you inspect its contents. It can still be sent, copied
and tagged, and goes back to the instrument byte for byte.

Audio files have their own controls. See [sample instruments](samples.md),
[piano libraries](pianos.md) and [WAV recordings](recording.md).
