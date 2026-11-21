# Two keys people ask about: # on home (the quick help, HOME-10, HELP-1, HELP-4) and hold * (the quick menu,
# HOME-17). Recorded from the simulator (the firmware's own screens). Format: see preview.story. Console's update
# video for 0.9.5 (ferritefw.dev/video/console-update.mp4). Captions only say what HOME-10 and HOME-17 say.

kind: intro
title: Two keys worth knowing
left: # for help, and hold * for the quick menu.
seconds: 4
music: music/chiptune3.wav
key_gap: 450
---
title: # on home: help
layout: right
left: Press # on home and the radio shows its own help, starting with every hold.
right: Hold ABC edits the slot, hold V/M opens the zones, hold MENU more settings, hold BACK goes home, hold * the quick menu, hold # locks the keys.
# Watch off for the recording (0 1 1, from the sample's A+B+C): while a watched slot is receiving, # pauses watch instead (HOME-10), and the
# simulator's signals would decide which the press does.
setup: menu 0 1 1; wait 300ms; knob -2; wait 300ms; home; wait 1500ms
script:
  wait 1000ms
  press #
  wait 4500ms
hold: 1
---
title: # again pages on
layout: right
left: Each # goes on through the help: the home keys, the side keys and PTT, the menu numbers most often wanted, and the manual.
right: ◀ ▶ page too. BACK goes home.
script:
  press #
  wait 3200ms
  press #
  wait 3200ms
  press #
  wait 3500ms
  press #
  wait 3000ms
  press BACK
  wait 1200ms
hold: 1
---
title: Hold * on home: the quick menu
layout: left
left: A small box over home gives the knob and the side keys a job each while it is up: brightness, reverse, night red, GPS and Bluetooth.
right: Each press acts at once. While the box is up the PTT keys never transmit.
setup: home
script:
  wait 900ms
  hold *
  wait 2200ms
  knob -2
  wait 1500ms
  knob 2
  wait 1500ms
hold: 0.5
---
title: Night red, reverse, Bluetooth
layout: left
left: PTT B switches to night red and back. PTT A reverses the active slot. PF2 turns Bluetooth on or off; PF1 steps GPS.
right: BACK, * again or 5 seconds without a key closes it.
script:
  press PTTB
  wait 2200ms
  press PTTB
  wait 1600ms
  press PTTA
  wait 1800ms
  press PTTA
  wait 1500ms
  press SIDE2
  wait 1800ms
  press SIDE2
  wait 1500ms
  press BACK
  wait 1500ms
hold: 1
---
kind: outro
title: ferritefw.dev
left: The user guide has every key: MENU 0 9 8, or the QR code on the Manual page.
seconds: 5
