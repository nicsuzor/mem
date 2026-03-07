# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.21](https://github.com/nicsuzor/mem/compare/v0.2.20...v0.2.21) - 2026-03-07

### Added

- semantic chunking, incremental saves, and granular progress

### Fixed

- preserve content_hash as Option<String> for bincode compat
- remove startup reindex from pkb server and drop content_hash backwards compat

### Other

- Merge branch 'main' into crew/sylvia_59

## [0.2.20](https://github.com/nicsuzor/mem/compare/v0.2.19...v0.2.20) - 2026-03-07

### Added

- add PKB linter with auto-fix capability

### Fixed

- lint --fix now actually fixes all fixable issues
- address PR review — security, correctness, and code quality
