/* softrender — a simple software renderer on SDL3
 *
 * CPU rasterizer: filled triangles (per-vertex color, barycentric
 * coordinates) plus filled rectangles, drawn directly into the window's
 * backing surface. No GPU, no shaders, no texture stage.
 *
 * The demo scene is a pure function of (width, height, frame index), so
 * headless runs (SDL_VIDEODRIVER=dummy) with
 *     --frames N --dump out.ppm
 * produce byte-reproducible output for verification.
 *
 * Build: make            Run: ./build/softrender [--width W --height H]
 *                              [--frames N] [--dump FILE]
 */
#include <SDL3/SDL.h>
#include <errno.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

typedef struct {
  double x, y;
  Uint8 r, g, b;
} Vex;

/* Twice the signed area of (a, b, p) — the 2D edge function.
 * Points inside the triangle give same-sign values for all three
 * edge evaluations (wa, wb, wc); the values are proportional to the
 * barycentric weights of the opposite vertices.
 */
static double edge2(const Vex *a, const Vex *b, double px, double py)
{
  return (b->x - a->x) * (py - a->y) - (b->y - a->y) * (px - a->x);
}

static void fill_triangle(SDL_Surface *s, const Vex *a, const Vex *b,
                          const Vex *c)
{
  double minx = fmin(a->x, fmin(b->x, c->x));
  double maxx = fmax(a->x, fmax(b->x, c->x));
  double miny = fmin(a->y, fmin(b->y, c->y));
  double maxy = fmax(a->y, fmax(b->y, c->y));

  /* Pixel (i, j) has center (i + 0.5, j + 0.5). */
  int x0 = (int)ceil(minx - 0.5);
  int x1 = (int)floor(maxx - 0.5);
  int y0 = (int)ceil(miny - 0.5);
  int y1 = (int)floor(maxy - 0.5);
  if (x0 < 0) x0 = 0;
  if (y0 < 0) y0 = 0;
  if (x1 >= s->w) x1 = s->w - 1;
  if (y1 >= s->h) y1 = s->h - 1;

  for (int y = y0; y <= y1; y++) {
    for (int x = x0; x <= x1; x++) {
      double px = x + 0.5, py = y + 0.5;
      double wa = edge2(b, c, px, py); /* weight of vertex a */
      double wb = edge2(c, a, px, py); /* weight of vertex b */
      double wc = edge2(a, b, px, py); /* weight of vertex c */
      if (!((wa >= 0 && wb >= 0 && wc >= 0) ||
          (wa <= 0 && wb <= 0 && wc <= 0)))
        continue;
      double sum = wa + wb + wc;
      double ta = wa / sum, tb = wb / sum, tc = wc / sum;
      Uint8 r = (Uint8)(ta * a->r + tb * b->r + tc * c->r + 0.5);
      Uint8 g = (Uint8)(ta * a->g + tb * b->g + tc * c->g + 0.5);
      Uint8 bl = (Uint8)(ta * a->b + tb * b->b + tc * c->b + 0.5);
      SDL_WriteSurfacePixel(s, x, y, r, g, bl, 255);
    }
  }
}

#define BG_R 20
#define BG_G 24
#define BG_B 40

/* Base colors of the rotating triangle, per vertex. */
static const Uint8 T1COL[3][3] = {
  {220, 60, 200},
  {60, 220, 80},
  {200, 80, 220},
};

/* Demo scene: background + a rotating equilateral triangle + a smaller
 * flat-color triangle orbiting around it. Everything derives from
 * (w, h, f) — no state, so frame f is reproducible.
 */
