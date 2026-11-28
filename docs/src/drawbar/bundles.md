# Bundles

A song file holds the locations of its programs, and a program can depend on a
piano or sample. A Nord Sound Manager *bundle* carries them together.

## Export a bundle

1. Connect the instrument that holds the sounds.
2. Select the programs or songs you want, on the instrument, on this computer,
   or across both.
3. Choose **Export as bundle…**. drawbar copies the sounds it needs from the
   instrument to this computer and includes the dependencies it can identify.
   In a browser, the bundle goes to your downloads.
4. Click the status line to open the activity log, and check whether anything
   was left out before you rely on the bundle elsewhere.

<details>
<summary>Why a piano or sample can be left out</summary>

A program file identifies its piano and sample by a number worked out from their
names, so drawbar finds one that is on this computer with or without an
instrument attached. The activity log lists one it could not find.

</details>

## Unpack a bundle

Drop the bundle on the window, or choose **File ▸ Import…**. Its contents appear
in a new folder named after the bundle. Keep the original bundle if you need it
again.

[Supported bundle formats](../getting-started/support.md#files).
