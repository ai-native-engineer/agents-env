#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_dir"

cargo test --all-features backend::tests::fake_provider_contract_separates_metadata_and_values -- --exact
cargo test --all-features backend::tests::metadata_debug_does_not_contain_values -- --exact
cargo test --all-features backend::tests::keychain_adapter_contract_uses_metadata_before_value_read -- --exact
cargo test --all-features --test cli migration_dry_run_is_metadata_only_and_preserves_source -- --exact
cargo test --all-features --test cli migration_rejects_duplicate_key_tag_identity -- --exact

case "$(uname -s)" in
  Darwin)
    cargo test --all-features backend::tests::native_keychain_smoke_uses_an_isolated_temporary_store -- --exact
    ;;
  *)
    echo "native macOS Keychain smoke: skipped on non-macOS; portable backend contract passed"
    ;;
esac

echo "proof-keychain-backend: PASS"
