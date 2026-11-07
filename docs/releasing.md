# Release workflow

Use Semantic Versioning and direct-to-`main` development.

1. Patch = compatible fix; minor = compatible feature; major = incompatible configuration, protocol, or lifecycle change.
2. Update `Cargo.toml`, `Cargo.lock`, `webos-app/appinfo.json`, IPK metadata, and `CHANGELOG.md` together.
3. Run `make release-check`, `make check`, and `make build` (native default) from [operations.md](operations.md). Run `make build-docker` for the CI/release fallback. Validate each artifact independently; a tested native artifact does not validate a different Docker/Zig binary.
4. Run `make package-app` with Rust Ares installed and `uv run python scripts/test_package_ipk.py`; inspect filename, archive/control metadata and manifest hash. Also run `uv run python scripts/test_native_toolchain.py`. Keep launcher-only packages/manifests local.
5. Build the Homebrew Channel package locally and install it on a test TV (`luna-send` `appInstallService/dev/install`, then open the app): confirm the launch page's install step reports `installed`, `upgraded` or `started`, the daemon runs the packaged hash, and `/var/lib/webosbrew/init.d/50-lg-hue-sync` links into the app. Run `python3 scripts/test_build_hbc_package.py`.
6. Commit and push directly to `main`.
7. After the pushed commit and artifacts are verified:

```bash
git tag -a vX.Y.Z -m "lg-hue-sync X.Y.Z"
git push origin vX.Y.Z
make build-docker
scripts/publish_hbc.sh vX.Y.Z
```

`scripts/publish_hbc.sh` creates the GitHub release with the daemon, `SHA256SUMS`,
the Homebrew Channel `.ipk` and its `manifest.json`, and updates the Homebrew
Channel repository index (`api/apps.json`) on the `gh-pages` branch, served by
GitHub Pages. The CI release job on the same tag leaves an existing release
untouched, so its separately built daemon never replaces the one inside the
package. Verify the release assets' SHA-256 against the manifest and the index
URL before calling publication complete.

Do not call a release device-validated without matching binary digests, service/dashboard health, and a physical capture-to-light check. Homebrew Channel publication additionally needs a complete lifecycle and a release manifest matching `webosbrew/apps-repo`.
