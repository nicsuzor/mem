# mem — pkb
# Cross-compile to Apple Silicon from Linux using cargo-zigbuild + zig 0.13

CARGO        ?= cargo
TARGET_MACOS  = aarch64-apple-darwin
TARGET_LINUX  = x86_64-unknown-linux-gnu
RELEASE_DIR   = target/release
MACOS_DIR     = target/$(TARGET_MACOS)/release
BINS          = pkb
VERSION       = $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')

# macOS SDK sysroot with framework stubs (needed for cross-compile)
MACOS_SYSROOT ?= $(HOME)/.local/share/macos-sdk

# ── Native (host) build ──────────────────────────────────────────────

.PHONY: build
build:
	$(CARGO) build --release

.PHONY: install
install:
	curl -fsSL https://raw.githubusercontent.com/nicsuzor/mem/main/install.sh | sh

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
	@file $(MACOS_DIR)/pkb

# ── Release ────────────────────────────────────────────────────────
# Automated via release-plz (see .github/workflows/release-plz.yml):
#
#   1. Use conventional commits (feat:, fix:, perf:, etc.)
#   2. Merge PR to main
#   3. release-plz opens a Release PR (version bump + CHANGELOG)
#   4. Merge the Release PR → CI builds + publishes automatically
#

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
	@echo "  version        Print current version"
	@echo "  setup-cross    Install rustup target + cargo-zigbuild + zig instructions"
	@echo "  check          Type-check without building"
	@echo "  test           Run tests"
	@echo "  clean          Remove target/"
	@echo "  sizes          Show binary sizes"
	@echo ""
	@echo "Releases are automated via release-plz. Just merge PRs to main"
	@echo "using conventional commits (feat:, fix:, perf:, etc.)."
	@echo ""
	@echo "Prerequisites for cross-compile:"
	@echo "  1. make setup-cross"
	@echo "  2. zig 0.13.x on PATH (see setup-cross output)"
