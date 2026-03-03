# mem — pkb, aops
# Cross-compile to Apple Silicon from Linux using cargo-zigbuild + zig 0.13

CARGO        ?= cargo
TARGET_MACOS  = aarch64-apple-darwin
TARGET_LINUX  = x86_64-unknown-linux-gnu
RELEASE_DIR   = target/release
MACOS_DIR     = target/$(TARGET_MACOS)/release
BINS          = pkb aops
VERSION       = $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')

# macOS SDK sysroot with framework stubs (needed for cross-compile)
MACOS_SYSROOT ?= $(HOME)/.local/share/macos-sdk

# ── Native (host) build ──────────────────────────────────────────────

.PHONY: build
build:
	$(CARGO) build --release

.PHONY: install
install:
	cargo binstall --git https://github.com/nicsuzor/mem mem

# ── Apple Silicon cross-build ────────────────────────────────────────
# Requires: zig 0.13, cargo-zigbuild, macOS sysroot with framework stubs
# Run `make setup-cross` first to install prerequisites

.PHONY: apple
apple: $(MACOS_SYSROOT)/usr/lib/libSystem.B.tbd
	SDKROOT=$(MACOS_SYSROOT) $(CARGO) zigbuild --release --target $(TARGET_MACOS)
	@echo ""
	@echo "Binaries:"
	@for b in $(BINS); do ls -lh $(MACOS_DIR)/$$b 2>/dev/null; done
	@echo ""
	@file $(MACOS_DIR)/aops

# ── Release ────────────────────────────────────────────────────────
# Two-step release (main branch requires PRs):
#
#   1. On PR branch:  make bump         # bump patch, commit, push branch
#      (or:           make bump-minor)
#   2. Merge PR on GitHub
#   3. On main:       make tag          # pull, tag, push tag → CI builds
#

.PHONY: bump
bump: bump-patch bump-commit

.PHONY: bump-commit
bump-commit:
	@$(CARGO) generate-lockfile
	@NEW_VER=$$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/') && \
		git add Cargo.toml Cargo.lock && \
		git commit -m "release: v$$NEW_VER" && \
		git push && \
		echo "" && \
		echo "Committed v$$NEW_VER — merge PR, then run: make tag"

.PHONY: tag
tag:
	@git pull
	@VER=$$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/') && \
		git tag -a "v$$VER" -m "Release v$$VER" && \
		git push origin "v$$VER" && \
		echo "" && \
		echo "Tagged v$$VER — CI will build and publish the release."

# ── Version bumping ───────────────────────────────────────────────

.PHONY: bump-patch
bump-patch:
	@OLD=$$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	MAJOR=$$(echo $$OLD | cut -d. -f1); \
	MINOR=$$(echo $$OLD | cut -d. -f2); \
	PATCH=$$(echo $$OLD | cut -d. -f3); \
	NEW="$$MAJOR.$$MINOR.$$((PATCH + 1))"; \
	sed "s/^version = \"$$OLD\"/version = \"$$NEW\"/" Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml; \
	echo "Version: $$OLD → $$NEW"

.PHONY: bump-minor
bump-minor:
	@OLD=$$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	MAJOR=$$(echo $$OLD | cut -d. -f1); \
	MINOR=$$(echo $$OLD | cut -d. -f2); \
	NEW="$$MAJOR.$$((MINOR + 1)).0"; \
	sed "s/^version = \"$$OLD\"/version = \"$$NEW\"/" Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml; \
	echo "Version: $$OLD → $$NEW"

.PHONY: bump-major
bump-major:
	@OLD=$$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/'); \
	MAJOR=$$(echo $$OLD | cut -d. -f1); \
	NEW="$$((MAJOR + 1)).0.0"; \
	sed "s/^version = \"$$OLD\"/version = \"$$NEW\"/" Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml; \
	echo "Version: $$OLD → $$NEW"

.PHONY: version
version:
	@echo $(VERSION)

# ── macOS SDK sysroot (auto-created) ─────────────────────────────────

