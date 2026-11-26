# Ferrite APRS, recorded from the simulator (the firmware's own screens). Format: see preview.story.
# The decoder numbers are docs/showcase/aprs-decoder.md's (Oct 22, 2026: the radio's receive path at 24 kHz, decision 240; Dire Wolf
# on its default settings, named as such, since tuned A+ with bit fixing beats us off the air): copy them from there when they
# change, never estimate them.

kind: intro
title: APRS on Ferrite
left: Position, messages and a decoder that holds its own, on the radio itself.
seconds: 4
music: music/chiptune3.wav
key_gap: 350
---
title: APRS on slot C
layout: right
left: MENU 0 5 3 puts APRS mode on slot C.
right: A and B stay yours: listen, scan and talk as usual while C watches the APRS frequency, on its small card.
setup: home
script:
  wait 1000ms
  menu 0 5 3
  wait 900ms
  knob 2
  wait 900ms
  home
  wait 2500ms
---
title: The APRS card
layout: left
left: SLOT to C and the card grows: stations heard, and in the last hour; the newest two with their age, distance and bearing.
right: Unread messages take the second row. The header says Muted while packets are decoded, not heard, and who gets the VHF chip first.
setup: wait 20s
script:
  wait 800ms
  press SLOT
  wait 300ms
  press SLOT
  wait 7000ms
hold: 2
---
title: Beacons, and who heard them
layout: right
left: Every 5 minutes (0 5 6 1): the first goes at once, and the card counts down to the next. Beacon now is OK 4 OK.
right: The card names the digipeater that repeated your last beacon; the Sent page (OK 3) lists your last 20 and who repeated each.
setup: wait 300ms
script:
  wait 600ms
  menu 0 5 6 1
  wait 700ms
  knob 4
  wait 900ms
  home
  wait 3500ms
  press OK
  wait 300ms
  press 3
  wait 2500ms
hold: 2
---
title: Telemetry in the comment
layout: right
left: 0 5 6 4 adds the battery, the radio's temperature and the beacon power in watts to the end of your comment.
right: One ordinary position report that every map and radio shows as it is: no extra packets.
setup: menu 0 5 5 3; type 66; press HASH; type 843; press HASH; type 7623; press OK; home
script:
  wait 600ms
  menu 0 5 5
  wait 1200ms
  menu 0 5 6 4
  wait 600ms
  knob 1
  wait 3500ms
hold: 2.5
# The packet as the firmware builds it for the sample radio (VA7FER, SSID 7, the default path and Person symbol,
# the sample's 3D fix at 74 m, 7.7 V and 24 C from the simulator, Low = 0.5 W, comment "On the road"): printed by a
# throwaway test calling aprs_live.rs's beacon (tick) with these inputs, and the comment as uaprs's parser reads it
# back. Rebuild both the same way when the format changes; never type them freehand.
packet: VA7FER-7>APZFER,WIDE1-1,WIDE2-1:=4525.29N/07541.83W[/A=000242On the road 7.7V 24C 0.5W
packet_comment: On the road 7.7V 24C 0.5W
packet_mark: 7.7V 24C 0.5W
packet_note: THE BEACON, AS A MAP OR ANOTHER RADIO SHOWS IT
packet_at: 8.6
---
title: FX.25, both ways
layout: left
left: Forward error correction around each packet, so one with a few bad bits still decodes. Ferrite hears FX.25 beside plain AX.25 and sends its own packets with it: on by default, 16 check bytes (0 5 9 5).
right: Radios without FX.25 still decode them as plain packets. A station's page says when it came as FX.25, and how many bytes were fixed. || APRS without compromise: error correction usually left to a PC TNC, built into the radio.
# All four of the simulator's live stations heard (the second, VA3MBK-9, comes as FX.25: sample/aprs.rs), so it is
# third in the heard list.
setup: home; wait 15s
script:
  wait 600ms
  menu 0 5 9 5
  wait 700ms
  knob 1
  wait 700ms
  knob 1
  wait 700ms
  knob 1
  wait 700ms
  knob -3
  wait 600ms
  menu 0 5 1
  wait 500ms
  press DOWN
  press DOWN
  press OK
  wait 2000ms
hold: 2
---
title: Messages
layout: right
left: OK 1 on the APRS card: your messages and where each stands, new, pending, acked, retrying or failed.
right: # writes one. Predictive text, quick messages and punctuation on 1: Hello, see you!
# Typing reads better quicker than menu steps.
key_gap: 150
setup: home
script:
  wait 600ms
  press OK
  wait 300ms
  press 1
  wait 1800ms
  press HASH
  wait 900ms
  press OK
  wait 700ms
  type 43556
  wait 300ms
  press 1
  press 1
  wait 500ms
  press HASH
  type 733
  press HASH
  type 968
  wait 300ms
  press 1
  press 1
  press 1
  press 1
  wait 900ms
  press PTT
  wait 2200ms
hold: 2
---
title: And more
layout: left
left: Mic-E beacons with a status (0 5 5 7), a digipeater, and a KISS TNC over Bluetooth or the cable.
right: It answers station queries, and sends a beacon after you talk if you want one.
setup: home
script:
  wait 600ms
  menu 0 5 5 7
  wait 700ms
  knob 2
  wait 2500ms
---
kind: intro
title: 411 packets, off the air
left: 45 minutes of real APRS, through the radio's own receive path: 411 packets. Dire Wolf 1.8.1 on its default settings: 353. A second recording: 394 against 367. In about 1.7 KB of RAM.
seconds: 7
---
kind: intro
title: 1,011 and 1,007 packets
left: The WA8LMF TNC Test CD, tracks 1 and 2. Dire Wolf 1.8.1, default: 1,005 and 982. The best ESP32 decoder: 1,006 and 997. The KPC-3+ hardware TNC: 989 and 925.
seconds: 7
---
kind: outro
title: ferritefw.dev
left: Free and Source Included.
seconds: 5
