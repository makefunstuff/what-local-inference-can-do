# softrender

Simple software renderer in C on SDL3: CPU rasterization of filled
triangles (per-vertex color, barycentric interpolation) and filled
rectangles, drawn directly into the SDL3 window's backing surface. No GPU,
no shaders, no texture stage — just the window surface and math.

Demo scene: a rotating equilateral triangle with per-vertex colors plus a
smaller flat-color triangle orbiting around it on a dark background. The
scene is a pure function of `(width, height, frame index)`, so headless
runs are byte-reproducible.

## Build

Requires SDL3 development files (`pkg-config sdl3`).

```sh
make            # cc -std=c11 -Wall -Wextra -Werror -g -O2
```

## Run

```sh
./build/softrender [--width W --height H] [--frames N] [--dump FILE]
```

- `--frames N` — render exactly N frames and exit (`0` = run until quit).
- `--dump FILE` — write the last rendered frame as PPM.
- Interactive: just run it; close the window to quit.

## Headless verification

```sh
make check      # renders frame 3 twice at 640x480, cmp byte-for-byte
```

`SDL_VIDEO_DRIVER=dummy` gives a real SDL window surface with no display
server (SDL3 renamed the SDL2 `SDL_VIDEODRIVER` variable).

## Verification

An independent Python oracle (`../local/oracle_softrender.py`, gitignored
scratch) re-implements the exact scene and barycentric fill in pure Python
(IEEE double, same expression order) and byte-compares its PPM against the
C binary's output. Verified: 640x480 frame 3 and 320x240 frame 6 both
byte-identical; corner pixel equals background; `make check` passes.
