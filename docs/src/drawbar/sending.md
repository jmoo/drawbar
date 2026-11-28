# Copy and send sounds

Copy a sound from the keyboard to work on it on your computer, then sync it
back when you're ready. Nothing is written to the instrument until you sync, so
you can check every sound, and which way it goes, first.

## Copy a sound to this computer

1. Right-click an occupied slot and choose **Copy to this computer**, or drag
   it onto **This computer**. Select several slots to copy them together.
2. Find the copy under **This computer** and double-click it to edit.

The copy is linked to the slot it came from. Saving it writes the file and
nothing on the instrument; **Sync** writes it to that slot. Opening the slot
itself is different: **Save** there writes the slot at once.

To copy programs or songs with the sounds they depend on, [export a bundle](bundles.md).

## Sync

A file is linked to a slot once you copy it from that slot or send it there.
**Link to** and **Unlink** in a row's menu change a link. Sync compares each
linked file with its slot.

1. To put a file on a slot it isn't linked to, place it there first:

   | Start from | Do this |
   |---|---|
   | A file under **This computer** | Drag it onto a slot in the matching instrument folder. |
   | An open document | Choose **Send to…** in its header. |
   | A row in **Library** | Choose **Send to…** in its menu. |

   Choose an empty slot to keep the sound already there.
2. Click **Sync** in the top bar, or choose **Instrument ▸ Sync…**. drawbar
   reads the keyboard and lists each sound that differs: `↑` writes a sound to
   the keyboard, and `↓` brings one to this computer. The list opens by itself
   when a keyboard you connect has changes of its own.
3. Check each row. The lit arrow is the way it goes. Click the other arrow to
   turn it, or the lit one to leave that sound out. A `!` row has none lit
   until you pick.
4. Store any unsaved changes on the keyboard's panel. Writing settings or the
   slot you're playing reloads the panel, and drawbar warns you before writing
   settings.
5. Choose **Sync**.
6. Load the program on the keyboard to hear it. **Load on instrument** in a
   document's header plays the version now in the slot.

**Edit ▸ Undo** puts back a file Sync brought in. **Undo** in the activity log
does too, and puts back a link Sync moved or removed, or one drawbar made on its
own. A write to the keyboard has no undo.

## When a sync stops

If the instrument refuses a transfer, the activity log says why, and the list
opens again on what is left. A sound that changed on disk is not sent until
you have looked at it.

Disconnecting keeps the sounds you placed on a slot. Check them again after
reconnecting.
