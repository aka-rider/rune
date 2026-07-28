# Inline image rendering — manual test

Run `./rune testdata/images.md` in **Kitty** or **Ghostty**. Standalone images
below should render inline at their position and scroll with the document. In a
non-Kitty terminal (or `TERM=dumb`), every image should fall back to its alt
text with no escape-sequence garbage.

## Standalone images

A PNG:

![blue rectangle](assets/x.png)

A JPEG photo:

![green photo](assets/photo.jpg)

An SVG vector:

![purple circle](assets/vector.svg)

An animated GIF (should cycle red → green → blue):

![animated](assets/anim.gif)

## List-item image

- ![orange square](assets/y.png)

## Truly-inline image (stays alt text)

This sentence has an inline ![inline](assets/x.png) image with text on both
sides, so it must remain `[image: inline]` rather than rendering.

## Editing behaviour

Put the cursor on a standalone image line: it collapses to its raw
`![alt](path)` source. Move off and it re-renders without the view jumping.
Quitting clears all images from the terminal; switching files clears the
previous file's images.