static void render_frame(SDL_Surface *s, int w, int h, int f)
{
  SDL_FillSurfaceRect(s, &(SDL_Rect){ 0, 0, (Uint32)w, (Uint32)h },
                       SDL_MapSurfaceRGB(s, BG_R, BG_G, BG_B));

  double cx = w * 0.5;
  double cy = h * 0.5;
  double R = fmin((double)w, (double)h) * 0.28;
  int shift = (f * 2) & 0xFF;

  /* Triangle 1: equilateral, rotating about the center. */
  double a0 = f * 0.05;
  Vex t1[3];
  for (int k = 0; k < 3; k++) {
    double ang = a0 + k * 2.0 * M_PI / 3.0;
    t1[k] = (Vex){
        cx + R * cos(ang),
        cy + R * sin(ang),
        (Uint8)((T1COL[k][0] + shift) & 0xFF),
        (Uint8)((T1COL[k][1] + shift) & 0xFF),
        (Uint8)((T1COL[k][2] + shift) & 0xFF),
    };
  }

  /* Triangle 2: flat color, orbiting on a larger radius. */
  double a2 = -f * 0.03 + 1.0;
  double R2 = R * 1.55;
  double ox = cx + R2 * cos(a2);
  double oy = cy + R2 * sin(a2);
  Vex t2[3];
  for (int k = 0; k < 3; k++) {
    double ang = a2 + k * 2.0 * M_PI / 3.0;
    t2[k] = (Vex){
        ox + R * 0.35 * cos(ang),
        oy + R * 0.35 * sin(ang),
        (Uint8)((240 + shift) & 0xFF),
        (Uint8)((220 + shift) & 0xFF),
        (Uint8)((60 + shift) & 0xFF),
    };
  }

  fill_triangle(s, &t1[0], &t1[1], &t1[2]);
  fill_triangle(s, &t2[0], &t2[1], &t2[2]);
}

static int write_ppm(const char *path, SDL_Surface *s)
{
  FILE *fp = fopen(path, "wb");
  if (!fp) {
    fprintf(stderr, "fopen %s: %s\n", path, strerror(errno));
    return -1;
  }
  fprintf(fp, "P6\n%d %d\n255\n", s->w, s->h);
  for (int y = 0; y < s->h; y++) {
    for (int x = 0; x < s->w; x++) {
      Uint8 r, g, b;
      SDL_ReadSurfacePixel(s, x, y, &r, &g, &b, NULL);
      fputc(r, fp);
      fputc(g, fp);
      fputc(b, fp);
    }
  }
  if (fclose(fp) != 0) {
    fprintf(stderr, "fclose %s: %s\n", path, strerror(errno));
    return -1;
  }
  return 0;
}

static int usage(void)
{
  fprintf(stderr,
          "usage: softrender [--width W] [--height H] [--frames N] [--dump FILE]\n"
          "  --frames N   render exactly N frames and exit (0 = run until quit)\n"
          "  --dump FILE  write the last rendered frame as PPM (requires --frames > 0,\n"
          "               or with --frames 0 writes the frame at quit)\n"
          "Headless: SDL_VIDEO_DRIVER=dummy softrender --frames 4 --dump out.ppm\n");
  return 2;
}

int main(int argc, char **argv)
{
  int w = 640, h = 480, frames = 0;
  const char *dump = NULL;

  for (int i = 1; i < argc; i++) {
    if (strcmp(argv[i], "--width") == 0 && i + 1 < argc)
      w = atoi(argv[++i]);
    else if (strcmp(argv[i], "--height") == 0 && i + 1 < argc)
      h = atoi(argv[++i]);
    else if (strcmp(argv[i], "--frames") == 0 && i + 1 < argc)
      frames = atoi(argv[++i]);
    else if (strcmp(argv[i], "--dump") == 0 && i + 1 < argc)
      dump = argv[++i];
    else if (strcmp(argv[i], "--help") == 0)
      return usage();
    else
      return usage();
  }
  if (w <= 0 || h <= 0 || frames < 0)
    return usage();

  if (!SDL_Init(SDL_INIT_VIDEO)) {
    fprintf(stderr, "SDL_Init: %s\n", SDL_GetError());
    return 1;
  }
  SDL_Window *win = SDL_CreateWindow("softrender", w, h, 0);
  if (!win) {
    fprintf(stderr, "SDL_CreateWindow: %s\n", SDL_GetError());
    SDL_Quit();
    return 1;
  }
  SDL_Surface *surf = SDL_GetWindowSurface(win);
  if (!surf) {
    fprintf(stderr, "SDL_GetWindowSurface: %s\n", SDL_GetError());
    SDL_DestroyWindow(win);
    SDL_Quit();
    return 1;
  }

  int f = 0;
  for (;;) {
    if (frames > 0 && f >= frames)
      break;
    if (frames == 0) {
      SDL_Event e;
      int quit = 0;
      while (SDL_PollEvent(&e)) {
        if (e.type == SDL_EVENT_QUIT)
          quit = 1;
      }
      if (quit)
        break;
    }
    render_frame(surf, w, h, f);
    SDL_UpdateWindowSurface(win);
    f++;
  }

  if (dump && write_ppm(dump, surf) != 0) {
    SDL_DestroyWindow(win);
    SDL_Quit();
    return 1;
  }

  SDL_DestroyWindow(win);
  SDL_Quit();
  return 0;
}
