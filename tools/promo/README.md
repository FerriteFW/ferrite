# Videos of the radio

A video from a storyboard: the simulator plays each block's key script (the firmware's own screens, frame by
frame), each block gets a 1920 x 1080 card with its captions, and the blocks are joined with fades and music.

```sh
tools/promo/music/fetch.sh                      # the music (once; not committed)
.venv/bin/python tools/promo/mkvideo.py                  # tools/promo/preview.story -> target/promo/preview.mp4
.venv/bin/python tools/promo/mkvideo.py my.story -o out.mp4
.venv/bin/python tools/promo/mkvideo.py --only 3         # just block 3, to tune its script
```

Needs ffmpeg, Google Chrome (headless, for the cards) and Pillow (the key chips; in the project's `.venv`). From `ferrite/`.

**Keys on screen:** every key the script presses shows under the radio's screen as it happens, as keycaps (`MENU 0 7 7`, `hold SLOT`, `knob +1`, `hold SIDE1 + SIDE2`): the videos double as tutorials for the manual. Presses less than 0.8 s apart share a chip; a chip stays 1.2 s. A block's `keys: off` hides them (for pure promo shots).

## Storyboard

Blocks separated by `---` lines; the format is described at the top of `preview.story`.

- `title`, `left`, `right`: the captions (`||` starts a paragraph).
- `layout`: `center` (captions both sides), `right` or `left` (all captions on that side).
- `hold`: seconds the last frame stays before the fade (default 1.5).
- `setup`: a script played before recording; `script`: the recorded one (indented lines).
- `packet:`: what the radio sends but doesn't show (a beacon), as a card under the captions from `packet_at:` seconds
  into the block: the sender, "position" and `packet_comment:` (the comment as a map or radio shows it), with the
  `packet_mark:` part in violet, then the raw TNC2 line small and dim; `packet_note:` is the label. Copy both from
  the firmware's own output (a throwaway test calling the beacon builder with the sim's values), never type them.
- `key_gap:` (ms): a pause between key presses, so each keycap and the screen it leads to can be seen; `menu` and
  `type` steps are played key by key. Set in any block it is the story's; a block can set its own.
- `kind: intro` / `kind: outro`: title cards, `seconds` long.
- `music`: an audio file, looped and faded to the video's length.

Scripts use the simulator harness's steps (`sim/tests/common/mod.rs`): `press K`, `hold K`, `knob N`,
`type 123`, `wait 2s`, `menu 0 5 1`, `home`, `event NAME`. One radio plays all the blocks in turn, so a block's
setup starts from where the last one left it.

## Rules

- Captions describe only what's built (check the manual's status before claiming a feature).
- Music: only tracks whose licence allows this use, listed with their source and licence in `music/README.md`.
  Never generated music.

## How it works

`ferrite-sim-record` (`sim/src/bin/record.rs`) runs the scripts through the test harness, with a frame hook, and
writes raw 240 x 320 frames. `mkvideo.py` draws the cards (`card.html`) with Chrome, lays the screen on them at 2x
(an even scale keeps pixel edges crisp in the video), fades, joins and adds the music.
