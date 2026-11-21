# Connect your keyboard

Connect a Nord keyboard over USB to see its banks and slots beside your own
files. Before making changes on the instrument, make a full backup with Nord
Sound Manager. [Check support for your keyboard](../getting-started/support.md#usb).

## Connecting

1. Close Nord Sound Manager so it releases the USB connection. Connect your
   powered-on instrument to the computer by USB.
2. Choose **Connect instrument…** at the top of drawbar. In a browser, select
   your instrument in the device picker. The desktop app opens the first Nord
   it finds.
3. Look for your instrument's name on the left. Expand **Programs** and a bank
   to see its slots. An address like **7:4** means bank 7, slot 4, just as on
   the panel. The **Keyboard** tab also shows empty slots.

If your instrument doesn't appear, check [browser USB support](../getting-started/support.md#usb)
or [USB permissions on Linux](../getting-started/install.md#connecting-on-linux).
To refresh the listing after changes on the keyboard, choose **Instrument ▸
Read everything**.

![drawbar with an instrument attached. Its folders are in the left panel below This computer, a piano library is open in the center, and Room on the right shows the space left for each kind of sound.](../assets/screenshot.png)

## Slots

Right-click a slot for its available actions. **Load on instrument** selects
it on the panel. You can rename, duplicate or delete a program there. Dragging
a slot onto another slot in the same instrument folder **swaps** them. These
actions work on the keyboard itself.

A file's name becomes the slot name without the extension. Samples and pianos
each need a unique name in their instrument folder. If the name is already in
use, drawbar targets that slot for replacement. Rename your local file first
to keep both. To duplicate or rename a sample or piano, copy it to this
computer and send the renamed copy.

To copy programs or songs with the sounds they depend on, [export a bundle](bundles.md).

## Safety

Before replacing an occupied program, song, piano or sample, drawbar keeps a
copy of the old sound. If the transfer fails, it tries to put that sound back.
If it can't, the activity log tells you where to find the copy. [Recover a sound after an interrupted write](recovery.md#a-change-was-interrupted).

If a browser tab was open during a drawbar update, reload it before replacing
a sound. Until it can keep a recovery copy, drawbar allows transfers only to
empty slots. A damaged piano or sample fails verification and cannot be sent.
If a piano is too large for the free memory, [trim it to fit](pianos.md#trim-to-fit).

A folder the app doesn't recognize is read-only. For the hardware on which
transfers and recovery have been tested, see [What is supported](../getting-started/support.md#usb).
