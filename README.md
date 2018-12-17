Snapmixer
=========

This is a console-based volume control for
[Snapcast](https://github.com/badaix/snapcast).

This is an initial version and is not yet configurable;
it connects to a Snapcast server on the local machine at the default port.

It is not yet very efficient or fast, but it works.

It is not yet packaged for release.

Requirements
------------

Node 10.x, NPM.

Other requirements are installed with `npm install`,
most notable of which is [neo-blessed](https://github.com/embark-framework/neo-blessed).

Usage
-----

    node index.js

Press `?` or `F1` to toggle the help box,
which gives information on the other keys.

Currently log messages are emitted to stdout, so it's probably best run as either

    node index.js 2>>log

to watch the messages (with something like `tail -F log` in another terminal), or

    node index.js 2>>/dev/null

to ignore them.
