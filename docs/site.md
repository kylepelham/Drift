# Marketing site

The public landing page lives in its own repository:
[kylepelham/drift-site](https://github.com/kylepelham/drift-site).

It is a standalone Astro static site (not part of the app build) with a procedural
three.js jellyfish hero, a showcase video recorded from the real app, live GitHub stats,
and download links. Layout, commands, and the video capture workflow are documented in
that repository's README.

One coupling to this repo remains: the capture script (`scripts/capture-video.ts` over
there) records real footage against Drift's dev server at `http://localhost:5180`, using
a scratch workspace at `%LOCALAPPDATA%\Temp\opencode\demo\tidepool`. Never point it at a
real workspace.
