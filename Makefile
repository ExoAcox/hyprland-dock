PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin

all: build

build:
	cargo build --release

install: build
	install -Dm755 target/release/hyprland-dock $(DESTDIR)$(BINDIR)/hyprland-dock

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/hyprland-dock

run: build
	./target/release/hyprland-dock

.PHONY: all build install uninstall run
