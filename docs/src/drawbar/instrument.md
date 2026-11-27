# Your keyboard

Connect a Nord keyboard over USB to see its sounds beside the files on your
computer. From here you copy sounds off the keyboard, send sounds to it and
rearrange its slots.

Before you change anything on the instrument, make a full backup with Nord
Sound Manager. [Check support for your keyboard](../getting-started/support.md#usb).

## Connecting

1. Close Nord Sound Manager so it releases the USB connection.
2. Turn the instrument on and connect it to the computer by USB.
3. Choose **Connect instrument…** at the top of drawbar. In a browser, select
   your instrument in the device picker. The desktop app opens the first Nord
   it finds.
4. Look for your instrument's name on the left, below **This computer**.

If your instrument doesn't appear, check [browser USB support](../getting-started/support.md#usb)
or [USB permissions on Linux](../getting-started/install.md#connecting-on-linux).

![drawbar's whole window with an instrument attached. On the left, This computer holds the demo sounds, and the instrument's Pianos folder is open below it. In the center, the demo piano drawbar-tine is open on its Key map, Trim to fit, Playback, Velocity layers and Roots. On the right, the inspector shows Selection, Tags and Room.](../assets/screenshot.png)

## Folders and slots

The instrument has a folder for each kind of sound it holds, such as
**Programs**. Expand a folder and a bank to see its slots. An address like
**7:4** means bank 7, slot 4, as on the panel. The **Keyboard** tab shows the
same slots bank by bank, empty ones included.

To refresh the listing after changes on the keyboard, choose **Instrument ▸
Read everything**. A folder drawbar doesn't recognize is listed but read-only.

## Work with a slot

Right-click a slot for its actions. Some change the instrument as soon as you
choose them.

| Action | What it does | Changes the instrument |
|---|---|---|
| **Open**, or double-click | Opens the slot's own copy for editing | At once, when you save |
| **Copy to this computer** | [Copies the sound](sending.md#copy-a-sound-to-this-computer) into your library | No |
| **Load on instrument** | Selects the slot on the panel | No |
| **Rename**, **Duplicate** | Renames the slot, or copies it to the first empty slot | At once |
| **Delete…** | Removes the sound after you confirm. There is no undo. | At once |
| Drag onto another slot in the same folder | Swaps the two slots | At once |

Everything else waits in the [send queue](sending.md#sending) until you choose
**Send all**. That includes a sound dragged from **This computer** onto a slot,
and saving a library file that came from a slot.

<details>
<summary>Names of samples and pianos</summary>

A file's name, without its extension, becomes the slot name. The instrument
keeps one sample or piano of each name in a folder, so if the name is already
in use, drawbar queues the file to replace that slot. Rename your file first
to keep both.

Samples and pianos cannot be renamed or duplicated on the instrument. Copy one
to this computer, rename the copy and send it.

</details>

## Safety

Before replacing an occupied program, song, piano or sample, drawbar keeps a
copy of the old sound. If the transfer fails, it tries to put that sound back.
If it can't, the activity log tells you where to find the copy. [Recover a sound after an interrupted write](recovery.md#a-change-was-interrupted).

If a browser tab was open during a drawbar update, reload it before replacing
a sound. Until it can keep a recovery copy, drawbar sends only to empty slots.

A damaged piano or sample fails verification and cannot be sent. If a piano is
too large for the free memory, [trim it to fit](pianos.md#trim-to-fit).

For the hardware on which transfers and recovery have been tested, see
[What is supported](../getting-started/support.md#usb).
