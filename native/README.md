# Native Qt client

This directory contains the Qt 6 host for Thallium Store. It is a small C++
application that embeds `ui/shell.qml`, exposes desktop theme icons, and bridges
QML to the Rust backend's newline-delimited JSON-RPC Unix socket.

It has no Quickshell, Electron, Chromium, Node, or webview dependency.

## Build

From the repository root:

```bash
sudo apt install cmake g++ qt6-base-dev qt6-declarative-dev \
  qml6-module-qtquick qml6-module-qtquick-controls \
  qml6-module-qtquick-layouts qt6-qpa-plugins
./scripts/build-native
```

The debug executable is written to
`target/native/thallium-store-native`. Set
`THALLIUM_NATIVE_BUILD_TYPE=Release` for a release build under
`target/native-release/`.

Use `./scripts/dev-run` to build and launch the complete application.