$(MACOS_SYSROOT)/usr/lib/libSystem.B.tbd:
	@echo "Creating macOS SDK sysroot at $(MACOS_SYSROOT)..."
	@mkdir -p $(MACOS_SYSROOT)/usr/lib
	@mkdir -p $(MACOS_SYSROOT)/System/Library/Frameworks/CoreFoundation.framework
	@ZIG_LIB=$$(zig env 2>/dev/null | grep lib_dir | head -1 | sed 's/.*": "//;s/".*//' ) && \
		cp "$$ZIG_LIB/libc/darwin/libSystem.tbd" $(MACOS_SYSROOT)/usr/lib/libSystem.tbd && \
		cp "$$ZIG_LIB/libc/darwin/libSystem.tbd" $(MACOS_SYSROOT)/usr/lib/libSystem.B.tbd
	@printf '%s\n' \
		'--- !tapi-tbd' \
		'tbd-version:     4' \
		'targets:         [ x86_64-macos, arm64-macos ]' \
		"install-name:    '/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation'" \
		'current-version: 1970' \
		'reexported-libraries:' \
		'  - targets:     [ x86_64-macos, arm64-macos ]' \
		"    libraries:   [ '/usr/lib/libSystem.B.dylib' ]" \
		'...' \
		> $(MACOS_SYSROOT)/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation.tbd
	@printf '%s\n' \
		'--- !tapi-tbd' \
		'tbd-version:     4' \
		'targets:         [ x86_64-macos, arm64-macos ]' \
		"install-name:    '/usr/lib/libiconv.2.dylib'" \
		'current-version: 7' \
		'reexported-libraries:' \
		'  - targets:     [ x86_64-macos, arm64-macos ]' \
		"    libraries:   [ '/usr/lib/libSystem.B.dylib' ]" \
		'...' \
		> $(MACOS_SYSROOT)/usr/lib/libiconv.tbd
	@echo "Sysroot ready."

# ── Setup cross-compilation toolchain ────────────────────────────────

.PHONY: setup-cross
setup-cross:
	rustup target add $(TARGET_MACOS)
	$(CARGO) install cargo-zigbuild
	@echo ""
	@echo "Zig 0.13 is required (bundles macOS libc stubs)."
	@echo "Install via: pip install 'ziglang>=0.13,<0.14'"
	@echo "Then ensure 'zig' is on your PATH."
	@echo ""
	@echo "Verify: zig version  (should be 0.13.x)"

# ── Utilities ────────────────────────────────────────────────────────

.PHONY: clean
clean:
	$(CARGO) clean

.PHONY: check
check:
	$(CARGO) check

.PHONY: test
test:
	$(CARGO) test

.PHONY: sizes
sizes:
	@echo "── Native ──"
	@for b in $(BINS); do ls -lh $(RELEASE_DIR)/$$b 2>/dev/null || true; done
	@echo "── Apple Silicon ──"
	@for b in $(BINS); do ls -lh $(MACOS_DIR)/$$b 2>/dev/null || true; done

.PHONY: help
help:
	@echo "Targets:"
	@echo "  build          Release build for current host"
	@echo "  install        Install release binaries via cargo-binstall"
	@echo "  apple          Cross-compile for Apple Silicon (aarch64-apple-darwin)"
	@echo "  bump           Bump patch, commit, push branch (step 1: on PR branch)"
	@echo "  bump-minor     Bump minor version in Cargo.toml (no commit)"
	@echo "  bump-patch     Bump patch version in Cargo.toml (no commit)"
	@echo "  tag            Pull main, tag, push tag → CI release (step 2: after merge)"
	@echo "  version        Print current version"
	@echo "  setup-cross    Install rustup target + cargo-zigbuild + zig instructions"
	@echo "  check          Type-check without building"
	@echo "  test           Run tests"
	@echo "  clean          Remove target/"
	@echo "  sizes          Show binary sizes"
	@echo ""
	@echo "Prerequisites for cross-compile:"
	@echo "  1. make setup-cross"
	@echo "  2. zig 0.13.x on PATH (see setup-cross output)"
