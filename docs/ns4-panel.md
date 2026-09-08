# Stage 4 panel semantics

Source: [Nord Stage 4 User Manual, OS v1.6x, Edition N](https://www.nordkeyboards.com/wt/documents/951/Nord%20Stage%204%20User%20Manual%20v1.6X-Edition-N.pdf).
Page references use the printed page numbers. This describes stored program
controls; performance history, held notes, effect tails, MIDI input and temporary
panel operations cannot be reconstructed from a program file.

## Documented dependencies

| Controls | Dependency | Pages |
| --- | --- | --- |
| Section and layer settings | Section and layer enables; both scenes share the sound parameters | 43 |
| Transpose amount and split boundaries | Transpose, split and individual boundary enables | 38–39 |
| Synth internal sound and effects | Unavailable in Extern mode; keyboard routing remains available | 46–47 |
| Synth glide and note priority | Mono or Legato | 34–35 |
| Filter parameters and envelope | Filter enable | 31–33 |
| Arpeggiator and pattern | Run, then Pattern enable | 35–37 |
| Keyboard hold switch | Accessible even with Synth off | 36 |
| Effect parameters | Layer Effects enable and the individual effect enable | 48 |
| Rotary controls | Shared across sections; Organ is only one routing source | 48, 52–53 |
| Rotary stop angle | Stop Mode and Slow/Stop speed | 53 |

The panel keeps enable controls outside the groups they govern. Disabled state
is retained in the file. Resolving the panel does not edit bytes or apply hardware
copy operations.

## Unverified storage semantics

The manual describes operation, not file encoding. The decoder's raw selectors
are insufficient to implement every documented dependency:

- **Scenes:** the polarity of `active_layer_scene` is unverified. Section and
  layer relevance therefore accept either scene's enable, independently. This
  can include inactive layers, including a section and layer enabled in different
  scenes. It avoids hiding Scene II-only settings but does not identify the
  currently playing scene. A controlled scene-toggle specimen pair is needed.
- **Sound modes:** Samples/Analog polarity, organ models and presets, piano
  types, filter types, vibrato modes and Arp/Poly/Gate selector indices are not
  established. Their dependent settings remain accessible. Examples include B3
  percussion, piano acoustic options, sample versus analog oscillator controls,
  Gate direction, and vibrato delay.
- **Global effects:** Compressor, Delay and Reverb can apply across all sections.
  The authoritative stored chain in Global mode is unverified. The panel exposes
  the flags and stored chains, without selecting a global owner or propagating
  edits. Group and Global transitions on hardware synchronize settings; a raw
  field edit does not emulate those operations.
- **Rotary routing:** the Amp/EQ To Rotary selector index is unknown. Shared
  rotary controls stay accessible rather than depend solely on the Organ route.
- **Clock and morphs:** rate encodings under clock sync, some clock/group fields,
  and filter resonance morph ownership remain incomplete. A zero base value is
  not used to disable a morphable parameter.

These limits are surfaced in the application. Full hardware state-machine
validation requires controlled specimens or hardware observations; the manual
alone cannot establish byte values.
