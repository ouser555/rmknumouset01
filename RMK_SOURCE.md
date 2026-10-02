# RMK source provenance

Vendored snapshot of /home/rmk/keyboard/pr-split-20260912/combined
Commit: f8d93fa52dc46990fb0e17fe4bf8b50a0ec4efd8
Includes joystick power-managed sampling and idle report filtering from the PR #1111 development work, including the subsequent local split/combined refactoring.
This is a pinned local snapshot, not a claim to be the latest remote PR head.
Backup of original project: /home/rmk/keyboard/rmknumouse-backup-20260917-210534

OLED is commented out; J2 supply P0.22 is held low after firmware initialization.
Keys A-L follow the supplied pin order; encoder defaults to volume up/down.
Joystick calibration, encoder detent resolution, sensor orientation and current draw require hardware verification.
