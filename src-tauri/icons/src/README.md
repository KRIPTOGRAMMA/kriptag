# Icon sources

`funnel.svg` — the original drawing (1024×1024, white wireframe on black).

`funnel-cropped.svg` — what the shipped icons are generated from. Identical
artwork: not one path differs, only the `viewBox` and the background rect.

## Why the crop

The drawing extends past its own canvas — measured ink bounds are x −158..932,
y 53..1251 against a 1024 canvas — so the funnel's stem runs off the bottom-left
corner and the bell occupies only the upper part of the frame. At icon sizes
that wastes most of the pixels on empty black.

Cropping to the bell (`viewBox="140 40 820 820"`) makes the funnel bigger in
frame and fully visible. Compared at 16/32/48/128 against two alternatives:
the original as-is (dimmer and clipped at every size) and a version with
stroke-width raised 6→26 (best at 16px, but at 128 the mesh fills in and the
stem becomes a solid wedge, losing the point of the drawing). The crop won
everywhere except 16px, where nothing helps much — 16 pixels cannot hold a
40-line mesh.

## Regenerating

```sh
rsvg-convert -w 1024 -h 1024 funnel-cropped.svg -o master.png
npx tauri icon master.png
rm -rf ../android ../ios ../64x64.png   # mobile targets this project does not build
```

`tauri icon` also emits Android and iOS sets; this project builds Linux and
Windows only, so those are deleted rather than committed.
