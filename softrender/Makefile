CC       ?= cc
CFLAGS   ?= -std=c11 -Wall -Wextra -Werror -g -O2
SDL_CFLAGS := $(shell pkg-config --cflags sdl3)
SDL_LIBS   := $(shell pkg-config --libs sdl3)
BUILD    := build
BIN      := $(BUILD)/softrender

.PHONY: all check clean

all: $(BIN)

$(BIN): softrender.c
	mkdir -p $(BUILD)
	$(CC) $(CFLAGS) $(SDL_CFLAGS) -o $(BIN) softrender.c -lm $(SDL_LIBS)

# Determinism check: render the same frames twice, compare byte-for-byte.
check: $(BIN)
	SDL_VIDEO_DRIVER=dummy $(BIN) --frames 4 --dump $(BUILD)/frame-a.ppm
	SDL_VIDEO_DRIVER=dummy $(BIN) --frames 4 --dump $(BUILD)/frame-b.ppm
	cmp $(BUILD)/frame-a.ppm $(BUILD)/frame-b.ppm

clean:
	rm -rf $(BUILD)
