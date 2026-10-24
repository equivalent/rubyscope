# Rubyscope logo

The mark combines a microscope with a ruby on its specimen stage.

- `rubyscope-dark.svg` and `.png`: ivory microscope for dark backgrounds.
- `rubyscope-light.svg` and `.png`: charcoal microscope for light backgrounds.
- `rubyscope.svg`: ivory mark on a dark tile for desktop launchers and window icons.
- `rubyscope-{size}.png`: desktop icons from 16 to 512 pixels.

The app selects the transparent mark using its theme. The website uses the dark
mark. Both website pages use `webpage/favicon.ico`; `favicon.ico` at the repo
root is the same multi-size icon. The SVG favicon remains available too.

After editing the SVG masters, run `bin/generate-icons`. It requires
`rsvg-convert` from librsvg and `magick` from ImageMagick. This rebuilds the
PNGs, copies the website mark, and creates ICO images at 16, 32, 48, 64, 128,
and 256 pixels. The Linux and macOS installers consume these assets as before.
