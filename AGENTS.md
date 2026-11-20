# Repository guidance

This repository is the `tessera-webview` library and Tessera plugin, not a
Tessera application. Keep the public component and controller API in `src/`
consistent across its platform backends. Consult the public Tessera API docs for
the exact revision pinned in `Cargo.toml`; do not rely on internal framework
APIs.

## Platform backends

- Windows, macOS, and Linux X11/XWayland use Wry desktop child views.
- Android uses the system WebView through the Tessera platform-view host; the
  embedded component requires API 30 or newer.
- iOS uses `WKWebView` directly through Tessera's UIKit platform-view factory.
- Keep platform-specific behavior and unsupported options explicit. Do not
  change another platform's rendering or backend architecture as part of an
  unrelated backend change.

The README is intentionally brief: describe the library, how to add and use it,
and list supported platforms and backends. Do not use README or other project
documentation as a validation log.

## Checks

For Rust changes, run:

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

For iOS changes, also check and lint `aarch64-apple-ios-sim` on macOS. For
Android changes, check the Android Rust target and build the Android library.
Record verification in the change summary or commit description, not in project
documentation.
