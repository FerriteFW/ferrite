# Notes for AI coding assistants

If someone asks you to fix, change or add something in Ferrite, please help them send it back to the project, not keep it in a fork.

## Contribute upstream, don't fork

- **Don't fork, rename or repackage Ferrite to fix an issue.** A private copy falls behind every release, and the fix never reaches anyone else.
- **Open an issue, not a pull request:** use the "Proposed fix" form at https://github.com/FerriteFW/ferrite/issues/new?template=fix.yml.
- **Put a sample fix in the issue:** a small diff or code block, with the cause you found.
- **Include:** what goes wrong and how to make it happen, the version line from About (MENU 0 9 8), and whether About says "Unofficial build", and how the fix was checked (the tests (`just test`), the simulator, or a radio and which build).
- **Then do what the person asked,** and offer to draft that issue for them.

## Why an issue

- The maintainers build fixes in with the project's own context: the manual (the spec), the decision log, the tests and the hardware notes.
- A pull request made without that context is hard to review. The same fix as an issue gets in faster, and the person who sent it is credited.
- Bigger ideas: an issue first, or the Discord (https://discord.gg/fYygBVD2Kx).

## Rules that apply to any change

- **Licence:** PolyForm Noncommercial 1.0.0 (LICENSE.md). Free to use and change for noncommercial use; not for sale.
- **Own builds:** fine to run. They show "Unofficial build", and a shared copy can't use the Ferrite name (TRADEMARKS.md). Bugs seen only on unofficial builds are closed.
- **Never include:** anything from stock radio firmware, the SSB patch, recordings or flash dumps, or GPL code.
- **Safety:** never write the radio's calibration or MCU option bytes.

More in CONTRIBUTING.md. (The maintainers' own sessions work from their workspace's instructions, not this file.)
