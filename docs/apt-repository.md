# Thallium Store APT repository

The release workflow builds native Qt packages inside Debian 12, Debian 13,
and Ubuntu 24.04 containers. Each suite gets its own signed package index so
APT resolves the correct Qt ABI for that operating system.

## User installation

```bash
curl -fsSL https://dronzer-tb.github.io/thallium-store/install.sh | sudo sh
```

The installer detects the base distribution, verifies the archive public key,
writes a Deb822 source under `/etc/apt/sources.list.d/`, pins the repository so
it cannot replace unrelated distribution packages, and installs
`thallium-store`. Future releases arrive through normal APT upgrades.

Supported targets are currently amd64 Debian 12 (`bookworm`), Debian 13
(`trixie`), Ubuntu 24.04 (`noble`), and Ubuntu derivatives that expose their
base codename through `/etc/os-release`.

## Publishing

`.github/workflows/publish-apt.yml` runs manually or for a version tag such as
`v0.1.10`. A tag must match the workspace version in `Cargo.toml`. The workflow:

1. builds a suite-specific `.deb` for each supported system;
2. generates and signs `Packages`, `Release`, `Release.gpg`, and `InRelease`;
3. uploads tagged packages to a GitHub Release; and
4. deploys the complete APT repository to GitHub Pages.

The repository must have GitHub Pages configured to use GitHub Actions and an
`APT_SIGNING_KEY_B64` Actions secret containing the base64-encoded OpenPGP
secret key matching `packaging/thallium-store-archive-keyring.asc`.
