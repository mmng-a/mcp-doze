# Homebrew distribution

The public tap is [mmng-a/homebrew-tap](https://github.com/mmng-a/homebrew-tap). Install the macOS package with:

```sh
brew install mmng-a/tap/mcp-doze
```

Homebrew adds the tap and trusts this specific Formula when installed by its fully qualified name. A separate `brew tap` command is optional. A personal tap is independent of `homebrew/core` and does not require submitting the project there.

## Package contents

The tap's `Formula/mcp-doze.rb` identifies an immutable source tag and its SHA-256, declares Rust as a build dependency, fetches locked Cargo dependencies and builds the application. Runtime MCP servers and their dependencies remain the user's responsibility. The test fixture is a Cargo example and is not installed.

The Formula test checks the binary's version and wraps a synthetic plugin definition under Homebrew's temporary test directory. It does not start the synthetic backend or change the user's Codex configuration.

The source [Formula template](../packaging/homebrew/mcp-doze.rb.in) and [tap README template](../packaging/homebrew/README.md.in) are retained for maintenance. The `.rb.in` file needs both version and checksum placeholders replaced before use.

A GitHub Release page is useful for release notes but is not required to download a tag archive. Prebuilt Homebrew bottles are optional; the initial package builds from source. Do not move a published tag or use a moving `main` branch as the release source.

## Publish a subsequent version

The examples below use `0.1.3` as a future version, following the initial `0.1.2` release. They are maintainer instructions, not commands run by the project.

1. Update the package version in `Cargo.toml` and `Cargo.lock`, commit the release contents and run:

   ```sh
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test --locked
   cargo build --release --locked
   ```

2. Tag the tested source commit and push the tag when ready to publish:

   ```sh
   git tag -a v0.1.3 -m "Release v0.1.3"
   git push origin refs/tags/v0.1.3
   ```

3. Work in Homebrew's checkout of the public tap so its canonical GitHub identity remains intact:

   ```sh
   brew tap mmng-a/tap
   doze_tap_dir="$(brew --repository mmng-a/tap)"
   git -C "$doze_tap_dir" pull --ff-only
   ```

4. From the `mcp-doze` source checkout, download the exact archive and render the updated Formula:

   ```sh
   doze_version=0.1.3
   doze_archive="${TMPDIR:-/tmp}/mcp-doze-${doze_version}.tar.gz"
   curl --fail --location \
     "https://github.com/mmng-a/mcp-doze/archive/refs/tags/v${doze_version}.tar.gz" \
     --output "$doze_archive"
   doze_sha256="$(shasum -a 256 "$doze_archive" | cut -d ' ' -f 1)"
   sed -e "s/@VERSION@/${doze_version}/g" -e "s/@SHA256@/${doze_sha256}/g" \
     packaging/homebrew/mcp-doze.rb.in \
     > "$doze_tap_dir/Formula/mcp-doze.rb"
   ```

5. Test the local Formula before pushing it. Use `install` instead of `reinstall` if it has not been installed on that Mac:

   ```sh
   brew trust --formula mmng-a/tap/mcp-doze
   brew style mmng-a/tap/mcp-doze
   brew audit --strict mmng-a/tap/mcp-doze
   brew reinstall --build-from-source mmng-a/tap/mcp-doze
   brew test mmng-a/tap/mcp-doze
   ```

6. Commit and push the tested Formula from that same tap checkout:

   ```sh
   git -C "$doze_tap_dir" add Formula/mcp-doze.rb
   git -C "$doze_tap_dir" commit -m "Package mcp-doze 0.1.3"
   git -C "$doze_tap_dir" push origin HEAD
   ```

## Existing installations

- `brew update` retrieves Formula changes; `brew upgrade mmng-a/tap/mcp-doze` installs a newer version once the Formula is updated.
- Homebrew installation does not rewrite MCP registrations or Codex launchers. Update absolute paths intentionally if switching from a Cargo installation.
- The Formula does not install a background service, modify plugin manifests during installation or configure an MCP client. Plugin wrapping remains an explicit `mcp-doze codex sync-plugins` operation.

## References

- [How to Create and Maintain a Tap](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap)
- [Tap Trust](https://docs.brew.sh/Tap-Trust)
- [Formula Cookbook](https://docs.brew.sh/Formula-Cookbook)
