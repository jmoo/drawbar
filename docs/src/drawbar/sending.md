# Copy and send sounds

Copy a sound from the keyboard to work on it on your computer, then send it
back when you're ready. Sends wait in a queue, so you can check every
destination before the instrument is written.

## Copy a sound to this computer

1. Right-click an occupied slot and choose **Copy to this computer**, or drag
   it onto **This computer**. Select several slots to copy them together.
2. Find the copy under **This computer** and double-click it to edit.

The copy remembers the slot it came from. Saving it queues it for that slot
and writes nothing yet. Opening the slot itself is different: **Save** there
writes the slot at once.

To copy programs or songs with the sounds they depend on, [export a bundle](bundles.md).

## Sending

1. Queue the sound for a slot in one of these ways:

   | Start from | Do this |
   |---|---|
   | A file under **This computer** | Drag it onto a slot in the matching instrument folder. |
   | A copy on this computer that came from a slot | Choose **Queue send** in its header, or save it. |
   | Several copies changed since they came off the keyboard | Choose **Queue** beside **Send** in the top bar. |
   | Checked files | Choose **Queue for sending** in the inspector's **Selection** card. |

   Choose an empty slot to keep the sound already there.
2. Click **Send** in the top bar. Its count tells you how many sounds are
   waiting. Select a row to compare what the slot holds now with what you're
   about to send. Click its destination to choose another slot, or **×** to
   take it out of the queue.
3. Store any unsaved changes on the keyboard's panel. Writing settings or the
   slot you're playing reloads the panel, and drawbar warns you before writing
   settings.
4. Choose **Send all**.
5. Load the program on the keyboard to hear it. **Load on instrument** in a
   document's header plays the version now in the slot.

## When a send stops

If the instrument refuses a transfer, the activity log says why, and the
sounds not yet sent stay queued. A sound that changed on disk is not sent until
you have looked at it.

Disconnecting keeps the queue. Review the destinations again after
reconnecting.
