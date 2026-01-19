Snapmixer
=========

This is a text-mode volume mixer for
[Snapcast](https://github.com/badaix/snapcast).

Usage
-----

```
snapmixer [--help] [--host|-h <HOSTNAME>] [--port|-h <PORT>] [--version|-v]
```

### Keys

- `↑`/`↓`: navigate up and down (with shift to jump to groups)
- `←`/`→`: adjust volume (with shift for larger increments)
- `h`/`j`/`k`/`l`: same as `←`/`↓`/`↑`/`→`
- `1`/`2`/…/`9`/`0`: snap volume to 10%, 20%, … 90%, 100%
- `m`: toggle mute
- `q`/`Esc`/`^C`: quit

### Operation

If a group has focus and volumes are adjusted,
the loudest client of that group is adjusted to the target
(whether a fixed number or an increment, depending on the command)
and the other clients in the group
are adjusted in proportion to their relative volumes.

Todo
----

Things I want to implement:

- Scrolling if there isn't enough screen space.
- Scrolling on the error overlay.

Things I think would be neat, but which I have no motivation to implement:

- Customizable colours? Patches welcome.
- Customizable keybinds? Patches welcome.
- Switching streams? Patches welcome.
