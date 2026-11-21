# Two keys people ask about: # on home (the quick help, HOME-10, HELP-1, HELP-4) and hold * (the quick menu,
# HOME-17). Recorded from the simulator (the firmware's own screens). Format: see preview.story. Console's update
# video for 0.9.5 (ferritefw.dev/video/console-update.mp4). Captions only say what HOME-10 and HOME-17 say.

kind: intro
title: Keys worth knowing
left: # for help, hold * for the quick menu, * to mute a slot or turn it off, and hold # to lock the keys.
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
  wait 1200ms
  press #
  wait 6000ms
hold: 1
---
title: # again pages on
layout: right
left: Each # goes on through the help: the home keys, the side keys and PTT, the menu numbers most often wanted, and the manual.
right: ◀ ▶ page too. BACK goes home.
script:
  press #
  wait 4500ms
  press #
  wait 4500ms
  press #
  wait 5500ms
  press #
  wait 3500ms
  press BACK
  wait 1500ms
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
left: PTT B switches to night red and back. PTT A reverses the active slot. PF2 turns Bluetooth on or off, and PF1 steps GPS through Off, Auto and On.
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
  press SIDE1
  wait 1600ms
  press SIDE1
  wait 1600ms
  press SIDE1
  wait 1600ms
  press BACK
  wait 1500ms
hold: 1
---
title: Tap *: mute
layout: right
left: * on home mutes the active slot; * again brings it back.
right: A muted slot still receives: its S-meter and the green light carry on, only its audio stays off the speaker. A power cycle clears it.
setup: menu 0 1 1; wait 300ms; knob 2; wait 300ms; home; wait 2000ms
script:
  wait 1500ms
  press *
  wait 4500ms
  press *
  wait 3500ms
hold: 1
---
title: * * : turn a slot off
layout: right
left: Two taps within a second turn the slot off: no chip listens to it, and its watch turns go to the other slots.
right: * turns it back on. While it is off, transmit on it is refused, with a note saying why.
script:
  wait 1200ms
  press *
  press *
  wait 6000ms
  press *
  wait 4000ms
hold: 1
---
title: Hold #: lock the keys
layout: right
left: Hold # locks the keypad and the knob at once, and a padlock shows in the status bar. Within 3 seconds, * locks every key, PTT too, and 0 all but PTT.
right: A locked key just says how to unlock: hold # again. The volume knob always works.
script:
  wait 1200ms
  hold #
  wait 4000ms
  press 5
  wait 3000ms
  hold #
  wait 3000ms
hold: 1
---
kind: outro
title: ferritefw.dev
left: The user guide has every key: MENU 0 9 8, or the QR code on the Manual page.
seconds: 5
