MANIFEST := flatpak/io.github.luminance_rs.Luminance.yml
APP_ID := io.github.luminance_rs.Luminance

.PHONY: vendor flatpak install bundle test dist exe deb

vendor:
	mkdir -p .cargo
	cargo vendor --locked --versioned-dirs vendor > .cargo/config.toml

flatpak: vendor
	flatpak run org.flatpak.Builder --user --force-clean build-dir $(MANIFEST)

install: vendor
	flatpak run org.flatpak.Builder --user --install --force-clean build-dir $(MANIFEST)

bundle: install
	flatpak build-bundle ~/.local/share/flatpak/repo luminance-rs.flatpak $(APP_ID)

test:
	cargo test

# Release packages into dist/ (see build.sh)
dist:
	./build.sh all

exe:
	./build.sh exe

deb:
	./build.sh deb
