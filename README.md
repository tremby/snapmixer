Snapmixer
=========

This is a text-mode volume mixer for [Snapcast][snapcast].

Usage
-----

```
snapmixer [--help|-h] [--version|-v] [--server|-s <HOSTNAME[:<PORT>]>]
```

By default (if the `--server` option is not used) it will try `localhost:1705`,
and if it can't connect there, it'll attempt to find a Snapcast server via mDNS autodiscovery.

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

### Logging

For debug logging, [set the `RUST_LOG` environment variable][logging-docs].

Examples:

- For maximum logging, `RUST_LOG=trace`.
- For debug-level messages specific to Snapmixer, `RUST_LOG=snapmixer=debug`.

Logging is to stderr, so it'll mess up the TUI display unless you redirect it.
For example, in one terminal run
`RUST_LOG=snapmixer=debug snapmixer 2>/tmp/snapmixer.log`
and in another run `tail -F /tmp/snapmixer.log`.

Todo
----

Things I want to implement, but may take a while to get to (patches welcome):

- Scrolling if there isn't enough screen space ([#13](https://github.com/tremby/snapmixer/issues/13)).
- Scrolling on the error overlay ([#14](https://github.com/tremby/snapmixer/issues/14)).

Things I think would be neat, but which I have no motivation to implement:

- Customizable colours? Patches welcome.
- Customizable keybinds? Patches welcome.
- Switching streams? Patches welcome.

[snapcast]: https://github.com/badaix/snapcast
[logging-docs]: https://docs.rs/env_logger/latest/env_logger/#enabling-logging
