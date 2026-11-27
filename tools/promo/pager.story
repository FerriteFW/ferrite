# Ferrite's Pager mode (POCSAG on slot C), recorded from the simulator (the firmware's own screens). Format: see
# preview.story. The simulator's pager hands over a sample page every 20 s while Pager mode listens (sim/src/fake.rs):
# ham-style texts only, never public-service ones. Rules: DEC-4, DEC-20 to DEC-22 (manual chapter 27).

kind: intro
title: Pager mode
left: POCSAG pages on slot C, decoded by the radio itself.
seconds: 4
key_gap: 350
---
title: Pager on slot C
layout: right
left: MENU 0 5 3 has a fifth choice, Pager. Slot C listens for POCSAG pages, 1200 or 2400 baud, on the radio's own modem.
right: A and B stay yours: listen, scan and talk as usual while C listens for pages.
setup: home
script:
  wait 1000ms
  menu 0 5 3
  wait 900ms
  press LEFT
  wait 1200ms
  home
  wait 2000ms
---
title: The pager card
layout: left
left: SLOT to C: the card says Pager, the frequency and the pages heard since power-on.
right: Its line is the newest page: the capcode and how long ago.
setup: wait 300ms
script:
  wait 800ms
  press SLOT
  wait 400ms
  press SLOT
  wait 21000ms
hold: 2
---
title: Read the pages
layout: right
left: OK on C opens the pager list; OK again shows the pages, newest first, each with its capcode, time and text.
right: PTT on C opens them too. It never transmits there.
setup: wait 300ms
script:
  wait 800ms
  press OK
  wait 1200ms
  press OK
  wait 3500ms
hold: 2
---
title: In Messages too
layout: left
left: Pages also sit in Messages (MENU 0 5 2) among your APRS messages, by time, marked Page, with their capcode.
right: OK opens one. They count in the inbox like any other.
setup: home
script:
  wait 800ms
  menu 0 5 2
  wait 3500ms
hold: 2
---
title: Only your capcode
layout: right
left: The capcode filter (MENU 0 4 2 4) keeps one capcode and drops the rest; # clears it.
right: The frequency is MENU 0 4 2 3, the speed MENU 0 4 2 5.
setup: home
script:
  wait 800ms
  menu 0 4 2 4
  wait 3500ms
hold: 2
---
kind: outro
title: ferritefw.dev
left: Free and Source Included.
seconds: 5
