PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin

all: build

build:
	cargo build --release

install: build
	install -Dm755 target/release/hypr-dock $(DESTDIR)$(BINDIR)/hypr-dock

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/hypr-dock

run: build
	./target/release/hypr-dock

.PHONY: all build install uninstall run
