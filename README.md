Snapmixer
=========

This is a text-mode volume mixer for
[Snapcast](https://github.com/badaix/snapcast).

It is not yet properly packaged for release.

Usage
-----

`snapmixer [--host <HOSTNAME>] [--port <PORT>]`

- `j` and `k` or cursors go up and down between clients and groups.
- `J` and `K` or shift-cursors go up and down between groups.
- `h` and `l` or cursors adjust volume in small increments.
- `H` and `L` or shift-cursors adjust volume in larger increments.
- `1` through `0` snap volume to 10%, 20%, ..., 90%, 100%.
- `m` toggles mute.
- `q` or `Esc` or `^C` quit.

If a group has focus and volumes are adjusted, the loudest client of that group
is adjusted to the target (whether a fixed number or an increment, depending on
the command) and the other clients in the group are adjusted in proportion to
their relative volumes.

Todo
----

Things I want to implement:

- Scrolling if there isn't enough screen space.
- Scrolling on the error overlay.

Things I think would be neat, but which I have no motivation to implement:

- Customizable colours? Patches welcome.
- Customizable keybinds? Patches welcome.
- Switching streams? Patches welcome.
