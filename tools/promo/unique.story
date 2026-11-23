# Ferrite: candidates for "what only Ferrite does". More than needed, to cut from.
# Sources for every caption: tools/promo/CLAIMS.md. Format: see the top of preview.story.

kind: intro
title: Only on Ferrite
left: Things we haven't seen in the stock firmware.
seconds: 4
music: music/chiptune3.wav
---
kind: intro
title: A radio that idles at 19 mA
left: The processor runs at 24 MHz while it waits and at full speed only while something needs it. || On our bench, a resting radio drew about 19 mA instead of about 65 mA at full speed all the time.
seconds: 6
---
title: APRS that isn't missed
layout: left
left: With APRS mode on slot C, C gets the receive path every other turn.
right: A and B keep watching; C is never deaf for more than 70 ms.
setup: press BACK; home; menu 0 5 3; wait 900ms; knob 2; wait 900ms; home
script:
  wait 6s
---
kind: intro
title: A decoder that holds its own
left: The WA8LMF TNC test CD, tracks 1 and 2, through the radio's own receive path: 1,011 and 1,007 packets. Dire Wolf 1.8.1 on its defaults: 1,005 and 982. In about 1.7 KB of RAM.
seconds: 7
---
title: Messages you can stop
layout: right
left: A message still being tried can be stopped, and a failed one sent again without retyping.
right: Delete it and its tries stop too.
setup: home
script:
  wait 600ms
  menu 0 5 2
  wait 1200ms
  press HASH
  wait 1200ms
  press OK
  wait 800ms
  type 4663
  wait 900ms
  press OK
  wait 1800ms
  press OK
  wait 1800ms
  hold MENU
  wait 2200ms
  press OK
  wait 2500ms
---
kind: intro
title: Nothing lost at power-off
left: The heard list and the event log come back after a restart, with real times when GPS has set the clock, and "earlier" when it hasn't.
seconds: 6
---
kind: intro
title: FX.25
left: Forward error correction around each AX.25 frame, sent and received. Plain receivers still decode it; FX.25 receivers repair damaged bytes.
seconds: 6
---
kind: intro
title: Bluetooth, about 12 times faster
left: Ferrite Console over Bluetooth: the home screen arrives in about 0.3 s instead of 3.4 s, and 176 channels read in about 6 s instead of 33. Measured on our bench.
seconds: 6
---
kind: intro
title: Built not to brick
left: Calibration is never written. Ferrite Console takes a full backup before it installs anything, and the same page puts the radio back to stock.
seconds: 6
---
title: Squelch you can see
layout: right
left: The squelch page shows the live signal against the opening level.
right: Set it against what's on the air right now, before you leave the page.
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
---
title: Low Power Mode
layout: right
left: One setting turns off what you can live without when the battery runs low, at 20 % by default.
right: It comes back by itself on the charger at 80 %. || The status bar says when it's on.
setup: home
script:
  wait 800ms
  menu 0 7 7
  wait 1500ms
  press DOWN
  wait 1200ms
  press OK
  wait 1200ms
  knob 1
  wait 2000ms
---
kind: outro
title: ferritefw.dev
left: Free firmware, source public.
seconds: 5
