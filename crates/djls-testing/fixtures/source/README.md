# Vendored source fixtures

This directory contains a bounded set of upstream files used as static-analysis fixtures. They are not executable package installations.

The sources come from the repositories pinned by `crates/djls-testing/manifest.lock`. Regenerate them with `just corpus vendor-spec-fixtures` and verify them with `just corpus vendor-spec-fixtures --check`. Each repository directory includes its existing license from `crates/djls-testing/licenses`.

Source files are copied byte-for-byte and intentionally left unedited. When a test contract requires another file, add its explicit repository-relative path to `SOURCE_FIXTURES` in `crates/djls-testing/src/vendor.rs`, sync the pinned corpus, and regenerate the fixtures.
