# Community

## Official software

[Nord Sound Manager](https://www.nordkeyboards.com/software-tools/nord-sound-manager)
is Clavia's app for the keyboard's memory. Use it to make a full backup of the
instrument before drawbar changes sounds on it. Close it before you connect
drawbar, so it releases the USB connection.

[Nord Sample Editor](https://www.nordkeyboards.com/software/nord-sample-editor-4/)
is Clavia's app for building sample instruments. drawbar
[opens its projects](drawbar/samples.md#open-an-existing-project), and the
projects drawbar makes from your WAVs open in Sample Editor too.

## Community projects

Other people's projects that work with Nord files.

### Explore a program

[NS3 Program Viewer](https://chris55.github.io/ns3-program-viewer/) displays
Stage 2, 2EX and 3 programs in a browser. Christian Florentz built the viewer
on the byte maps described below. [Source and GPL-3.0-or-later license](https://github.com/Chris55/ns3-program-viewer).

[ns4decode](https://ns4decode.netlify.app) is Randy's Stage 4 file viewer.
It published the first public decode of Stage 4 programs and presets. drawbar
uses its MIT-licensed tables to find settings in those files.

### Prepare recordings

[StageLoop](https://livekeytools.com/stageloop) by LiveKeyTools is a free,
closed-source sample editor for macOS and Windows. It trims and normalizes
recordings, finds loops and crossfades, and maps samples to root keys and zones.
It exports a Nord Sample Editor project that you can
[open in drawbar](drawbar/samples.md#open-an-existing-project).

### Learn about the formats

[Nord documentation](https://chris55.github.io/nord-documentation/) by
Christian Florentz maps Stage 2, Stage 3 and Lead A1 program files by comparing
saves after changing one control at a time. It was the first public decode of
the Stage 2 and 3 formats. drawbar uses these BSD-3-Clause maps for its Stage 2
and 3 settings. [Source and license](https://github.com/Chris55/nord-documentation).

---

## Add your project

If you maintain a project that works with Nord files or instruments, suggest it
with a [pull request against this page](https://github.com/jmoo/drawbar/edit/master/docs/src/community.md).
If you don't use GitHub, send it through the [feedback form](feedback.md).
