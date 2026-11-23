# Ferrite preview: the main features, recorded from the simulator (the firmware's own screens).
# Blocks are separated by lines of ---. Fields:
#   kind: intro or outro for a title card (no radio), shown `seconds` long
#   title, left, right: caption text (|| starts a new paragraph)
#   layout: center (screen in the middle, left and right captions beside it; the default), right (all captions on
#           the right of the screen) or left (all on the left)
#   hold: seconds the last frame stays before the fade (default 1.5)
#   setup: a script played before recording, not shown; script: recorded (the indented lines below it)
#   keys: off hides the keys pressed (shown under the screen by default, as a tutorial)
#   music: on any block, an audio file for the whole video (relative to this file), with a licence that allows
#          it (note its source and licence in tools/promo/music/README.md); without one the video is silent
# Scripts use the simulator harness's steps: press K, hold K, knob N, type 123, wait 2s, menu 0 5 1, home, event NAME.

kind: intro
title: Ferrite
left: Free firmware for the Radtel RT-950 Pro, written from scratch.
seconds: 4
music: music/chiptune3.wav
---
title: Three slots, one radio
layout: center
left: Three receivers on one screen: listen on one while the others watch or scan.
right: Each slot keeps its own frequency, mode, tones and power. || SLOT switches between them; the knob tunes. || Where a slot can't transmit, it says so.
setup: home
script:
  wait 1500ms
  press SLOT
  wait 1200ms
  press SLOT
  wait 1200ms
  press SLOT
  wait 800ms
  knob 4
  wait 1500ms
---
title: Band scope
layout: right
left: Hold 1 to see what's on the air around you, live.
right: The knob moves the marker along the band; the strongest signal is named below.
setup: home
script:
  hold 1
  wait 3s
  knob 3
  wait 2500ms
---
title: A channel on one page
layout: left
left: MENU 8 opens everything about a channel on a single screen.
right: Name, zone, tones, offset, power, squelch, scan and APRS: pick a field, turn the knob.
setup: home; press SLOT
script:
  wait 600ms
  press OK
  wait 500ms
  type 8
  wait 300ms
  press OK
  wait 1500ms
  press DOWN
  wait 600ms
  press RIGHT
  wait 600ms
  knob 2
  wait 1500ms
  press BACK
  wait 300ms
---
title: Squelch you can see
layout: right
left: Set the squelch against what's on the air right now.
right: The live signal bar shows where the audio opens, before you leave the page.
setup: press BACK; home
script:
  wait 500ms
  press OK
  wait 900ms
  press OK
  wait 1800ms
  knob 2
  wait 1500ms
  knob -1
  wait 1500ms
  press BACK
  wait 300ms
---
title: Crossband repeater
layout: left
left: Two bands, one radio: what's heard on A goes out on B, and back.
right: Live: who's passing, the timeout, and the last 30 minutes of activity both ways.
setup: press BACK; home; menu 0 3 4; press HASH; wait 18s
script:
  wait 9s
---
title: Messages, without typing callsigns
layout: center
left: An empty recipient lists your contacts, recent conversations, stations heard and APRS services.
right: Pick one, then write with predictive text on the keypad. || Acks and retries are handled for you, and a message still trying can be stopped or sent again.
setup: press BACK; home
script:
  menu 0 5 2
  wait 1200ms
  press HASH
  wait 1500ms
  press DOWN
  wait 500ms
  press RIGHT
  wait 900ms
  press LEFT
  wait 500ms
  press OK
  wait 800ms
  type 4663
  wait 900ms
  press HASH
  wait 2500ms
---
title: APRS on slot C
layout: left
left: MENU 0 5 3 puts APRS mode on slot C, beside your two voice slots.
right: The card counts the stations heard since power-on and in the last hour, and names the last one. || C gets the receive path every other turn, so packets aren't missed while A and B watch.
setup: press BACK; home
script:
  wait 800ms
  menu 0 5 3
  wait 900ms
  knob 2
  wait 900ms
  home
  wait 1200ms
  press SLOT
  wait 300ms
  press SLOT
  wait 6500ms
---
title: Everything it heard, in one place
layout: right
left: The event log keeps every decode and alert, newest first, through a restart.
right: Filter by source; open one for its details, or tune straight to it. || The heard list comes back after a restart too.
setup: home
script:
  wait 800ms
  menu 0 4 1
  wait 2000ms
  press RIGHT
  wait 1200ms
  press LEFT
  wait 900ms
  press OK
  wait 2500ms
---
title: Shortwave on a handheld
layout: right
left: The broadcast receiver doubles as an HF receiver: AM on long wave, medium wave and shortwave.
right: Its own tuning screen, with the signal and the station's band at a glance.
setup: press BACK; home; menu 0 3 3; wait 2700ms; type 9580; press OK; wait 3000ms
script:
  wait 1200ms
  knob 3
  wait 1500ms
  knob -3
  wait 2500ms
---
kind: intro
title: Ferrite Console
left: Program the radio and watch it live from a browser, over the cable or Bluetooth. Channels, settings, APRS messages, the heard list on a map, and a backup before anything is installed.
seconds: 6
---
kind: outro
title: ferritefw.dev
left: Free firmware, source public.
seconds: 5
