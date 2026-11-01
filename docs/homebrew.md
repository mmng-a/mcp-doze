# Homebrew distribution

The repository contains a Formula template, not a published tap. No release tag or tap repository is created by the build or test suite. Complete the publication steps below when a release is ready.

## What a tap provides

A tap is a Git repository containing Homebrew package definitions, called Formulae. For this project, the proposed repository is `mmng-a/homebrew-tap`, with the package definition at `Formula/mcp-doze.rb`. Users would install the published package with:

```sh
brew install mmng-a/tap/mcp-doze
```

That command adds the tap as needed. A separate `brew tap mmng-a/tap` is optional. A personal tap is independent of the official `homebrew/core` repository and does not require submitting the project there.

## Required files

| Item | Purpose |
| --- | --- |
| An immutable `v<VERSION>` tag in `mmng-a/mcp-doze` | Identifies the source being distributed |
| The tag archive URL and its SHA-256 | Downloads and verifies that source |
| `mmng-a/homebrew-tap/Formula/mcp-doze.rb` | Declares the build, dependencies and installation test |
| A tap README | Explains installation and links to the project |

A GitHub Release page is useful for release notes but is not required to download a GitHub tag archive. Do not move or overwrite a published tag; publish a new version for changes.

## Initial packaging approach

Use the [Formula template](../packaging/homebrew/mcp-doze.rb.in) to build from source on macOS. It declares Rust as a build dependency, fetches the locked Cargo dependencies before installation, and installs only the application binary. Runtime MCP servers and their dependencies remain the user's responsibility.

The installation test checks the binary's version and wraps a synthetic plugin definition under Homebrew's temporary test directory. It does not start the synthetic backend or change the user's Codex configuration.

This approach needs neither prebuilt release binaries nor a signing service. Users build the binary during installation. Homebrew bottles can later provide prebuilt packages once distribution and updates justify the extra CI setup. A locally trusted signing identity is still relevant to Xcode's separate agent approval; Homebrew installation does not grant that approval.

## Prepare and publish a release

The commands below are publication instructions, not steps run by the project. They assume the repositories are checked out under `~/Developer`.

1. Confirm that `Cargo.toml`, `Cargo.lock` and the intended tag have the same version. The examples use `0.1.2`. Commit the release contents and run:

   ```sh
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test --locked
   cargo build --release --locked
   ```

2. Tag the tested commit and push the tag when ready to publish:

   ```sh
   git tag -a v0.1.2 -m "Release v0.1.2"
   git push origin refs/tags/v0.1.2
   ```

3. Create the public GitHub repository `mmng-a/homebrew-tap` and clone it to `~/Developer/homebrew-tap`. Copy `packaging/homebrew/README.md.in` from the project checkout to the tap's `README.md`. Creating that repository does not require Homebrew approval.

4. From the `mcp-doze` checkout, download the exact published archive and render the Formula into the tap:

   ```sh
   doze_version=0.1.2
   doze_archive="${TMPDIR:-/tmp}/mcp-doze-${doze_version}.tar.gz"
   curl --fail --location \
     "https://github.com/mmng-a/mcp-doze/archive/refs/tags/v${doze_version}.tar.gz" \
     --output "$doze_archive"
   doze_sha256="$(shasum -a 256 "$doze_archive" | cut -d ' ' -f 1)"
   mkdir -p "$HOME/Developer/homebrew-tap/Formula"
   sed -e "s/@VERSION@/${doze_version}/g" -e "s/@SHA256@/${doze_sha256}/g" \
     packaging/homebrew/mcp-doze.rb.in \
     > "$HOME/Developer/homebrew-tap/Formula/mcp-doze.rb"
   ```

   The `.rb.in` template is not installable until both placeholders are replaced. Never use a checksum from a different archive, or leave a moving `main` branch as the release source.

5. Commit the generated Formula and README locally, then add that checkout as a tap and test it. Homebrew clones the local Git repository, so uncommitted files would not be included:

   ```sh
   git -C "$HOME/Developer/homebrew-tap" add Formula/mcp-doze.rb README.md
   git -C "$HOME/Developer/homebrew-tap" commit -m "Package mcp-doze for Homebrew"
   brew tap mmng-a/tap "$HOME/Developer/homebrew-tap"
   brew style mmng-a/tap/mcp-doze
   brew audit --strict mmng-a/tap/mcp-doze
   brew install --build-from-source mmng-a/tap/mcp-doze
   brew test mmng-a/tap/mcp-doze
   ```

   Homebrew may request formula trust. Review the generated file and trust that formula when prompted. The existing-installation notes below explain how to switch paths from a Cargo installation.

6. Push the tested tap commit and point the local test tap at the public GitHub repository for subsequent updates:

   ```sh
   git -C "$HOME/Developer/homebrew-tap" push origin HEAD
   git -C "$(brew --repository mmng-a/tap)" remote set-url origin \
     https://github.com/mmng-a/homebrew-tap.git
   ```

   Update the project READMEs from preparation status to the real installation command. The next release repeats the version, source archive, checksum and Formula update.

## Maintenance and existing installations

- `brew update` retrieves Formula changes; `brew upgrade mmng-a/tap/mcp-doze` installs a newer version when the Formula is updated.
- Installing through Homebrew does not rewrite existing MCP registrations or Codex launchers. Update any absolute paths intentionally if switching from a Cargo installation.
- After an upgrade, reapply any local Xcode signing identity to the new executable as described in the [README](../README.md#xcode).
- The Formula does not install a background service, modify plugin manifests during installation, or configure an MCP client. Plugin wrapping remains an explicit `mcp-doze codex sync-plugins` operation.
- Validate the Formula against the Homebrew version used for publication. The template follows the current dependency-fetch/install split.

## References

- [How to Create and Maintain a Tap](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap)
- [Formula Cookbook](https://docs.brew.sh/Formula-Cookbook)
- [Package Acceptance Policy](https://docs.brew.sh/Package-Acceptance-Policy)
